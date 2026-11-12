#!/bin/sh
# pre.17 验收（可自动化的部分）
#
# 目的：把「装完 pre.17 到底好没好」变成一条命令。覆盖本轮新增的控制面契约：
# - 配置目录是网络配置的唯一权威（`<config-dir>/<id>.toml`，禁用为 `.toml.disabled`）
# - `exvlan-cli config` 读写可用（服务 IPC 或裸核直写）
# - doctor 的 `config.dir` 为 ok（不可写 = 保存网络会失败）
# - 旧数据已迁移（有遗留但没 marker → 需要启动一次 App）
# - 本机 gateway（若在跑）能应答 `/api/v1/system`
#
# 不覆盖（必须人眼看界面）：
#  1. 首次启动的迁移页（列出遗留文件 → 开始迁移 → 自动继续）
#  2. 新建/编辑网络后 `<config-dir>/<id>.toml` 出现，关闭网络后变成 `.toml.disabled`
#  3. session 策略下 `exvlan-cli doctor` 不应断网（本轮修的就是这个）
#  4. 远端：对端 `curl -H "Authorization: Bearer exv_..." https://<host>:2016/api/v1/system`
#
# 用法：
#   sh script/lab/pre17-acceptance.sh
#   EXVLAN_CLI=/path/to/exvlan-cli sh script/lab/pre17-acceptance.sh
set -u

fail=0
warn=0

ok() { printf '  ✓ %s\n' "$1"; }
bad() { printf '  ✗ %s\n' "$1"; fail=$((fail + 1)); }
note() { printf '  ! %s\n' "$1"; warn=$((warn + 1)); }
step() { printf '\n[%s] %s\n' "$1" "$2"; }

# ---------------------------------------------------------------- CLI 定位
find_cli() {
  [ -n "${EXVLAN_CLI:-}" ] && { printf '%s' "$EXVLAN_CLI"; return; }
  for p in \
    "$(dirname "$0")/../../target/release/exvlan-cli" \
    "$(dirname "$0")/../../target/debug/exvlan-cli" \
    "/Library/Application Support/era.exvlan.studio/core/exvlan-cli" \
    "/opt/era.exvlan.studio/core/exvlan-cli" \
    "/var/lib/era.exvlan.studio/core/exvlan-cli" \
    "$(command -v exvlan-cli 2>/dev/null || true)"
  do
    [ -n "$p" ] && [ -x "$p" ] && { printf '%s' "$p"; return; }
  done
}

CLI=$(find_cli)
step 0 "环境"
if [ -z "$CLI" ]; then
  bad "找不到 exvlan-cli（用 EXVLAN_CLI=/path/to/exvlan-cli 指定）"
  printf '\n结果：%d 项失败\n' "$fail"
  exit 1
fi
ok "CLI: $CLI"
"$CLI" --version 2>/dev/null | head -1 | sed 's/^/    /' || true

# ---------------------------------------------------------------- 1. doctor
step 1 "doctor（config.dir 必须 ok）"
# CLI 输出是 pretty-print JSON；先压成一行再匹配，避免依赖缩进/空格。
DOC=$("$CLI" doctor --json 2>/dev/null | tr -d ' \n\r\t')
if [ -z "$DOC" ]; then
  bad "doctor 无输出"
else
  case "$DOC" in
    *'"id":"config.dir","level":"ok"'*) ok "config.dir ok" ;;
    *'"id":"config.dir"'*) bad "config.dir 不是 ok（配置目录不可写？见上面的 summary）" ;;
    *) note "doctor 没给出 config.dir（旧版本 CLI？）" ;;
  esac
  case "$DOC" in
    *'"id":"migrate.legacy","level":"warn"'*) note "检测到旧版本遗留数据（启动一次 App 完成迁移）" ;;
    *'"id":"migrate.legacy","level":"error"'*) bad "旧数据迁移失败（见 migrate.log）" ;;
  esac
  case "$DOC" in
    *'"id":"service.ipc","level":"error"'*) bad "服务 IPC 不可达" ;;
    *'"id":"service.ipc","level":"ok"'*) ok "服务 IPC 可达" ;;
  esac
fi

# ---------------------------------------------------------------- 2. 配置往返
step 2 "config 读写往返（用 .disabled，不动内核运行时）"
ID="acceptance-$(date +%s)"
TMP=$(mktemp 2>/dev/null || echo "/tmp/exvlan-acceptance.toml")
cat > "$TMP" <<EOF
hostname = "acceptance"
instance_id = "$ID"

