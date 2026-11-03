#!/usr/bin/env python3
"""把合并后的内核 manifest 裁剪到「镜像实际托管」的平台。

背景：Release 自带的 manifest.json 包含全部 14 个平台，而镜像脚本只下载
`PLATFORMS`（默认 6 个 GUI 平台）。不裁剪的话，GUI 会看到「有更新」，
但下载对应 zip 时 404。

用法：filter_manifest_platforms.py <manifest.json> <platform> [platform...]
"""

from __future__ import annotations

import json
import sys


def main() -> int:
    if len(sys.argv) < 3:
        print("usage: filter_manifest_platforms.py <manifest.json> <platform>...", file=sys.stderr)
        return 2

    path, platforms = sys.argv[1], set(sys.argv[2:])
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)

    kept = []
    for version in data.get("versions", []):
        assets = version.get("assets", {})
        filtered = {k: v for k, v in assets.items() if k in platforms}
        if filtered:
            version["assets"] = filtered
            kept.append(version)
        elif assets:
            print(
                f"[sync] {version.get('version')} 只有未镜像平台，已从清单剔除",
                file=sys.stderr,
            )

    data["versions"] = kept
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2, ensure_ascii=False)
        fh.write("\n")
    print(f"[sync] 清单平台过滤完成：保留 {len(kept)} 个版本，平台 {sorted(platforms)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
