#!/usr/bin/env bash
# 离线校验：对配置目录下每个 .toml 运行 `exvlan --check-config`（不启动、不触碰运行中的服务）。
# 用法: check_config.sh <new-binary-dir> [config-dir]
set -uo pipefail

BIN_DIR="${1:-}"; CFG_DIR="${2:-/etc/exvlan}"
[ -z "$BIN_DIR" ] && { echo "用法: check_config.sh <new-binary-dir> [config-dir]" >&2; exit 2; }
BIN="$BIN_DIR/exvlan"
[ -x "$BIN" ] || { echo "找不到可执行: $BIN" >&2; exit 2; }

echo "== 新版: $("$BIN" --version 2>&1 | head -1) =="
rc=0
mapfile -t files < <(find "$CFG_DIR" -maxdepth 1 -name '*.toml' | sort)
[ "${#files[@]}" -eq 0 ] && { echo "无 .toml 于 $CFG_DIR" >&2; exit 2; }
for f in "${files[@]}"; do
  if out=$("$BIN" --check-config -c "$f" 2>&1); then echo "  [OK]   $f"; else echo "  [FAIL] $f"; echo "$out" | sed 's/^/         /'; rc=1; fi
done
echo "结论：$([ $rc -eq 0 ] && echo '配置兼容 ✓' || echo '存在不兼容 ✗')"
exit $rc
