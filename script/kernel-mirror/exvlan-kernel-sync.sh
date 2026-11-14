#!/usr/bin/env bash
# ⚠️ legacy：已由 index.ts 取代，保留一个周期（下一个 minor 删除）。
#     新入口：script/kernel-mirror/index.ts（bun）；设计说明：docs/mirror-toolchain.md

# 从 GitHub Release 拉取 GUI 需要的资产，按**通道**发布到静态目录（供 Caddy 托管）。
#
# 通道布局（docs/decisions-7.3.md §14.13）：
#   <DEST=core>/<channel>/manifest.json + *.zip
#   <UPGRADE_DEST=upgrade>/<channel>/{latest.json,manifest.json,magisk_update.json} + 资产
#   <DEST>/manifest.json                 → stable/manifest.json      （符号链接）
#   <UPGRADE_DEST>/manifest.json         → stable/manifest.json      （符号链接）
#   <UPGRADE_DEST>/latest.json           → stable/latest.json        （符号链接）
#   <UPGRADE_DEST>/magisk_update.json    → stable/magisk_update.json （符号链接）
#
# tag → 通道：
#   `vX.Y.Z`（且未标 prerelease）→ stable
#   `-rc.*` → rc；`-beta.*` → beta；`-dev.*` → dev
#   固定 tag `nightly`（以及 `*-nightly.*`）→ nightly
#   `-pre.*` / `preview-*` 是 legacy，**不镜像**
#
# 设计要点：
# - 只镜像 GUI 真正用到的平台（默认 6 个），避免把 14 个平台全拖下来。
# - 原子发布：先落资产、后落清单；落盘前校验「清单引用的文件都在」。
# - 通道独立：某个通道拉不到/半发布，只影响该通道，不动其它通道与旧清单。
# - 别名是**符号链接**，指向 stable，不复制文件（避免两份不一致）。
# - 需要 GH_TOKEN（fine-grained：Contents = Read-only），仅当仓库为 private 时必需。
#
# 增量同步的三条硬规则（写在这里，改代码前先读）：
# - **最新版只前进不后退**：`latest.json` / `magisk_update.json` 取「该通道最新的、
#   且清单可用的 tag」；本轮因指纹命中没重下的版本，复用目标目录里已有的清单
#   （按 tag 版本比较，不看目录 mtime）。只补下旧 tag 时不会把 latest 降级。
# - **未版本化资产归最新 tag**：`app-*-release.apk` / `*.msi` / `*-setup.exe` /
#   `*.app.tar.gz` 这类名字不带版本号、跨版本重复；只允许通道最新 tag 提供，
#   旧 tag 的同名资产不发布（否则最旧的后写赢 → 客户端 sha256 校验失败）。
# - **MAX_BYTES 是窗口预算**：超限时从**最旧**版本开始收缩 core/app 窗口，
#   合并出的清单同步收缩（不留「清单引用但文件已删」的不一致状态）。
#
# 状态文件（两份）：
#   <DEST>/mirror-status.json          新 schema（按通道汇总）
#   <DEST 的上一级>/mirror-status.json 旧 schema 兼容文件（发布机看门狗读它）
#
# 用法：
#   GH_TOKEN=xxx ./exvlan-kernel-sync.sh
#   ./exvlan-kernel-sync.sh --dry-run --tags-file /tmp/tags   # 只打印计划，不下载/不写盘
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHANNELS="stable rc beta dev nightly"

info() { printf '[sync] %s\n' "$*"; }
warn() { printf '[sync] 警告：%s\n' "$*" >&2; }
die() { printf '[sync] 错误：%s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'USAGE'
按通道同步 EXVlan 的 Release 资产到镜像目录（docs/decisions-7.3.md §14.13）。

用法：
  exvlan-kernel-sync.sh [--dry-run] [--tags-file <path>]

选项：
  --dry-run            只打印「每个通道会同步哪些 tag、会产出哪些路径」，不下载、不写盘
  --tags-file <path>   从文件读 tag 列表（每行一个，可附带 <tab>published_at），跳过 gh api（测试用）
  -h, --help           显示本帮助

环境变量：
  REPO=exvlan/exvlan                  Release 所在仓库
  DEST=/srv/exvlan/core               内核镜像根目录（其下按通道建子目录）
  UPGRADE_DEST=/srv/exvlan/upgrade    应用更新镜像根目录（其下按通道建子目录）
  MIRROR_STATUS=$DEST/mirror-status.json  状态文件（新 schema，按通道）
  MIRROR_STATUS_COMPAT=${DEST%/core}/mirror-status.json
                                      旧 schema 兼容状态（发布机看门狗读它）
  MAX_BYTES=8589934592                core+app 合计预算；超限从最旧版本开始收缩窗口（0 = 不限）
  KEEP_CORE_STABLE=5  KEEP_APP_STABLE=5   stable 通道保留的版本数
  KEEP_RC=3  KEEP_BETA=3  KEEP_DEV=3      预览通道保留的版本数（nightly 固定 1）
  SINCE_TAG=                          只同步 >= 该 tag 的 stable 版本（其余通道不受影响）
  PLATFORMS="..."                     镜像的平台（默认 6 个 GUI 平台）
  FORCE=1                             忽略增量指纹，强制重新下载
USAGE
}

DRY_RUN=0
TAGS_FILE=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --tags-file) [ "$#" -ge 2 ] || die "--tags-file 需要路径"; TAGS_FILE="$2"; shift 2 ;;
    --tags-file=*) TAGS_FILE="${1#*=}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "未知参数：$1" ;;
  esac
done

REPO="${REPO:-exvlan/exvlan}"
DEST="${DEST:-/srv/exvlan/core}"
UPGRADE_DEST="${UPGRADE_DEST:-/srv/exvlan/upgrade}"
PLATFORMS="${PLATFORMS:-macos-x86_64 macos-aarch64 windows-x86_64 windows-arm64 linux-x86_64 linux-aarch64}"
FORCE="${FORCE:-0}"

