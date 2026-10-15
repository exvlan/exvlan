# 本地 exvlan 服务更新 runbook

在本机升级正在运行的 exvlan 守护进程（`exvlan.service`）到某个发布版本。**原则：先离线校验、再隔离试运行、然后备份替换、观察窗判定、异常回滚。**

## 前置事实（本机约定）

- 二进制：`/usr/local/bin/{exvlan,exvlan-cli,exvlan-api,exvlan-api-embed}`；服务 `exvlan.service`
  （`ExecStart=/usr/local/bin/exvlan --config-dir /etc/exvlan -w udp://<config-server>/<user>`）。
- 配置：`/etc/exvlan/*.toml`（每个实例一个文件）；`15888` 为本地 RPC 端口。
- **机器身份**：`/usr/local/bin/et_machine_id`（遗留文件）。新版会读取并迁移为状态文件 → **换版期间必须保留该文件**，否则 config server 认不出机器、可能不下发配置。
  - config server 相关配置在复制的 toml 中 `network_identity` / `[[peer]]`。
- 配置 schema 兼容性：主实例 `Config` 不启用 `deny_unknown_fields`；`[flags]` 键需存在于该版本的 `FlagsInConfig`（proto）。检查见 `--check-config`。

## 步骤

```bash
NEW=/tmp/exvlan-update            # 解压后的新二进制目录
CFG=/etc/exvlan

# 1) 落盘新二进制（从 GitHub Release 下载对应平台 zip 并解压）

# 2) 离线校验（不启动、不碰运行中的服务）
script/service-update/check_config.sh "$NEW" "$CFG"     # 期望全部 exit 0

# 3) 隔离试运行（改 network_name/no_tun/去 peers/异端口/不带 -w；不触碰生产网络）
script/service-update/trial_run.sh "$NEW/exvlan" "$CFG"

# 4) 备份（二进制 + et_machine_id + /etc/exvlan）
script/service-update/backup_restore.sh backup

# 5) 替换
systemctl stop exvlan
install -m0755 "$NEW/exvlan" "$NEW/exvlan-cli" "$NEW/exvlan-api" "$NEW/exvlan-api-embed" /usr/local/bin/
ls -l /usr/local/bin/et_machine_id        # 必须仍在
systemctl start exvlan

# 6) 观察窗校验
sleep 25
systemctl is-active exvlan
journalctl -u exvlan -n 80 --no-pager | grep -iE 'error|panic|failed' | tail
exvlan-cli node        # 实例在列、版本为新版
exvlan-cli peer        # 对端建立、loss 正常

# 7) 异常回滚
script/service-update/backup_restore.sh restore
```

## 判定

- 成功：`is-active=active` + 实例在列 + 对端建立（p2p/relay，loss 正常）+ 版本为新版。
- 失败：实例加载错误、config server 握手失败、端口/身份异常 → **立即回滚**。
- 注意：日志中对某上游域名的 `connect timeout` 若该域名解析到本机公网 IP，属**自连**既有现象，不代表升级失败；以 peers 实际建立为准。

## 风险

- 跨版本较大时行为可能变化（如 smoltcp/private_mode 语义），格式兼容不代表行为一致。
- config server 为远端：客户端新旧协议一般向后兼容，但需启动后看 journal 握手。
- 替换只覆盖 4 个二进制，**不要删 `/usr/local/bin/et_machine_id`**。
