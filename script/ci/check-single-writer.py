#!/usr/bin/env python3
"""单一写者守卫：关键状态字段只允许从「既定的一处」写入。

背景：pre.13–pre.16 的阻断问题里，一半是「同一个概念有多个写入点」
（`runtime` 在前端 prefs、服务 `service.json`、legacy `app_mode` 三处并存；
`remotes` 同时被 localStorage 与后端写）。修一次、漂一次，永远对不齐。

做法：对每个关键 key 统计写入点数量，与基线比较；**只允许减少，不允许增加**。
如果确实需要新增写入点，必须同时修改本文件的 BASELINE 并说明理由（review 时可见）。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

SCAN_ROOTS = [
    "exvlan/service",
    "exvlan/control",
    "exvlan/gateway",
    "packages/studio/src",
    "packages/studio/src-tauri/src",
    "packages/std/src",
]
SKIP_DIRS = {"node_modules", "target", "dist", "build", "gen", "__pycache__"}

KEYS = [
    "runtime",
    "policy",
    "remotes",
    "target",
    "app_mode",
    "lang",
    "log_level",
    "last_network_instance_id",
]

# 基线（当前实现的写入点数量）。只允许减少。
BASELINE = {
    "runtime": 0,
    "policy": 0,
    "remotes": 0,
    "target": 0,
    "app_mode": 3,
    "lang": 4,
    "log_level": 1,
    "last_network_instance_id": 3,
}

WRITE_PATTERNS = {
    "ts": [
        r'setItem\(\s*["\']{key}["\']',
        r'persistUiStateNow\(\s*["\']{key}["\']',
        r'removeItem\(\s*["\']{key}["\']',
    ],
    "rs": [
        r'set_key\(\s*&mut\s*prefs,\s*"{key}"',
        r'set_ui_state\([^,]+,\s*"{key}"',
        r'misc\.insert\(\s*"{key}"',
        r'"mode"|"policy"' if False else r'(?!)',
    ],
}


def count_hits(text: str, suffix: str, key: str) -> int:
    total = 0
    for pat in WRITE_PATTERNS["ts" if suffix in {".ts", ".vue"} else "rs"]:
        total += len(re.findall(pat.format(key=re.escape(key)), text))
    return total


def main() -> int:
    print_mode = "--print" in sys.argv
    counts = {k: 0 for k in KEYS}
    for root in SCAN_ROOTS:
        base = ROOT / root
        if not base.exists():
            continue
        for path in base.rglob("*"):
            if not path.is_file() or path.suffix not in {".ts", ".vue", ".rs"}:
                continue
            rel = path.relative_to(ROOT).as_posix()
            if any(part in SKIP_DIRS for part in rel.split("/")):
                continue
            if rel.endswith("auto-imports.d.ts"):
                continue
            try:
                text = path.read_text(encoding="utf-8")
            except (UnicodeDecodeError, OSError):
                continue
            for key in KEYS:
                counts[key] += count_hits(text, path.suffix, key)

    if print_mode:
        for key in KEYS:
            print(f"{key}: {counts[key]} (baseline {BASELINE.get(key, 0)})")
        return 0

    bad = [k for k in KEYS if counts[k] > BASELINE.get(k, 0)]
    if bad:
        print("[single-writer] 关键字段出现了新的写入点（应保持单一权威）：")
        for k in bad:
            print(f"  {k}: {counts[k]} > baseline {BASELINE[k]}")
        print("若确有必要，请修改 script/ci/check-single-writer.py 的 BASELINE 并在提交信息里说明。")
        return 1
    print(f"[single-writer] OK（{len(KEYS)} 个关键字段写入点未增加）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
