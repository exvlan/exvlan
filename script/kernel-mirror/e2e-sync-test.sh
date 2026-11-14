#!/usr/bin/env bash
# 离线端到端：假 `gh` + 假 Release，验证镜像同步的两条硬规则不回归。
#
# 覆盖（对应 docs/decisions-7.3.md §14.13 的补充口径）：
#   A. 首轮发布两版（20711 最新 / 20710 旧），latest.json / magisk_update.json / 资产字节
#      与 upgrade manifest 的 6 个平台 sha256 全部对齐。
#   B. 只重下旧 tag（模拟最新版指纹命中）：latest.json / magisk_update.json **不降级**、
#      未版本化资产（app-*.apk / msi / setup.exe）仍是新版字节、sha256 仍 MATCH。
#   B2. 再跑一轮无变化：降级不会「粘住」，也不会每轮重下。
#   C. FORCE=1 全量重下（报告里的事故触发条件）：仍是新版字节（旧 tag 同名资产不覆盖）。
#   D. MAX_BYTES 预算：超限从最旧版本收缩窗口，清单与磁盘一致。
#   E. 部署兼容：<DIST>/mirror-status.json 是旧 schema；core/<zip> 旧布局符号链接存在。
#
# 用法：script/kernel-mirror/e2e-sync-test.sh        # 退出码 0 = 全绿
#       E2E_KEEP=1 script/kernel-mirror/e2e-sync-test.sh  # 保留临时目录（排查用）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SYNC="$SCRIPT_DIR/exvlan-kernel-sync.sh"
ROOT="$(mktemp -d)"
if [ -n "${E2E_KEEP:-}" ]; then
  printf 'E2E_KEEP=1：保留测试目录 %s\n' "$ROOT"
else
  trap 'rm -rf "$ROOT"' EXIT
fi

BIN="$ROOT/bin"
ASSETS="$ROOT/releases"       # 假 Release 的资产（每 tag 一个目录）
RELEASES="$ROOT/releases.tsv" # 假 gh api 的输出（R/A 行）
DIST="$ROOT/dist"
LOG="$ROOT/sync.log"
mkdir -p "$BIN" "$ASSETS" "$ROOT/tmp"

VERSIONS=()
FAILED=0
PASSED=0
pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$*"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$*" >&2; }
assert_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期望 '$2'，实际 '$3'）"; fi; }
assert_ne() { if [ "$2" != "$3" ]; then pass "$1"; else fail "$1（不应等于 '$3'）"; fi; }
assert_has() { if grep -qF -- "$2" "$3"; then pass "$1"; else fail "$1（$3 里没有 '$2'）"; fi; }
assert_lacks() { if grep -qF -- "$2" "$3"; then fail "$1（$3 里不应出现 '$2'）"; else pass "$1"; fi; }

# ---------------------------------------------------------------------------
# 假 gh：只实现同步脚本用到的 api / release download
# ---------------------------------------------------------------------------
cat > "$BIN/gh" <<'FAKE_GH'
#!/usr/bin/env bash
# 离线 e2e 用的假 gh（不联网、不需要凭据）
set -euo pipefail
releases="${FAKE_GH_RELEASES:?}"
assets="${FAKE_GH_ASSETS:?}"
case "${1:-}" in
  api)
    # 同步脚本只用它取 release 列表（R/A 的 TSV 行）；--jq 表达式在这里等价展开
    cat "$releases"
    ;;
  release)
    [ "${2:-}" = download ] || { echo "fake gh: 只支持 release download" >&2; exit 1; }
    tag="${3:?fake gh: 缺 tag}"
    shift 3
    dir=""; patterns=()
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --repo) shift 2 ;;
        -p) patterns+=("$2"); shift 2 ;;
        --dir) dir="$2"; shift 2 ;;
        --clobber) shift ;;
        *) echo "fake gh: 未知参数 $1" >&2; exit 1 ;;
      esac
    done
    [ -n "$dir" ] && [ -d "$assets/$tag" ] || { echo "fake gh: 没有 release $tag" >&2; exit 1; }
    shopt -s nullglob
    n=0
    for p in "${patterns[@]}"; do
      for f in "$assets/$tag"/$p; do
        [ -f "$f" ] || continue
        cp -f "$f" "$dir/"
        n=$((n + 1))
      done
    done
    [ "$n" -gt 0 ] || { echo "fake gh: $tag 没有匹配的资产" >&2; exit 1; }
    ;;
  *)
    echo "fake gh: 不支持的子命令 ${1:-（空）}" >&2
    exit 1
    ;;
