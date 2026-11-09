#!/usr/bin/env bash
#
# 部署诊断 Worker（`exvlan-diag`）。
#
# 本机只有 `cf` CLI 的 OAuth 登录态；实测 wrangler 可以直接把它当作
# `CLOUDFLARE_API_TOKEN` 使用（写权限正常），因此这里统一走 wrangler +
# `wrangler.jsonc`（KV、ratelimits、routes 都在那个文件里）。
#
# 用法：
#   bash deploy.sh                             # 部署代码与 bindings/route
#   bash deploy.sh --set-secret TOKEN_SECRET   # 从 stdin 读值并设置 secret
#
# 凭据优先级：CLOUDFLARE_API_TOKEN → cf CLI 的 OAuth 登录态

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

WRANGLER="./node_modules/.bin/wrangler"
[[ -x "$WRANGLER" ]] || { echo "缺少 wrangler（先在 packages/diagnostics 执行 pnpm/npm install）" >&2; exit 1; }

token="${CLOUDFLARE_API_TOKEN:-}"
if [[ -z "$token" ]]; then
  cfg="${HOME}/.config/cloudflare/config/default.json"
  if [[ ! -f "$cfg" ]]; then
    echo "缺少 CLOUDFLARE_API_TOKEN，且未找到 cf CLI 登录态（$cfg）" >&2
    exit 1
  fi
  token=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['oauth_token'])" "$cfg")
fi
export CLOUDFLARE_API_TOKEN="$token"

if [[ "${1:-}" == "--set-secret" ]]; then
  name="${2:?用法: deploy.sh --set-secret NAME}"
  value=$(cat)
  printf '%s' "$value" | "$WRANGLER" secret put "$name" --name exvlan-diag
  echo "secret $name 已更新"
  exit 0
fi

"$WRANGLER" deploy "$@"
