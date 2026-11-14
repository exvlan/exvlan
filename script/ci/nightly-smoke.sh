#!/usr/bin/env bash
#
# nightly 发布前烟测：只验「内核包能装、能跑、清单对得上」这个最小集合。
#
# 任何一项失败都必须**不发**（保留上一轮 nightly），所以全部硬失败：
#   1. `manifest.json` 里 6 个镜像平台齐全；
#   2. linux-x86_64 包的 sha256 与清单一致；
#   3. 包里同时有内核本体与 `<entry>.sig`；
#   4. 能在当前 runner（ubuntu）直接跑 `--version`；
#   5. 版本串确实是本轮 nightly（`EXVLAN_BUILD_VERSION` 注入是否生效）。
#
# 输入（env）：OUT（默认 zipped_assets）、VERSION（必填）

set -euo pipefail

out="${OUT:-zipped_assets}"
: "${VERSION:?nightly-smoke: 需要 VERSION（nightly 版本串）}"
manifest="$out/manifest.json"

die() {
  echo "::error::$*" >&2
  exit 1
}

[ -f "$manifest" ] || die "缺少 $manifest"

# 1) 平台齐全
missing=$(python3 - "$manifest" <<'PY'
import json
import sys

want = [
    "macos-x86_64",
    "macos-aarch64",
    "windows-x86_64",
    "windows-arm64",
    "linux-x86_64",
    "linux-aarch64",
]
data = json.load(open(sys.argv[1], encoding="utf-8"))
have = set(data["versions"][0]["assets"])
print(" ".join(p for p in want if p not in have))
PY
)
[ -z "$missing" ] || die "内核清单缺少平台：$missing"

# 2) linux-x86_64 包：存在 + sha256 与清单一致
read -r file want_sha < <(python3 - "$manifest" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1], encoding="utf-8"))
asset = data["versions"][0]["assets"]["linux-x86_64"]
print(asset["file"], asset["sha256"])
PY
)
[ -f "$out/$file" ] || die "清单引用的 $file 不存在"
got_sha=$(sha256sum "$out/$file" | cut -d' ' -f1)
[ "$got_sha" = "$want_sha" ] || die "sha256 与清单不一致：$file（清单 ${want_sha}，实际 ${got_sha}）"

# 3) 解压：内核本体 + 签名都在
tmp=$(mktemp -d "${RUNNER_TEMP:-/tmp}/nightly-smoke.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
unzip -q "$out/$file" -d "$tmp"
[ -f "$tmp/exvlan" ] || die "包里缺少内核本体 exvlan"
[ -f "$tmp/exvlan.sig" ] || die "包里缺少内核签名 exvlan.sig（服务端会以 sig_invalid 拒绝）"
chmod +x "$tmp/exvlan"

# 4) 能跑 --version
set +e
ver_out=$(timeout 60 "$tmp/exvlan" --version 2>&1)
rc=$?
set -e
[ "$rc" -eq 0 ] || die "exvlan --version 退出码 ${rc}：$(printf '%s' "$ver_out" | head -3)"
echo "exvlan --version → $ver_out"

# 5) 版本串注入生效（内核的 EXVLAN_VERSION 就是 CI 注入的构建串）
case "$ver_out" in
  *"$VERSION"*) echo "版本串校验通过：$VERSION" ;;
  *) die "内核版本串不是本轮 nightly（期望含 ${VERSION}）：$ver_out" ;;
esac

# exvlan-cli 同样打印注入的构建串（存在才校验，避免因裁剪而误报）
if [ -f "$tmp/exvlan-cli" ]; then
  chmod +x "$tmp/exvlan-cli"
  set +e
  cli_out=$(timeout 60 "$tmp/exvlan-cli" --version 2>&1)
  cli_rc=$?
  set -e
  [ "$cli_rc" -eq 0 ] || die "exvlan-cli --version 退出码 ${cli_rc}：$(printf '%s' "$cli_out" | head -3)"
  case "$cli_out" in
    *"$VERSION"*) echo "exvlan-cli --version → $cli_out" ;;
    *) die "exvlan-cli 版本串不是本轮 nightly（期望含 ${VERSION}）：$cli_out" ;;
  esac
fi

echo "nightly 烟测通过：$file"
