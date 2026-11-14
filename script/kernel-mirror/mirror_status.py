#!/usr/bin/env python3
"""写 `mirror-status.json`：按通道汇报镜像状态。

通道化后镜像有多个子目录，监控需要知道每条线各自的最新版本与占用：

    {
      "updated": "...",
      "repo": "...", "dest": "...", "upgrade_dest": "...",
      "latest": "<stable 最新>", "versions": ["<stable tag>", ...],
      "bytes": 123, "core_bytes": 100, "upgrade_bytes": 23,
      "channels": {"stable": {"latest": "...", "tags": [...], "bytes": 123}, ...}
    }

顶层 `latest` / `versions` / `bytes` 仍是 **stable** 的汇总值（旧监控直接读这几项）；
`channels.*.bytes` 是该通道 core + upgrade 两个目录的占用。

**保留既有顶层字段**：文件已存在时先读出来，只覆盖本脚本负责的键，
发布机看门狗写的其它字段（`max_bytes` / `disk_free` 之类）原样保留。

用法：mirror_status.py --path P --channels-file F [--repo R] [--dest D] [--upgrade-dest U]
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import sys
from datetime import datetime, timezone

# 通道清单文件的一行：`channel \t latest \t core_bytes \t upgrade_bytes \t tag[,tag...]`
_SPLIT = re.compile(r"[,\s]+")


def read_channels(path: pathlib.Path) -> dict[str, dict[str, object]]:
    channels: dict[str, dict[str, object]] = {}
    for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) < 1 or not parts[0]:
            print(f"[sync] 状态输入第 {lineno} 行不规范，跳过：{line!r}", file=sys.stderr)
            continue
        padded = parts + [""] * (5 - len(parts))
        name, latest, core_bytes, upgrade_bytes, tags = padded[:5]
        try:
            core = int(core_bytes or 0)
            upgrade = int(upgrade_bytes or 0)
        except ValueError:
            print(f"[sync] 状态输入第 {lineno} 行字节数非数字，按 0 处理：{line!r}", file=sys.stderr)
            core = upgrade = 0
        channels[name] = {
            "latest": latest,
            "tags": [t for t in _SPLIT.split(tags.strip()) if t],
            "bytes": core + upgrade,
            "core_bytes": core,
            "upgrade_bytes": upgrade,
        }
    return channels


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--path", required=True, type=pathlib.Path)
    parser.add_argument("--channels-file", required=True, type=pathlib.Path)
    parser.add_argument("--repo", default="")
    parser.add_argument("--dest", default="")
    parser.add_argument("--upgrade-dest", default="")
    args = parser.parse_args()

    channels = read_channels(args.channels_file)

    data: dict[str, object] = {}
    if args.path.is_file():
        try:
            loaded = json.loads(args.path.read_text(encoding="utf-8"))
            if isinstance(loaded, dict):
                data = loaded
            else:
                print(f"[sync] 旧状态文件不是对象，覆盖：{args.path}", file=sys.stderr)
        except (OSError, ValueError) as exc:
            print(f"[sync] 旧状态文件无法解析（覆盖）：{exc}", file=sys.stderr)

    stable = channels.get("stable", {})
    core_bytes = sum(int(c.get("core_bytes", 0)) for c in channels.values())
    upgrade_bytes = sum(int(c.get("upgrade_bytes", 0)) for c in channels.values())

    data.update(
        {
            "updated": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "repo": args.repo,
            "dest": args.dest,
            "upgrade_dest": args.upgrade_dest,
            # 兼容旧监控：顶层仍是 stable 的汇总
            "latest": stable.get("latest", ""),
            "versions": stable.get("tags", []),
            "bytes": core_bytes + upgrade_bytes,
            "core_bytes": core_bytes,
            "upgrade_bytes": upgrade_bytes,
            "channels": channels,
        }
    )

    args.path.parent.mkdir(parents=True, exist_ok=True)
    tmp = args.path.with_name(args.path.name + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.replace(tmp, args.path)
    print(f"[sync] 状态已写入 {args.path}（{len(channels)} 个通道）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
