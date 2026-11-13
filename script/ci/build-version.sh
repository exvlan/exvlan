#!/usr/bin/env bash
#
# 计算并导出 `EXVLAN_BUILD_VERSION`，供 Rust 构建脚本注入内核版本串。
#
# 规则（见 README_CN.md「年份版本规范」）：
#   <标准版本>[-<channel>.<n>][+<sha6>]
#   · tag `v26.410.20712-pre.8` → `26.410.20712-pre.8+abc123`
#   · 分支构建                 → `26.410.20712+abc123`
#
# 用法：
#   bash script/ci/build-version.sh            # 版本基线取自 tauri.conf.json
#   bash script/ci/build-version.sh 26.410.20712
#
# 在 GitHub Actions 中会把结果写入 `$GITHUB_ENV`，本地执行则直接打印。

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

base="${1:-}"
if [[ -z "$base" ]]; then
  base=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
    packages/studio/src-tauri/tauri.conf.json | head -1)
fi
if [[ -z "$base" ]]; then
  echo "build-version: 无法确定基线版本" >&2
  exit 1
fi

# 显式覆盖优先：nightly 的 tag 固定叫 `nightly`（不是 v*），版本串里的
# `-nightly.<MMDD>` 只能由 workflow 注入。
if [[ -n "${EXVLAN_BUILD_VERSION_OVERRIDE:-}" ]]; then
  version="${EXVLAN_BUILD_VERSION_OVERRIDE}"
else
  ref="${GITHUB_REF_NAME:-$(git rev-parse --abbrev-ref HEAD)}"
  if [[ "$ref" == v* ]]; then
    version="${ref#v}"
  else
    version="$base"
  fi
fi

sha="${GITHUB_SHA:-$(git rev-parse HEAD)}"
version="${version}+${sha:0:6}"

if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "EXVLAN_BUILD_VERSION=${version}" >>"$GITHUB_ENV"
fi
echo "EXVLAN_BUILD_VERSION=${version}"
