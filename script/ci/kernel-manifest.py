#!/usr/bin/env python3
"""汇总内核包 sha256，产出 nightly release 的 `manifest.json`。

结构与 `release.yml` 的「Generate kernel manifest」步骤一致：

    {schema, channel, latest, versions[{version, rpc_schema, min_studio_version,
                                        assets{<platform>: {file, sha256, entry}}}]}

镜像脚本（`script/kernel-mirror/merge_manifests.py`）按这个结构合并通道，
客户端据此判断版本并校验完整性（sha256 针对下载到的 zip 本体）。

与 release.yml 的关系：正式发版那条路径本轮不动（避免动到已验证的发布语义），
nightly 只是它的**子集**（6 个镜像平台，不带 magisk/dmg/msi）。字段必须保持一致，
改动时请同时看 `release.yml` 的同名步骤。

额外硬校验（正式发版步骤里没有，nightly 需要）：
  · 每个平台的 zip 里必须同时有内核本体与 `<entry>.sig` —— 缺签名的内核包
    服务端 `kernel.install.*` 会以 `sig_invalid` 拒绝，等于白发。

用法：
    python3 script/ci/kernel-manifest.py --assets-dir zipped_assets \\
        --version 26.410.20713-nightly.1115 --channel nightly
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys
import zipfile

# 镜像托管的内核平台（与 script/kernel-mirror/exvlan-kernel-sync.sh 的 PLATFORMS 一致）
DEFAULT_PLATFORMS = [
    "macos-x86_64",
    "macos-aarch64",
    "windows-x86_64",
    "windows-arm64",
    "linux-x86_64",
    "linux-aarch64",
]


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--assets-dir", required=True, type=pathlib.Path)
    parser.add_argument("--version", required=True, help="nightly 版本串（不带 v 前缀）")
    parser.add_argument("--channel", default="nightly")
    parser.add_argument(
        "--compat",
        type=pathlib.Path,
        default=None,
        help="RPC 契约/最低 GUI 基线（默认 script/ci/kernel-compat.json）",
    )
    args = parser.parse_args()

    version = args.version.lstrip("v")
    compat_path = args.compat or pathlib.Path(__file__).with_name("kernel-compat.json")
    compat = json.loads(compat_path.read_text(encoding="utf-8"))

    assets: dict[str, dict[str, str]] = {}
    for platform in DEFAULT_PLATFORMS:
        zip_path = args.assets_dir / f"{platform}-{version}.zip"
        if not zip_path.is_file():
            sys.exit(f"[kernel-manifest] 缺少内核包 {zip_path}")

        with zipfile.ZipFile(zip_path) as zf:
            names = set(zf.namelist())
        entry = "exvlan.exe" if "exvlan.exe" in names else "exvlan"
        if entry not in names:
            sys.exit(f"[kernel-manifest] {zip_path} 里没有内核本体 {entry}")
        if f"{entry}.sig" not in names:
            sys.exit(
                f"[kernel-manifest] {zip_path} 里没有 {entry}.sig（minisign），"
                "服务端会以 sig_invalid 拒绝这个包"
            )

        assets[platform] = {
            "file": zip_path.name,
            "sha256": sha256(zip_path),
            "entry": entry,
        }

    manifest = {
        "schema": 1,
        "channel": args.channel,
        "latest": version,
        "versions": [
            {
                "version": version,
                "rpc_schema": int(compat["rpc_schema"]),
                "min_studio_version": str(compat["min_studio_version"]),
                "assets": assets,
            }
        ],
    }
    out = args.assets_dir / "manifest.json"
    out.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"[kernel-manifest] {len(assets)} 个平台已写入 {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
