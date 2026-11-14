# 镜像机部署 / 升级 Runbook（`dl.exvlan.era-x.cn`）

> 本文档是**可独立执行**的部署说明：镜像机维护者按顺序复制命令即可完成
> 「首次部署」或「从旧版同步脚本升级」。所有文件都在本仓库 `script/kernel-mirror/`。
>
> 同步实现是 **Bun 单一真源**（`index.ts` + `src/**`），另有 `/mirror/trigger` 触发服务：
> 设计、API、`deploy.yaml`/Caddy 的精确 diff 见 [`docs/mirror-toolchain.md`](../../mirror-toolchain.md)。
> 旧的 `exvlan-kernel-sync.sh` / `*.py` 已标 legacy，保留一个周期。
>
> 仓库为 **private**，因此镜像机需要一个只读 token（fine-grained，`Contents: Read`）。

## 0. 目标布局

```
/srv/exvlan/
├── core/                       # 内核资产（GUI 内核在线升级）
│   ├── manifest.json           # 合并最近 KEEP 个 release 的 versions[]
│   └── <platform>-v<ver>.zip
└── upgrade/                    # 应用更新 + Magisk 在线更新
    ├── latest.json             # 官方 tauri-plugin-updater 清单（含 minisign 签名）
    ├── manifest.json           # 自建回退清单（含 sha256）
    ├── magisk_update.json      # Magisk 管理器在线更新清单
    ├── EXVlan.Studio_<ver>_x64.dmg
    ├── EXVlan.Studio_<m>-<p>_x64_en-US.msi        # msi 版本号是 Tauri 自有命名
    ├── EXVlan.Studio_<m>-<p>_x64-setup.exe(.sig)
    ├── EXVlan.Studio_<ver>_x86_64.app.tar.gz.sig
    ├── app-{arm64,arm,x86,x86_64}-release.apk
    └── Exvlan-Magisk-v<ver>.zip
```

对外仅暴露这两个子路径，其余一律 404。

## 1. 一次性准备（首次部署）

```bash
# 专用系统用户（同步脚本以该用户运行）
sudo useradd --system --home /srv/exvlan --shell /usr/sbin/nologin exvlan-mirror || true

sudo install -d -o exvlan-mirror -g exvlan-mirror /srv/exvlan/core /srv/exvlan/upgrade

# 只读 token（fine-grained PAT，仅 Contents: Read-only，绑定本仓库）
sudo install -d -m 750 /etc/exvlan-kernel-mirror
sudo tee /etc/exvlan-kernel-mirror/env >/dev/null <<'EOF'
GH_TOKEN=github_pat_xxxxxxxxxxxxxxxxxxxx
REPO=exvlan/exvlan
DEST=/srv/exvlan/core
UPGRADE_DEST=/srv/exvlan/upgrade
KEEP=5
EOF
sudo chmod 600 /etc/exvlan-kernel-mirror/env
```

## 2. 安装 / 更新脚本与单元

从仓库取出最新脚本（在**任意有仓库副本的机器**上执行，然后拷到镜像机；
或直接在镜像机上 `git clone`/`curl` 内部源）：

```bash
# 前置：bun（同步与 trigger 都是 TS）
curl -fsSL https://bun.sh/install | bash                      # 默认装到 ~/.bun/bin/bun
sudo install -m 755 ~/.bun/bin/bun /usr/local/bin/bun         # systemd 默认 PATH（/usr/local/bin）能找到
/usr/local/bin/bun --version

# 在镜像机上，假设仓库已在 /tmp/exvlan
sudo install -d /usr/local/lib/exvlan-mirror/src
sudo install -m 755 /tmp/exvlan/script/kernel-mirror/index.ts         /usr/local/lib/exvlan-mirror/index.ts
sudo install -m 755 /tmp/exvlan/script/kernel-mirror/trigger-server.ts /usr/local/lib/exvlan-mirror/trigger-server.ts
sudo install -m 644 /tmp/exvlan/script/kernel-mirror/src/*.ts         /usr/local/lib/exvlan-mirror/src/
sudo ln -sfn /usr/local/lib/exvlan-mirror/index.ts /usr/local/bin/exvlan-kernel-sync
sudo install -m 644 /tmp/exvlan/script/kernel-mirror/exvlan-kernel-mirror.service /etc/systemd/system/
sudo install -m 644 /tmp/exvlan/script/kernel-mirror/exvlan-kernel-mirror.timer   /etc/systemd/system/

sudo systemctl daemon-reload
sudo systemctl enable --now exvlan-kernel-mirror.timer
```

