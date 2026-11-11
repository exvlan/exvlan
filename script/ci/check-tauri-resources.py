#!/usr/bin/env python3
"""校验「平台覆盖文件没有丢掉基础配置里的 bundle.resources」。

背景（pre.13 真实事故）：Tauri 的 `tauri.<platform>.conf.json` 对 `bundle.resources`
是**整体替换**而不是合并。基础配置加了 `resources/service/*`，Windows 覆盖文件
没跟上 → MSI/NSIS 里根本没有 `exvlan-service.exe` → 安装直接失败，
而 CI 只校验了构建机上的 staging 目录，完全没拦住。

规则：
  · 基础 `tauri.conf.json` 里 `bundle.resources` 的每一项，
    都必须出现在所有声明了 `bundle.resources` 的平台覆盖文件里；
  · 覆盖文件可以有额外项（`wintun.dll` 这类平台专属资源）。

用法：python3 script/ci/check-tauri-resources.py
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CONF_DIR = ROOT / "packages" / "studio" / "src-tauri"


def load_resources(path: Path) -> list[str] | None:
    data = json.loads(path.read_text(encoding="utf-8"))
    resources = data.get("bundle", {}).get("resources")
    if resources is None:
        return None
    if not isinstance(resources, list):
        sys.exit(f"[resources] {path.name} 的 bundle.resources 不是数组")
    return [str(item) for item in resources]


def main() -> int:
    base_path = CONF_DIR / "tauri.conf.json"
    base = load_resources(base_path)
    if base is None:
        sys.exit("[resources] 基础配置缺少 bundle.resources")

    overrides = sorted(CONF_DIR.glob("tauri.*.conf.json"))
    if not overrides:
        print("[resources] 没有平台覆盖文件，跳过")
        return 0

    failed = False
    for path in overrides:
        override = load_resources(path)
        if override is None:
            # 不声明就继承基础配置，没问题
            print(f"[resources] {path.name}: 未声明 resources，继承基础配置")
            continue
        missing = [item for item in base if item not in override]
        if missing:
            failed = True
            print(
                f"::error::{path.name} 的 bundle.resources 会整体替换基础配置，"
                f"但缺少: {', '.join(missing)}"
            )
        else:
            extra = [item for item in override if item not in base]
            note = f"（额外: {', '.join(extra)}）" if extra else ""
            print(f"[resources] {path.name}: 覆盖基础配置的 {len(base)} 项齐全{note}")

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
