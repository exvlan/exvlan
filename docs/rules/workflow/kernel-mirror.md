# 内核资产托管（`dl.exvlan.era-x.cn/core/`）

Studio GUI 不再静态链接内核，内核以**独立二进制**运行并支持在线升级。GUI 从固定的
HTTPS 端点拉取 `manifest.json` 与对应平台的内核包，因此需要一个**无需认证**的下载源。

> 主仓为 private，私有仓的 Release 资产下载需要认证，用户端不可能内置 token —— 这就是
> 引入镜像端点的原因。

## 拓扑

```
GitHub Release（private，CI 产出）
  ├── manifest.json
  ├── macos-x86_64-v26.410.20706.zip
  ├── macos-aarch64-v26.410.20706.zip
  ├── windows-x86_64-v26.410.20706.zip
  ├── windows-arm64-v26.410.20706.zip
  ├── linux-x86_64-v26.410.20706.zip
  └── linux-aarch64-v26.410.20706.zip
                │
                │  exvlan-kernel-sync（systemd timer，15 min）
                ▼
        /srv/exvlan/core/            ← Caddy 静态托管（自动 HTTPS）
                │
                ▼
   https://dl.exvlan.era-x.cn/core/
```

只镜像 GUI 真正用到的 6 个平台；其余平台仍随 Release 提供，但不进镜像。

## manifest 契约

```jsonc
{
  "schema": 1,
  "latest": "26.410.20706",
  "versions": [{
    "version": "26.410.20706",
    "rpc_schema": 1,
    "min_studio_version": "26.410.20700",
    "assets": {
      "macos-x86_64":   { "file": "macos-x86_64-v26.410.20706.zip",   "sha256": "…", "entry": "exvlan" },
      "windows-x86_64": { "file": "windows-x86_64-v26.410.20706.zip", "sha256": "…", "entry": "exvlan.exe" }
    }
  }]
}
```

- `rpc_schema` 从 `1` 起，RPC 破坏性变更才 +1；GUI 不匹配则拒绝升级。
- `min_studio_version` 用 `YY.QMM.Mmpp` 数值比较。
- `sha256` 针对 zip 内的**入口二进制**（`entry`），GUI 解压后校验。
- `file` 必须是同一 Release 内存在的资产，`release.yml` 会校验，否则发版失败。

## 服务器准备

```bash
sudo useradd --system --home /srv/exvlan --shell /usr/sbin/nologin exvlan-mirror
sudo install -d -o exvlan-mirror -g exvlan-mirror -m 755 /srv/exvlan/core
sudo install -d -o root -g exvlan-mirror -m 750 /etc/exvlan-kernel-mirror

# fine-grained PAT：Contents = Read-only，仅 exvlan/exvlan
sudo tee /etc/exvlan-kernel-mirror/env >/dev/null <<'EOF'
GH_TOKEN=github_pat_xxx
REPO=exvlan/exvlan
DEST=/srv/exvlan/core
KEEP=5
EOF
sudo chmod 640 /etc/exvlan-kernel-mirror/env
sudo chown root:exvlan-mirror /etc/exvlan-kernel-mirror/env

sudo install -m 755 script/kernel-mirror/exvlan-kernel-sync.sh /usr/local/bin/exvlan-kernel-sync
sudo install -m 644 script/kernel-mirror/exvlan-kernel-mirror.service /etc/systemd/system/
sudo install -m 644 script/kernel-mirror/exvlan-kernel-mirror.timer   /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now exvlan-kernel-mirror.timer
```

DNS：`dl.exvlan.era-x.cn` A/AAAA 指向该主机，放行 80/443。若前面还有反向代理，
记得配置 `trusted_proxies`，否则日志里拿不到真实 IP。

## Caddy

见 `script/kernel-mirror/caddy.Caddyfile`。要点：

- `manifest.json` → `Cache-Control: no-cache`（必须实时）
- `*.zip` → `Cache-Control: public, max-age=31536000, immutable`（版本化文件名）
- `browse false`：禁止目录列表；未匹配路径 `respond 404`

## 验收

```bash
# 清单可达且结构正确
curl -fsS https://dl.exvlan.era-x.cn/core/manifest.json | jq '.schema, .latest, (.versions|length)'

# 缓存头
curl -sI https://dl.exvlan.era-x.cn/core/manifest.json | grep -i cache-control          # no-cache
curl -sI https://dl.exvlan.era-x.cn/core/macos-x86_64-v26.410.20706.zip | grep -i cache-control  # immutable

# 目录列表必须关闭
curl -s -o /dev/null -w '%{http_code}\n' https://dl.exvlan.era-x.cn/core/               # 404
```

## 排错

| 症状 | 原因 | 处理 |
|---|---|---|
| GUI 提示「更新源尚未同步」 | 发版后镜像还没拉 | 等下一个 timer，或 `systemctl start exvlan-kernel-mirror.service` |
| manifest 里引用的 zip 404 | 同步被中断在「落 zip / 落 manifest」之间 | 重跑同步（脚本先落 zip 后落 manifest，重跑即修复） |
| 同步报 401/404 | PAT 过期或权限不足 | 重新签发 fine-grained PAT（Contents: Read-only） |
| 磁盘持续增长 | `KEEP` 太大 | 调整 `KEEP`（默认 5 版）后重跑，脚本会清理未引用文件 |
