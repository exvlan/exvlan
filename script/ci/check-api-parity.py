#!/usr/bin/env python3
"""控制面契约一致性守卫。

不变量：**gateway 暴露的每个路由，必须同时出现在 `/api/v1/system` 的
`manifest` 里**（CLI 的 `api --list`、Web UI 的能力发现都读它）。
漏一条 = 客户端「看不到」这个能力，属于契约缺陷。

另外校验：CLI 的快捷子命令引用的路径都真实存在（防止改名后 CLI 悄悄失效）。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
API = ROOT / "exvlan/gateway/src/api.rs"
CLI = ROOT / "exvlan/exvlan/src/ctl.rs"

ROUTE_RE = re.compile(r'\.route\(\s*"(/api/v1[^"]*)"')
MANIFEST_RE = re.compile(r'"((?:GET|POST|PUT|PATCH|DELETE)\s+/api/v1[^"]*)"')


def main() -> int:
    if not API.is_file():
        print(f"[api-parity] 找不到 {API}")
        return 2
    src = API.read_text(encoding="utf-8")
    routes = set(ROUTE_RE.findall(src))
    manifest = set()
    for entry in MANIFEST_RE.findall(src):
        method, path = entry.split(None, 1)
        manifest.add(path.strip())

    missing = sorted(r for r in routes if r not in manifest)
    if missing:
        print("[api-parity] 以下路由没有登记到 manifest（`/api/v1/system`）：")
        for path in missing:
            print(f"  {path}")
        return 1

    # CLI 快捷命令里的路径必须存在（允许带 {id} 占位）
    if CLI.is_file():
        cli = CLI.read_text(encoding="utf-8")
        cli_paths = {p.rstrip("/") for p in re.findall(r'"(/api/v1[^"]*)"', cli)}
        known = {r.rstrip("/") for r in routes}
        broken = sorted(p for p in cli_paths if p not in known)
        if broken:
            print("[api-parity] CLI 引用了不存在的路径：")
            for path in broken:
                print(f"  {path}")
            return 1

    print(f"[api-parity] OK：{len(routes)} 条路由全部登记在 manifest")
    return 0


if __name__ == "__main__":
    sys.exit(main())