# KEEP：兼容旧的单个 KEEP（只管 stable），通道化后按通道分开
KEEP_CORE_STABLE="${KEEP_CORE_STABLE:-${KEEP:-5}}"
KEEP_APP_STABLE="${KEEP_APP_STABLE:-${KEEP:-5}}"
KEEP_RC="${KEEP_RC:-3}"
KEEP_BETA="${KEEP_BETA:-3}"
KEEP_DEV="${KEEP_DEV:-3}"
SINCE_TAG="${SINCE_TAG:-}"
# core + app 合计预算：超限时从最旧版本开始收缩窗口（0 = 不限）。
# 历史上的 MAX_BYTES 只是写在文档里、脚本从未读取；现在真正生效。
MAX_BYTES="${MAX_BYTES:-8589934592}"

# 状态文件放在 DEST 里，这样 systemd unit 的 ReadWritePaths 只需覆盖 core/upgrade
MIRROR_STATUS="${MIRROR_STATUS:-$DEST/mirror-status.json}"
# 部署兼容：发布机看门狗读的是 <EXVLAN_DIST>/mirror-status.json（core 的上一级）与旧 schema
MIRROR_STATUS_COMPAT="${MIRROR_STATUS_COMPAT:-$(dirname "$DEST")/mirror-status.json}"

# 组装 gh release download 的 --pattern 参数
patterns=()
for p in $PLATFORMS; do
  patterns+=(-p "${p}-v*.zip")
  # nightly 用固定 tag 构建时，产物名可能是 `<platform>-<ver>-nightly.N.zip`（无 v 前缀）
  patterns+=(-p "${p}-*nightly*.zip")
done
patterns+=(-p 'manifest.json')
# 应用更新：安装包 + 官方 updater 产物（.app.tar.gz / setup.exe / .sig）+ 清单
# APK 文件名允许带版本后缀（nightly 会写 `app-arm64-release-<ver>.apk`：
# 镜像对 .apk 是 immutable 缓存，同名覆盖会让客户端永远拿到旧包）。
patterns+=(-p '*.dmg' -p '*.msi' -p '*-setup.exe' -p 'app-*-release*.apk')
patterns+=(-p '*.app.tar.gz' -p '*.sig')
patterns+=(-p 'upgrade-manifest.json' -p 'upgrade-latest.json')
# Magisk 在线更新：模块包 + 清单（module.prop 的 updateJson 指向镜像）
patterns+=(-p 'Exvlan-Magisk-*.zip' -p 'magisk_update.json')

# 内核 zip 的资产名形如 `<platform>-v26.410.20711.zip`
PLATFORM_ZIP_RE="^($(printf '%s' "$PLATFORMS" | tr ' ' '|'))-.*[.]zip$"
# upgrade 目录收的资产（与 publish_app 的 case 保持同步；预算估算用）
# 用 [.] 而不是 `\.`：这个正则要经 awk -v 传入，`\.` 会被 awk 当成转义并告警
UPGRADE_ASSET_RE='([.]dmg$|[.]msi$|-setup[.]exe$|app-.*-release.*[.]apk$|[.]app[.]tar[.]gz$|[.]sig$|^Exvlan-Magisk-.*[.]zip$)'

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

count_lines() { [ -s "$1" ] && wc -l < "$1" | tr -d ' ' || echo 0; }

# `$1 >= $2`（版本号比较，支持 -rc.1 这类后缀）
version_ge() {
  [ "$1" = "$2" ] && return 0
  [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$1" ]
}

# 通道保留数（nightly 固定 1，发布时整目录替换）
keep_count_core() {
  case "$1" in
    stable) echo "$KEEP_CORE_STABLE" ;;
    rc) echo "$KEEP_RC" ;;
    beta) echo "$KEEP_BETA" ;;
    dev) echo "$KEEP_DEV" ;;
    *) echo 1 ;;
  esac
}

keep_count_app() {
  case "$1" in
    stable) echo "$KEEP_APP_STABLE" ;;
    rc) echo "$KEEP_RC" ;;
    beta) echo "$KEEP_BETA" ;;
    dev) echo "$KEEP_DEV" ;;
    *) echo 1 ;;
  esac
}

