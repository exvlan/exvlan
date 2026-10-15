#!/usr/bin/env bash
# 扫描全仓“品牌字样”残余，并列出应保留的白名单（依赖坐标/License/上游映射）。
# 用法: debrand_scan.sh [brand-pattern]   默认 pattern 由环境变量 BRAND 或第一个参数给出。
#   BRAND=easytier .agents/skills/fork-rebrand-audit/scripts/debrand_scan.sh
set -uo pipefail

BRAND="${1:-${BRAND:-easytier}}"
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$ROOT"

EXCLUDES=(--exclude-dir=node_modules --exclude-dir=target --exclude-dir=.git
          --exclude-dir=dist --exclude-dir=.pnpm-store --exclude-dir=vendor)

echo "== 扫描品牌字样: '$BRAND' (根: $ROOT) =="
echo

echo "--- 命中文件（按命中数排序）---"
grep -rInE "$BRAND" . "${EXCLUDES[@]}" 2>/dev/null \
  | awk -F: '{print $1}' | sort | uniq -c | sort -rn | head -40

echo
echo "--- 白名单（应保留，非“品牌叙述”）---"
for f in Cargo.toml Cargo.lock "$(ls */Cargo.toml 2>/dev/null | head -1)" \
         docs/rules/workflow/upstream-path-map.yaml LICENSE LICENSE.md; do
  if [ -f "$f" ]; then
    n=$(grep -ciE "$BRAND" "$f" 2>/dev/null || echo 0)
    [ "$n" -gt 0 ] && echo "  [依赖坐标/许可证/映射] $f : $n 处（保留）"
  fi
done
grep -rInE "github\.com/[A-Za-z0-9_.-]*${BRAND}|tun-${BRAND}" . "${EXCLUDES[@]}" \
  --include='*.toml' --include='*.lock' 2>/dev/null | head -10 \
  | sed 's/^/  [依赖坐标] /'

echo
echo "--- 非白名单的可疑品牌叙述（建议去名）---"
grep -rInE "$BRAND" . "${EXCLUDES[@]}" --include='*.md' --include='*.yml' --include='*.yaml' \
  --include='*.rs' --include='*.go' --include='*.ts' --include='*.vue' --include='*.json' 2>/dev/null \
  | grep -viE 'github\.com/[A-Za-z0-9_.-]*'"$BRAND"'|tun-'"$BRAND" \
  | head -30

echo
echo "提示：二进制制品请用 'wasm-tools print/parse' 或 'strings' 单列，勿用文本替换。"
