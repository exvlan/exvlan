#!/usr/bin/env bash
#
# 计算 nightly 构建版本串：`<base 的 patch +1>-nightly.<MMDD>`
#
# 规则（docs/decisions-7.3.md §14.13「nightly 版本」）：
#   · patch +1：nightly 永远高于同 base 的 stable，用户从 nightly 回到正式版
#     时才不会被 updater 判成「降级」而卡在旧包上；
#   · MMDD 取**北京时间**：调度 21:00 UTC = 次日 05:00 CST，包名里的日期要和
#     「用户下载那天」一致，否则跨零点前后两轮会写出同一天的文件名不同内容。
#
# 用法：
#   bash script/ci/nightly-version.sh                          # 打印版本串
#   NIGHTLY_NOW=2026-11-15 bash script/ci/nightly-version.sh    # 固定时钟（测试用）
#
# 跨年守卫：base 首段是年份后两位（26 → 2026）。进入新的一年后 base 还没升时，
# 算出来的 nightly（26.x）会顶在「27.x 的 stable」之上，客户端再也回不到正式版
# —— 直接失败，要求先把 base 升上去。

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)

base=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
  "$repo_root/packages/studio/src-tauri/tauri.conf.json" | head -1)
if [[ -z "$base" ]]; then
  echo "nightly-version: 无法从 packages/studio/src-tauri/tauri.conf.json 读出 base 版本" >&2
  exit 1
fi

IFS=. read -r yy qmm mmpp <<<"$base"
for part in "$yy" "$qmm" "$mmpp"; do
  if [[ ! "$part" =~ ^[0-9]+$ ]]; then
    echo "nightly-version: base 版本格式不合法：$base（期望 YY.QMM.Mmpp）" >&2
    exit 1
  fi
done

now=${NIGHTLY_NOW:-$(TZ=Asia/Shanghai date +%Y-%m-%d)}
if [[ ! "$now" =~ ^([0-9]{4})-([0-9]{2})-([0-9]{2})$ ]]; then
  echo "nightly-version: NIGHTLY_NOW 格式不合法：$now（期望 YYYY-MM-DD）" >&2
  exit 1
fi
now_year=${BASH_REMATCH[1]}
mmdd="${BASH_REMATCH[2]}${BASH_REMATCH[3]}"

# 10# 前缀：`08`/`09` 这类前导零不要被当成八进制。
base_year="20$((10#$yy))"
if [[ "$base_year" != "$now_year" ]]; then
  echo "nightly-version: base 已跨年，请先升到 $((10#$yy + 1)).x（base=$base → $base_year，当前 $now_year）" >&2
  exit 1
fi

printf '%s.%s.%s-nightly.%s\n' "$yy" "$qmm" "$((10#$mmpp + 1))" "$mmdd"