### 2.1 触发服务（`/mirror/trigger`，可选但推荐）

发版流水线会在 release 成功后 `POST /mirror/trigger` 立刻同步，省掉「等一个 15 min 周期」。
令牌单独放 600 文件（不要写进 `env` / 仓库）：

```bash
sudo sh -c 'umask 077; head -c 32 /dev/urandom | base64 | tr -d "\n/" > /etc/exvlan-kernel-mirror/trigger-token'
sudo chown root:exvlan-mirror /etc/exvlan-kernel-mirror/trigger-token

sudo install -m 644 /tmp/exvlan/script/kernel-mirror/exvlan-kernel-mirror-trigger.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now exvlan-kernel-mirror-trigger.service

# 自检（本机回环，绕过 Caddy）
curl -fsS http://127.0.0.1:8899/healthz | jq
curl -fsS -H "X-Mirror-Token: $(sudo cat /etc/exvlan-kernel-mirror/trigger-token)" \
  http://127.0.0.1:8899/mirror/trigger | jq '.running,.result,.round'
```

Caddy 侧只加一段反代（精确 diff 见 `docs/mirror-toolchain.md` §5.2），然后
`gh variable set MIRROR_TRIGGER_URL` + `gh secret set MIRROR_TRIGGER_TOKEN` 即可。

`exvlan-kernel-mirror.service` 必须允许写两个目录：

```ini
ReadWritePaths=/srv/exvlan/core /srv/exvlan/upgrade
```

（本仓库的 unit 已包含；若你之前手工改过 unit，记得同步这一行。）

## 3. Caddy

仓库提供完整站点文件 `script/kernel-mirror/caddy.Caddyfile`（`/core/*` + `/upgrade/*`）。
把它放到现有 Caddy 的站点目录后 reload：

```bash
sudo install -m 644 /tmp/exvlan/script/kernel-mirror/caddy.Caddyfile /etc/caddy/sites/dl.exvlan.era-x.cn.caddy
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl reload caddy
```

关键点：
- `/core/manifest.json`、`/upgrade/{manifest,latest,magisk_update}.json` → `no-cache`
- 安装包/签名/zip → 1 年 `immutable`
- 目录浏览关闭，未匹配路径 404

## 4. 首次同步与验证

```bash
sudo systemctl start exvlan-kernel-mirror.service
sudo journalctl -u exvlan-kernel-mirror.service -n 40 --no-pager
```

验证（应全部通过）：

```bash
curl -fsS https://dl.exvlan.era-x.cn/core/manifest.json | jq '.schema, .latest'
curl -fsS https://dl.exvlan.era-x.cn/upgrade/manifest.json | jq '.latest, (.versions[0].assets|keys)'
curl -fsS https://dl.exvlan.era-x.cn/upgrade/latest.json | jq '.version, (.platforms|keys)'
curl -fsS https://dl.exvlan.era-x.cn/upgrade/magisk_update.json | jq '.version, .versionCode, .zipUrl'

# 清单里引用的文件必须真实存在（随机抽一个）
curl -fsSI "https://dl.exvlan.era-x.cn/upgrade/$(curl -fsS https://dl.exvlan.era-x.cn/upgrade/manifest.json | jq -r '.versions[0].assets["macos-x86_64"].file')" | head -1

# 缓存头
curl -sI https://dl.exvlan.era-x.cn/upgrade/latest.json | grep -i cache-control    # no-cache
curl -sI https://dl.exvlan.era-x.cn/upgrade/magisk_update.json | grep -i cache-control  # no-cache

# 目录浏览必须关闭
curl -s -o /dev/null -w '%{http_code}\n' https://dl.exvlan.era-x.cn/upgrade/      # 404
```