# ---------------------------------------------------------------------------
# 1) 取 release 元数据（tag / published_at / prerelease / 资产名+大小）
# ---------------------------------------------------------------------------
collect_release_metadata() {
  if [ -n "$TAGS_FILE" ]; then
    [ -r "$TAGS_FILE" ] || die "读不到 --tags-file：$TAGS_FILE"
    awk -F'\t' '
      { sub(/#.*/, "") }
      { tag = $1; gsub(/^[[:space:]]+|[[:space:]]+$/, "", tag); if (tag == "") next
        pub = ""
        if (NF > 1) { pub = $2; gsub(/^[[:space:]]+|[[:space:]]+$/, "", pub) }
        print tag "\t" pub "\tfalse" }
    ' "$TAGS_FILE" > "$work/releases.tsv"
    return 0
  fi

  gh api "repos/$REPO/releases?per_page=100" --paginate --jq '
    .[] | select(.draft == false) | . as $r
    | (["R", $r.tag_name, ($r.published_at // ""), ($r.prerelease | tostring)] | @tsv),
      (($r.assets // [])[] | ["A", $r.tag_name, .name, (.size | tostring)] | @tsv)
  ' > "$work/raw.tsv"

  awk -F'\t' -v dir="$work" '
    $1 == "R" { print $2 "\t" $3 "\t" $4 >> (dir "/releases.tsv"); next }
    $1 == "A" { print $3 "\t" $4 >> (dir "/assets." $2); next }
  ' "$work/raw.tsv"
}

# ---------------------------------------------------------------------------
# 2) tag → 通道
# ---------------------------------------------------------------------------
classify_tag() {
  local tag="$1"
  case "$tag" in
    nightly) echo nightly; return ;;
    *-pre.*|preview-*) echo skip; return ;;   # legacy，不镜像
    *-rc.*) echo rc; return ;;
    *-beta.*) echo beta; return ;;
    *-dev.*) echo dev; return ;;
    *-nightly.*) echo nightly; return ;;
  esac
  if [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo stable
    return
  fi
  echo skip
}

build_channel_lists() {
  : > "$work/skipped.tsv"
  local tag pub prerelease ch
  while IFS=$'\t' read -r tag pub prerelease; do
    [ -n "$tag" ] || continue
    ch="$(classify_tag "$tag")"
    if [ "$ch" = skip ]; then
      printf '%s\n' "$tag" >> "$work/skipped.tsv"
      continue
    fi
    # stable 沿用旧语义：标了 prerelease 的 vX.Y.Z 不算正式版
    if [ "$ch" = stable ] && [ "$prerelease" = true ]; then
      printf '%s\n' "$tag" >> "$work/skipped.tsv"
      continue
    fi
    printf '%s\t%s\n' "$tag" "$pub" >> "$work/ch.$ch"
  done < "$work/releases.tsv"

  for ch in $CHANNELS; do
    local f="$work/ch.$ch"
    [ -s "$f" ] || continue
    if [ "$ch" = nightly ]; then
      # 固定 tag：按发布时间倒序（同一 tag 只留一行）
      sort -t$'\t' -k2,2r -u "$f" -o "$f"
    else
      sort -t$'\t' -k1,1rV -u "$f" -o "$f"
    fi
    cut -f1 "$f" > "$work/tags.$ch"
  done
}

# ---------------------------------------------------------------------------
# 3) 按通道选 tag（保留策略 + SINCE_TAG 只约束 stable + MAX_BYTES 预算）
# ---------------------------------------------------------------------------
# 单 tag 的预计占用（PLATFORMS 内核 zip + upgrade 资产）；没有资产元数据（--tags-file
# 模式）时返回 1，调用方据此放弃预算裁剪。
tag_bytes() {
  local tag="$1" f="$work/assets.$tag"
  [ -s "$f" ] || return 1
  awk -F'\t' -v re="$PLATFORM_ZIP_RE" -v up="$UPGRADE_ASSET_RE" \
    '($1 ~ re || $1 ~ up) { s += $2 } END { print s + 0 }' "$f"
}

# MAX_BYTES 窗口预算：core+app 合计超限时，从**最旧**版本开始收缩（清单同步收缩，
# 不留「清单还引用、文件已删」的不一致状态）。只裁到每边剩 1 个版本为止。
budget_trim() {
  local ch="$1" list="$work/budget.tags" total tag b dropped=0 core_n app_n
  [ "${MAX_BYTES:-0}" -gt 0 ] || return 0
  while :; do
    cat "$work/core.$ch" "$work/app.$ch" 2>/dev/null | awk 'NF' | sort -u > "$list"
    [ -s "$list" ] || return 0
    total=0
    while read -r tag; do
      [ -n "$tag" ] || continue
      b="$(tag_bytes "$tag")" || return 0
      total=$((total + b))
    done < "$list"
    if [ "$total" -le "$MAX_BYTES" ]; then
      if [ "$dropped" -gt 0 ]; then
        info "$ch：MAX_BYTES=$MAX_BYTES 预算裁剪后保留 core $(count_lines "$work/core.$ch") / app $(count_lines "$work/app.$ch") 个版本（约 $total 字节）"
      fi
      return 0
    fi
    core_n="$(count_lines "$work/core.$ch")"
    app_n="$(count_lines "$work/app.$ch")"
    if [ "$core_n" -le 1 ] && [ "$app_n" -le 1 ]; then
      warn "$ch：单个版本就超过 MAX_BYTES=$MAX_BYTES（约 $total 字节），不再裁剪"
      return 0
    fi
    if [ "$core_n" -ge "$app_n" ] && [ "$core_n" -gt 1 ]; then
      info "$ch：超出 MAX_BYTES=$MAX_BYTES，从最旧版本收缩 core 窗口：$(tail -n 1 "$work/core.$ch")"
      head -n -1 "$work/core.$ch" > "$work/.trim.tmp"
      mv -f "$work/.trim.tmp" "$work/core.$ch"
    else
      info "$ch：超出 MAX_BYTES=$MAX_BYTES，从最旧版本收缩 app 窗口：$(tail -n 1 "$work/app.$ch")"
      head -n -1 "$work/app.$ch" > "$work/.trim.tmp"
      mv -f "$work/.trim.tmp" "$work/app.$ch"
    fi
    dropped=$((dropped + 1))
  done
}

select_tags() {
  local ch="$1"
  local src="$work/tags.$ch"
  : > "$work/core.$ch"
  : > "$work/app.$ch"
  : > "$work/dl.$ch"
  [ -s "$src" ] || return 0

  local tags=() tag
  mapfile -t tags < "$src"

  if [ "$ch" = stable ] && [ -n "$SINCE_TAG" ]; then
    local filtered=()
    for tag in "${tags[@]}"; do
      if version_ge "$tag" "$SINCE_TAG"; then filtered+=("$tag"); fi
    done
    tags=("${filtered[@]}")
    if [ "${#tags[@]}" -eq 0 ]; then
      warn "stable：>= SINCE_TAG=$SINCE_TAG 没有 release，跳过该通道"
      return 0
    fi
  fi

  local keep_core keep_app
  keep_core="$(keep_count_core "$ch")"
  keep_app="$(keep_count_app "$ch")"

  printf '%s\n' "${tags[@]}" | head -n "$keep_core" > "$work/core.$ch"
  printf '%s\n' "${tags[@]}" | head -n "$keep_app" > "$work/app.$ch"

  # MAX_BYTES 预算：超限时把窗口从最旧端收缩（清单随后按裁过的窗口合并）
  budget_trim "$ch"

  # 下载集合 = core 保留 ∪ app 保留（同一 tag 只下载一次）
  cat "$work/core.$ch" > "$work/dl.$ch"
  while read -r tag; do
    [ -n "$tag" ] || continue
    grep -qxF "$tag" "$work/dl.$ch" || printf '%s\n' "$tag" >> "$work/dl.$ch"
  done < "$work/app.$ch"
}

# ---------------------------------------------------------------------------
# 4) 增量指纹（LOCAL 增强：资产名+大小未变且本地齐全 → 不重复下载）
# ---------------------------------------------------------------------------
tag_fingerprint() {
  local tag="$1"
  local f="$work/assets.$tag"
  [ -s "$f" ] || return 1
  sort "$f" | sha256sum | cut -d' ' -f1
}

core_asset_names() {
  local tag="$1"
  local f="$work/assets.$tag"
  [ -s "$f" ] || return 0
  awk -F'\t' -v re="$PLATFORM_ZIP_RE" '$1 ~ re { print $1 }' "$f"
}

release_published_at() {
  awk -F'\t' -v t="$1" '$1 == t { print $2; exit }' "$work/releases.tsv"
}

# 0 = 需要下载；1 = 可以跳过
need_download() {
  local ch="$1" tag="$2" fp recorded name ver
  [ "$FORCE" = 1 ] && return 0

  if [ "$ch" = nightly ]; then
    # 固定 tag：published_at 是主要信号；再叠一层资产指纹，
    # 防止「同一个 tag 覆盖上传」时 published_at 不变而漏掉新构建。
    local pub last_pub last_fp
    pub="$(release_published_at "$tag")"
    last_pub="$(cat "$DEST/$ch/.published-at" 2>/dev/null || true)"
    if [ -n "$pub" ] && [ "$pub" = "$last_pub" ]; then
      fp="$(tag_fingerprint "$tag")" || return 0
      last_fp="$(awk -F'\t' -v t="$tag" '$1 == t { print $2; exit }' "$DEST/$ch/.sync-state" 2>/dev/null || true)"
      [ "$fp" = "$last_fp" ] && return 1
      return 0
    fi
    return 0
  fi

  fp="$(tag_fingerprint "$tag")" || return 0
  recorded="$(awk -F'\t' -v t="$tag" '$1 == t { print $2; exit }' "$DEST/$ch/.sync-state" 2>/dev/null || true)"
  [ "$recorded" = "$fp" ] || return 0

  if grep -qxF "$tag" "$work/core.$ch"; then
    while read -r name; do
      [ -n "$name" ] || continue
      [ -f "$DEST/$ch/$name" ] || return 0
    done < <(core_asset_names "$tag")
  fi
  if grep -qxF "$tag" "$work/app.$ch"; then
    ver="${tag#v}"
    [ -f "$UPGRADE_DEST/$ch/manifest.json" ] || return 0
    grep -qF "\"version\": \"$ver\"" "$UPGRADE_DEST/$ch/manifest.json" || return 0
  fi
  return 1
}

write_sync_state() {
  local ch="$1"
  local tmp="$DEST/$ch/.sync-state.tmp" tag fp
  [ -d "$DEST/$ch" ] || return 0
  : > "$tmp"
  while read -r tag; do
    [ -n "$tag" ] || continue
    if [ -d "$work/rel/$ch/$tag" ]; then
      # 本轮重新下载过：记录新指纹
      fp="$(tag_fingerprint "$tag")" || continue
    else
      # 本轮因指纹命中而跳过下载：沿用上次记录的指纹，避免下次被迫重下
      fp="$(awk -F'\t' -v t="$tag" '$1 == t { print $2; exit }' "$DEST/$ch/.sync-state" 2>/dev/null || true)"
      [ -n "$fp" ] || continue
    fi
    printf '%s\t%s\n' "$tag" "$fp" >> "$tmp"
  done < "$work/dl.$ch"
  mv -f "$tmp" "$DEST/$ch/.sync-state"
}

write_published_at() {
  local ch="$1" tag="$2" pub tmp
  pub="$(release_published_at "$tag")"
  [ -n "$pub" ] || return 0
  [ -d "$DEST/$ch" ] || return 0
  tmp="$DEST/$ch/.published-at.tmp"
  printf '%s\n' "$pub" > "$tmp"
  mv -f "$tmp" "$DEST/$ch/.published-at"
}

# ---------------------------------------------------------------------------
# 5) 下载
# ---------------------------------------------------------------------------
download_channels() {
  local ch tag rel downloaded=0
  for ch in $CHANNELS; do
    [ -s "$work/dl.$ch" ] || continue
    while read -r tag; do
      [ -n "$tag" ] || continue
      if need_download "$ch" "$tag"; then :; else
        info "$ch/$tag：指纹/发布时间未变，跳过下载"
        continue
      fi
      rel="$work/rel/$ch/$tag"
      rm -rf "$rel"
      mkdir -p "$rel"
      if ! gh release download "$tag" --repo "$REPO" "${patterns[@]}" --dir "$rel" --clobber; then
        warn "$ch/$tag 下载失败，本轮跳过该 tag"
        rm -rf "$rel"
        continue
      fi
      downloaded=$((downloaded + 1))
      info "$ch/$tag：下载 $(find "$rel" -type f | wc -l | tr -d ' ') 个文件"
    done < "$work/dl.$ch"
  done
  info "下载完成：$downloaded 个 tag 有新内容"
}

# ---------------------------------------------------------------------------
# 6) 发布工具
# ---------------------------------------------------------------------------
# 清单里 `"file": "xxx"` 的值（相对文件名）
manifest_files() {
  grep -oE '"file"[[:space:]]*:[[:space:]]*"[^"]+"' "$1" 2>/dev/null \
    | sed -E 's/.*"([^"]+)"$/\1/' || true
}

# 从 JSON 里抽某字段的 URL，取最后一段文件名（`"url"` / `"zipUrl"`）
json_url_names() {
  local field="$1" file="$2"
  grep -oE "\"$field\"[[:space:]]*:[[:space:]]*\"[^\"]+\"" "$file" 2>/dev/null \
    | sed -E 's#.*/([^/"]+)"$#\1#' || true
}

# 清单的顶层 `"version"`（updater 清单是 `26.410.20711`，内核清单是 `v26.410.20711`）
json_version() {
  sed -nE 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/p' "$1" 2>/dev/null \
    | head -n 1 || true
}

# 该 tag 的版本号是否就是 $2（nightly 是固定 tag，无法从清单 version 反推 → 有清单即算匹配）
tag_matches_version() {
  local tag="$1" ver="$2"
  [ -n "$ver" ] || return 1
  case "$tag" in
    nightly) return 0 ;;
  esac
  [ "${tag#v}" = "$ver" ]
}

# tag 的版本是否 >= $2（固定 tag nightly 一旦下载就是更新，不看版本串）
tag_not_older_than() {
  local tag="$1" ver="$2"
  [ -n "$ver" ] || return 0
  case "$tag" in
    nightly) return 0 ;;
  esac
  version_ge "${tag#v}" "$ver"
}

