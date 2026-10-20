#!/usr/bin/env bash
# 清理 Actions 运行历史（GitHub 无批量删除，走 REST 循环）。
# 用法: clean_runs.sh [--failed] [--all] [--repo <owner/repo>]
#   默认：仅清理**本仓 workflow**（run.path 以 .github/workflows/ 开头）的非 success 运行；
#         GitHub 自动的 "Graph Update" / Dependabot "Update" 运行会保留（它们会自行重生）。
#   --failed  仅删 conclusion=failure
#   --all     连 GitHub 自动运行一起删（旧行为）
set -uo pipefail

REPO="${REPO:-$(git remote get-url origin 2>/dev/null | sed -E 's#\.git$##; s#.*[:/]([^/]+/[^/]+)$#\1#')}"
ONLY_FAILED=0
ALL=0
while [ $# -gt 0 ]; do
  case "$1" in
    --failed) ONLY_FAILED=1; shift;;
    --all) ALL=1; shift;;
    --repo) REPO="$2"; shift 2;;
    *) shift;;
  esac
done
[ -z "${REPO:-}" ] && { echo "无法推断仓库，请用 --repo owner/repo 或设置 REPO" >&2; exit 2; }

if [ "$ONLY_FAILED" = 1 ]; then
  CONCL='select(.conclusion=="failure")'
else
  CONCL='select(.conclusion!="success")'
fi
if [ "$ALL" = 1 ]; then
  SCOPE='.'
else
  SCOPE='select((.path // "") | startswith(".github/workflows/"))'
fi
FILTER="$SCOPE | $CONCL"

echo "repo=$REPO  范围=$([ "$ALL" = 1 ] && echo 全部 || echo 仅本仓workflow)  过滤=$([ "$ONLY_FAILED" = 1 ] && echo failure || echo '非 success')"

mapfile -t ids < <(gh api --paginate "repos/$REPO/actions/runs?per_page=100" --jq ".workflow_runs[]|$FILTER|.id" 2>/dev/null)
echo "待删除: ${#ids[@]}"
ok=0; fail=0
for id in "${ids[@]}"; do
  [ -z "$id" ] && continue
  if gh api -X DELETE "repos/$REPO/actions/runs/$id" >/dev/null 2>&1; then ok=$((ok+1));
  else fail=$((fail+1)); echo "  fail: $id（可能仍在运行/权限不足）"; fi
done
echo "deleted $ok, failed $fail"
echo "剩余统计: $(gh api --paginate "repos/$REPO/actions/runs?per_page=100" --jq '[.workflow_runs[].conclusion]|group_by(.)|map({(.[0]//"none"):length})|add' 2>/dev/null)"
