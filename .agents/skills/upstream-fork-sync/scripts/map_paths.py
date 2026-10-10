#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""按 upstream-path-map.yaml 映射上游路径 → 本 fork 布局，并标记未映射项（无遗漏校验）。

用法:
    python3 map_paths.py [--tag v2.7.0] [--map docs/rules/workflow/upstream-path-map.yaml] [--quiet]

退出码: 0 全部已映射；1 存在未映射项；2 参数/文件错误。
"""
from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

try:
    import yaml
except ImportError:  # pragma: no cover
    print("需要 PyYAML: pip install pyyaml", file=sys.stderr)
    sys.exit(2)

DEFAULT_MAP = "docs/rules/workflow/upstream-path-map.yaml"


def sh(*args: str) -> str:
    return subprocess.run(args, capture_output=True, text=True, check=True).stdout


def top_levels(paths: list[str]) -> list[str]:
    return sorted({p.split("/", 1)[0] for p in paths})


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", default=None, help="上游 tag/ref（默认取 manifest 的 upstream_ref）")
    ap.add_argument("--map", default=DEFAULT_MAP)
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    map_path = Path(args.map)
    if not map_path.exists():
        print(f"找不到映射文件: {map_path}", file=sys.stderr)
        return 2
    manifest = yaml.safe_load(map_path.read_text(encoding="utf-8"))

    ref = args.tag or manifest.get("upstream_ref", "upstream/main")
    try:
        files = sh("git", "ls-tree", "-r", "--name-only", ref).splitlines()
    except subprocess.CalledProcessError:
        print(f"无法读取 git 树: {ref}", file=sys.stderr)
        return 2
    files = [f for f in files if f]

    # 归集上游已知路径（map + adopt + ignore）
    mapped: dict[str, str] = {}
    for entry in manifest.get("map", []) or []:
        up = entry["upstream"].strip("/")
        tgt = entry.get("target", "")
        mapped[up] = tgt
    adopt = {a.strip("/") for a in (manifest.get("adopt", []) or [])}
    ignore = {i.strip("/") for i in (manifest.get("ignore", []) or [])}

    def resolve(path: str) -> str | None:
        # 最长前缀匹配 map（按路径分段）
        parts = path.split("/")
        for depth in range(len(parts), 0, -1):
            prefix = "/".join(parts[:depth])
            if prefix in mapped:
                suffix = "/".join(parts[depth:])
                base = mapped[prefix]
                return f"{base}/{suffix}" if suffix and base else (base or suffix)
        top = parts[0]
        if top in adopt:
            return path
        if top in ignore:
            return None  # 忽略
        return None

    tops = top_levels(files)
    unmapped: list[str] = []
    if not args.quiet:
        print(f"# 映射审计 ref={ref}  （顶层 {len(tops)} 项）\n")
        for top in tops:
            # 顶层已覆盖：显式映射，或有以该顶层为前缀的子项映射（如 easytier-contrib/*）
            covered = top in mapped or any(k.split("/")[0] == top for k in mapped)
            if covered:
                tgt = resolve(top + "/x")
                shown = tgt.split("/x")[0] if tgt else top
                print(f"  -> {shown:40} {top}")
            elif top in ignore:
                print(f"  {'（忽略）':12} {top}")
            elif top in adopt:
                print(f"  -> {top:40} {top}")
            else:
                print(f"  {'❓ 未映射':12} {top}")
                unmapped.append(top)

    if unmapped:
        print(f"\n❌ 存在 {len(unmapped)} 个未映射顶层项: {', '.join(unmapped)}", file=sys.stderr)
        return 1
    if not args.quiet:
        print("\n✅ 全部顶层项已映射/采纳/忽略（无遗漏）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
