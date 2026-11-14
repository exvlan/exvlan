#!/usr/bin/env bash
#
# 一次性把全仓版本对齐到 <version>（AGENTS.md「版本对齐范围」）。
#
# 覆盖范围：
#   · 核心 Rust：根 `Cargo.toml`（`[workspace.dependencies]` 里带 `path` 的
#     版本号）+ 每个 crate 的 `[package].version` + `Cargo.lock` 里自身包版本
#     （有 `source` 的 registry/git 包一律不动）；
#   · 前端：`packages/{studio,std,ui,generator}/package.json`、`packages/sdk/**`
#     （嵌套独立 workspace：子包 `version` + 内部 `workspace:<v>` specifier +
#     `pnpm-lock.yaml` 的 specifier，最后由 `pnpm install --lockfile-only` 复核）、
#     `exvlan/contrib/uptime/frontend`、`exvlan/plugin/vpnservice`、ohos 的
#     `oh-package.json5`；
#   · Tauri：`tauri.conf.json` / `tauri.windows.conf.json`（含 `bundle.windows.wix.version`，
#     由 `script/ci/msi-version.py` 的 stable 公式推导；CI 构建时会按通道再覆盖）；
#   · Magisk：`module.prop` / `magisk_update.json`，`versionCode` 按
#     `YY*10^11 + QMM*10^8 + Mmpp*10^3`（26.410.20712 → 2641020712000）重算；
#   · `script/ci/kernel-compat.json` 的 `min_studio_version`（现状恒等于基线版本，
#     内核清单的 GUI 下限随之抬高）；
#   · 版本号出现在注释/示例里的引用点（workflow 的 `default`、`build-version.sh`、
#     `install.ps1`、`constants.rs`、`build/main.rs` 等）一并跟随，保证
#     `grep -rn '<旧版本>'` 在源码/配置里清零。
#
# 白名单（不改，脚本末尾自检会列出来并说明理由）：
#   · `docs/**`、`README*.md`、`CHANGELOG*`：历史文档/示例输出；
#   · `packages/studio/src-tauri/src/upgrade.rs`：版本排序测试夹具，必须同时保留
#     「同 base 不同通道」和「跨 patch」两种样本，跟着升级会改变被测语义；
#   · `script/kernel-mirror/**`：内核镜像 e2e 夹具的版本阶梯（20710→20711→20712），
#     与应用基线无关。
#
# 用法：
#   bash script/ci/set-version.sh 26.410.20713            # 写入
#   bash script/ci/set-version.sh 26.410.20713 --dry-run  # 只打印计划
#
# 不负责的事：不提交、不打 tag、不改 `docs/`。跑完请自身到：
#   cargo check --workspace && bash script/ci/check-paths.sh ...
# 并确认 `git diff` 只有版本号变化。

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

usage() {
  sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

new_version=""
dry_run=0
for arg in "$@"; do
  case "$arg" in
    -h | --help) usage 0 ;;
    -n | --dry-run) dry_run=1 ;;
    -*) echo "set-version: 未知选项 $arg" >&2; usage 1 ;;
    *)
      if [[ -n "$new_version" ]]; then
        echo "set-version: 只能指定一个版本号（已给 $new_version，又给 $arg）" >&2
        exit 1
      fi
      new_version="$arg"
      ;;
  esac
done

if [[ -z "$new_version" ]]; then
  echo "set-version: 缺少目标版本号" >&2
  usage 1
