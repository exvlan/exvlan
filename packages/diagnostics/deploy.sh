#!/usr/bin/env bash
#
# 部署单文件 Worker（不依赖 wrangler 登录态）。
#
# 背景：本机只有 `cf` CLI 的 OAuth 登录态，而 `cf workers versions create`
# 要求 `cloudflare.config.ts`（会走 `--experimental-new-config`），读不了
# `wrangler.jsonc`。这个 Worker 是单文件、无 import，直接调 Cloudflare API
# 上传 ES module + bindings 即可。
#
# 用法：
#   bash deploy.sh                       # 部署代码与 bindings
#   bash deploy.sh --set-secret TOKEN_SECRET   # 从 stdin 读值并设置 secret
#
# 凭据优先级：CLOUDFLARE_API_TOKEN → cf CLI 的 OAuth 登录态
# 账号：CLOUDFLARE_ACCOUNT_ID，缺省取第一个账号

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

WORKER="${WORKER:-exvlan-diag}"
SCRIPT="src/index.js"
KV_ID="${KV_ID:-56e5109429704b1abf8a17e46b922d78}"
COMPAT_DATE="${COMPAT_DATE:-2026-01-01}"

token="${CLOUDFLARE_API_TOKEN:-}"
if [[ -z "$token" ]]; then
  cfg="${HOME}/.config/cloudflare/config/default.json"
  if [[ ! -f "$cfg" ]]; then
    echo "缺少 CLOUDFLARE_API_TOKEN，且未找到 cf CLI 登录态（$cfg）" >&2
    exit 1
  fi
  token=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['oauth_token'])" "$cfg")
fi

api="https://api.cloudflare.com/client/v4"

account="${CLOUDFLARE_ACCOUNT_ID:-}"
if [[ -z "$account" ]]; then
  account=$(
    curl -sS -H "authorization: Bearer $token" "$api/accounts" |
      python3 -c "import json,sys;print(json.load(sys.stdin)['result'][0]['id'])"
  )
fi

if [[ "${1:-}" == "--set-secret" ]]; then
  name="${2:?用法: deploy.sh --set-secret NAME}"
  value=$(cat)
  payload=$(python3 -c "import json,sys;print(json.dumps({'name':sys.argv[1],'text':sys.argv[2]}))" "$name" "$value")
  curl -sS -X PUT "$api/accounts/$account/workers/scripts/$WORKER/secrets" \
    -H "authorization: Bearer $token" \
    -H "content-type: application/json" \
    --data "$payload" |
    python3 -c "import json,sys;d=json.load(sys.stdin);print('set secret:',d.get('success'),d.get('errors'));sys.exit(0 if d.get('success') else 1)"
  exit 0
fi

metadata=$(python3 - "$KV_ID" "$COMPAT_DATE" <<'PY'
import json
import sys

kv_id, compat = sys.argv[1], sys.argv[2]
print(
    json.dumps(
        {
            "main_module": "index.js",
            "compatibility_date": compat,
            "bindings": [
                {"type": "kv_namespace", "name": "DIAG_KV", "namespace_id": kv_id},
                {
                    "type": "ratelimit",
                    "name": "RL_TOKEN",
                    "namespace_id": "1010",
                    "simple": {"limit": 10, "period": 60},
                },
                {
                    "type": "ratelimit",
                    "name": "RL_UPLOAD",
                    "namespace_id": "1011",
                    "simple": {"limit": 20, "period": 60},
                },
            ],
        }
    )
)
PY
)

echo "部署 $WORKER（account=$account）…"
curl -sS -X PUT "$api/accounts/$account/workers/scripts/$WORKER" \
  -H "authorization: Bearer $token" \
  -F "metadata=$metadata;type=application/json" \
  -F "index.js=@$SCRIPT;type=application/javascript+module" |
  python3 -c "import json,sys;d=json.load(sys.stdin);print('deploy:',d.get('success'),d.get('errors'));sys.exit(0 if d.get('success') else 1)"
