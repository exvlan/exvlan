#!/usr/bin/env python3
"""生成应用更新清单（在 release.yml 里对打包后的资产运行）。

产出两个文件（都随 Release 上传，由镜像脚本搬到 `https://dl.exvlan.era-x.cn/upgrade/`）：

- `upgrade-manifest.json` —— **自建回退**格式，GUI 的 `upgrade::app` 直接消费：
  `{schema, latest, versions[{version, assets{<platform>: {file, sha256}}}]}`
- `upgrade-latest.json` —— **官方 tauri-plugin-updater** 格式（镜像为 `latest.json`）：
  `{version, notes, pub_date, platforms{<target>: {signature, url}}}`

签名来自 Tauri 在 `createUpdaterArtifacts=true` 时生成的 `<artifact>.sig`（minisign）。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import sys
import urllib.parse
from datetime import datetime, timezone

# 自建 manifest 的平台键 → 安装包匹配规则（GUI 的 platform_key() 命名）
APP_PLATFORM_RULES: list[tuple[str, re.Pattern[str]]] = [
    ("macos-x86_64", re.compile(r"^.*_x64.*\.dmg$", re.I)),
    ("macos-aarch64", re.compile(r"^.*(aarch64|arm64).*\.dmg$", re.I)),
    ("windows-x86_64", re.compile(r"^.*_x64.*\.msi$", re.I)),
    ("windows-arm64", re.compile(r"^.*(aarch64|arm64).*\.msi$", re.I)),
    # tauri 用 rust target 的 arch 命名安装包：aarch64→arm64、armv7→arm、i686→x86
    ("android-arm64-v8a", re.compile(r"^app-arm64-release\.apk$", re.I)),
    ("android-armeabi-v7a", re.compile(r"^app-arm-release\.apk$", re.I)),
    ("android-x86_64", re.compile(r"^app-x86_64-release\.apk$", re.I)),
    ("android-x86", re.compile(r"^app-x86-release\.apk$", re.I)),
]

# 官方 updater 的平台键（tauri 的 target triple 简写）→ 更新包匹配规则
UPDATER_TARGET_RULES: list[tuple[str, re.Pattern[str]]] = [
    ("darwin-x86_64", re.compile(r"^.*_x64.*\.app\.tar\.gz$", re.I)),
    ("darwin-aarch64", re.compile(r"^.*(aarch64|arm64).*\.app\.tar\.gz$", re.I)),
    ("windows-x86_64", re.compile(r"^.*_x64.*setup\.exe$", re.I)),
    ("windows-arm64", re.compile(r"^.*(aarch64|arm64).*setup\.exe$", re.I)),
    ("linux-x86_64", re.compile(r"^.*_amd64.*\.AppImage$", re.I)),
    ("linux-aarch64", re.compile(r"^.*(aarch64|arm64).*\.AppImage$", re.I)),
]


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def find_one(files: list[pathlib.Path], pattern: re.Pattern[str]) -> pathlib.Path | None:
    matched = [f for f in files if pattern.match(f.name)]
    if len(matched) > 1:
        names = ", ".join(sorted(f.name for f in matched))
        sys.exit(f"[upgrade] 规则 {pattern.pattern} 命中多个文件：{names}")
    return matched[0] if matched else None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--assets-dir", required=True, type=pathlib.Path)
    parser.add_argument("--version", required=True, help="不带 v 前缀，例如 26.410.20706")
    parser.add_argument(
        "--base-url",
        default="https://dl.exvlan.era-x.cn/upgrade/",
        help="更新包对外下载前缀（需以 / 结尾）",
    )
    parser.add_argument("--notes", default="")
    args = parser.parse_args()

    version = args.version.lstrip("v")
    base_url = args.base_url if args.base_url.endswith("/") else args.base_url + "/"
    files = sorted(p for p in args.assets_dir.iterdir() if p.is_file())

    # ---- 自建 manifest ----
    assets: dict[str, dict[str, str]] = {}
    for platform, pattern in APP_PLATFORM_RULES:
        hit = find_one(files, pattern)
        if hit is None:
            continue
        assets[platform] = {"file": hit.name, "sha256": sha256(hit)}
    if not assets:
        sys.exit("[upgrade] 没有匹配到任何安装包，无法生成应用更新清单")

    manifest = {
        "schema": 1,
        "latest": version,
        "versions": [{"version": version, "assets": assets}],
    }

    # ---- 官方 updater latest.json ----
    platforms: dict[str, dict[str, str]] = {}
    missing_sig: list[str] = []
    for target, pattern in UPDATER_TARGET_RULES:
        hit = find_one(files, pattern)
        if hit is None:
            continue
        sig_path = hit.with_name(hit.name + ".sig")
        if not sig_path.is_file():
            missing_sig.append(hit.name)
            continue
        platforms[target] = {
            "signature": sig_path.read_text(encoding="utf-8").strip(),
            "url": base_url + urllib.parse.quote(hit.name),
        }
    if missing_sig:
        sys.exit(f"[upgrade] 以下更新包缺少 .sig（createUpdaterArtifacts 未生效？）：{missing_sig}")

    latest = {
        "version": version,
        "notes": args.notes,
        "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }

    out_dir = args.assets_dir
    (out_dir / "upgrade-manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    (out_dir / "upgrade-latest.json").write_text(
        json.dumps(latest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )

    print(f"[upgrade] 自建 manifest：{len(assets)} 个平台 {sorted(assets)}")
    if platforms:
        print(f"[upgrade] 官方 latest.json：{len(platforms)} 个平台 {sorted(platforms)}")
    else:
        print("[upgrade] 警告：没有任何签名更新包，latest.json 的 platforms 为空")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
