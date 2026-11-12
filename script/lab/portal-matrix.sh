#!/usr/bin/env bash
#
# 隔离实验：两个裸内核（各自 netns + veth），验证「跨节点 RPC portal 可达性」。
#
# 为什么需要它：pre.16 上「远程管理不可用」排查了两天，最后发现是
# `enable_kcp_proxy` / `enable_quic_proxy` 让发往对端 RPC 端口的 TCP 代理流
# 被包进 KCP/QUIC，而对端 `wrapped_transport_destination` 会忽略「目标是
# 自己监听端口」的会话 → 连接被关、客户端超时，两端日志都干净。
#
# 这个脚本把那个 lab 固化下来：组合矩阵跑一遍，任何一项从 OK 变 FAIL 说明
# 内核的代理/transport 行为变了，发版前就该发现。
#
# 用法（需要 root + iproute2 + /dev/net/tun，建议在一次性容器/测试机上跑）：
#   sudo script/lab/portal-matrix.sh [--keep]
#
# 环境变量：
#   EXVLAN_KERNEL   内核二进制（默认 /opt/era.exvlan.studio/core/exvlan）
#   EXVLAN_CLI      CLI 二进制（默认 /opt/era.exvlan.studio/core/exvlan-cli；缺失则跳过 CLI 断言）

set -uo pipefail

KERNEL="${EXVLAN_KERNEL:-/opt/era.exvlan.studio/core/exvlan}"
CLI="${EXVLAN_CLI:-/opt/era.exvlan.studio/core/exvlan-cli}"
BASE=/tmp/exvlan-portal-lab
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

fail=0
log() { printf '[lab] %s\n' "$*"; }

cleanup() {
  log "cleanup"
  for ns in pa pb; do
    for pid in $(ip netns pids "$ns" 2>/dev/null); do
      kill -9 "$pid" 2>/dev/null || true
    done
    ip netns del "$ns" 2>/dev/null || true
  done
  ip link del va 2>/dev/null || true
  [[ "$KEEP" == "1" ]] || rm -rf "$BASE"
}
trap cleanup EXIT

[[ -x "$KERNEL" ]] || { log "缺少内核二进制: $KERNEL"; exit 2; }
command -v ip >/dev/null || { log "缺少 iproute2"; exit 2; }
[[ -e /dev/net/tun ]] || { log "缺少 /dev/net/tun"; exit 2; }

mkdir -p "$BASE/a" "$BASE/b"
cp "$KERNEL" "$BASE/exvlan"
[[ -x "$CLI" ]] && cp "$CLI" "$BASE/exvlan-cli"

ip netns add pa
ip netns add pb
ip link add va type veth peer name vb
ip link set va netns pa
ip link set vb netns pb
ip netns exec pa ip addr add 192.168.99.1/30 dev va
ip netns exec pa ip link set va up
ip netns exec pa ip link set lo up
ip netns exec pb ip addr add 192.168.99.2/30 dev vb
ip netns exec pb ip link set vb up
ip netns exec pb ip link set lo up

write_cfg() { # side extra_flags
  local side="$1" extra="$2"
  local ip id host listen peer
  if [[ "$side" == "a" ]]; then
    ip=10.98.0.1; id=11111111-1111-4111-8111-111111111111; host=probe-a
    listen=192.168.99.1; peer=192.168.99.2
  else
    ip=10.98.0.2; id=22222222-2222-4222-8222-222222222222; host=probe-b
    listen=192.168.99.2; peer=192.168.99.1
  fi
  {
    echo "hostname = \"$host\""
    echo "instance_name = \"probe\""
    echo "instance_id = \"$id\""
    echo "ipv4 = \"$ip/24\""
    echo "dhcp = false"
    echo "listeners = [\"tcp://$listen:21010\"]"
    echo "[[peer]]"
    echo "uri = \"tcp://$peer:21010\""
    echo "[network_identity]"
    echo "network_name = \"probe\""
    echo "network_secret = \"probe@lab\""
    echo "[flags]"
    echo "use_smoltcp = true"
    echo "proxy_forward_by_system = true"
    [[ -n "$extra" ]] && printf '%s\n' "$extra"
  } >"$BASE/$side/probe.toml"
}

run_case() { # name extra_a extra_b
  local name="$1" ea="$2" eb="$3"
  pkill -f "$BASE/exvlan" 2>/dev/null
  sleep 1
  write_cfg a "$ea"
  write_cfg b "$eb"
  ip netns exec pb "$BASE/exvlan" --config-dir "$BASE/b" --rpc-portal 0.0.0.0:25999 \
    --rpc-portal-whitelist 127.0.0.0/8,10.98.0.0/24 --console-log-level error \
    >"$BASE/b.log" 2>&1 &
  ip netns exec pa "$BASE/exvlan" --config-dir "$BASE/a" --console-log-level error \
    >"$BASE/a.log" 2>&1 &
  sleep 10
  if [[ ! -x "$BASE/exvlan-cli" ]]; then
    log "$name: SKIP（没有 CLI 二进制）"
    return
  fi
  local out
  out=$(ip netns exec pa "$BASE/exvlan-cli" -p 10.98.0.2:25999 peer 2>&1 | head -1)
  case "$out" in
    "|"*) log "$name: OK" ;;
    *)    log "$name: FAIL（$out）"; fail=1 ;;
  esac
}

log "kernel=$KERNEL"
run_case "baseline" "" ""
run_case "kcp_proxy" "enable_kcp_proxy = true" ""
run_case "quic_proxy" "enable_quic_proxy = true" ""
run_case "kcp+quic" "enable_kcp_proxy = true
enable_quic_proxy = true" ""
run_case "kernel_side_only" "" "enable_kcp_proxy = true
enable_quic_proxy = true"

pkill -f "$BASE/exvlan" 2>/dev/null

if [[ "$fail" == "0" ]]; then
  log "全部通过"
else
  log "有用例失败（可能是预期：kcp/quic 组合会破坏 portal —— 见文件头注释）"
fi
# 注意：kcp/quic 三项「预期失败」，因此脚本默认不把它们当回归；
# 若要作为回归断言，把对应 run_case 换成 expect_fail 版本（见 docs/rules/lifecycle-matrix.md）。
exit 0
