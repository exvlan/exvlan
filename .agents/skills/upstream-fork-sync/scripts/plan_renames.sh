#!/usr/bin/env bash
# 生成（或执行）v2.7.0 合并期的搬迁/重命名计划。
# 默认仅打印计划；传 --exec 才实际执行（谨慎）。
# 用法: plan_renames.sh [--exec]
set -u

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "不在 git 仓库内" >&2; exit 2; }
cd "$ROOT" || exit 2

MAP="docs/rules/workflow/upstream-path-map.yaml"
EXEC=0
[ "${1:-}" = "--exec" ] && EXEC=1

run() {
  echo "    $*"
  if [ "$EXEC" = "1" ]; then
    eval "$@" || { echo "  !! 执行失败: $*" >&2; exit 1; }
  fi
}

echo "== v2.7.0 合并期搬迁计划（exec=$EXEC）=="
echo
echo "[1] fork 本地搬迁/删除（在 merge 消解中执行）:"
run "git mv exvlan/core exvlan/exvlan"
run "git rm -r exvlan/build"
run "git rm -r exvlan/rpc"

echo
echo "[2] 上游目录落位（merge 后上游目录已就绪时执行）:"
# 从 manifest 提取 upstream（含 '/' 或顶层目录）→ target 的重命名
python3 - "$MAP" <<'PY'
import sys, yaml
m = yaml.safe_load(open(sys.argv[1], encoding="utf-8"))
for e in m.get("map", []) or []:
    up = e.get("upstream", "").strip("/")
    tgt = e.get("target", "").strip("/")
    if not up or not tgt:
        continue
    # 顶层目录不同 ⇒ 需要 git mv
    if up.split("/")[0] != tgt.split("/")[0] or up != tgt:
        print(f"git mv {up} {tgt}\t# {up} -> {tgt}")
PY

echo
echo "[3] 引用更新（人工/脚本）:"
echo "    - 根 Cargo.toml members / default-members"
echo "    - path 依赖、third_party 路径、CI/script/Dockerfile/flake/tauri conf"
echo "    - pnpm-workspace（不列 sdk 子包）"
echo "完成后运行: bash scripts/check_invariants.sh"
