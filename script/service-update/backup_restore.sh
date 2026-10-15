#!/usr/bin/env bash
# 备份/回滚本地 exvlan 服务：4 个二进制 + et_machine_id + /etc/exvlan。
# 用法:
#   backup_restore.sh backup  [--bin-dir /usr/local/bin] [--cfg-dir /etc/exvlan] [--bak-root /root/exvlan-bak]
#   backup_restore.sh restore [--bak <dir>]
#   backup_restore.sh list
set -uo pipefail

BIN_DIR=/usr/local/bin; CFG_DIR=/etc/exvlan; BAK_ROOT=/root/exvlan-bak
BINS=(exvlan exvlan-cli exvlan-api exvlan-api-embed)
CMD="${1:-}"; shift || true
BAK=""
while [ $# -gt 0 ]; do case "$1" in
  --bin-dir) BIN_DIR="$2"; shift 2;; --cfg-dir) CFG_DIR="$2"; shift 2;;
  --bak-root) BAK_ROOT="$2"; shift 2;; --bak) BAK="$2"; shift 2;; *) shift;;
esac; done

case "$CMD" in
  backup)
    TS="$(date +%Y%m%d-%H%M%S)"; BAK="$BAK_ROOT/$TS"; mkdir -p "$BAK"
    for b in "${BINS[@]}"; do [ -f "$BIN_DIR/$b" ] && cp -a "$BIN_DIR/$b" "$BAK/"; done
    [ -f "$BIN_DIR/et_machine_id" ] && cp -a "$BIN_DIR/et_machine_id" "$BAK/"
    [ -d "$CFG_DIR" ] && tar czf "$BAK/etc-exvlan.tgz" -C / "${CFG_DIR#/}"
    echo "已备份到: $BAK"; ls -l "$BAK";;
  restore)
    [ -z "$BAK" ] && BAK="$(ls -dt "$BAK_ROOT"/* 2>/dev/null | head -1)"
    [ -z "$BAK" ] || [ ! -d "$BAK" ] && { echo "用法: backup_restore.sh restore --bak <dir>" >&2; exit 2; }
    echo "从 $BAK 回滚..."
    systemctl stop exvlan 2>/dev/null || true
    for b in "${BINS[@]}"; do [ -f "$BAK/$b" ] && install -m0755 "$BAK/$b" "$BIN_DIR/$b" && echo "  还原 $b"; done
    [ -f "$BAK/et_machine_id" ] && install -m0755 "$BAK/et_machine_id" "$BIN_DIR/et_machine_id"
    [ -f "$BAK/etc-exvlan.tgz" ] && tar xzf "$BAK/etc-exvlan.tgz" -C /
    systemctl start exvlan 2>/dev/null || true
    echo "回滚完成: $("$BIN_DIR/exvlan" --version 2>/dev/null)";;
  list)
    ls -dt "$BAK_ROOT"/* 2>/dev/null || echo "(无备份)";;
  *)
    echo "用法: backup_restore.sh backup|restore|list [--bin-dir D] [--cfg-dir D] [--bak-root D] [--bak D]" >&2; exit 2;;
esac
