#!/usr/bin/env bash
# 从 GitHub Release 拉取 GUI 需要的内核资产，发布到静态目录（供 Caddy 托管）。
#
# 设计要点：
# - 只镜像 GUI 真正用到的平台（默认 6 个），避免把 14 个平台全拖下来。
# - 原子发布：先落 zip，最后落 manifest.json —— 客户端不会看到指向缺失文件的清单。
# - 需要 GH_TOKEN（fine-grained：Contents = Read-only），仅当仓库为 private 时必需。
#
# 同时同步「应用更新」资产：安装包 + minisign 签名 + upgrade-manifest.json /
# upgrade-latest.json（后者在目标目录里重命名为 manifest.json / latest.json）。
#
# 用法：
#   GH_TOKEN=xxx DEST=/srv/exvlan/core ./exvlan-kernel-sync.sh
#
# 环境变量：
#   REPO     默认 exvlan/exvlan
#   DEST     默认 /srv/exvlan/core
#   UPGRADE_DEST 默认 /srv/exvlan/upgrade（应用更新镜像目录）
#   KEEP     保留最近多少个版本的 zip（默认 5）
#   PLATFORMS 以空格分隔的平台名（默认 6 个 GUI 平台）
set -euo pipefail

REPO="${REPO:-exvlan/exvlan}"
DEST="${DEST:-/srv/exvlan/core}"
UPGRADE_DEST="${UPGRADE_DEST:-/srv/exvlan/upgrade}"
KEEP="${KEEP:-5}"
PLATFORMS="${PLATFORMS:-macos-x86_64 macos-aarch64 windows-x86_64 windows-arm64 linux-x86_64 linux-aarch64}"

# 组装 gh release download 的 --pattern 参数
patterns=()
for p in $PLATFORMS; do
  patterns+=(-p "${p}-v*.zip")
done
patterns+=(-p 'manifest.json')
# 应用更新：安装包 + 官方 updater 产物（.app.tar.gz / setup.exe / .sig）+ 清单
patterns+=(-p '*.dmg' -p '*.msi' -p '*-setup.exe' -p 'app-*-release.apk')
patterns+=(-p '*.app.tar.gz' -p '*.sig')
patterns+=(-p 'upgrade-manifest.json' -p 'upgrade-latest.json')
# Magisk 在线更新：模块包 + 清单（module.prop 的 updateJson 指向镜像）
patterns+=(-p 'Exvlan-Magisk-*.zip' -p 'magisk_update.json')

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# 1) 最近 KEEP 个正式 release（tag 形如 v26.410.207xx）
gh api "repos/$REPO/releases?per_page=100" --paginate \
  --jq '.[] | select(.draft == false and .prerelease == false) | .tag_name' \
  | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' \
  | sort -rV | head -n "$KEEP" > "$work/tags"

if [ ! -s "$work/tags" ]; then
  echo "[sync] 没有可同步的 release，退出"
  exit 0
fi

# 2) 下载
while read -r tag; do
  [ -z "$tag" ] && continue
  mkdir -p "$work/$tag"
  if ! gh release download "$tag" --repo "$REPO" "${patterns[@]}" --dir "$work/$tag" --clobber; then
    echo "[sync] 警告：$tag 下载失败，跳过该版本" >&2
    rm -rf "$work/$tag"
    continue
  fi
  echo "[sync] $tag: $(find "$work/$tag" -type f | wc -l) 个文件"
done < "$work/tags"

# 3) 发布：先 zip，后 manifest
install -d "$DEST"
stage="$DEST/.staging"
install -d "$stage"

# Magisk 模块包属于 /upgrade，这里排除，避免先进 core 再被清理
find "$work" -type f -name '*.zip' ! -name 'Exvlan-Magisk-*' -exec cp -f {} "$stage/" \;
if [ -z "$(find "$stage" -maxdepth 1 -name '*.zip' -print -quit)" ]; then
  echo "[sync] 没有下载到任何 zip，保持现状并退出" >&2
  rm -rf "$stage"
  exit 1
fi
mv -f "$stage"/*.zip "$DEST/"

# 合并所有已下载版本的 manifest（保证 GUI 能看到历史版本，便于回滚）
manifests="$(find "$work" -type f -name 'manifest.json' | tr '\n' ':')"
if [ -z "$manifests" ]; then
  echo "[sync] 未找到 manifest.json，保持旧清单并退出" >&2
  rm -rf "$stage"
  exit 1
fi

# 应用更新镜像：先落安装包/签名，最后落清单（原子发布顺序不变）
upgrade_stage="$UPGRADE_DEST/.staging"
install -d "$UPGRADE_DEST" "$upgrade_stage"

copied=0
while IFS= read -r -d '' f; do
  base="$(basename "$f")"
  case "$base" in
    upgrade-manifest.json|upgrade-latest.json|manifest.json|magisk_update.json) continue ;;
  esac
  cp -f "$f" "$upgrade_stage/"
  copied=$((copied + 1))
done < <(find "$work" -type f \( -name '*.dmg' -o -name '*.msi' -o -name '*-setup.exe' \
  -o -name 'app-*-release.apk' -o -name '*.app.tar.gz' -o -name '*.sig' \
  -o -name 'Exvlan-Magisk-*.zip' \) -print0)

if [ "$copied" -gt 0 ]; then
  mv -f "$upgrade_stage"/* "$UPGRADE_DEST"/
  echo "[sync] 应用更新资产：$copied 个文件已发布到 $UPGRADE_DEST"
else
  echo "[sync] 未发现应用更新资产，跳过 upgrade 镜像" >&2
fi

# 清单最后落盘，且优先用最新 release 的那份
newest="$(head -n 1 "$work/tags")"
for pair in "upgrade-manifest.json:manifest.json" "upgrade-latest.json:latest.json" "magisk_update.json:magisk_update.json"; do
  src="${pair%%:*}"; dst="${pair##*:}"
  if [ -f "$work/$newest/$src" ]; then
    cp -f "$work/$newest/$src" "$upgrade_stage/$dst"
  fi
done
if compgen -G "$upgrade_stage/*.json" > /dev/null; then
  echo "[sync] 应用更新清单：$(cd "$upgrade_stage" && ls -1 *.json | tr '\n' ' ')"
  mv -f "$upgrade_stage"/*.json "$UPGRADE_DEST"/
fi
rm -rf "$upgrade_stage"

python3 "$(dirname "$0")/merge_manifests.py" "$manifests" "$stage/manifest.json"
mv -f "$stage/manifest.json" "$DEST/manifest.json"
rm -rf "$stage"

# 4) 清理：只保留 manifest 引用到的 zip，再按 KEEP 兜底
referenced="$(grep -oE '"file"[[:space:]]*:[[:space:]]*"[^"]+"' "$DEST/manifest.json" \
  | cut -d'"' -f4 | sort -u)"

for f in "$DEST"/*.zip; do
  [ -e "$f" ] || continue
  base="$(basename "$f")"
  if ! grep -qxF "$base" <<<"$referenced"; then
    echo "[sync] 清理未引用文件：$base"
    rm -f "$f"
  fi
done

echo "[sync] 完成：$(find "$DEST" -maxdepth 1 -type f | wc -l) 个文件，$(du -sh "$DEST" | cut -f1)"
