#!/usr/bin/env bash
# 隔离试运行：把配置复制到临时目录并“去生产化”，前台短跑新版守护进程，验证配置解析与实例启动，
# 不触碰生产网络/TUN/端口。
# 用法: trial_run.sh <new-exvlan-binary> [config-dir=/etc/exvlan] [seconds=25]
set -uo pipefail

BIN="${1:-}"; CFG_DIR="${2:-/etc/exvlan}"; SECS="${3:-25}"
[ -z "$BIN" ] && { echo "用法: trial_run.sh <new-exvlan-binary> [config-dir] [seconds]" >&2; exit 2; }
[ -x "$BIN" ] || { echo "找不到可执行: $BIN" >&2; exit 2; }

T="$(mktemp -d /tmp/exvlan-trial.XXXXXX)"
trap 'rm -rf "$T"' EXIT
echo "== 隔离试运行（$T，$SECS s）=="
for f in "$CFG_DIR"/*.toml; do
  b="$(basename "$f")"
  sed -e 's/^network_name = .*/network_name = "exvlan-trial"/' \
      -e 's/^dev_name = .*/dev_name = ""/' \
      -e '/^\[\[peer\]\]/d' -e '/^uri = /d' "$f" \
    | sed '/^\[flags\]/a no_tun = true' > "$T/$b"
done
ls -l "$T"
# 异端口 15889、无监听、不带 -w（不连 config server）
timeout "$SECS" "$BIN" --config-dir "$T" --rpc-portal 127.0.0.1:15889 --no-listener > "$T/run.log" 2>&1
rc=$?
echo "run exit=$rc（124=到时正常结束）"
echo "--- 日志 tail ---"; tail -25 "$T/run.log"
echo "--- 错误关键字 ---"; grep -iE 'error|panic|failed|invalid|unknown field' "$T/run.log" | grep -viE 'no error' | head -20
[ $rc -eq 124 ] || [ $rc -eq 0 ]