esac
FAKE_GH
chmod 0755 "$BIN/gh"

# ---------------------------------------------------------------------------
# 假 Release 生成（名字与真实产出一致，便于验证「未版本化资产」判定）
# ---------------------------------------------------------------------------
sha256() { sha256sum "$1" | cut -d' ' -f1; }

mkfile() {  # $1=路径 $2=种子（大小按种子长度，够用且互不相同）
  printf '%s\n' "$2" > "$1"
}

# 未版本化资产名（跨版本重复）——真实产出里也是这几个
APKS="app-arm64-release.apk app-arm-release.apk app-x86_64-release.apk app-x86-release.apk"
MSI_NAME="EXVlan.Studio_26.104.207-5_x64_en-US.msi"
SETUP_NAME="EXVlan.Studio_26.104.207-5_x64-setup.exe"
PLATFORMS="macos-x86_64 macos-aarch64 windows-x86_64 windows-arm64 linux-x86_64 linux-aarch64"
APP_PLATFORMS="macos-x86_64 windows-x86_64 android-arm64-v8a android-armeabi-v7a android-x86_64 android-x86"

build_release() {  # $1=版本号（如 26.410.20711）
  local ver="$1" tag="v$1" dir="$ASSETS/v$1" p iso name
  mkdir -p "$dir"
  # 内核 zip：名字带版本 → 各版本互不冲突
  for p in $PLATFORMS; do
    mkfile "$dir/$p-$tag.zip" "core $p $tag"
  done
  # 应用：版本化资产
  mkfile "$dir/EXVlan.Studio_${ver}_x64.dmg" "dmg $ver"
  mkfile "$dir/EXVlan.Studio_${ver}_x86_64.app.tar.gz" "updater $ver"
  mkfile "$dir/EXVlan.Studio_${ver}_x86_64.app.tar.gz.sig" "sig app $ver"
  mkfile "$dir/Exvlan-Magisk-$tag.zip" "magisk $tag"
  # 应用：未版本化资产（同名跨版本重复，字节按版本不同）
  for name in $APKS; do mkfile "$dir/$name" "apk $ver $name"; done
  mkfile "$dir/$MSI_NAME" "msi $ver"
  mkfile "$dir/$MSI_NAME.sig" "sig msi $ver"
  mkfile "$dir/$SETUP_NAME" "setup $ver"
  mkfile "$dir/$SETUP_NAME.sig" "sig setup $ver"

  # 内核 manifest（只含镜像托管的 6 个平台）
  {
    printf '{\n  "schema": 1,\n  "latest": "%s",\n  "versions": [\n    {\n      "version": "%s",\n' "$tag" "$tag"
    printf '      "assets": {\n'
    local first=1
    for p in $PLATFORMS; do
      [ "$first" = 1 ] || printf ',\n'
      first=0
      iso="$(sha256 "$dir/$p-$tag.zip")"
      printf '        "%s": {"file": "%s-%s.zip", "sha256": "%s", "entry": "exvlan"}' "$p" "$p" "$tag" "$iso"
    done
    printf '\n      }\n    }\n  ]\n}\n'
  } > "$dir/manifest.json"

  # 自建升级清单（镜像里是 manifest.json）
  {
    printf '{\n  "schema": 1,\n  "latest": "%s",\n  "versions": [\n    {\n      "version": "%s",\n' "$ver" "$ver"
    printf '      "assets": {\n'
    local first=1
    for p in $APP_PLATFORMS; do
      [ "$first" = 1 ] || printf ',\n'
      first=0
      case "$p" in
        macos-x86_64) name="EXVlan.Studio_${ver}_x64.dmg" ;;
        windows-x86_64) name="$MSI_NAME" ;;
        android-arm64-v8a) name="app-arm64-release.apk" ;;
        android-armeabi-v7a) name="app-arm-release.apk" ;;
        android-x86_64) name="app-x86_64-release.apk" ;;
        android-x86) name="app-x86-release.apk" ;;
      esac
      iso="$(sha256 "$dir/$name")"
      printf '        "%s": {"file": "%s", "sha256": "%s"}' "$p" "$name" "$iso"
    done
    printf '\n      }\n    }\n  ]\n}\n'
  } > "$dir/upgrade-manifest.json"

  # 官方 updater 清单（镜像里是 latest.json；URL 是绝对地址，由 channelize_urls.py 改写）
  {
    printf '{\n  "version": "%s",\n  "notes": "Exvlan %s",\n' "$ver" "$ver"
    printf '  "pub_date": "2026-11-04T00:00:00Z",\n  "platforms": {\n'
    printf '    "darwin-x86_64": {"signature": "sig-%s", "url": "https://dl.example.invalid/upgrade/EXVlan.Studio_%s_x86_64.app.tar.gz"},\n' "$ver" "$ver"
    printf '    "windows-x86_64": {"signature": "sig-%s", "url": "https://dl.example.invalid/upgrade/%s"}\n' "$ver" "$SETUP_NAME"
    printf '  }\n}\n'
  } > "$dir/upgrade-latest.json"

  # Magisk 在线更新清单（镜像里是 magisk_update.json）
  printf '{\n  "version": "%s",\n  "versionCode": 1,\n  "zipUrl": "https://dl.example.invalid/upgrade/Exvlan-Magisk-%s.zip",\n  "changelog": "Exvlan %s"\n}\n' \
    "$ver" "$tag" "$ver" > "$dir/magisk_update.json"

  VERSIONS+=("$ver")
  rebuild_releases
}

