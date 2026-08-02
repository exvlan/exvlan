#!/usr/bin/env bash

REPO="meta-club/exvlan"
RUN_ID="25345053354"

# 1. 获取所有包含失败步骤的 Job 信息，以 Base64 编码一行一个，防止空格导致解析错误
JOBS_DATA=$(gh run view $RUN_ID --repo $REPO --json jobs \
--jq '.jobs[] | select(.steps[].conclusion == "failure") | {id: .databaseId, job_name: .name, failed_step: (.steps[] | select(.conclusion == "failure") | .name)} | @base64')

for row in $JOBS_DATA; do
    # 解码 JSON 字段
    _decode() { echo ${row} | base64 --decode | jq -r ${1}; }
    
    JOB_ID=$(_decode '.id')
    JOB_NAME=$(_decode '.job_name')
    STEP_NAME=$(_decode '.failed_step')

    echo "=========================================================="
    echo "❌ 任务名称: $JOB_NAME"
    echo "🔍 失败步骤: $STEP_NAME"
    echo "🆔 Job ID  : $JOB_ID"
    echo "--------------------------- 日志提取 ---------------------------"

    # 获取日志并精准提取失败步骤
    # 使用 fgrep 定位纯文本行号，避免 sed 处理括号报错
    FULL_LOG=$(gh run view --job "$JOB_ID" --repo "$REPO" --log)
    
    if [ -z "$FULL_LOG" ]; then
        # echo "[提示] 无法获取日志，请检查权限或 Job 是否已过期。"
        # echo "请尝试手动执行验证: gh run view --job $JOB_ID --repo $REPO --log"
        gh run view --job $JOB_ID --repo $REPO
    else
        # 提取从失败步骤开始后的日志，显示前 30 行关键报错
        echo "$FULL_LOG" | grep -A 30 -F "$STEP_NAME" | grep -v "Post Run" | head -n 35
    fi

    echo -e "==========================================================\n"
done
