#!/usr/bin/env python3
"""禁止在代码里硬编码产品端口。

背景：pre.16 及更早把 RPC 端口 15999 写死在十几处（service / src-tauri / 前端 /
i18n），改端口需要全仓搜索，漏一处就是「连不上 / 端口被占」的线上问题。
pre.17 起端口可配置，且客户端通过 `/api/v1/system.endpoints` 发现，
因此只允许在「默认值定义处」出现字面量。

规则：
1. 代码文件（*.rs / *.ts / *.vue / *.yaml / *.yml / *.json）里，
   禁止出现下列端口字面量，除非文件在白名单里；
2. 白名单文件里也只允许这些端口作为「默认值/兼容解析/诊断探测」出现，
   新增白名单需要在本文件登记并写明理由；
3. 文档（*.md）不受限制 —— 端口表需要写在文档里。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# 只约束「管理面端口」：这些必须来自 service.toml / endpoints 发现。
# peer 监听端口（11010/11011/11012）属于网络配置，由界面模板提供默认值、内核可 :0 自动分配，
# 不在本守卫范围内（否则会和网络编辑器的默认值冲突）。
# 产品端口 + 兼容/历史端口
PORTS = [
    "1995",  # rpc portal（默认）
    "1996",  # exvlan-api REST
    "2016",  # gateway
    "2018",  # exvlan-api config-server
    # 历史/兼容：只允许出现在默认值定义与兼容解析处
    "15999",
    "16000",
    "11211",
    "22020",
]
# 避免把 1995123 之类误判：前后不能紧邻数字
PATTERN = re.compile(r"(?<![0-9])(?:" + "|".join(PORTS) + r")(?![0-9])")

SCAN_SUFFIXES = {".rs", ".ts", ".tsx", ".vue", ".yaml", ".yml", ".json"}

# 只扫描「我方产品代码」：上游内核（exvlan/core|exvlan|proto|api）有它自己的端口默认值，
# 不在这里跟；安装脚本里的 peer 监听端口是用户可见默认值，单独放行。
SCAN_ROOTS = [
    "exvlan/service",
    "exvlan/service-client",
    "exvlan/gateway",
    "exvlan/migrate",
    "packages/studio/src",
    "packages/studio/src-tauri/src",
    "packages/std/src",
]
SKIP_DIRS = {
    "node_modules",
    "target",
    ".git",
    "dist",
    "build",
    "gen",
    "__pycache__",
}
# 测试夹具允许出现端口
ALLOW_NAME_PATTERNS = [
    re.compile(r"(^|/)tests?/"),
    re.compile(r"(^|/)test_[^/]+$"),
    re.compile(r"[^/]+_test\.(rs|ts)$"),
    re.compile(r"[^/]+\.test\.ts$"),
    re.compile(r"[^/]+\.spec\.ts$"),
]

# 白名单：默认值定义、兼容解析、诊断探测、CI 冒烟
ALLOW_FILES = {
    "exvlan/service/src/main.rs": "RPC_PORT / RPC_PORTAL / LOCAL_RPC 默认值 + port.owner 探测",
    "packages/studio/src-tauri/src/lib.rs": "LOCAL_DAEMON_RPC_PORTAL 默认值",
    "packages/studio/src-tauri/src/service_client.rs": "ensure_normal/ensure_service 返回的本地地址",
    "packages/studio/src-tauri/src/supervisor.rs": "Linux core 模式启动内核的 --rpc-portal",
    "packages/studio/src-tauri/src/upgrade.rs": "升级提示里的本地地址",
    "packages/studio/src-tauri/src/diagnostics.rs": "端口探测正则（含历史端口，便于排障）",
    "packages/studio/src/pages/index.vue": "LOCAL_RPC_PORTAL 默认值",
    "packages/studio/src/composables/target.ts": "DEFAULT_RPC_PORT 默认值 + 兼容粘贴的旧端口",
    "packages/studio/src/components/ServiceSettings.vue": "重装服务时的 rpc_portal 默认值",
    "packages/studio/src/components/SettingsDialog.vue": "重装服务时的 rpc_portal 默认值",
    "packages/std/src/locales/cn.yaml": "端口文案（不含字面量时也不报错）",
    "packages/std/src/locales/en.yaml": "端口文案（不含字面量时也不报错）",
    "exvlan/rpc/src/rpc.rs": "URL 解析单测夹具",
    "exvlan/service-client/src/lib.rs": "DEFAULT_RPC_PORT / DEFAULT_GATEWAY_PORT 默认值",
    "packages/studio/src-tauri/src/gateway_remote.rs": "DEFAULT_GATEWAY_PORT 默认值（对端可在 service.toml 改，配对成功后由用户覆盖）",
}


def is_allowed(name: str) -> bool:
    if name in ALLOW_FILES:
        return True
    return any(p.search(name) for p in ALLOW_NAME_PATTERNS)


def main() -> int:
    hits: list[tuple[str, int, str]] = []
    for path in ROOT.rglob("*"):
        if not path.is_file() or path.suffix not in SCAN_SUFFIXES:
            continue
        rel = path.relative_to(ROOT).as_posix()
        if not any(rel == root or rel.startswith(root + "/") for root in SCAN_ROOTS):
            continue
        if any(part in SKIP_DIRS for part in rel.split("/")):
            continue
        if rel.endswith("auto-imports.d.ts"):
            continue
        if is_allowed(rel):
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        for lineno, line in enumerate(text.splitlines(), 1):
            stripped = line.strip()
            # 注释里写端口是合理的（文档/说明），只约束**代码**。
            if stripped.startswith(("//", "#", "*", "/*")):
                continue
            m = PATTERN.search(line)
            if m:
                hits.append((rel, lineno, stripped[:120]))

    if not hits:
        print(f"[ports] OK：{len(ALLOW_FILES)} 个白名单之外没有硬编码端口")
        return 0

    print("[ports] 发现硬编码端口（应改为常量 / service.toml / endpoints 发现）：")
    for rel, lineno, line in hits[:40]:
        print(f"  {rel}:{lineno}: {line}")
    if len(hits) > 40:
        print(f"  … 共 {len(hits)} 处")
    print("\n若确属「默认值定义 / 兼容解析 / 诊断探测」，请登记到 script/ci/check-no-hardcoded-ports.py 的 ALLOW_FILES 并写明理由。")
    return 1


if __name__ == "__main__":
    sys.exit(main())
