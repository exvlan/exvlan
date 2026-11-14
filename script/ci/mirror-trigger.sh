#!/usr/bin/env bash
# 发版后触发镜像机同步（`/mirror/trigger`，见 docs/mirror-toolchain.md）。
#
# 设计约束：
# - **不能卡死 release**：本步骤任何失败都只 `::warning::`，退出码始终 0；镜像机还有
#   15 min 定时同步兜底，最坏情况只是客户端晚一个周期看到更新。
# - **不泄露凭据**：token 只从 `MIRROR_TRIGGER_TOKEN`（GitHub secret）读，绝不 echo、
#   不开 `curl -v`、不写进日志；URL 里的查询串也不打印。
# - 未配置（首次上线前的分支/tag）则跳过，不报红。
#
# 环境变量：
#   MIRROR_TRIGGER_URL    https://dl.exvlan.era-x.cn/mirror/trigger
#   MIRROR_TRIGGER_TOKEN  与服务端 `MIRROR_TRIGGER_TOKEN_FILE` 内容一致
#   MIRROR_TRIGGER_WAIT   就绪轮询上限秒数（默认 120）
set -uo pipefail

url="${MIRROR_TRIGGER_URL:-}"
token="${MIRROR_TRIGGER_TOKEN:-}"
wait_limit="${MIRROR_TRIGGER_WAIT:-120}"

if [ -z "$url" ] || [ -z "$token" ]; then
  echo "::notice::未配置 MIRROR_TRIGGER_URL / MIRROR_TRIGGER_TOKEN，跳过镜像触发（定时同步兜底）"
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "::warning::缺少 jq，跳过镜像触发"
  exit 0
fi

api="${url%/}"

request() { # $1=method → 输出 "<http_code>\n<body>"
  local method="$1" body status
  body="$(curl -sS -m 20 -X "$method" \
    -H "X-Mirror-Token: $token" \
    -H "Accept: application/json" \
    -w $'\n%{http_code}' \
    "$api" 2>/dev/null)" || {
    echo "000"
    return 0
  }
  status="${body##*$'\n'}"
  body="${body%$'\n'*}"
  printf '%s\n%s\n' "$body" "$status"
}

echo "[mirror] POST ${api}"
post="$(request POST)"
post_status="$(printf '%s' "$post" | tail -n 1)"
post_body="$(printf '%s' "$post" | sed '$d')"

case "$post_status" in
  202)
    echo "[mirror] 已接受触发：$(printf '%s' "$post_body" | jq -c '{running,round,last_run}' 2>/dev/null || echo '{}')"
    ;;
  409)
    echo "[mirror] 镜像机已有一轮同步在进行，改为等待其完成"
    ;;
  401)
    echo "::warning::镜像触发鉴权失败（401）：检查 MIRROR_TRIGGER_TOKEN 与镜像机一致；本轮回退到定时同步"
    exit 0
    ;;
  000)
    echo "::warning::镜像触发请求失败（网络/超时）：回退到定时同步"
    exit 0
    ;;
  *)
    echo "::warning::镜像触发返回 HTTP ${post_status}：回退到定时同步"
    exit 0
    ;;
esac

target_round="$(printf '%s' "$post_body" | jq -r '.round // 0' 2>/dev/null || echo 0)"
case "$target_round" in
  ''|*[!0-9]*) target_round=0 ;;
esac

deadline=$(( SECONDS + wait_limit ))
while [ "$SECONDS" -lt "$deadline" ]; do
  sleep 5
  poll="$(request GET)"
  poll_status="$(printf '%s' "$poll" | tail -n 1)"
  poll_body="$(printf '%s' "$poll" | sed '$d')"
  if [ "$poll_status" != "200" ]; then
    echo "::warning::镜像状态查询返回 HTTP ${poll_status}，停止轮询（同步仍在镜像机上执行）"
    exit 0
  fi
  running="$(printf '%s' "$poll_body" | jq -r '.running // false')"
  result="$(printf '%s' "$poll_body" | jq -r '.result // "null"')"
  round="$(printf '%s' "$poll_body" | jq -r '.round // 0' 2>/dev/null || echo 0)"
  case "$round" in ''|*[!0-9]*) round=0 ;; esac
  if [ "$running" = "false" ] && [ "$round" -ge "$target_round" ]; then
    if [ "$result" = "ok" ]; then
      echo "::notice::镜像同步完成（round=${round}, result=ok）"
    else
      echo "::warning::镜像同步结束但结果=${result}（round=${round}）：镜像机保留旧内容，等待下一轮"
    fi
    printf '%s\n' "$poll_body" | jq -c '{round,running,result,finished_at}' 2>/dev/null || true
    exit 0
  fi
done

echo "::warning::等待镜像同步超时（>${wait_limit}s）：不阻塞发版，镜像机继续执行/定时兜底"
exit 0
