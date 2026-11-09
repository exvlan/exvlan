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

# 版本串：`YY.QMM.Mmpp[-channel.n][+build]`（预览版带 prerelease 段）
_VERSION_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-[A-Za-z]+\.(\d+))?(?:\+.*)?$")


def parse_version(version: str) -> tuple[int, int, int, int]:
    """拆出 (major, minor, patch, 预览序号)。

    预览序号用于 Android/Magisk 的 versionCode 末三位；正式版为 0。
    """
    m = _VERSION_RE.match(version)
    if not m:
        sys.exit(f"[upgrade] 无法解析版本 {version}")
    return (
        int(m.group(1)),
        int(m.group(2)),
        int(m.group(3)),
        int(m.group(4) or 0),
    )


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
    # 注意：macOS 的 .app.tar.gz 文件名里没有架构标记（如 "EXVlan Studio.app.tar.gz"），
    # 只能由调用方用 --macos-arch 指定，这里用占位符 {macos_arch}。
    ("darwin-{macos_arch}", re.compile(r"^.*\.app\.tar\.gz$", re.I)),
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
    parser.add_argument(
        "--magisk-changelog",
        default="",
        help="Magisk 更新说明（写入 magisk_update.json）",
    )
    parser.add_argument(
        "--macos-arch",
        default="x86_64",
        choices=["x86_64", "aarch64"],
        help="macOS 更新包对应的架构（产物名里没有架构标记，需显式给出）",
    )
    args = parser.parse_args()

    version = args.version.lstrip("v")
    base_url = args.base_url if args.base_url.endswith("/") else args.base_url + "/"
    files = sorted(p for p in args.assets_dir.iterdir() if p.is_file())

    # GitHub Release 会把资产名里的空格替换成点（"EXVlan Studio_x.dmg" → "EXVlan.Studio_x.dmg"）。
    # 清单必须引用「上传后的名字」，因此这里提前做同样的规范化，保证本地名 == 资产名 == 镜像名。
    for path in list(files):
        if " " not in path.name or not path.is_file():
            continue
        target = path.with_name(path.name.replace(" ", "."))
        path.rename(target)
        if path.with_name(path.name + ".sig").is_file():
            path.with_name(path.name + ".sig").rename(target.with_name(target.name + ".sig"))
        print(f"[upgrade] 规范化文件名：{path.name} -> {target.name}")
    files = sorted(p for p in args.assets_dir.iterdir() if p.is_file())

    # macOS 的 updater 产物默认叫 "EXVlan Studio.app.tar.gz"（不含版本号）。
    # 镜像对 .gz 用了 immutable 缓存，同名覆盖会让客户端拿到旧包 —— 这里补上版本与架构。
    for path in list(files):
        if not path.is_file():
            continue
        if not path.name.lower().endswith(".app.tar.gz") or version in path.name:
            continue
        stem = path.name[: -len(".app.tar.gz")]
        target = path.with_name(f"{stem}_{version}_{args.macos_arch}.app.tar.gz")
        path.rename(target)
        if path.with_name(path.name + ".sig").is_file():
            path.with_name(path.name + ".sig").rename(target.with_name(target.name + ".sig"))
        print(f"[upgrade] 重命名 macOS 更新包：{path.name} -> {target.name}")
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
        target = target.format(macos_arch=args.macos_arch)
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

    # ---- Magisk 在线更新清单（镜像为 /upgrade/magisk_update.json）----
    magisk_zip = find_one(files, re.compile(r"^Exvlan-Magisk-.*\.zip$", re.I))
    if magisk_zip is not None:
        # versionCode 约定：YY | QMM | Mmpp | NNN（NNN = 预览序号，正式版为 000）
        major, minor, patch, pre = parse_version(version)
        version_code = int(f"{major:02d}{minor:03d}{patch:05d}{pre:03d}")
        magisk_update = {
            "version": version,
            "versionCode": version_code,
            "zipUrl": base_url + urllib.parse.quote(magisk_zip.name),
            "changelog": args.magisk_changelog or args.notes,
        }
        (args.assets_dir / "magisk_update.json").write_text(
            json.dumps(magisk_update, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        print(f"[upgrade] Magisk 更新清单：{magisk_zip.name} versionCode={version_code}")

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