同步是**原子**的：先落安装包/签名/zip，最后落清单，客户端不会看到指向缺失文件的清单。

## 5. 从旧版脚本升级（只有 `/core` 的机器）

1. 按第 2 节覆盖 `/usr/local/bin/exvlan-kernel-sync`（新版会额外同步 `/upgrade`）；
2. 按第 2 节确认 unit 的 `ReadWritePaths` 含 `/srv/exvlan/upgrade`；
3. `sudo install -d -o exvlan-mirror -g exvlan-mirror /srv/exvlan/upgrade`；
4. 按第 3 节更新 Caddy 站点文件并 reload；
5. `sudo systemctl start exvlan-kernel-mirror.service`，按第 4 节验证。

旧文件不会被删除；`/core` 的清理逻辑（只保留 manifest 引用的 zip）保持不变。

## 6. 回滚

```bash
# 停定时同步与触发服务
sudo systemctl disable --now exvlan-kernel-mirror.timer exvlan-kernel-mirror-trigger.service

# 还原上一版（Bun 之前是 shell 入口；若你保留了备份）
sudo cp /root/exvlan-kernel-sync.bak /usr/local/bin/exvlan-kernel-sync

# Caddy 还原站点文件后 reload
sudo systemctl reload caddy
```

`/srv/exvlan/upgrade` 目录即使保留也无害（旧客户端不会访问）。

## 7. 常见问题

| 现象 | 原因 | 处理 |
|---|---|---|
| `gh: Not Found (HTTP 404)` | token 无权限 / 仓库名不对 / token 过期 | 检查 `/etc/exvlan-kernel-mirror/env` 的 `GH_TOKEN`、`REPO` |
| `/mirror/trigger` 502 | trigger 服务没起 / 端口不对 | `systemctl status exvlan-kernel-mirror-trigger`、`journalctl -u`；确认真在 `127.0.0.1:8899` |
| `/mirror/trigger` 401 | 令牌不一致 | 宿主 `trigger-token` 与 GitHub secret `MIRROR_TRIGGER_TOKEN` 必须一致 |
| bun: command not found（unit 起不来） | bun 不在 systemd PATH | 装到 `/usr/local/bin/bun`，或改 unit 的 `ExecStart` 用绝对路径 |
| `/upgrade/latest.json` 404，但 `/core/manifest.json` 正常 | 脚本是旧版（没有 `/upgrade` 逻辑）或 Caddy 未加 `/upgrade/*` 段 | 按第 2、3 节更新 |
| `magisk_update.json` 的 `zipUrl` 404 | 该 release 未产出 Magisk 包，或 zip 未同步 | 看 `journalctl` 里 `应用更新资产` 计数；确认 release 内有 `Exvlan-Magisk-*.zip` |
| 客户端更新提示「签名校验失败」 | 公钥与私钥不匹配（换过密钥） | 必须用 `tauri.conf.json` 里那把公钥对应的私钥重新发版 |
| `apply` 后仍是旧版本（macOS） | 下载到的是同名旧包（历史上 `.app.tar.gz` 不带版本） | 新脚本已把版本写进文件名，确认清单里的 URL 带版本号 |

## 8. 与发版的关系

发版（tag）会自动产出 `/upgrade/*` 所需的一切：清单、安装包、`.sig`、Magisk zip 与
`magisk_update.json`。镜像机只负责**拉取并发布**，不需要人工拼装任何文件。
