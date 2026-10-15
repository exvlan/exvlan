#!/usr/bin/env bash
# 切换仓库可见性（私仓 Actions 额度耗尽时临时 public 跑 CI，跑完立即 private）。
# 用法: toggle_visibility.sh public|private [--repo owner/repo] [--check]
set -uo pipefail

MODE="${1:-}"; shift || true
REPO="${REPO:-$(git remote get-url origin 2>/dev/null | sed -E 's#.*[:/]([^/]+/[^/]+?)(\.git)?$#\1#')}"
CHECK=0
while [ $# -gt 0 ]; do case "$1" in --repo) REPO="$2"; shift 2;; --check) CHECK=1; shift;; *) shift;; esac; done

show(){ gh repo view "$REPO" --json visibility,isPrivate --jq '"\(.visibility) private=\(.isPrivate)"' 2>/dev/null; }

if [ "$CHECK" = 1 ] || [ -z "$MODE" ]; then echo "当前: $(show)"; [ -z "$MODE" ] && exit 0; fi

case "$MODE" in public|private) ;; *) echo "用法: toggle_visibility.sh public|private [--repo owner/repo] [--check]" >&2; exit 2;; esac

for a in $(seq 1 6); do
  out=$(gh api -X PATCH "repos/$REPO" -f "visibility=$MODE" --jq '.visibility' 2>&1)
  echo "$out" | grep -qi "$MODE" && { echo "已设为 $MODE: $(show)"; exit 0; }
  echo "try $a: $(echo "$out" | tail -1)"; sleep 6
done
echo "切换失败；当前: $(show)"; exit 1