fi
if [[ ! "$new_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "set-version: 版本号必须形如 YY.QMM.Mmpp（收到 $new_version）" >&2
  echo "              通道后缀由 tag 决定，不要写进基线版本" >&2
  exit 1
fi

old_version=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
  packages/studio/src-tauri/tauri.conf.json | head -1)
if [[ -z "$old_version" ]]; then
  echo "set-version: 无法从 packages/studio/src-tauri/tauri.conf.json 读出当前基线版本" >&2
  exit 1
fi

echo "基线：$old_version → $new_version$([[ $dry_run -eq 1 ]] && echo '（dry-run）')"
if [[ "$old_version" == "$new_version" ]]; then
  echo "已是 $new_version，无需改动。"
  exit 0
fi

python3 - "$old_version" "$new_version" "$dry_run" "$repo_root" <<'PY'
from __future__ import annotations

import os
import pathlib
import re
import subprocess
import sys

old, new, dry_raw, root_raw = sys.argv[1:5]
dry = dry_raw == "1"
root = pathlib.Path(root_raw)

SKIP_DIRS = {
    ".git", "node_modules", "target", "vendor", "dist", "dist-js",
    "__pycache__", ".venv", ".direnv", "gen",
}

# 版本号出现在「注释 / 示例 / workflow 默认值」里的引用点：整体跟随更新，
# 避免旧基线在源码里残留（这些文件里旧版本号没有任何运行时语义）。
REFERENCE_FILES = [
    ".github/workflows/docker.yml",
    ".github/workflows/pipeline.yml",
    ".github/workflows/publish-go.yml",
    ".github/workflows/release.yml",
    ".github/workflows/service.yml",
    "script/ci/build-version.sh",
    "script/install.ps1",
    "exvlan/exvlan/build/main.rs",
    "exvlan/exvlan/src/common/constants.rs",
    "exvlan/service/src/main.rs",
    "packages/studio/src-tauri/src/service_client.rs",
]

# 显式点名的单文件（glob 抓不到或需要专用规则）。
OHOS_PACKAGE = "exvlan/contrib/ohrs/package/oh-package.json5"
TAURI_CONFS = [
    "packages/studio/src-tauri/tauri.conf.json",
    "packages/studio/src-tauri/tauri.windows.conf.json",
]
MAGISK_PROP = "exvlan/contrib/magisk/module.prop"
MAGISK_UPDATE = "exvlan/contrib/magisk/magisk_update.json"
KERNEL_COMPAT = "script/ci/kernel-compat.json"
SDK_LOCK = "packages/sdk/pnpm-lock.yaml"


def walk(suffix: str) -> list[pathlib.Path]:
    found = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in filenames:
            if name.endswith(suffix) or name == suffix:
                found.append(pathlib.Path(dirpath) / name)
    return sorted(found)


tracked = set(
    subprocess.run(
        ["git", "ls-files", "-z"], cwd=root, check=True, capture_output=True
    ).stdout.decode().split("\0")
)


def is_tracked(path: pathlib.Path) -> bool:
    return str(path.relative_to(root)) in tracked


def version_code(v: str) -> int:
    yy, qmm, mmpp = (int(x) for x in v.split("."))
    return yy * 10**11 + qmm * 10**8 + mmpp * 10**3


msi_version = subprocess.run(
    [sys.executable, str(root / "script/ci/msi-version.py"), new],
    check=True, capture_output=True, text=True,
).stdout.strip()

changed: dict[pathlib.Path, int] = {}
warnings: list[str] = []


def write(path: pathlib.Path, text: str, count: int) -> None:
    if count <= 0:
        return
    rel = path.relative_to(root)
    changed[rel] = changed.get(rel, 0) + count
    if not dry:
        path.write_text(text, encoding="utf-8")


def sub(path: pathlib.Path, pattern: str, repl: str, flags: int = 0) -> int:
    """按正则改写，返回命中次数；只动匹配到的片段，不做整体重排。"""
    if not path.exists():
        warnings.append(f"缺少文件：{path.relative_to(root)}")
        return 0
    text = path.read_text(encoding="utf-8")
    new_text, n = re.subn(pattern, repl, text, flags=flags)
    write(path, new_text, n)
    return n


# ---------------------------------------------------------------- Rust 清单
def edit_manifest(path: pathlib.Path) -> None:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    section = ""
    hits = 0
    for i, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped.strip("[]")
            continue
        if section == "package" and re.fullmatch(
            rf'version = "{re.escape(old)}"', stripped
        ):
            lines[i] = line.replace(f'"{old}"', f'"{new}"')
            hits += 1
        elif section == "workspace.dependencies" and "path = " in line:
            replaced = line.replace(f'"{old}"', f'"{new}"')
            if replaced != line:
                lines[i] = replaced
                hits += 1
    write(path, "".join(lines), hits)


for manifest in walk("Cargo.toml"):
    edit_manifest(manifest)


# ---------------------------------------------------------------- Cargo.lock
def edit_lock(path: pathlib.Path) -> None:
    text = path.read_text(encoding="utf-8")
    blocks = text.split("[[package]]")
    hits = 0
    for idx, block in enumerate(blocks[1:], start=1):
        # path 依赖（本仓 crate）没有 `source` 行；registry/git 包都有。
        if re.search(r"^source = ", block, flags=re.MULTILINE):
            continue
        fixed, n = re.subn(
            rf'^version = "{re.escape(old)}"$',
            f'version = "{new}"',
            block,
            flags=re.MULTILINE,
        )
        if n:
            blocks[idx] = fixed
            hits += n
    write(path, "[[package]]".join(blocks), hits)


for lock in walk("Cargo.lock"):
    # 嵌套 workspace（如 exvlan/contrib/ohrs）的 lock 是 gitignore 的生成物，
    # 不跟着改；只处理入库的 lock。
    if is_tracked(lock):
        edit_lock(lock)

# ------------------------------------------------------- package.json / oh-package
pkg_files = [
    p for p in walk("package.json") if p.relative_to(root).parts[0] != "archive"
]
for extra in (OHOS_PACKAGE,):
    p = root / extra
    if p.exists() and p not in pkg_files:
        pkg_files.append(p)

for pkg in pkg_files:
    sub(pkg, rf'"version": "{re.escape(old)}"', f'"version": "{new}"')
    sub(pkg, rf"workspace:{re.escape(old)}\b", f"workspace:{new}")

# sdk 的 lockfile：specifier 行是版本号的真源，pnpm 复核在 bash 侧完成。
sub(root / SDK_LOCK, rf"workspace:{re.escape(old)}\b", f"workspace:{new}")

# ---------------------------------------------------------------- Tauri
for rel in TAURI_CONFS:
    sub(root / rel, rf'"version": "{re.escape(old)}"', f'"version": "{new}"')
sub(
    root / "packages/studio/src-tauri/tauri.windows.conf.json",
    r'("wix"\s*:\s*\{\s*"version"\s*:\s*")[^"]*(")',
    rf"\g<1>{msi_version}\g<2>",
    flags=re.DOTALL,
)

# ---------------------------------------------------------------- Magisk
code = version_code(new)
sub(root / MAGISK_PROP, rf"^version=v{re.escape(old)}$", f"version=v{new}", re.MULTILINE)
sub(root / MAGISK_PROP, r"^versionCode=\d+$", f"versionCode={code}", re.MULTILINE)
sub(root / MAGISK_UPDATE, rf'"version": "{re.escape(old)}"', f'"version": "{new}"')
sub(root / MAGISK_UPDATE, r'"versionCode": \d+', f'"versionCode": {code}')

# ---------------------------------------------------------------- 内核基线
sub(root / KERNEL_COMPAT, rf'"min_studio_version": "{re.escape(old)}"',
    f'"min_studio_version": "{new}"')

# ---------------------------------------------------------------- 注释/示例引用
for rel in REFERENCE_FILES:
    sub(root / rel, re.escape(old), new)

for rel, count in sorted(changed.items()):
    print(f"  {'[dry] ' if dry else ''}更新 {rel}（{count} 处）")
if not changed:
    print("  无改动")
print(f"  派生值：magisk versionCode={code}，wix.version={msi_version}")
for w in warnings:
    print(f"  ! {w}", file=sys.stderr)
PY

if [[ "$dry_run" -eq 1 ]]; then
  echo "dry-run：未写入文件。"
  exit 0
fi

# ---------------------------------------------------------------- sdk lockfile
if command -v pnpm >/dev/null 2>&1; then
  echo "→ 重生成 packages/sdk/pnpm-lock.yaml"
  (cd packages/sdk && pnpm install --lockfile-only --ignore-scripts >/dev/null)
else
  echo "! 未找到 pnpm：packages/sdk/pnpm-lock.yaml 已按 specifier 文本更新，但未复核" >&2
fi

# ---------------------------------------------------------------- 自检
echo "→ 自检：$old_version 残留"
# 分类白名单：只允许出现在文档、明确的测试夹具、以及本脚本自身的公式示例里。
allow_docs='^(docs/|README(_CN)?\.md$|CHANGELOG)'
allow_fixture='^(packages/studio/src-tauri/src/upgrade\.rs|script/kernel-mirror/)'
allow_self='^script/ci/set-version\.sh$'

unexpected=0
docs_hits=()
fixture_hits=()
self_hits=()
# `git grep` 只扫入库文件：嵌套 workspace 里 gitignore 的生成物（ohrs 的
# Cargo.lock）不参与判定。
while IFS= read -r line; do
  file=${line%%:*}
  file=${file#./}
  if [[ "$file" =~ $allow_docs ]]; then
    docs_hits+=("$line")
  elif [[ "$file" =~ $allow_fixture ]]; then
    fixture_hits+=("$line")
  elif [[ "$file" =~ $allow_self ]]; then
    self_hits+=("$line")
  else
    echo "  ✗ 残留：$line" >&2
    unexpected=$((unexpected + 1))
  fi
done < <(git grep -n -I -e "$old_version" -- . ':!*.pyc' || true)

if ((${#docs_hits[@]})); then
  echo "  · 文档白名单（不改，历史/示例）：${#docs_hits[@]} 处"
  printf '      %s\n' "${docs_hits[@]}" | sed -n '1,12p'
  ((${#docs_hits[@]} > 12)) && echo "      …（共 ${#docs_hits[@]} 处）"
fi
if ((${#fixture_hits[@]})); then
  echo "  · 测试夹具白名单（不改，被测语义依赖旧版本号）：${#fixture_hits[@]} 处"
  printf '      %s\n' "${fixture_hits[@]}" | sed -n '1,8p'
  ((${#fixture_hits[@]} > 8)) && echo "      …（共 ${#fixture_hits[@]} 处）"
fi
if ((${#self_hits[@]})); then
  echo "  · 本脚本注释里的公式示例：${#self_hits[@]} 处"
fi
if [[ "$unexpected" -ne 0 ]]; then
  echo "set-version: 有 $unexpected 处未对齐的旧版本号，请补充规则" >&2
  exit 1
fi

echo "→ 自检：关键点版本 = $new_version"
fail=0
for probe in \
  "packages/studio/src-tauri/tauri.conf.json" \
  "packages/studio/src-tauri/tauri.windows.conf.json" \
  "packages/studio/package.json" \
  "packages/std/package.json" \
  "packages/ui/package.json" \
  "packages/generator/package.json" \
  "packages/sdk/package.json" \
  "exvlan/service/Cargo.toml" \
  "exvlan/contrib/magisk/module.prop"; do
  if grep -q -- "$new_version" "$probe"; then
    echo "  ✓ $probe"
  else
    echo "  ✗ $probe 未包含 $new_version" >&2
    fail=1
  fi
done
if ! grep -q '^version = "'"$new_version"'"$' Cargo.lock; then
  echo "  ✗ Cargo.lock 未对齐" >&2
  fail=1
fi
if grep -q "workspace:$old_version" packages/sdk/pnpm-lock.yaml; then
  echo "  ✗ packages/sdk/pnpm-lock.yaml 仍有旧 specifier" >&2
  fail=1
fi

[[ "$fail" -eq 0 ]] || exit 1
echo "完成：全仓版本已对齐到 $new_version"