# 选「该通道最新的、且清单可用的 tag」——增量同步下不能只从「本轮下载过的 tag」里挑，
# 否则只补下旧 tag 时 latest/magisk 会被降级并一直粘住。
#   $1=通道 $2=通道 tag 列表（新→旧） $3=清单文件名 $4=目标目录里已发布的同名清单
#   输出 `<tag>\t<rel|existing>`：rel = 本轮下载的，existing = 已发布内容就是它（不动）
pick_manifest_tag() {
  local ch="$1" list="$2" name="$3" published="$4"
  local tag rel pubver=""
  [ -s "$list" ] || return 1
  if [ -f "$published" ]; then pubver="$(json_version "$published")"; fi
  while read -r tag; do
    [ -n "$tag" ] || continue
    rel="$work/rel/$ch/$tag"
    if [ -f "$rel/$name" ]; then
      if tag_not_older_than "$tag" "$pubver"; then
        printf '%s\t%s\n' "$tag" rel
        return 0
      fi
      warn "app/$ch：$tag 的 $name 比已发布的 $pubver 旧，忽略（不降级）"
      continue
    fi
    if tag_matches_version "$tag" "$pubver"; then
      # 最新版这轮因指纹命中没重下：目标目录里已有的清单就是它的内容
      printf '%s\t%s\n' "$tag" existing
      return 0
    fi
  done < "$list"
  return 1
}

