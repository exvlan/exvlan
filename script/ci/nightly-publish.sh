#!/usr/bin/env bash
#
# 发布 nightly：固定 tag `nightly` + 同一个 release 原地覆盖（见 docs/decisions-7.3.md §14.13）。
#
#   · tag `nightly` 强推到本轮 commit；tag 不是 `v*`，且 GITHUB_TOKEN 推送不会级联
#     触发其它工作流 —— 正式发版（release.yml 的 `if: github.ref_type == 'tag'`）不受影响；
#   · release 已存在 → 先删掉本轮不再产出的旧资产，再 `gh release upload --clobber`；
#     不存在 → `gh release create --prerelease --latest=false`；
#   · body：固定说明 + commit/run 链接 + 最近 10 条 commit 标题；
#   · 收尾清理历史遗留的 `v*-nightly.*` release（nightly 只保留 1 条）。
#
# 输入（env）：
#   GH_TOKEN            必填（gh CLI）
#   GITHUB_REPOSITORY   必填
#   GITHUB_SHA          必填
#   GITHUB_RUN_ID       必填
#   VERSION             必填（nightly 版本串）
#   OUT                 资产目录（默认 zipped_assets）
#   TAG                 固定 tag（默认 nightly）

set -euo pipefail

: "${GH_TOKEN:?nightly-publish: 需要 GH_TOKEN}"
: "${GITHUB_REPOSITORY:?nightly-publish: 需要 GITHUB_REPOSITORY}"
: "${GITHUB_SHA:?nightly-publish: 需要 GITHUB_SHA}"
: "${GITHUB_RUN_ID:?nightly-publish: 需要 GITHUB_RUN_ID}"
: "${VERSION:?nightly-publish: 需要 VERSION}"

out="${OUT:-zipped_assets}"
tag="${TAG:-nightly}"
server="${GITHUB_SERVER_URL:-https://github.com}"
repo="$GITHUB_REPOSITORY"
sha="$GITHUB_SHA"
commit_url="${server}/${repo}/commit/${sha}"
run_url="${server}/${repo}/actions/runs/${GITHUB_RUN_ID}"

[ -d "$out" ] || {
  echo "::error::nightly-publish: 资产目录不存在 ${out}" >&2
  exit 1
}

body=$(mktemp)
trap 'rm -f "$body"' EXIT

{
  echo "Development builds from \`develop\`. This prerelease is updated in place."
  echo
  echo "- **版本**：\`${VERSION}\`"
  echo "- **commit**：[${sha:0:8}](${commit_url})"
  echo "- **run**：[${GITHUB_RUN_ID}](${run_url})"
  echo
  echo "### 最近 10 个提交"
  echo
  git log -10 --pretty='- %s (`%h`)'
} >"$body"

# 1) 固定 tag 强推到本轮 commit
# `-c tag.gpgSign=false`：CI 没有 gpg 密钥；某些环境全局开了 tag.gpgSign，
# 不加这个会卡在口令输入上（runner 上表现为任务挂死）。
git -c tag.gpgSign=false tag -f "$tag" "$sha" >/dev/null
git push --force origin "refs/tags/${tag}"

# 2) release：已存在就原地覆盖，否则新建
if gh release view "$tag" --repo "$repo" >/dev/null 2>&1; then
  echo "nightly release 已存在，原地覆盖"
  # 先清掉上一轮有、这一轮没有的资产（zip 名带版本号，不清理会一直堆积）
  keep=$(cd "$out" && printf '%s\n' *)
  gh release view "$tag" --repo "$repo" --json assets \
    --jq '.assets[] | "\(.id) \(.name)"' |
    while read -r id name; do
      [ -n "$id" ] || continue
      if ! printf '%s\n' "$keep" | grep -qxF "$name"; then
        gh api -X DELETE "repos/${repo}/releases/assets/${id}" >/dev/null
        echo "删除旧资产: $name"
      fi
    done
  gh release upload "$tag" "$out"/* --repo "$repo" --clobber
  gh release edit "$tag" --repo "$repo" --title "Nightly" --prerelease --latest=false \
    --notes-file "$body"
else
  echo "创建 nightly release"
  gh release create "$tag" --repo "$repo" --title "Nightly" --prerelease --latest=false \
    --target "$sha" --notes-file "$body" "$out"/*
fi

# 3) nightly 只保留 1 条：清掉历史上 `v*-nightly.*` 之类的旧 release
gh release list --repo "$repo" --limit 100 --json tagName \
  --jq '.[].tagName' |
  while read -r old; do
    case "$old" in
      *-nightly.*)
        gh release delete "$old" --repo "$repo" --yes --cleanup-tag
        echo "清理旧 nightly release: $old"
        ;;
    esac
  done

echo "nightly 已发布：${server}/${repo}/releases/tag/${tag}（${VERSION}）"
