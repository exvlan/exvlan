#!/usr/bin/env bash
#
# 单测：`script/ci/nightly-version.sh` 的版本串与跨年守卫。
#
# 不联网、不写仓库（BASE 从 tauri.conf.json 现取，base 版本前进后单测不会失效）。
#
# 用法：bash script/ci/tests/test_nightly_version.sh

set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$here/../../.." && pwd)
script="$repo_root/script/ci/nightly-version.sh"

pass=0
fail=0

check_eq() { # $1=用例名 $2=期望 $3=实际
  if [ "$2" = "$3" ]; then
    pass=$((pass + 1))
    printf 'ok   %s\n' "$1"
  else
    fail=$((fail + 1))
    printf 'FAIL %s\n  期望: %s\n  实际: %s\n' "$1" "$2" "$3"
  fi
}

check_contains() { # $1=用例名 $2=子串 $3=实际
  case "$3" in
    *"$2"*)
      pass=$((pass + 1))
      printf 'ok   %s\n' "$1"
      ;;
    *)
      fail=$((fail + 1))
      printf 'FAIL %s\n  期望包含: %s\n  实际: %s\n' "$1" "$2" "$3"
      ;;
  esac
}

base=$(python3 - "$repo_root/packages/studio/src-tauri/tauri.conf.json" <<'PY'
import json
import sys

print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])
PY
)
IFS=. read -r yy qmm mmpp <<<"$base"
patch=$((10#$mmpp + 1))
year=$((2000 + 10#$yy))

# 1) 正常 +1，MMDD 用固定时钟
got=$(NIGHTLY_NOW="${year}-11-15" bash "$script")
check_eq "patch +1（${base} → ${yy}.${qmm}.${patch}）" \
  "${yy}.${qmm}.${patch}-nightly.1115" "$got"

# 2) MMDD 必须补零（个位月/日）
got=$(NIGHTLY_NOW="${year}-01-02" bash "$script")
check_eq "MMDD 补零" "${yy}.${qmm}.${patch}-nightly.0102" "$got"

# 3) 默认时钟也要产出 4 位 MMDD（不依赖 CI 注入）
got=$(bash "$script")
case "$got" in
  "${yy}.${qmm}.${patch}-nightly."[0-9][0-9][0-9][0-9])
    pass=$((pass + 1))
    printf 'ok   默认时钟 MMDD 格式（%s）\n' "$got"
    ;;
  *)
    fail=$((fail + 1))
    printf 'FAIL 默认时钟 MMDD 格式\n  实际: %s\n' "$got"
    ;;
esac

# 4) 跨年守卫：新年但 base 未升级 → 失败，并提示升到 <next>.x
set +e
out=$(NIGHTLY_NOW="$((year + 1))-01-05" bash "$script" 2>&1)
rc=$?
set -e
check_eq "跨年守卫 exit code" "1" "$rc"
check_contains "跨年守卫提示" "请先升到 $((10#$yy + 1)).x" "$out"

# 5) 与 cwd 无关（workflow 里可能在任意目录调用）
got=$(cd /tmp && NIGHTLY_NOW="${year}-11-15" bash "$script")
check_eq "任意 cwd 可用" "${yy}.${qmm}.${patch}-nightly.1115" "$got"

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
