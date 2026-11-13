#!/usr/bin/env bash
#
# 路径守卫：绝对路径字面量只允许出现在 `paths.rs`。
#
# 规则见 docs/paths.md —— 三端路径统一由 `paths.rs` 定义，
# 其它文件出现 `/Library/...`、`/var/...`、`C:\ProgramData\...` 一律红灯。

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

# 每个“组件根”允许在自己的 paths 定义里出现字面量，其余文件一律禁止。
pairs=(
  "packages/studio/src-tauri/src:packages/studio/src-tauri/src/paths.rs"
  "exvlan/control/src:exvlan/control/src/paths.rs"
  # 服务的路径定义目前仍在 main.rs 的 paths() 里（后续统一到 exvlan/control）
  "exvlan/service/src:exvlan/service/src/main.rs"
)

hits=""
for pair in "${pairs[@]}"; do
  src_dir="${pair%%:*}"
  allow="${pair##*:}"
  out=$(
    grep -rnE \
      --include='*.rs' \
      '"(/Library/(Application Support|Logs|LaunchDaemons|PrivilegedHelperTools)|/var/(lib|log|run)|/opt/|/etc/systemd|/tmp/)|r"C:\\ProgramData' \
      "$src_dir" \
      | grep -v "^${allow}:" \
      || true
  )
  if [[ -n "$out" ]]; then
    hits+="${out}"$'\n'
  fi
done

if [[ -n "$hits" ]]; then
  echo "以下文件出现了绝对路径字面量，请改成 paths 定义里的函数/常量："
  echo "$hits"
  exit 1
fi

echo "paths 守卫通过：绝对路径字面量仅存在于各组件根 paths 定义"
