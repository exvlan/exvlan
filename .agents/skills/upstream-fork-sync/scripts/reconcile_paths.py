#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""逐文件核对：我方分支 vs 上游 release（按 rebrand+重定位映射），报告遗漏/多余文件。

用法:
    python3 reconcile_paths.py [--upstream upstream/releases/v2.7.0] [--ours feature]

退出码: 0 无关键遗漏；1 存在 MISSING。
"""
from __future__ import annotations
import argparse, subprocess, sys
from collections import Counter

IGNORE_NAMES = {"Cargo.lock", "pnpm-lock.yaml", "package-lock.json", "flake.lock"}
ADOPT = {".cargo", ".config", ".envrc", ".gitattributes", ".gitignore", "assets", "script",
         "android.nix", "flake.nix", "rust-toolchain.toml", "Cargo.toml", "package.json",
         "pnpm-workspace.yaml", "README.md", "README_CN.md", "CONTRIBUTING.md",
         "CONTRIBUTING_zh.md", "LICENSE", "docs", ".github", "CONTEXT.md"}
LOCAL_ONLY = {"AGENTS.md", "CLAUDE.md", "UPSTREAM_SYNC.md", "rules.md", ".env", ".vite-hooks",
              "compose.mobile.yml", "compose.test.yml", "Dockerfile.mobile", "Dockerfile.test",
              "vite.config.ts", "exvlan.code-workspace", ".agents"}
MAP = [("easytier-contrib/easytier-", "exvlan/contrib/"),
       ("easytier-web/frontend-lib", "packages/std"),
       ("easytier-web/frontend", "packages/ui"),
       ("easytier-web/config-generator", "packages/generator"),
       ("easytier-web", "exvlan/api"),
       ("easytier-core", "exvlan/core"),
       ("easytier-proto", "exvlan/proto"),
       ("easytier-rpc-build", "exvlan/proto/build"),
       ("easytier-go", "exvlan/go"),
       ("easytier-gui", "packages/studio"),
       ("easytier-js", "packages/sdk"),
       ("tauri-plugin-vpnservice", "exvlan/plugin/vpnservice"),
       ("easytier", "exvlan/exvlan")]
RENAME = {"easytier/src/easytier-core.rs": "exvlan/exvlan/src/main.rs",
          "easytier/src/easytier-cli.rs": "exvlan/exvlan/src/cli.rs",
          "docs/easytier-web-managed-config-sync-plan.md": "docs/exvlan-web-managed-config-sync-plan.md",
          "EasyTier.code-workspace": "exvlan.code-workspace"}


def tree(ref: str) -> set[str]:
    return set(subprocess.run(["git", "ls-tree", "-r", "--name-only", ref],
                              capture_output=True, text=True).stdout.splitlines())


def rebrand_seg(p: str) -> str:
    return "/".join(s.replace("easytier", "exvlan").replace("EasyTier", "Exvlan")
                     .replace("Easytier", "Exvlan") for s in p.split("/"))


def ignored(p: str) -> bool:
    return p.split("/")[-1] in IGNORE_NAMES


def mapup(p: str) -> str | None:
    if ignored(p):
        return None
    if p in RENAME:
        return RENAME[p]
    for up, ds in MAP:
        if up.endswith("-"):
            if p == up or p.startswith(up):
                return rebrand_seg(ds.rstrip("/") + "/" + p[len(up):].lstrip("/"))
        else:
            if p == up:
                return rebrand_seg(ds)
            if p.startswith(up + "/"):
                return rebrand_seg(ds.rstrip("/") + "/" + p[len(up) + 1:])
    if p.split("/")[0] in ADOPT:
        return rebrand_seg(p)
    return "??" + p


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--upstream", default="upstream/releases/v2.7.0")
    ap.add_argument("--ours", default="feature")
    a = ap.parse_args()
    up, ours = tree(a.upstream), tree(a.ours)
    mapped, unmapped = {}, []
    for p in up:
        t = mapup(p)
        if t is None:
            continue
        if t.startswith("??"):
            unmapped.append(p)
        else:
            mapped[t] = p
    missing = sorted(t for t in mapped if t not in ours)
    extra = sorted(f for f in ours if f not in mapped and f.split("/")[0] not in LOCAL_ONLY)
    def buck(paths, k=3):
        return dict(Counter("/".join(p.split("/")[:k]) for p in paths).most_common(20))
    print(f"upstream={len(up)} mapped={len(mapped)} unmapped={len(unmapped)}")
    print(f"MISSING={len(missing)}  EXTRA={len(extra)}")
    if unmapped:
        print("unmapped:", unmapped[:10])
    print("MISSING by dir:", buck(missing))
    print("EXTRA by dir:", buck(extra))
    for x in missing:
        print("  MISSING", x, "<-", mapped[x])
    return 1 if missing else 0


if __name__ == "__main__":
    raise SystemExit(main())
