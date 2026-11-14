#!/usr/bin/env bash
#
# 飞书文本告警（无人值守 / nightly 失败用）。
#
# 与 pipeline.yml 里 `notify` 作业的做法一致：POST `{"msg_type":"text",...}`，
# 未配置 webhook 时只打一条 notice 直接成功，绝不阻断流水线。
#
# 用法：
#   FEISHU_WEBHOOK=... bash script/ci/feishu-notify.sh "多行文本"

set -euo pipefail

webhook="${FEISHU_WEBHOOK:-}"
text="${1:-}"

if [ -z "$webhook" ]; then
  echo "::notice::未配置 FEISHU_WEBHOOK，跳过飞书告警"
  exit 0
fi
if [ -z "$text" ]; then
  echo "feishu-notify: 缺少消息内容" >&2
  exit 1
fi

body=$(python3 -c 'import json, sys; print(json.dumps({"msg_type": "text", "content": {"text": sys.argv[1]}}))' "$text")
curl -sS -X POST -H 'Content-Type: application/json' -d "$body" "$webhook" -o /dev/null -w 'feishu=%{http_code}\n'