# 资产名里是否带该 tag 的版本号 → 0 = 带（各版本名字天然不同，谁都能发布）
# 不带版本号的名字（app-*-release.apk / *.msi / *-setup.exe / *.app.tar.gz）跨版本重复，
# 只允许通道最新 tag 发布，旧 tag 同名资产不得覆盖（最旧后写赢会让清单 sha256 对不上）。
name_scoped_to_tag() {
  local name="$1" tag="$2" ver="${tag#v}"
  [ -n "$ver" ] || return 1
  case "$name" in
    *"$ver"*) return 0 ;;
  esac
  return 1
}

# nightly：整目录替换（保留策略固定 1 份）
wipe_dir_files() {
  local dir="$1"
  [ -d "$dir" ] || return 0
  find "$dir" -mindepth 1 -maxdepth 1 \
    ! -name '.sync-state' ! -name '.published-at' \
    -exec rm -rf {} +
}

# 应用目录里被清单引用的文件名（含成对的 .sig）
app_refs() {
  local dir="$1" manifest="$2"
  {
    manifest_files "$manifest"
    if [ -f "$dir/latest.json" ]; then json_url_names url "$dir/latest.json"; fi
    if [ -f "$dir/magisk_update.json" ]; then json_url_names zipUrl "$dir/magisk_update.json"; fi
  } | while read -r name; do
    [ -n "$name" ] || continue
    printf '%s\n%s\n' "$name" "$name.sig"
  done | sort -u || true
}

# 按清单引用清理（`*.json` 与点文件不动）
prune_dir() {
  local dir="$1" manifest="$2" kind="$3" name refs=""
  case "$kind" in
    core) refs="$(manifest_files "$manifest")" ;;
    app) refs="$(app_refs "$dir" "$manifest")" ;;
  esac
  if [ -z "$refs" ]; then
    warn "prune_dir：$manifest 里没有任何引用，跳过清理（避免误删）"
    return 0
  fi
  for f in "$dir"/*; do
    [ -e "$f" ] || continue
    name="$(basename "$f")"
    case "$name" in .*|*.json) continue ;; esac
    if ! grep -qxF "$name" <<<"$refs"; then
      info "清理未引用文件：$dir/$name"
      rm -f "$f"
    fi
  done
}

# 别名：符号链接指向 stable（不复制文件），用 rename 原子替换
link_alias() {
  local target="$1" dest="$2" parent tmp
  parent="$(dirname "$dest")"
  if [ ! -f "$parent/$target" ]; then
    info "别名跳过：$parent/$target 不存在"
    return 0
  fi
  if [ "$DRY_RUN" = 1 ]; then
    info "别名（dry-run）：$dest → $target"
    return 0
  fi
  tmp="$parent/.$(basename "$dest").tmp.$$"
  ln -sfn "$target" "$tmp"
  mv -fT "$tmp" "$dest"
  info "别名：$dest → $target"
}

# 老布局兼容：老客户端缓存过的旧清单、以及发布机看门狗（按 `core/<file>` 校验
# 清单引用的 zip 是否存在）都按 `DEST/<zip>` 找内核包，而文件现在在 `DEST/stable/`。
# 为缺失的名字补**相对符号链接**：不复制、不占空间，也不动根上已有的真实旧文件。
link_legacy_core_files() {
  local manifest="$1" dest="$2" name count=0
  [ -f "$manifest" ] || return 0
  if [ "$DRY_RUN" = 1 ]; then
    info "旧布局兼容（dry-run）：$dest 根上缺失的内核 zip 会补符号链接 → stable/"
    return 0
  fi
  while read -r name; do
    [ -n "$name" ] || continue
    case "$name" in */*) continue ;; esac
    [ -f "$dest/stable/$name" ] || continue
    [ -e "$dest/$name" ] && continue
    ln -sfn "stable/$name" "$dest/$name" && count=$((count + 1))
  done < <(manifest_files "$manifest")
  if [ "$count" -gt 0 ]; then
    info "旧布局兼容：$dest 根上补了 $count 个指向 stable/ 的符号链接"
  fi
  return 0
}

