#!/usr/bin/env python3
"""禁止「静默失败」：错误必须被记录或显式忽略并说明原因。

背景（pre.13–pre.16 的教训）：三类问题都源于错误被吞掉 ——
1. RPC portal 拒绝连接只打 `warn`，被 console 级别过滤掉（排查两天）；
2. 前端写 localStorage 失败被静默吞掉，表现为「保存了但读不回来」；
3. 服务里 `let _ = ...` 吞掉盘 IO 错误。

规则（只看我方产品代码，不打扰上游内核）：
- TS/Vue：空的 `catch {}` / `catch (e) {}` → 违规（必须至少 `console.warn` 或上报）
- Rust：`if let Err(_) =` / `Err(_) => {}` → 违规（必须记录或写明 `// ignore:`）
- 允许显式标注：同一行或上一行含 `// ignore-error:` 时跳过（用于真正可忽略的场景）
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
    "exvlan/migrate",
    "packages/studio/src",
    "packages/studio/src-tauri/src",
    "packages/std/src",
]
SKIP_DIRS = {"node_modules", "target", "dist", "build", "gen", "__pycache__"}

TS_EMPTY_CATCH = re.compile(r"catch\s*(?:\([^)]*\))?\s*\{\s*\}", re.S)
RS_ERR_IGNORE = re.compile(r"if\s+let\s+Err\(_\)|Err\(_\)\s*=>\s*\{\s*\}")


def allowed(path: str, line_no: int, lines: list[str]) -> bool:
    for idx in (line_no - 1, line_no - 2):
        if 0 <= idx < len(lines) and "ignore-error:" in lines[idx]:
            return True
    return False


def main() -> int:
    hits: list[tuple[str, int, str]] = []
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
            if "/tests/" in rel or rel.endswith("_test.rs") or ".test.ts" in rel:
                continue
            try:
                text = path.read_text(encoding="utf-8")
            except (UnicodeDecodeError, OSError):
                continue
            lines = text.splitlines()

            if path.suffix in {".ts", ".vue"}:
                for m in TS_EMPTY_CATCH.finditer(text):
                    line_no = text[: m.start()].count("\n") + 1
                    if allowed(rel, line_no, lines):
                        continue
                    hits.append((rel, line_no, "空的 catch 块（错误被吞掉）"))
            else:
                for i, line in enumerate(lines, 1):
                    if RS_ERR_IGNORE.search(line) and not allowed(rel, i, lines):
                        hits.append((rel, i, line.strip()[:100]))

    if not hits:
        print("[silent-error] OK：没有吞错的写法")
        return 0

    print("[silent-error] 发现静默失败（请记录错误，或加 `// ignore-error: <原因>`）：")
    for rel, line_no, text in hits[:40]:
        print(f"  {rel}:{line_no}: {text}")
    if len(hits) > 40:
        print(f"  … 共 {len(hits)} 处")
    return 1


if __name__ == "__main__":
    sys.exit(main())
