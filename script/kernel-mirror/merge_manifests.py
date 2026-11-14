"""合并多个内核/应用 manifest 的 `versions[]`，产出单个清单。

用法：merge_manifests.py [--keep N] [--since TAG] <manifest1:manifest2:...> <输出.json>

镜像按通道调用：一个通道内只会传入该通道各版本的清单，**新下载的在前、通道现有
清单在后**（同一版本以先出现的为准，这样重传过的资产用新的 sha256）。

`--keep N` 只保留最近的 N 个版本（通道保留策略）；不传则全部保留。
`--since TAG` 丢掉早于 TAG 的版本（镜像 `SINCE_TAG`，只对 stable 使用）。

排序键与客户端 `upgrade::Version` 一致：`(base, stage, seq)`，其中 stage 让
stable > rc > beta > dev > nightly > legacy pre。
"""

import json
import pathlib
import re
import sys

_VERSION_RE = re.compile(r"^v?(\d+)\.(\d+)\.(\d+)(?:-([A-Za-z]+)\.(\d+))?(?:\+.*)?$")
# 与 packages/studio/src-tauri/src/upgrade.rs 的 Channel::compare_stage 对齐
_STAGE_RANK = {"stable": 5, "rc": 4, "beta": 3, "dev": 2, "nightly": 1}


def key(version):
    """`26.410.20712-rc.1` → (26, 410, 20712, 4, 1)。

    旧实现按 `.` 切分后丢掉非纯数字段，`26.410.20712-rc.1` 会退化成
    `[26, 410, 1]`，预发布版本之间无法比较先后。
    """
    match = _VERSION_RE.match(version.strip())
    if not match:
        return (0, 0, 0, 0, 0)
    major, minor, patch, stage, seq = match.groups()
    return (
        int(major),
        int(minor),
        int(patch),
        _STAGE_RANK.get((stage or "stable").lower(), 0),
        int(seq or 0),
    )


def main() -> int:
    argv = sys.argv[1:]
    keep = 0
    since = ""
    while len(argv) >= 2 and argv[0] in ("--keep", "--since"):
        if argv[0] == "--keep":
            try:
                keep = int(argv[1])
            except ValueError:
                print(f"[sync] --keep 需要整数，收到 {argv[1]!r}", file=sys.stderr)
                return 2
        else:
            since = argv[1].strip()
        argv = argv[2:]
    if len(argv) < 2:
        print("usage: merge_manifests.py [--keep N] <manifest:...> <out.json>", file=sys.stderr)
        return 2

    paths = [p for p in argv[0].split(":") if p]
    out = pathlib.Path(argv[1])

    versions = {}
    channel = ""
    for p in paths:
        try:
            data = json.loads(pathlib.Path(p).read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            print(f"[sync] 跳过损坏的 manifest {p}: {exc}", file=sys.stderr)
            continue
        if not channel and isinstance(data.get("channel"), str):
            # 通道字段由发版流水线写入（排查用），合并时保留
            channel = data["channel"]
        for entry in data.get("versions", []):
            name = entry.get("version")
            if not name:
                continue
            # 同一版本以先出现的为准（新下载的清单在前）
            versions.setdefault(name, entry)

    if not versions:
        print("[sync] manifest 里没有任何版本", file=sys.stderr)
        return 1

    ordered = sorted(versions, key=key, reverse=True)
    if since:
        floor = key(since)
        dropped = [v for v in ordered if key(v) < floor]
        ordered = [v for v in ordered if key(v) >= floor]
        for name in dropped:
            print(f"[sync] 早于 SINCE_TAG={since}，移出清单：{name}", file=sys.stderr)
    if keep > 0:
        dropped = ordered[keep:]
        ordered = ordered[:keep]
        for name in dropped:
            print(f"[sync] 超出保留数（--keep {keep}），移出清单：{name}", file=sys.stderr)

    manifest = {
        "schema": 1,
        "latest": ordered[0],
        "versions": [versions[v] for v in ordered],
    }
    if channel:
        manifest["channel"] = channel
    out.write_text(json.dumps(manifest, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"[sync] manifest: {len(ordered)} 个版本, latest={ordered[0]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
