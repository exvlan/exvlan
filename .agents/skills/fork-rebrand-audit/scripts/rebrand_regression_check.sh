#!/usr/bin/env bash
# rebrand 回归抽查：生成物/锁文件/脚本路径。只读、可反复运行。
# 用法: rebrand_regression_check.sh [--repo <dir>]
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
while [ $# -gt 0 ]; do case "$1" in --repo) ROOT="$2"; shift 2;; *) shift;; esac; done
cd "$ROOT"
rc=0
ok(){ echo "  [OK]   $*"; }
warn(){ echo "  [WARN] $*"; rc=1; }

echo "== rebrand 回归抽查 (root=$ROOT) =="

echo "--- 1) 生成代码是否疑似被文本替换（含品牌名，可能损坏 rawDesc）---"
hits=$(grep -rInE 'easytier' --include='*.pb.go' --include='*.pb.rs' --include='*.generated.*' . 2>/dev/null | head -5)
if [ -n "$hits" ]; then warn "生成物仍含旧品牌名（若非可控替换，建议用生成器重生成）:"; echo "$hits" | sed 's/^/        /'; else ok "生成物未见旧品牌名"; fi

echo "--- 2) 嵌套 workspace 锁文件一致性 ---"
for d in $(find . -maxdepth 3 -name pnpm-workspace.yaml -not -path '*/node_modules/*' 2>/dev/null); do
  dir="$(dirname "$d")"
  if command -v pnpm >/dev/null 2>&1 && [ -f "$dir/pnpm-lock.yaml" ]; then
    if (cd "$dir" && pnpm install --frozen-lockfile --lockfile-only >/dev/null 2>&1); then ok "锁文件一致: $dir"; else warn "锁文件不一致(需重生成): $dir"; fi
  else
    warn "跳过(无 pnpm 或无锁文件): $dir"
  fi
done

echo "--- 3) 脚本相对路径是否偏移（repo-root 计算）---"
hits=$(grep -rInE "resolve\([^)]*['\"]\.\./\.\.['\"]" --include='*.mjs' --include='*.js' --include='*.ts' . 2>/dev/null | grep -v node_modules | head -8)
if [ -n "$hits" ]; then warn "发现 '../..' 计算根路径，relayout 后易偏移，请人工核对:"; echo "$hits" | sed 's/^/        /'; else ok "未见可疑 '../..' 根路径计算"; fi

echo "--- 4) 品牌白名单仍在（依赖坐标/许可证/映射）---"
for f in docs/rules/workflow/upstream-path-map.yaml LICENSE; do
  [ -f "$f" ] && ok "白名单存在: $f" || warn "白名单缺失: $f"
done

echo
echo "结论：$([ $rc -eq 0 ] && echo '抽查通过' || echo '存在需人工确认项')"
exit $rc