# ---------------------------------------------------------------------------
# 7) 发布 core（内核）
# ---------------------------------------------------------------------------
publish_core() {
  local ch="$1"
  local dir="$DEST/$ch"
  local stage="$DEST/.staging-core.$ch"
  local tag rel manifests="" count=0 found f name missing=0 keep
  local since_args=()
  keep="$(keep_count_core "$ch")"
  if [ "$ch" = stable ] && [ -n "$SINCE_TAG" ]; then
    # SINCE_TAG 只约束 stable；现有清单里的更早版本也要剔除，否则「增量合并」会把它们留下
    since_args=(--since "$SINCE_TAG")
  fi

  rm -rf "$stage"
  install -d "$stage"

  while read -r tag; do
    [ -n "$tag" ] || continue
    rel="$work/rel/$ch/$tag"
    # 没有 rel 目录 = 本轮因指纹命中跳过下载（资产已在通道目录里），静默跳过
    [ -d "$rel" ] || continue
    if [ ! -f "$rel/manifest.json" ]; then
      warn "core/$ch：$tag 缺 manifest.json（半发布？），本轮不发布该版本"
      continue
    fi
    found=0
    for f in "$rel"/*.zip; do
      [ -e "$f" ] || continue
      case "$(basename "$f")" in
        Exvlan-Magisk-*) continue ;;   # Magisk 包属于 upgrade
      esac
      cp -f "$f" "$stage/"
      found=1
    done
    if [ "$found" -eq 0 ]; then
      warn "core/$ch：$tag 没有可用内核 zip，本轮不发布该版本"
      continue
    fi
    manifests="${manifests}${manifests:+:}$rel/manifest.json"
    count=$((count + 1))
  done < "$work/core.$ch"

  if [ "$count" -eq 0 ]; then
    # 没有新版本时一般不重写清单（省事且安全）；但若 stable 设了 SINCE_TAG，
    # 需要把现有清单里的更早版本也剔掉，所以仍要重写一次。
    if [ "${#since_args[@]}" -eq 0 ] || [ ! -f "$dir/manifest.json" ]; then
      info "core/$ch：本轮没有可发布的内核包（保持现状）"
      rm -rf "$stage"
      return 2
    fi
    info "core/$ch：没有新版本，但 SINCE_TAG 要求重写清单"
    manifests="$dir/manifest.json"
  fi

  # 保留策略靠清单裁剪实现：本轮重新下载的版本在前，通道现有清单补足其余版本，
  # 这样「增量跳过」的旧版本不会从清单里消失。
  if [ "$count" -gt 0 ] && [ "$ch" != nightly ] && [ -f "$dir/manifest.json" ]; then
    manifests="$manifests:$dir/manifest.json"
  fi

  # Release 自带 manifest 含全部 14 个平台，镜像只托管 PLATFORMS：
  # 不裁剪会让 GUI 看到「有更新」但下载 404。
  python3 "$SCRIPT_DIR/merge_manifests.py" --keep "$keep" "${since_args[@]}" \
    "$manifests" "$stage/manifest.json"
  # shellcheck disable=SC2086  # PLATFORMS 按空格分隔，需展开成多个参数
  python3 "$SCRIPT_DIR/filter_manifest_platforms.py" "$stage/manifest.json" $PLATFORMS

  while read -r name; do
    [ -n "$name" ] || continue
    if [ ! -f "$stage/$name" ] && [ ! -f "$dir/$name" ]; then
      warn "core/$ch：清单引用的 $name 不存在（半发布防御：保持现状）"
      missing=1
    fi
  done < <(manifest_files "$stage/manifest.json")
  if [ "$missing" -eq 1 ]; then
    rm -rf "$stage"
    return 1
  fi

  install -d "$dir"
  if [ "$ch" = nightly ]; then wipe_dir_files "$dir"; fi

  for f in "$stage"/*.zip; do
    [ -e "$f" ] || continue
    mv -f "$f" "$dir/"
  done
  mv -f "$stage/manifest.json" "$dir/manifest.json"
  rm -rf "$stage"
  info "core/$ch：已发布清单（新版本 $count 个）→ $dir"
  prune_dir "$dir" "$dir/manifest.json" core
  return 0
}

# ---------------------------------------------------------------------------
# 8) 发布 app（安装包 + 官方 updater 清单 + Magisk）
# ---------------------------------------------------------------------------
publish_app() {
  local ch="$1"
  local dir="$UPGRADE_DEST/$ch"
  local stage="$UPGRADE_DEST/.staging-app.$ch"
  local tag rel manifests="" latest_tag="" latest_src="" magisk_tag="" magisk_src="" keep
  local newest_tag="" pick=""
  local since_args=()
  local copied=0 skipped=0 missing=0 f asset name
  keep="$(keep_count_app "$ch")"
  if [ "$ch" = stable ] && [ -n "$SINCE_TAG" ]; then
    since_args=(--since "$SINCE_TAG")
  fi
  # 通道最新 tag：未版本化资产只由它提供（app.$ch 已按版本倒序）
  newest_tag="$(head -n 1 "$work/app.$ch" 2>/dev/null || true)"

  rm -rf "$stage"
  install -d "$stage"

  while read -r tag; do
    [ -n "$tag" ] || continue
    rel="$work/rel/$ch/$tag"
    # 没有 rel 目录 = 本轮因指纹命中跳过下载，静默跳过
    [ -d "$rel" ] || continue
    [ -f "$rel/upgrade-manifest.json" ] || continue
    manifests="${manifests}${manifests:+:}$rel/upgrade-manifest.json"
    for f in "$rel"/*; do
      [ -e "$f" ] || continue
      asset="$(basename "$f")"
      case "$asset" in
        *.json) continue ;;
        *.dmg|*.msi|*-setup.exe|app-*-release*.apk|*.app.tar.gz|*.sig|Exvlan-Magisk-*.zip)
          # 未版本化资产跨版本同名：旧 tag 的副本会覆盖最新版字节（客户端 sha256 校验失败）
          if [ "$tag" != "$newest_tag" ] && ! name_scoped_to_tag "$asset" "$tag"; then
            info "app/$ch：跳过 $tag 的未版本化资产 $asset（只从最新 tag ${newest_tag:-?} 发布）"
            skipped=$((skipped + 1))
            continue
          fi
          cp -f "$f" "$stage/"
          copied=$((copied + 1))
          ;;
      esac
    done
  done < "$work/app.$ch"

  if [ -z "$manifests" ]; then
    # 没有新版本时一般不重写清单；但 stable 设了 SINCE_TAG 时要顺带剔除更早的版本
    if [ "${#since_args[@]}" -eq 0 ] || [ ! -f "$dir/manifest.json" ]; then
      info "app/$ch：本轮没有应用更新清单（保持现状）"
      rm -rf "$stage"
      return 2
    fi
    info "app/$ch：没有新版本，但 SINCE_TAG 要求重写清单"
    manifests="$dir/manifest.json"
  elif [ "$ch" != nightly ] && [ -f "$dir/manifest.json" ]; then
    # 与 core 同理：现有清单补足「本轮跳过下载」的旧版本，再按 KEEP 裁剪
    manifests="$manifests:$dir/manifest.json"
  fi
  python3 "$SCRIPT_DIR/merge_manifests.py" --keep "$keep" "${since_args[@]}" \
    "$manifests" "$stage/manifest.json"

  # 官方 updater 清单与 Magisk 清单取「该通道最新的、且清单可用的 tag」：
  # 本轮下载到的直接落盘；本轮因指纹命中没重下的最新版，目标目录里的清单就是它，
  # 保持不动（只补下旧 tag 时不会把 latest 降级，也不会粘在旧版本上）。
  # URL 改写成**本通道**目录（绝对地址是 tauri updater 的硬要求）。
  if pick="$(pick_manifest_tag "$ch" "$work/tags.$ch" upgrade-latest.json "$dir/latest.json")"; then
    latest_tag="${pick%%$'\t'*}"
    latest_src="${pick##*$'\t'}"
    if [ "$latest_src" = rel ]; then
      cp -f "$work/rel/$ch/$latest_tag/upgrade-latest.json" "$stage/latest.json"
      python3 "$SCRIPT_DIR/channelize_urls.py" "$stage/latest.json" "$ch"
    else
      info "app/$ch：latest.json 已是 $latest_tag 的内容（本轮未重下最新版），保持不动"
    fi
  else
    warn "app/$ch：找不到可用的 upgrade-latest.json，官方 updater 清单保持旧内容"
  fi
  if pick="$(pick_manifest_tag "$ch" "$work/tags.$ch" magisk_update.json "$dir/magisk_update.json")"; then
    magisk_tag="${pick%%$'\t'*}"
    magisk_src="${pick##*$'\t'}"
    if [ "$magisk_src" = rel ]; then
      cp -f "$work/rel/$ch/$magisk_tag/magisk_update.json" "$stage/magisk_update.json"
      python3 "$SCRIPT_DIR/channelize_urls.py" "$stage/magisk_update.json" "$ch"
    else
      info "app/$ch：magisk_update.json 已是 $magisk_tag 的内容（本轮未重下最新版），保持不动"
    fi
  fi

  while read -r name; do
    [ -n "$name" ] || continue
    if [ ! -f "$stage/$name" ] && [ ! -f "$dir/$name" ]; then
      warn "app/$ch：清单引用的 $name 不存在（半发布防御：保持现状）"
      missing=1
    fi
  done < <(manifest_files "$stage/manifest.json")
  if [ -f "$stage/latest.json" ]; then
    while read -r name; do
      [ -n "$name" ] || continue
      if [ ! -f "$stage/$name" ] && [ ! -f "$dir/$name" ]; then
        warn "app/$ch：latest.json 引用的 $name 不存在（半发布防御：保持现状）"
        missing=1
      fi
    done < <(json_url_names url "$stage/latest.json")
  fi
  if [ -f "$stage/magisk_update.json" ]; then
    while read -r name; do
      [ -n "$name" ] || continue
      if [ ! -f "$stage/$name" ] && [ ! -f "$dir/$name" ]; then
        warn "app/$ch：magisk_update.json 引用的 $name 不存在（半发布防御：保持现状）"
        missing=1
      fi
    done < <(json_url_names zipUrl "$stage/magisk_update.json")
  fi
  if [ "$missing" -eq 1 ]; then
    rm -rf "$stage"
    return 1
  fi

  install -d "$dir"
  if [ "$ch" = nightly ]; then wipe_dir_files "$dir"; fi

  # 先资产、后清单
  for f in "$stage"/*; do
    [ -e "$f" ] || continue
    case "$(basename "$f")" in *.json) continue ;; esac
    mv -f "$f" "$dir/"
  done
  for f in "$stage"/*.json; do
    [ -e "$f" ] || continue
    mv -f "$f" "$dir/"
  done
  rm -rf "$stage"
  local skip_note=""
  [ "$skipped" -gt 0 ] && skip_note="（跳过 $skipped 个旧 tag 同名未版本化资产）"
  info "app/$ch：已发布 $copied 个资产 + 清单 → $dir$skip_note"
  prune_dir "$dir" "$dir/manifest.json" app
  return 0
}

# ---------------------------------------------------------------------------
# 9) 状态文件
# ---------------------------------------------------------------------------
channel_latest() {
  local manifest="$1"
  sed -nE 's/.*"latest"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/p' "$manifest" 2>/dev/null | head -n 1
}

channel_versions() {
  local manifest="$1"
  sed -nE 's/.*"version"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/p' "$manifest" 2>/dev/null | tr '\n' ','
}

dir_bytes() {
  local dir="$1"
  [ -d "$dir" ] || { echo 0; return 0; }
  du -sb "$dir" 2>/dev/null | cut -f1
}

build_status_input() {
  local out="$work/status.tsv" ch dir appdir source latest core_bytes app_bytes versions
  : > "$out"
  for ch in $CHANNELS; do
    dir="$DEST/$ch"
    appdir="$UPGRADE_DEST/$ch"
    # 只有 app（或只有 core）也算存在：空通道才不汇报
    [ -d "$dir" ] || [ -d "$appdir" ] || continue
    source="$dir/manifest.json"
    [ -f "$source" ] || source="$appdir/manifest.json"
    latest="$(channel_latest "$source")"
    versions="$(channel_versions "$source")"
    core_bytes="$(dir_bytes "$dir")"
    app_bytes="$(dir_bytes "$appdir")"
    printf '%s\t%s\t%s\t%s\t%s\n' "$ch" "$latest" "$core_bytes" "$app_bytes" "$versions" >> "$out"
  done
}

# 写两份状态：$MIRROR_STATUS（新 schema，按通道）+ $MIRROR_STATUS_COMPAT（旧 schema）。
# 发布机看门狗读旧的，并且用**文件 mtime** 判「同步停滞」——所以正常轮次也必须写，
# 早退路径（没有可同步的 release）同样要写，否则 2 小时后误报。
write_status_files() {  # $1=result(ok|waiting|error) $2=detail（空 = 按通道自动生成）
  local result="$1" detail="$2"
  touch "$work/status.tsv"
  python3 "$SCRIPT_DIR/mirror_status.py" \
    --path "$MIRROR_STATUS" \
    --compat-path "$MIRROR_STATUS_COMPAT" \
    --channels-file "$work/status.tsv" \
    --repo "$REPO" \
    --dest "$DEST" \
    --upgrade-dest "$UPGRADE_DEST" \
    --result "$result" \
    --detail "$detail" \
    --since-tag "$SINCE_TAG" \
    --max-bytes "$MAX_BYTES"
}

# ---------------------------------------------------------------------------
# 10) dry-run：只打印计划
# ---------------------------------------------------------------------------
print_plan() {
  local ch tag total=0 core_n app_n legacy
  info "dry-run：不下载、不写盘"
  info "分类结果：$(for ch in $CHANNELS; do printf '%s=%s ' "$ch" "$(count_lines "$work/tags.$ch")"; done)"
  legacy="$(paste -sd' ' "$work/skipped.tsv" 2>/dev/null || true)"
  if [ -n "$legacy" ]; then
    info "跳过（legacy / 非通道 tag）：$legacy"
  fi
  info "保留策略：stable core=$KEEP_CORE_STABLE app=$KEEP_APP_STABLE；rc=$KEEP_RC beta=$KEEP_BETA dev=$KEEP_DEV；nightly=1"
  info "MAX_BYTES=$MAX_BYTES（core+app 合计预算，超限从最旧版本收缩窗口；0 = 不限）"
  [ -n "$SINCE_TAG" ] && info "SINCE_TAG=$SINCE_TAG（只约束 stable）"

  for ch in $CHANNELS; do
    core_n="$(count_lines "$work/core.$ch")"
    app_n="$(count_lines "$work/app.$ch")"
    if [ "$core_n" -eq 0 ] && [ "$app_n" -eq 0 ]; then
      info "[$ch] 无 release，不创建目录"
      continue
    fi
    info "[$ch] core 同步 $core_n 个 tag：$(paste -sd' ' "$work/core.$ch" 2>/dev/null || true)"
    if [ "$app_n" -gt 0 ] && ! cmp -s "$work/core.$ch" "$work/app.$ch"; then
      info "[$ch] app  同步 $app_n 个 tag：$(paste -sd' ' "$work/app.$ch" 2>/dev/null || true)"
    fi
    while read -r tag; do
      [ -n "$tag" ] || continue
      info "[$ch] core → $DEST/$ch/manifest.json"
      for p in $PLATFORMS; do
        info "[$ch] core → $DEST/$ch/${p}-${tag}.zip（预计名，实际以 Release 资产名为准）"
      done
    done < "$work/core.$ch"
    [ "$app_n" -gt 0 ] && info "[$ch] app  → $UPGRADE_DEST/$ch/{manifest.json,latest.json,magisk_update.json} + 安装包/签名"
    total=$((total + core_n + app_n))
  done

  info "别名：$DEST/manifest.json → stable/manifest.json"
  info "别名：$UPGRADE_DEST/manifest.json → stable/manifest.json"
  info "别名：$UPGRADE_DEST/latest.json → stable/latest.json"
  info "别名：$UPGRADE_DEST/magisk_update.json → stable/magisk_update.json"
  info "状态文件：$MIRROR_STATUS（channels.{stable,rc,beta,dev,nightly}）"
  info "状态文件：$MIRROR_STATUS_COMPAT（旧 schema 兼容，发布机看门狗读它）"
  info "旧布局兼容：缺失的 $DEST/<zip> 会补符号链接 → stable/"
  info "dry-run 结束：共 $total 个通道-版本待同步"
}

# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------
collect_release_metadata
build_channel_lists
for ch in $CHANNELS; do
  select_tags "$ch"
done

if [ "$DRY_RUN" = 1 ]; then
  print_plan
  exit 0
fi

total_tags=0
for ch in $CHANNELS; do
  total_tags=$((total_tags + $(count_lines "$work/tags.$ch")))
done
if [ "$total_tags" -eq 0 ]; then
  info "没有可同步的 release，退出"
  # 仍然刷新状态文件：否则看门狗按 mtime 判「同步停滞」
  write_status_files waiting "没有满足 SINCE_TAG=${SINCE_TAG:-（不限）} 的 release，等待下一轮"
  exit 0
fi

install -d "$DEST" "$UPGRADE_DEST"
download_channels

declare -A CORE_OK=()
declare -A APP_OK=()
failed=()
rc=0
for ch in $CHANNELS; do
  rc=0
  publish_core "$ch" || rc=$?
  case "$rc" in
    0) CORE_OK["$ch"]=1 ;;
    2) info "core/$ch：无新内容，跳过" ;;
    *) warn "core/$ch 发布未完成（保留旧内容）"; failed+=("core/$ch") ;;
  esac
  rc=0
  publish_app "$ch" || rc=$?
  case "$rc" in
    0) APP_OK["$ch"]=1 ;;
    2) info "app/$ch：无新内容，跳过" ;;
    *) warn "app/$ch 发布未完成（保留旧内容）"; failed+=("app/$ch") ;;
  esac
done

# 增量状态：只记录真正发布成功的通道
for ch in $CHANNELS; do
  [ "${CORE_OK[$ch]:-0}" = 1 ] || continue
  write_sync_state "$ch"
  if [ "$ch" = nightly ]; then
    write_published_at "$ch" "$(head -n 1 "$work/core.$ch")"
  fi
done

# 最后才是 stable 别名（原子替换链接）
link_alias stable/manifest.json "$DEST/manifest.json"
link_alias stable/manifest.json "$UPGRADE_DEST/manifest.json"
link_alias stable/latest.json "$UPGRADE_DEST/latest.json"
link_alias stable/magisk_update.json "$UPGRADE_DEST/magisk_update.json"

# 老布局 / 看门狗兼容：$DEST/<zip> 必须能解析到实际文件（现放在 stable/ 下）
link_legacy_core_files "$DEST/stable/manifest.json" "$DEST"

build_status_input
if [ "${#failed[@]}" -gt 0 ]; then
  # 通道半发布/下载失败只影响该通道（旧内容保持），按「等待」上报：
  # 看门狗只在等待超过 24h 时才升级告警，不会把发布窗口刷成红灯。
  write_status_files waiting "通道发布未完成（已保持旧内容）：${failed[*]}"
else
  write_status_files ok ""
fi

# 旧布局遗留在 DEST 根上的 zip 不删：老客户端缓存过的旧清单还指向它们。
legacy_root="$(find "$DEST" -maxdepth 1 -type f -name '*.zip' | wc -l | tr -d ' ')"
if [ "$legacy_root" -gt 0 ]; then
  info "注意：$DEST 根上仍有 $legacy_root 个旧布局 zip（不自动清理，确认无老客户端后再手动删）"
fi

for ch in $CHANNELS; do
  [ -d "$DEST/$ch" ] || continue
  info "$ch：core $(find "$DEST/$ch" -maxdepth 1 -type f ! -name '.*' | wc -l | tr -d ' ') 个文件 / app $(find "$UPGRADE_DEST/$ch" -maxdepth 1 -type f ! -name '.*' 2>/dev/null | wc -l | tr -d ' ') 个文件"
done
info "完成：$DEST $(du -sh "$DEST" | cut -f1)，$UPGRADE_DEST $(du -sh "$UPGRADE_DEST" | cut -f1)"
