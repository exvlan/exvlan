#!/usr/bin/env bash
# 校验 EXVlan 分支不变量（develop 线性、上游非祖先、release 已并入 feature）。
# 用法: check_invariants.sh [upstream-release-tag]   （默认从 manifest 读取）
set -u

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "不在 git 仓库内" >&2; exit 2; }
cd "$ROOT" || exit 2

MAP="docs/rules/workflow/upstream-path-map.yaml"
REL="${1:-}"
if [ -z "$REL" ]; then
  REL="$(grep -E '^upstream_ref:' "$MAP" 2>/dev/null | sed -E 's#.*releases/##; s/[[:space:]]//g')"
fi
[ -n "$REL" ] || REL="v2.7.0"

fail=0
pass() { echo "  ✅ $1"; }
bad()  { echo "  ❌ $1"; fail=1; }

echo "== 不变量校验（release=$REL）=="

# 1) develop 线性：0 merge
n="$(git rev-list --merges --count develop 2>/dev/null || echo err)"
if [ "$n" = "0" ]; then pass "develop merge 数 = 0"; else bad "develop merge 数 = $n（应为 0）"; fi

# 2) develop 不含上游链
for ref in upstream/main "upstream/releases/$REL"; do
  if git merge-base --is-ancestor "$ref" develop 2>/dev/null; then
    bad "$ref 是 develop 祖先（不应）"
  else
    pass "$ref 非 develop 祖先"
  fi
done

# 3) release 已并入 feature
if git merge-base --is-ancestor "upstream/releases/$REL" feature 2>/dev/null; then
  pass "upstream/releases/$REL 是 feature 祖先"
else
  bad "upstream/releases/$REL 非 feature 祖先（尚未同步？）"
fi

echo "--"
if [ "$fail" = "0" ]; then echo "✅ 不变量全部通过"; else echo "❌ 存在不变量违规"; fi
exit "$fail"
