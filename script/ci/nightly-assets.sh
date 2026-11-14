#!/usr/bin/env bash
#
# 把 Actions 产物整理成 nightly release 的最小资产集（见 docs/decisions-7.3.md §14.13）：
#
#   内核（镜像的 6 个平台，含 minisign 签名）：`<platform>-<version>.zip`
#   应用：macOS x86_64/aarch64 `.app.tar.gz` + `.sig`、Windows x86_64 `*-setup.exe` + `.sig`、
#        Android arm64 APK
#   清单：`manifest.json`（内核）、`upgrade-manifest.json` + `upgrade-latest.json`（应用）
#
# 不做：SignPath 签名、Apple 公证、magisk、GUI 的 dmg/msi（那些只在正式发版里发）。
#
# 文件名的版本段一律换成 nightly 版本串：镜像对 zip/安装包用了 immutable 缓存，
# 同名覆盖会让客户端长期拿到上一轮的包（见 script/kernel-mirror/caddy.Caddyfile）。
#
# 输入（env）：
#   VERSION   nightly 版本串，如 26.410.20715-nightly.1115（必填）
#   ASSETS    下载产物根目录（默认 release_assets）
#   OUT       输出目录（默认 zipped_assets）

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)

: "${VERSION:?nightly-assets: 需要 VERSION（nightly 版本串）}"
assets="${ASSETS:-release_assets}"
out="${OUT:-zipped_assets}"

die() {
  echo "::error::$*" >&2
  exit 1
}

base=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
  "$repo_root/packages/studio/src-tauri/tauri.conf.json" | head -1)
[ -n "$base" ] || die "读不到 tauri.conf.json 的 base 版本"

mkdir -p "$out"
out_abs=$(cd "$out" && pwd)

shopt -s nullglob

# --- 1) 内核：core-<platform> → <platform>-<version>.zip（签名进 zip） ---
for d in "$assets"/core-*; do
  name="${d##*/}"
  name="${name#core-}"
  if [ -f "$d/exvlan.exe" ]; then entry=exvlan.exe; else entry=exvlan; fi

  sig=""
  # 顺序：nightly-sign-kernels.sh 已落进产物目录的签名 → studio 的 kernel-sig 产物 → 摊平到根。
  for cand in "$d/$entry.sig" "$assets/kernel-sig-$name/$entry.sig" "$assets/$entry.sig"; do
    if [ -f "$cand" ]; then
      sig="$cand"
      break
    fi
  done
  # nightly 必须有签名：没有 minisign 的内核包装上也装不进去（服务端以 sig_invalid
  # 拒绝），与其发一个装不上的包，不如这轮不发、保留上一轮。
  [ -n "$sig" ] || die "core-$name 缺少内核签名 $entry.sig（kernel-sig-$name 产物不完整）"

  if [ "$sig" != "$d/$entry.sig" ]; then
    cp "$sig" "$d/$entry.sig"
  fi
  ( cd "$d" && zip -qr "$out_abs/${name}-${VERSION}.zip" . )
  echo "kernel: ${name}-${VERSION}.zip"
done

# --- 2) 应用安装包 ---
# macOS：`.app.tar.gz` 名字里没有架构标记，两个架构完全同名；这里补上版本与架构，
# 否则 x86_64 与 aarch64 会互相覆盖，清单也分不清。
stage_macos() { # $1=产物目录后缀 $2=架构标记
  local dir="$assets/studio-$1" arch="$2" src dst
  [ -d "$dir" ] || die "缺少 studio-$1 产物（macOS $arch）"
  src=$(find "$dir" -maxdepth 1 -name '*.app.tar.gz' | head -1)
  [ -n "$src" ] || die "studio-$1 里没有 .app.tar.gz（createUpdaterArtifacts 未生效？）"
  [ -f "$src.sig" ] || die "studio-$1 缺少 $(basename "$src").sig，updater 会拒绝这个包"
  dst="$out_abs/EXVlan.Studio_${VERSION}_${arch}.app.tar.gz"
  mv "$src" "$dst"
  mv "$src.sig" "$dst.sig"
  echo "app(macos-$arch): $(basename "$dst")"
}
stage_macos macos-x86_64 x86_64
stage_macos macos-aarch64 aarch64

# Windows：NSIS `*-setup.exe`（官方 updater 的更新包）+ `.sig`。
win_dir="$assets/studio-windows-x86_64"
[ -d "$win_dir" ] || die "缺少 studio-windows-x86_64 产物"
setup=$(find "$win_dir" -maxdepth 1 -name '*-setup.exe' | head -1)
[ -n "$setup" ] || die "studio-windows-x86_64 里没有 NSIS setup.exe"
[ -f "$setup.sig" ] || die "缺少 $(basename "$setup").sig，updater 会拒绝这个包"
win_name=$(basename "$setup")
win_name=${win_name//$base/$VERSION}
case "$win_name" in
  *"$VERSION"*) ;;
  *) die "setup.exe 名称里没有版本串（$win_name）：镜像对 .exe 是 immutable 缓存，同名会拿到旧包" ;;
esac
mv "$setup" "$out_abs/$win_name"
mv "$setup.sig" "$out_abs/$win_name.sig"
echo "app(windows-x86_64): $win_name"

# Android：只要 arm64 APK（正式版才有 split-per-abi 全量）。
# 产物名用 mobile.yml 的矩阵键（rust arch，aarch64），不是 ABI 名。
apk_dir="$assets/mobile-aarch64"
[ -d "$apk_dir" ] || die "缺少 mobile-aarch64 产物"
apk=$(find "$apk_dir" -maxdepth 1 -name 'app-arm64-release.apk' | head -1)
[ -n "$apk" ] || die "mobile-aarch64 里没有 app-arm64-release.apk"
mv "$apk" "$out_abs/app-arm64-release-${VERSION}.apk"
echo "app(android-arm64-v8a): app-arm64-release-${VERSION}.apk"

# --- 3) 清单 ---
# 应用：macOS 两个架构在同一次调用里生成（`--macos-arch` 可重复）；两个
# `.app.tar.gz` 已在上一步按架构改名，清单能分别对上。
python3 "$repo_root/script/gen_upgrade_manifests.py" \
  --assets-dir "$out_abs" \
  --version "$VERSION" \
  --channel nightly \
  --macos-arch x86_64 \
  --macos-arch aarch64 \
  --notes "Exvlan nightly ${VERSION}"

# 内核：sha256 + rpc_schema + min_studio_version（逐包校验签名是否进包）。
python3 "$repo_root/script/ci/kernel-manifest.py" \
  --assets-dir "$out_abs" \
  --version "$VERSION" \
  --channel nightly

echo "=== nightly assets ==="
ls -l "$out_abs"
