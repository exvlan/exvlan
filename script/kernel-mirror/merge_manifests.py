import json
import pathlib
import sys

paths = [p for p in sys.argv[1].split(":") if p]
out = pathlib.Path(sys.argv[2])

versions = {}
for p in paths:
    try:
        data = json.loads(pathlib.Path(p).read_text(encoding="utf-8"))
    except Exception as exc:  # noqa: BLE001
        print(f"[sync] 跳过损坏的 manifest {p}: {exc}", file=sys.stderr)
        continue
    for entry in data.get("versions", []):
        key = entry.get("version")
        if not key:
            continue
        # 同一版本以先出现的为准（下载顺序按版本号倒序）
        versions.setdefault(key, entry)

if not versions:
    sys.exit("[sync] manifest 里没有任何版本")


def key(v):
    return [int(x) for x in v.replace("v", "").split(".") if x.isdigit()]


ordered = sorted(versions, key=key, reverse=True)
manifest = {
    "schema": 1,
    "latest": ordered[0],
    "versions": [versions[v] for v in ordered],
}
out.write_text(json.dumps(manifest, indent=2, ensure_ascii=False), encoding="utf-8")
print(f"[sync] manifest: {len(ordered)} 个版本, latest={ordered[0]}")
