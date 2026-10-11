#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Fork 一致性守卫：
  A. WASI ABI 六面一致（core abi/imports/runtime + go + js + 预构建 wasm 符号全为 exvlan_*，无 easytier_ 残留）
  B. 功能对齐抽查（各 crate 的 [features] 键集合 vs 上游 v2.7.0）

用法: python3 check_fork_consistency.py [--upstream upstream/releases/v2.7.0]
退出码: 0 全部通过；1 存在不一致。
"""
from __future__ import annotations
import argparse, re, subprocess, sys, os

ROOT = subprocess.run(["git", "rev-parse", "--show-toplevel"],
                      capture_output=True, text=True).stdout.strip()
os.chdir(ROOT)

ABI_RE = re.compile(r"(easytier|exvlan)_(host|buffer_alloc|buffer_free|instance_create|"
                    r"instance_start|instance_stop|instance_drive|instance_state|"
                    r"web_client_create|rpc_request_submit|rpc_response_take|"
                    r"host_tunnel_abi_version|data_plane_)")
ABI_SCAN_GLOBS = [
    "exvlan/core/src/wasi", "exvlan/go/internal", "packages/sdk/runtime",
]
FEATURE_MAP = [("exvlan/core/Cargo.toml", "easytier-core/Cargo.toml"),
               ("exvlan/exvlan/Cargo.toml", "easytier/Cargo.toml"),
               ("exvlan/proto/Cargo.toml", "easytier-proto/Cargo.toml")]


def sh(*a):
    return subprocess.run(a, capture_output=True, text=True).stdout


def files_under(path):
    out = sh("git", "ls-files", path)
    return out.splitlines()


def features_of(text: str) -> set[str]:
    m = re.search(r"(?ms)^\[features\]\s*(.*?)(^\[|\Z)", text)
    if not m:
        return set()
    return set(re.findall(r"(?m)^([A-Za-z0-9_-]+)\s*=", m.group(1)))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--upstream", default="upstream/releases/v2.7.0")
    a = ap.parse_args()
    fail = 0

    # ---- A. WASI ABI 一致 ----
    print("== A. WASI ABI 六面一致 ==")
    bad = []
    for g in ABI_SCAN_GLOBS:
        for f in files_under(g):
            if not f.endswith((".rs", ".ts", ".mjs", ".go")):
                continue
            for i, line in enumerate(open(f, encoding="utf-8", errors="surrogateescape"), 1):
                for m in ABI_RE.finditer(line):
                    if m.group(1) == "easytier":
                        bad.append(f"{f}:{i}: {m.group(0)}")
    # wasm 符号
    art = "exvlan/go/internal/artifact/exvlan_core.wasm"
    if os.path.exists(art):
        data = open(art, "rb").read()
        for name in (b"easytier_instance_create", b"easytier_buffer_alloc",
                     b"easytier_host_tunnel_abi_version"):
            if name in data:
                bad.append(f"{art}: 含旧符号 {name.decode()}")
        if b"exvlan_instance_create" not in data:
            bad.append(f"{art}: 缺 exvlan_instance_create 导出")
    if bad:
        fail = 1
        print("  ❌ 不一致：")
        for b in bad[:50]:
            print("    ", b)
    else:
        print("  ✅ 六面一致（全 exvlan_*）")

    # ---- B. 功能对齐抽查 ----
    print("== B. 功能对齐（features 键 vs 上游）==")
    for ours, up in FEATURE_MAP:
        if not os.path.exists(ours):
            continue
        ft = sh("git", "show", f"{a.upstream}:{up}")
        if not ft:
            print(f"  (跳过 {up}: 上游不存在)")
            continue
        fo = features_of(open(ours, encoding="utf-8").read())
        fu = features_of(ft)
        missing = sorted(fu - fo)
        extra = sorted(fo - fu)
        status = "✅" if not missing else "❌"
        if missing:
            fail = 1
        print(f"  {status} {ours}: 上游 {len(fu)} / 我方 {len(fo)}; missing={missing[:8]} extra={extra[:8]}")

    print("--")
    print("✅ 通过" if not fail else "❌ 存在不一致/缺功能")
    return fail


if __name__ == "__main__":
    raise SystemExit(main())