[network_identity]
network_name = "exvlan"
network_secret = "acceptance-only"
EOF
if "$CLI" config set "$ID" --file "$TMP" --disabled >/dev/null 2>&1; then
  ok "写入 $ID.toml.disabled"
  # 注意：serde_json 的 map 是 BTreeMap（键按字母序），不要假设 `id` 在 `enabled` 前面。
  if "$CLI" config list --json 2>/dev/null | tr -d ' \n\r\t' | grep -q "\"id\":\"$ID\""; then
    ok "列表里能看到"
  else
    bad "列表里看不到刚写的配置"
  fi
  if "$CLI" config get "$ID" >/dev/null 2>&1; then ok "读回成功"; else bad "读回失败"; fi
  if "$CLI" config rm "$ID" >/dev/null 2>&1; then ok "删除成功"; else bad "删除失败"; fi
  if "$CLI" config list --json 2>/dev/null | tr -d ' \n\r\t' | grep -q "\"id\":\"$ID\""; then
    bad "删除后仍在列表里（残留）"
  fi
else
  bad "写入失败（配置目录不可写？服务没装？）"
fi
rm -f "$TMP"

# ---------------------------------------------------------------- 3. 非法输入
step 3 "非法输入必须被拒绝"
if "$CLI" config set "../evil" --file /dev/null >/dev/null 2>&1; then
  bad "接受了路径穿越 id"
else
  ok "拒绝路径穿越 id"
fi
if printf 'hostname = "x"\n' > /tmp/exvlan-bad.toml 2>/dev/null &&
  "$CLI" config set "acceptance-bad" --file /tmp/exvlan-bad.toml >/dev/null 2>&1; then
  bad "接受了缺少 network_identity 的配置"
else
  ok "拒绝缺少 [network_identity] 的配置"
fi
rm -f /tmp/exvlan-bad.toml

# ---------------------------------------------------------------- 4. gateway
step 4 "本机 gateway（可选）"
TOKEN_FILE="${HOME}/.local/state/era.exvlan.studio/gateway-token"
[ "$(uname -s)" = "Darwin" ] && TOKEN_FILE="${HOME}/Library/Application Support/era.exvlan.studio/gateway-token"
if command -v curl >/dev/null 2>&1 && [ -f "$TOKEN_FILE" ]; then
  TOK=$(cat "$TOKEN_FILE" 2>/dev/null | tr -d '\r\n')
  CODE=$(curl -s -o /dev/null -w '%{http_code}' -m 3 \
    -H "Authorization: Bearer $TOK" http://127.0.0.1:2016/api/v1/system 2>/dev/null)
  case "$CODE" in
    200) ok "/api/v1/system → 200" ;;
    000) note "2016 没在监听（gateway 未启动，可忽略）" ;;
    401) bad "token 被拒（401）：可能与 gateway 不同源，试 `exvlan-cli auth login`" ;;
    *) bad "/api/v1/system → $CODE" ;;
  esac
  CODE=$(curl -s -o /dev/null -w '%{http_code}' -m 3 \
    -H "Authorization: Bearer $TOK" http://127.0.0.1:2016/api/v1/configs 2>/dev/null)
  case "$CODE" in
    200) ok "/api/v1/configs → 200" ;;
    000) : ;;
    *) bad "/api/v1/configs → $CODE" ;;
  esac
else
  note "跳过（没有 curl 或没有本地 token）"
fi

# ---------------------------------------------------------------- 5. 策略
step 5 "生命周期（policy 词汇）"
ST=$("$CLI" service status 2>/dev/null)
if [ -n "$ST" ]; then
  case "$ST" in
    *'"policy":"resident"'* | *'"policy":"session"'* | *'"policy":"stopped"'*)
      ok "policy 使用规范词汇" ;;
    *'"mode"'*) note "policy 仍显示旧词汇（旧服务未升级）" ;;
    *) note "没读到 policy（core 后端？）" ;;
  esac
fi

# ---------------------------------------------------------------- 收尾
printf '\n手工确认（界面相关，无法自动判定）：\n'
cat <<'EOF'
  1. 首次启动出现迁移页 → 点「开始迁移」→ 自动继续；之后不再出现
  2. 新建网络后：sudo ls "<config-dir>" 应出现 <id>.toml
  3. 关闭某个网络：文件应变 <id>.toml.disabled，重启后仍不加载
  4. policy=session 时跑一次 `exvlan-cli doctor`，网络不应断
  5. 远端：对端 curl -H "Authorization: Bearer exv_..." http://<host>:2016/api/v1/system
EOF

printf '\n结果：%d 项失败 / %d 项提示\n' "$fail" "$warn"
[ "$fail" -eq 0 ] || exit 1
