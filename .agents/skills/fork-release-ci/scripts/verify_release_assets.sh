#!/usr/bin/env bash
# 校验 GitHub Release：published、资产齐全、正文无禁用字样；可选校验 Go tag / npm 包。
# 用法: verify_release_assets.sh <tag> [--repo owner/repo] [--forbid "<word>"] [--go-prefix exvlan/go] [--npm-owner <org>]
set -uo pipefail

TAG="${1:-}"; shift || true
REPO="${REPO:-$(git remote get-url origin 2>/dev/null | sed -E 's#\.git$##; s#.*[:/]([^/]+/[^/]+)$#\1#')}"
FORBID="${FORBID:-EasyTier}"
GO_PREFIX="${GO_PREFIX:-}"
NPM_OWNER="${NPM_OWNER:-}"
while [ $# -gt 0 ]; do case "$1" in
  --repo) REPO="$2"; shift 2;; --forbid) FORBID="$2"; shift 2;;
  --go-prefix) GO_PREFIX="$2"; shift 2;; --npm-owner) NPM_OWNER="$2"; shift 2;; *) shift;;
esac; done
[ -z "$TAG" ] && { echo "用法: verify_release_assets.sh <tag> [--repo owner/repo] ..." >&2; exit 2; }

rc=0; ok(){ echo "  [OK]   $*"; }; bad(){ echo "  [FAIL] $*"; rc=1; }

echo "== 校验 Release $TAG ($REPO) =="
j="$(gh release view "$TAG" -R "$REPO" --json isDraft,isPrerelease,assets,body 2>/dev/null)" \
  || { echo "取不到 Release $TAG"; exit 1; }
[ "$(echo "$j" | jq -r .isDraft)" = "false" ] && ok "已发布(draft=false)" || bad "仍为 draft"

echo "--- 资产 ---"
echo "$j" | jq -r '.assets[].name' | sed 's/^/  /'
n=$(echo "$j" | jq '.assets|length'); echo "  资产数=$n"
for pat in 'windows-x86_64' 'macos-x86_64' 'linux-x86_64' '\.msi' '\.dmg' '\.apk' 'Magisk' '^manifest\.json$'; do
  echo "$j" | jq -r '.assets[].name' | grep -qiE "$pat" && ok "含 $pat" || bad "缺 $pat"
done

# manifest.json：内核分发清单（GUI 在线升级用）
if echo "$j" | jq -r '.assets[].name' | grep -qx 'manifest.json'; then
  tmp="$(mktemp -d)"
  if gh release download "$TAG" -R "$REPO" -p 'manifest.json' -D "$tmp" >/dev/null 2>&1; then
    ver="$(jq -r '.versions[0].version // empty' "$tmp/manifest.json" 2>/dev/null)"
    latest="$(jq -r '.latest // empty' "$tmp/manifest.json" 2>/dev/null)"
    echo "  manifest: latest=$latest versions=$(jq -r '.versions|length' "$tmp/manifest.json" 2>/dev/null)"
    # manifest 里引用的每个 file 必须在本 Release 资产中
    miss=0
    for f in $(jq -r '.versions[].assets[].file' "$tmp/manifest.json" 2>/dev/null | sort -u); do
      echo "$j" | jq -r '.assets[].name' | grep -qx "$f" || { bad "manifest 引用了不存在的资产: $f"; miss=1; }
    done
    [ "$miss" = "0" ] && ok "manifest 引用完整（${ver:-unknown}）"
  else
    bad "无法下载 manifest.json"
  fi
  rm -rf "$tmp"
fi

if [ -n "$FORBID" ]; then
  echo "$j" | jq -r .body | grep -qiE "$FORBID" && bad "正文含禁用字样: $FORBID" || ok "正文无 '$FORBID'"
fi

if [ -n "$GO_PREFIX" ]; then
  git ls-remote --tags "$(git remote get-url origin)" "*${GO_PREFIX}/${TAG}*" 2>/dev/null | grep -q . \
    && ok "Go tag ${GO_PREFIX}/${TAG} 存在" || bad "Go tag ${GO_PREFIX}/${TAG} 缺失"
fi
if [ -n "$NPM_OWNER" ]; then
  echo "  GitHub Packages(npm): $(gh api "orgs/$NPM_OWNER/packages?package_type=npm" --jq '[.[].name]|join(",")' 2>/dev/null || echo '(不可读)')"
fi

echo
echo "结论：$([ $rc -eq 0 ] && echo '通过' || echo '存在问题')"; exit $rc
