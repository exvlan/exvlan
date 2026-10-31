#!/usr/bin/env bash
# 从 GitHub Release 拉取 GUI 需要的内核资产，发布到静态目录（供 Caddy 托管）。
#
# 设计要点：
# - 只镜像 GUI 真正用到的平台（默认 6 个），避免把 14 个平台全拖下来。
# - 原子发布：先落 zip，最后落 manifest.json —— 客户端不会看到指向缺失文件的清单。
# - 需要 GH_TOKEN（fine-grained：Contents = Read-only），仅当仓库为 private 时必需。
#
# 用法：
#   GH_TOKEN=xxx DEST=/srv/exvlan/core ./exvlan-kernel-sync.sh
#
# 环境变量：
#   REPO     默认 exvlan/exvlan
#   DEST     默认 /srv/exvlan/core
#   KEEP     保留最近多少个版本的 zip（默认 5）
#   PLATFORMS 以空格分隔的平台名（默认 6 个 GUI 平台）
set -euo pipefail

REPO="${REPO:-exvlan/exvlan}"
DEST="${DEST:-/srv/exvlan/core}"
KEEP="${KEEP:-5}"
PLATFORMS="${PLATFORMS:-macos-x86_64 macos-aarch64 windows-x86_64 windows-arm64 linux-x86_64 linux-aarch64}"

# 组装 gh release download 的 --pattern 参数
patterns=()
for p in $PLATFORMS; do
  patterns+=(-p "${p}-v*.zip")
done
patterns+=(-p 'manifest.json')

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

find "$work" -type f -name '*.zip' -exec cp -f {} "$stage/" \;
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
