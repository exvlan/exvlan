#!/usr/bin/env python3
# ⚠️ legacy：已由 index.ts 取代，保留一个周期（下一个 minor 删除）。
#     新入口：script/kernel-mirror/index.ts（bun）；设计说明：docs/mirror-toolchain.md

"""把清单里的绝对 URL 改写到**通道目录**。

`upgrade-latest.json`（镜像为 `<channel>/latest.json`）与 `magisk_update.json`
由 `script/gen_upgrade_manifests.py` 生成，里面的下载地址是**绝对 URL**（Tauri
updater 只接受 URL，不能给相对文件名）。镜像改成按通道存放后，这些 URL 必须指向
自己的通道目录，否则老客户端拿到 `/upgrade/latest.json`（指向 stable 的符号链接）
也会去根目录下载 —— 那里已经没有文件了。

规则：保留原 URL 的 origin，路径一律重建成 `<root>/<channel>/<文件名>`。
只动 URL 字段，其它字段原样保留；重复执行是幂等的（不会叠加通道目录）。

用法：channelize_urls.py <清单.json> <channel> [--root /upgrade]
"""

from __future__ import annotations

import argparse
import json
import os
import posixpath
import pathlib
import sys
import urllib.parse

# 需要改写的字段：官方 updater 的 `platforms.*.url`、Magisk 的 `zipUrl`。
URL_FIELDS = ("url", "zipUrl")


def channelize(url: str, channel: str, root: str) -> tuple[str, bool]:
    """把 `https://host/upgrade/<任意>/<file>` 重写成 `https://host/upgrade/<channel>/<file>`。"""
    parts = urllib.parse.urlsplit(url)
    if not parts.scheme or not parts.netloc:
        return url, False
    name = posixpath.basename(urllib.parse.unquote(parts.path))
    if not name:
        return url, False
    path = f"{root.rstrip('/')}/{channel}/{urllib.parse.quote(name)}"
    return urllib.parse.urlunsplit((parts.scheme, parts.netloc, path, "", "")), True


def rewrite(obj: object, channel: str, root: str) -> int:
    if not isinstance(obj, dict):
        return 0
    changed = 0
    platforms = obj.get("platforms")
    if isinstance(platforms, dict):
        for entry in platforms.values():
            if not isinstance(entry, dict):
                continue
            for field in URL_FIELDS:
                value = entry.get(field)
                if isinstance(value, str) and value:
                    entry[field], hit = channelize(value, channel, root)
                    changed += hit
    for field in URL_FIELDS:
        value = obj.get(field)
        if isinstance(value, str) and value:
            obj[field], hit = channelize(value, channel, root)
            changed += hit
    return changed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("path", type=pathlib.Path, help="要改写的 JSON 清单")
    parser.add_argument("channel", help="通道目录名（stable/rc/beta/dev/nightly）")
    parser.add_argument("--root", default="/upgrade", help="URL 根路径（默认 /upgrade）")
    args = parser.parse_args()

    try:
        data = json.loads(args.path.read_text(encoding="utf-8"))
    except OSError as exc:
        print(f"[sync] 读不到 {args.path}: {exc}", file=sys.stderr)
        return 1
    except ValueError as exc:
        print(f"[sync] 解析 {args.path} 失败: {exc}", file=sys.stderr)
        return 1

    changed = rewrite(data, args.channel, args.root)
    tmp = args.path.with_name(args.path.name + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.replace(tmp, args.path)

    if changed == 0:
        # 没有 URL 字段（例如只有自建清单）属正常，不当作失败。
        print(f"[sync] URL 通道化：{args.path.name} 没有绝对 URL，保持原样")
        return 0
    print(f"[sync] URL 通道化：{args.path.name} 的 {changed} 条地址 → {args.channel}/")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
