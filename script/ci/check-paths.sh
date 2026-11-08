#!/usr/bin/env bash
#
# 路径守卫：绝对路径字面量只允许出现在 `paths.rs`。
#
# 规则见 docs/paths.md —— 三端路径统一由 `paths.rs` 定义，
# 其它文件出现 `/Library/...`、`/var/...`、`C:\ProgramData\...` 一律红灯。

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

src_dir="packages/studio/src-tauri/src"
allow="packages/studio/src-tauri/src/paths.rs"

hits=$(
  grep -rnE \
    --include='*.rs' \
    '"(/Library/(Application Support|Logs|LaunchDaemons|PrivilegedHelperTools)|/var/(lib|log|run)|/opt/|/etc/systemd|/tmp/)|r"C:\\ProgramData' \
    "$src_dir" \
    | grep -v "^${allow}:" \
    || true
)

if [[ -n "$hits" ]]; then
  echo "以下文件出现了绝对路径字面量，请改成 crate::paths 提供的函数/常量："
  echo "$hits"
  exit 1
fi

echo "paths 守卫通过：绝对路径字面量仅存在于 ${allow}"
