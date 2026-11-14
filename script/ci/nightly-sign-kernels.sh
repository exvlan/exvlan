#!/usr/bin/env bash
#
# 给 nightly 的 6 个内核平台补齐 minisign 签名。
#
# studio 只签它自己打包的桌面平台（`kernel-sig-<platform>` 产物），linux-* /
# windows-arm64 在正式发版里同样没有签名产出（那些包只能走提权路径安装）。
# nightly 要发镜像托管的 6 个平台，而服务端 `kernel.install` 只安装 minisign
# 校验通过的内核 —— 这里用同一把私钥（`TAURI_SIGNING_PRIVATE_KEY`）把缺的补上。
# 签名与平台无关，ubuntu runner 上就能完成，不必为此多跑三台构建机。
#
# 已由 studio 签过名的平台直接复用原签名（不重签，避免出现两套签名来源）。
#
# 输入（env）：
#   ASSETS                    产物根目录（默认 release_assets）
#   TAURI_SIGNING_PRIVATE_KEY 必填
#   TAURI_CLI_VERSION         tauri CLI 版本（默认 ~2.12.4，与 packages/studio 对齐）

set -euo pipefail

assets="${ASSETS:-release_assets}"
cli_version="${TAURI_CLI_VERSION:-~2.12.4}"

die() {
  echo "::error::$*" >&2
  exit 1
}

[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || die "缺少 TAURI_SIGNING_PRIVATE_KEY：nightly 的内核必须 minisign 签名"
[ -d "$assets" ] || die "资产目录不存在：$assets"

export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

shopt -s nullglob

# npx 在本仓库目录里会被 package.json 的 `devEngines`（要求用 pnpm）直接拒绝，
# 而签名本身不需要仓库上下文 —— 在临时目录里跑，只给绝对路径。
sign() { # $1=待签名文件
  local abs
  abs="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
  (
    cd "${RUNNER_TEMP:-/tmp}" &&
      npx --yes "@tauri-apps/cli@${cli_version}" signer sign "$abs"
  )
}

signed=0
for d in "$assets"/core-*; do
  name="${d##*/}"
  name="${name#core-}"
  if [ -f "$d/exvlan.exe" ]; then entry=exvlan.exe; else entry=exvlan; fi
  bin="$d/$entry"
  [ -f "$bin" ] || die "core-$name 里没有内核本体 $entry"

  if [ -f "$assets/kernel-sig-$name/$entry.sig" ]; then
    cp "$assets/kernel-sig-$name/$entry.sig" "$bin.sig"
    echo "core-$name：复用 studio 的签名（kernel-sig-$name）"
  elif [ -f "$bin.sig" ]; then
    echo "core-$name：已有签名，跳过"
  else
    sign "$bin"
    echo "core-$name：已补签 → ${entry}.sig"
  fi

  [ -f "$bin.sig" ] || die "core-$name 签名后仍没有 $entry.sig"
  signed=$((signed + 1))
done

[ "$signed" -gt 0 ] || die "没有找到任何内核产物（${assets}/core-*）"
echo "内核签名完成：${signed} 个平台"