rebuild_releases() {  # 从资产目录重建「gh api」输出（R/A 行，与真实 --jq 一致）
  local ver tag f
  : > "$RELEASES"
  for ver in "${VERSIONS[@]}"; do
    printf 'R\tv%s\t2026-11-04T00:00:00Z\tfalse\n' "$ver" >> "$RELEASES"
  done
  for ver in "${VERSIONS[@]}"; do
    tag="v$ver"
    for f in "$ASSETS/$tag"/*; do
      [ -f "$f" ] || continue
      printf 'A\t%s\t%s\t%s\n' "$tag" "$(basename "$f")" "$(stat -c%s "$f")" >> "$RELEASES"
    done
  done
}

# ---------------------------------------------------------------------------
# 跑一轮同步
# ---------------------------------------------------------------------------
run_sync() {
  : > "$LOG"
  env -i \
    PATH="$BIN:/usr/local/bin:/usr/bin:/bin" \
    HOME="$ROOT" \
    TMPDIR="$ROOT/tmp" \
    FAKE_GH_RELEASES="$RELEASES" \
    FAKE_GH_ASSETS="$ASSETS" \
    REPO="exvlan/exvlan" \
    DEST="$DIST/core" \
    UPGRADE_DEST="$DIST/upgrade" \
    MAX_BYTES="${MAX_BYTES:-8589934592}" \
    KEEP_CORE_STABLE=5 KEEP_APP_STABLE=5 \
    "${FORCE_ENV[@]}" \
    bash "$SYNC" >> "$LOG" 2>&1 || {
      printf '同步脚本失败（退出码 %s），日志尾部：\n' "$?" >&2
      tail -n 40 "$LOG" >&2
      return 1
    }
}
FORCE_ENV=()

# ---------------------------------------------------------------------------
# 断言：最新版清单 / 资产字节 / 6 个平台 sha256
# ---------------------------------------------------------------------------
json_field() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get(sys.argv[2],""))' "$1" "$2" || true; }

# 校验 upgrade/manifest.json 里「最新版本」那一份的 6 个平台：文件存在且 sha256 对得上
check_app_manifest() {  # $1=期望版本号
  python3 - "$DIST/upgrade/stable/manifest.json" "$DIST/upgrade/stable" "$1" <<'PY'
import hashlib, json, sys
manifest, base, want = sys.argv[1], sys.argv[2], sys.argv[3]
data = json.load(open(manifest))
latest = data.get("latest")
entries = {v.get("version"): v for v in data.get("versions") or []}
entry = entries.get(latest)
if entry is None:
    print(f"manifest.latest={latest} 没有对应版本条目")
    raise SystemExit(1)
if latest != want:
    print(f"manifest.latest={latest} != 期望 {want}")
    raise SystemExit(1)
assets = entry.get("assets") or {}
problems = []
for platform, meta in sorted(assets.items()):
    name = (meta or {}).get("file")
    path = f"{base}/{name}"
    try:
        digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
    except OSError as exc:
        problems.append(f"{platform}: {name} 读不到（{exc}）")
        continue
    if digest != meta.get("sha256"):
        problems.append(f"{platform}: {name} sha256 不符")
if len(assets) != 6:
    problems.append(f"平台数 {len(assets)} != 6")
if problems:
    print("; ".join(problems))
    raise SystemExit(1)
print(f"{latest}: 6 个平台 sha256 全部 MATCH")
PY
}

check_app_state() {  # $1=期望版本（如 26.410.20711）
  local want="$1" apk got
  assert_eq "latest.json 版本" "$want" "$(json_field "$DIST/upgrade/stable/latest.json" version)"
  assert_eq "magisk_update.json 版本" "$want" "$(json_field "$DIST/upgrade/stable/magisk_update.json" version)"
  for apk in $APKS; do
    got="$(cat "$DIST/upgrade/stable/$apk" 2>/dev/null || true)"
    assert_eq "未版本化资产 $apk 仍是 $want" "apk $want $apk" "$got"
  done
  got="$(cat "$DIST/upgrade/stable/$MSI_NAME" 2>/dev/null || true)"
  assert_eq "未版本化资产 msi 仍是 $want" "msi $want" "$got"
  got="$(cat "$DIST/upgrade/stable/$SETUP_NAME" 2>/dev/null || true)"
  assert_eq "未版本化资产 setup.exe 仍是 $want" "setup $want" "$got"
  if out="$(check_app_manifest "$want")"; then pass "upgrade/manifest.json 最新版 sha256：$out"; else fail "upgrade/manifest.json 校验：$out"; fi
}

check_compat_status() {  # $1=期望 result
  python3 - "$DIST/mirror-status.json" "$1" "${MAX_BYTES:-8589934592}" <<'PY'
import json, sys
path, want_result, max_bytes = sys.argv[1], sys.argv[2], int(sys.argv[3])
d = json.load(open(path))
need = ["checked_at", "round", "result", "detail", "since_tag", "latest_release",
        "tags", "first_ok_at", "last_change_at", "wait_since", "bytes"]
missing = [k for k in need if k not in d]
problems = []
if missing:
    problems.append("缺字段 " + ",".join(missing))
if d.get("result") != want_result:
    problems.append(f"result={d.get('result')} != {want_result}")
if int(d.get("round") or 0) < 1:
    problems.append("round 未递增")
if int((d.get("bytes") or {}).get("max") or 0) != max_bytes:
    problems.append(f"bytes.max={(d.get('bytes') or {}).get('max')} != {max_bytes}")
if "channels" in d:
    problems.append("兼容文件混入了新 schema 的 channels 字段")
if problems:
    print("; ".join(problems))
    raise SystemExit(1)
print(f"旧 schema 兼容状态 OK（result={d['result']} round={d['round']} tags={len(d['tags'])}）")
PY
}

check_legacy_root_links() {
  local name ok=1
  for name in linux-x86_64-v26.410.20711.zip macos-aarch64-v26.410.20710.zip; do
    if [ -L "$DIST/core/$name" ] && [ -f "$DIST/core/$name" ]; then :; else ok=0; fail "旧布局符号链接缺失：core/$name"; fi
  done
  [ "$ok" = 1 ] && pass "旧布局符号链接：core/<zip> → stable/<zip>"
  return 0
}

section() { printf '\n== %s ==\n' "$*"; }

# ---------------------------------------------------------------------------
# 场景 A：首轮发布 20711（新）+ 20710（旧）
# ---------------------------------------------------------------------------
section "A. 首轮发布 v26.410.20711 + v26.410.20710"
build_release 26.410.20711
build_release 26.410.20710
run_sync
assert_has "两版都下载了" "v26.410.20710：下载" "$LOG"
assert_has "最新版也下载了" "v26.410.20711：下载" "$LOG"
check_app_state 26.410.20711
assert_eq "core/manifest.json 的 latest" "v26.410.20711" "$(json_field "$DIST/core/stable/manifest.json" latest)"
if out="$(check_compat_status ok)"; then pass "$out"; else fail "$out"; fi
check_legacy_root_links
assert_eq "core/stable/manifest.json 别名" "stable/manifest.json" "$(readlink "$DIST/core/manifest.json")"

# ---------------------------------------------------------------------------
# 场景 B：只重下旧 tag（改 20710 的一个资产 → 它指纹变、20711 指纹命中）
# ---------------------------------------------------------------------------
section "B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）"
printf 'apk re-uploaded 26.410.20710\n' > "$ASSETS/v26.410.20710/app-x86-release.apk"
rebuild_releases
run_sync
assert_has "旧 tag 被重下" "v26.410.20710：下载" "$LOG"
assert_lacks "最新版没有再下载（指纹命中）" "v26.410.20711：下载" "$LOG"
check_app_state 26.410.20711
assert_eq "core/manifest.json 的 latest 未回退" "v26.410.20711" "$(json_field "$DIST/core/stable/manifest.json" latest)"

# ---------------------------------------------------------------------------
# 场景 B2：再跑一轮无变化 → 降级不会粘住、也不重下
# ---------------------------------------------------------------------------
section "B2. 无变化再跑一轮（不降级、不重下）"
run_sync
assert_lacks "没有重下任何 tag" "：下载" "$LOG"
check_app_state 26.410.20711

# ---------------------------------------------------------------------------
# 场景 C：FORCE=1 全量重下（事故触发条件：新旧都下，最旧的后写赢）
# ---------------------------------------------------------------------------
section "C. FORCE=1 全量重下"
FORCE_ENV=(FORCE=1)
run_sync
FORCE_ENV=()
assert_has "旧 tag 也重下了" "v26.410.20710：下载" "$LOG"
assert_has "最新版也重下了" "v26.410.20711：下载" "$LOG"
check_app_state 26.410.20711

# ---------------------------------------------------------------------------
# 场景 D：MAX_BYTES 预算（超限从最旧版本收缩窗口）
# ---------------------------------------------------------------------------
section "D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）"
build_release 26.410.20712
per_tag="$(awk -F'\t' '$1 == "A" && $2 == "v26.410.20712" && $3 !~ /[.]json$/ { s += $4 } END { print s + 0 }' "$RELEASES")"
MAX_BYTES=$(( per_tag * 3 / 2 ))
export MAX_BYTES
rm -rf "$DIST"
run_sync
assert_has "打印预算裁剪日志" "超出 MAX_BYTES" "$LOG"
assert_eq "预算裁剪后只保留 1 个版本" "1" "$(python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))["versions"]))' "$DIST/core/stable/manifest.json")"
assert_eq "裁剪后 core latest 仍是最新 stable" "v26.410.20712" "$(json_field "$DIST/core/stable/manifest.json" latest)"
check_app_state 26.410.20712
if out="$(check_compat_status ok)"; then pass "$out"; else fail "$out"; fi
assert_eq "单版本下载量等于单 tag 预算以内" "1" "$(grep -c '：下载' "$LOG" || true)"
unset MAX_BYTES

# ---------------------------------------------------------------------------
section "结果"
printf '通过 %s 项，失败 %s 项\n' "$PASSED" "$FAILED"
[ "$FAILED" -eq 0 ]
