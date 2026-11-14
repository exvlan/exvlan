#!/usr/bin/env python3
# ⚠️ legacy：已由 index.ts 取代，保留一个周期（下一个 minor 删除）。
#     新入口：script/kernel-mirror/index.ts（bun）；设计说明：docs/mirror-toolchain.md

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

通道化还带来一个部署兼容问题：发布机看门狗读的是 `<EXVLAN_DIST>/mirror-status.json`
（`core/` 的上一级）与**旧 schema**。所以除了上面的新格式，本脚本还能再写一份
旧 schema 的兼容文件（`--compat-path`），字段与历史上同步脚本写的一致：
`checked_at / round / result / detail / since_tag / latest_release / tags /
first_ok_at / last_change_at / wait_since / bytes{core,app,total,max} / free_gb / window`。
`round` 递增、`first_ok_at`/`last_change_at` 跨轮连续，都靠读上一份兼容文件得出
（不额外落状态文件）；`tags`/`latest` 取 stable 通道。

用法：mirror_status.py --path P --channels-file F [--repo R] [--dest D] [--upgrade-dest U]
                       [--compat-path P] [--result ok|waiting|error] [--detail D]
                       [--since-tag T] [--max-bytes N]
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import shutil
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


def free_gb_of(path: pathlib.Path) -> int:
    """所在分区剩余 GB（兼容文件里的 `free_gb`，拿不到就 0）。"""
    for candidate in (path.parent, path.parent.parent):
        try:
            return int(shutil.disk_usage(str(candidate)).free // (1 << 30))
        except OSError:
            continue
    return 0


def write_compat(
    path: pathlib.Path,
    channels: dict[str, dict[str, object]],
    *,
    result: str,
    detail: str,
    since: str,
    max_bytes: int,
) -> None:
    """写旧 schema 的 `mirror-status.json`（发布机看门狗读的那份）。"""
    prev: dict[str, object] = {}
    if path.is_file():
        try:
            loaded = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(loaded, dict):
                prev = loaded
        except (OSError, ValueError) as exc:
            print(f"[sync] 旧兼容状态无法解析（覆盖）：{exc}", file=sys.stderr)

    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    stable = channels.get("stable", {})
    latest = str(stable.get("latest") or "")
    tags = list(stable.get("tags") or [])  # type: ignore[arg-type]
    app_bytes = sum(int(c.get("upgrade_bytes", 0)) for c in channels.values())
    core_bytes = sum(int(c.get("core_bytes", 0)) for c in channels.values())

    try:
        round_no = int(prev.get("round") or 0) + 1  # type: ignore[arg-type]
    except (TypeError, ValueError):
        round_no = 1

    first_ok = str(prev.get("first_ok_at") or "")
    if result == "ok" and not first_ok:
        first_ok = now
    # 只有 stable 最新版真的变了才算「有变化」，避免每轮都刷新时间
    last_change = str(prev.get("last_change_at") or "")
    if result == "ok" and latest and latest != str(prev.get("latest_release") or ""):
        last_change = now
    wait_since = str(prev.get("wait_since") or "") if result == "waiting" else ""
    if result == "waiting" and not wait_since:
        wait_since = now

    doc = {
        "checked_at": now,
        "round": round_no,
        "result": result,
        "detail": detail or None,
        "since_tag": since or None,
        "latest_release": latest or None,
        "tags": tags,
        "first_ok_at": first_ok or None,
        "last_change_at": last_change or None,
        "wait_since": wait_since or None,
        "bytes": {
            "core": core_bytes,
            "app": app_bytes,
            "total": core_bytes + app_bytes,
            "max": int(max_bytes),
        },
        "free_gb": free_gb_of(path),
        "window": {"core": len(tags), "app": len(tags)},
    }

    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.replace(tmp, path)
    print(f"[sync] 兼容状态已写入 {path}（旧 schema，round={round_no}）")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--path", required=True, type=pathlib.Path)
    parser.add_argument("--channels-file", required=True, type=pathlib.Path)
    parser.add_argument("--repo", default="")
    parser.add_argument("--dest", default="")
    parser.add_argument("--upgrade-dest", default="")
    parser.add_argument("--compat-path", type=pathlib.Path, default=None)
    parser.add_argument("--result", default="ok", choices=("ok", "waiting", "error"))
    parser.add_argument("--detail", default="")
    parser.add_argument("--since-tag", default="")
    parser.add_argument("--max-bytes", type=int, default=0)
    args = parser.parse_args()

    channels: dict[str, dict[str, object]] = {}
    if args.channels_file.is_file():
        channels = read_channels(args.channels_file)
    else:
        print(f"[sync] 状态输入不存在，按空处理：{args.channels_file}", file=sys.stderr)

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

    if args.compat_path is not None:
        if args.detail:
            detail = args.detail
        elif channels:
            detail = "同步完成: " + ",".join(str(t) for t in stable.get("tags") or [])
        else:
            detail = "本轮没有可同步的 release"
        write_compat(
            args.compat_path,
            channels,
            result=args.result,
            detail=detail,
            since=args.since_tag,
            max_bytes=args.max_bytes,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
