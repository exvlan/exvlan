# 7.3（`26.410.20712-pre.8`）决策记录

> 本轮为硬切版本：路径、版本串、上传鉴权全部重做，**不保留向后兼容**。
> 首次启动需重装一次系统组件（macOS 1 次授权、Windows 1 次 UAC、Android 0 弹窗），此后恢复升级 0 弹窗。

---

## 1. 根因与修复

| 问题                                                | 根因                                                                                                                                                                                                                             | 修复                                                             |
| --------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| 桌面「上传诊断」点击无反应，连 loading 都没有       | `SettingsDialog.vue` 的 `window.confirm()` 在 macOS WKWebView 中永远同步返回 `false` 且不弹窗（wry 0.57.0 的 `WryWebViewUIDelegate` 未实现 `runJavaScriptConfirmPanelWithMessage`）；`uploading.value = true` 在该 `return` 之后 | 改为应用内 PrimeVue 确认框，仅首次；`uploading` 前置             |
| Android「实例在运行，但流量没有进入隧道」           | `syncMobileVpnService()` 只在挂载时调用一次，实例晚于它 ready 时不会补启动                                                                                                                                                       | `runningInstances` 变化时补调用，1s/3s/9s 退避，排除手动断开     |
| 节点列表版本串混乱（`26.410.20711-v26.410.20711~`） | `git_version!` 用 `git describe`，有 `v*` tag 时返回 tag 名再被前缀拼接；`--dirty=~` 追加 `~`                                                                                                                                    | 改为构建时注入 `EXVLAN_BUILD_VERSION`，sha6，去掉 `~`            |
| 诊断 zip 落点不可预测                               | 5 级 fallback（error.log 目录 → cache → config → temp → /tmp）                                                                                                                                                                   | 固定 `app_log_dir/diagnostics`                                   |
| helper 日志不可诊断                                 | `/var/log` 写死、无轮转、无时间戳、不进诊断包                                                                                                                                                                                    | `/Library/Logs/era.exvlan.studio/helper.log`，自写 + 轮转 + 进包 |
| 日志可能静默                                        | 互斥锁毒化后只写 stderr；`file` 为 `None` 时只进内存缓冲                                                                                                                                                                         | 毒化恢复 + 回落 events.log + 30s 重试                            |

---

## 2. 版本规则

| 用途                                                            | 值                                              |
| --------------------------------------------------------------- | ----------------------------------------------- |
| 标准版本（Cargo / package.json / tauri.conf / magisk / README） | `26.410.20712`（`YY.QMM.Mmpp`）                 |
| 展示串（UI / `[boot]` / 内核 / 诊断 / issue）                   | `26.410.20712-pre.8+<sha6>`                     |
| tag                                                             | `v26.410.20712-pre.8`                           |
| Windows MSI                                                     | `26.104.207.12`（`YY.MMQ.mmmm.pp`，四段纯数字） |
| Android versionCode                                             | `26\|410\|20712\|NNN`（末三位 = 预览序号）      |
| 产物文件名                                                      | 手动加 `-pre.8`                                 |

- 序号：**基线内递增，换基线重置为 1**
- 脏构建不标记版本串，只记日志
- `tauri.conf.json` 的 `version` 保持不变（保护 MSI UpgradeCode 行为）

---

## 3. 路径规范 v3（硬切）

```
macOS 用户  ~/Library/Application Support/era.exvlan.studio/   prefs.toml · networks.toml · machine_id
            ~/Library/Logs/era.exvlan.studio/                  error.log · events.log · studio.log · diagnostics/
macOS 系统  /Library/Application Support/era.exvlan.studio/    config/ · core/
            /Library/Logs/era.exvlan.studio/                   exvlan.log · helper.log
            /Library/LaunchDaemons/era.exvlan.studio{,.helper}.plist
            /Library/PrivilegedHelperTools/era.exvlan.studio.helper{,.version}
            /var/run/era.exvlan.studio/helper.sock

Windows     %APPDATA%\era.exvlan.studio\        prefs.toml · networks.toml
            %LOCALAPPDATA%\era.exvlan.studio\   machine_id · logs\ (error/events/studio.log + diagnostics\)
            %ProgramData%\era.exvlan.studio\    config\ · core\ · logs\exvlan.log

Android     filesDir/   prefs.toml · networks.toml · machine_id · logs/
            cacheDir/   diagnostics/ · tmp/
```

- 单一真源 `packages/studio/src-tauri/src/paths.rs`；绝对路径字面量不得出现在其它文件（CI 守卫）
- 临时文件统一 `env::temp_dir()/era.exvlan.studio/`，不再写死 `/tmp`
- Linux 分支保留，仅用于本机可编译验证

---

## 4. 权限

| 对象                                                | 权限                                                     |
| --------------------------------------------------- | -------------------------------------------------------- |
| `prefs.toml` / `networks.toml`（含 network_secret） | `0600`                                                   |
| 服务 `config/`                                      | `0700`                                                   |
| 日志                                                | `0644`                                                   |
| `/var/run/era.exvlan.studio/` + socket              | 目录 `0755`、socket `0666`（鉴权靠 peercred + minisign） |
| Windows                                             | 依赖用户目录默认 DACL（等价 0600）                       |

---

## 5. 日志

| 项       | 结论                                                                               |
| -------- | ---------------------------------------------------------------------------------- |
| 轮转     | 统一 16 MB × 3（三端一致）                                                         |
| 内核 CLI | 新增 `--file-log-size` / `--file-log-count`，由服务启动参数统一传 `16` / `3`       |
| 文件     | `error.log` · `events.log` · `studio.log`(GUI) · `exvlan.log`(内核) · `helper.log` |
| 自愈     | 毒化 `into_inner`、回落 events.log（`[log-fallback]`）、句柄丢失 30s 重试          |

---

## 6. 诊断上传

| 项       | 结论                                                                                                  |
| -------- | ----------------------------------------------------------------------------------------------------- |
| 授权     | 首次应用内确认框 → `diag_upload_consent=1` → 之后 0 弹窗                                              |
| 链路     | 直连（IPv4 优先 + 固定 resolve，`no_proxy`）15s → 系统代理 15s → `/usr/bin/curl` 60s；外层 60s 看门狗 |
| 日志     | `[upload] start`（打包前）→ `zip ok` → 每层 `try/ok/failed + 耗时`                                    |
| 失败     | toast + 自动复制 zip 路径 + 「打开文件夹」                                                            |
| 自动上传 | helper / 服务 / 内核安装失败时触发（含 Android 内核装载），复用首次同意，之后静默 + toast             |
| 采集     | 所有 RPC 3s 超时，坏实例跳过并记录                                                                    |

### Worker 契约

```
GET  /token                → {"token":"<b64url>","exp":<unix>}
     token = HMAC-SHA256(TOKEN_SECRET, `${exp}.${nonce}.${sha256(CF-Connecting-IP)}`)
POST /zip  Authorization: Bearer <token>
     200 → {"urls":[...],"zipUrl":"..."}
     401 → {"reason":"expired|ip-mismatch|bad-signature"}（客户端自动重取一次 token 重试）
     429 → Retry-After（token 10/h/IP、upload 20/h/IP）
     413 → >20MB
```

- 客户端不再持有任何长期密钥（`UPLOAD_KEY` 删除，改用 `TOKEN_SECRET` 服务端签发）
- 不保留 key 回滚开关（硬切）
- issue：正文 = 摘要 + ≤60KB 文本片段（error.log → events.log → exvlan.log 尾）+ KV 链接；标题 `[diag] <版本> <os>-<arch>`；标签 `diagnostics` + `manual` / `install-failed`
- KV 复用 `56e51094…`，随机 32 位 id，3 天过期

---

## 7. 服务 / helper / 实例

| 项              | 结论                                                                      |
| --------------- | ------------------------------------------------------------------------- |
| 首次授权        | 启动即申请                                                                |
| helper 健康检查 | 安装后轮询 `--status` 3s，失败把 `launchctl print` + 日志尾部写 error.log |
| helper 重启     | `KeepAlive` + `ThrottleInterval=10`，启动竞态不算失败                     |
| 实例去重        | 保留 `last_network_instance_id`，其余置 disabled 并记录                   |
| 旧快照          | 删除 `ui-state.json` / `localStorage.networkList`                         |
| legacy 清理     | 删除迁移/拷贝逻辑；仅保留幂等的「发现即删」扫描（7.4 再彻底删）           |

---

## 8. 待办边界

- 服务端节点（relay / host）本轮不升级，后续单独处理
- 7.4 再删除最小残留扫描
- Android 版本列的完整版本串只能 hover 查看（手机无 hover，已接受）

---

## 9. pre.9 补充决策（7.3 预览期修复）

| 项             | 结论                                                                                                                                          |
| -------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| 内核同基线更新 | `want_version` 改用 `BUILD_VERSION`（含 `-pre.N+sha`），同一基线下的预览版之间也能更新内核                                                    |
| helper 强更    | `HELPER_VERSION` 1 → 2（GUI 与 daemon 两处同步），强制重装以启用新 socket/日志路径与时间戳                                                    |
| GUI 日志       | 文件名改为 `studio.log`（内核仍是 `exvlan.log`），上限 32MB → 16MB                                                                            |
| 诊断包         | 分别收 `studio.log`（用户目录）与 `exvlan.log`（系统目录），并带 `prefs.toml`（脱敏整份）                                                     |
| 模式丢失       | prefs 写入加全局锁串行化；`hydrateUiState` 在后端缺键时用前端值反向回填；模式缺失且服务在运行 → 弹一次让用户选（默认服务模式）                |
| 实例去重       | 先按采集结果判断 `running && !error_msg`，优先保留真正能跑的实例                                                                              |
| Windows 安装包 | `tauri.windows.conf.json` 的 `resources` 数组会**整体替换**基础配置 → 补回 `resources/kernel/*`（否则 MSI 里没有内核，整个 Windows 端不可用） |
| Android 版本串 | APK 由 `mobile.yml` 构建，需补 `build-version.sh` 注入，否则版本串缺 `-pre.N+sha`                                                             |
| MSI 版本       | 预览 `1000+序号`、正式 `60000+patch`，保证严格递增                                                                                            |

---

## 10. pre.11 补充决策（上传 / 诊断 / 普通模式 / Android）

| 项                   | 结论                                                                                                                                                                                                                                                                         |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Worker 限流          | 删除 KV 计数兜底，只用 Rate Limiting binding（拿不到计数就放行）。原因：KV 兜底每请求 1–2 次写，一天几十次上传就能把免费额度（1000 写/天）打满，连带 zip 存储一起不可用                                                                                                      |
| Worker zip 存储失败  | `DIAG_KV.put` 包 try/catch，返回 `503 {"code":"kv_unavailable"}`，不再让网关吐 `error code: 1101`                                                                                                                                                                            |
| 上传兜底             | 客户端在 `/zip` 返回 `503/5xx/413` 时自动 POST 文本（`error.log` 尾部 + 网络自检）到 `/`（该路由不写 KV）                                                                                                                                                                    |
| 上传防误点           | 前端 3 秒防抖 + 按钮置灰；后端 `UPLOAD_INFLIGHT` 单飞（第二个调用立即返回「上传进行中」）；自动上传每次启动最多 1 次。**不做**长冷却、zip 复用、跨重启持久化                                                                                                                 |
| 诊断闪窗             | 所有控制台子进程统一走 `kernel::hidden_command`（Windows `CREATE_NO_WINDOW`）；覆盖 `diagnostics::run`、`sc`、`schtasks`、`curl`、`cmd`                                                                                                                                      |
| 诊断提速             | 采集器并行（std 线程 + 独立超时），日志记录每项耗时；Windows 上 70s → 预计 5–10s                                                                                                                                                                                             |
| 诊断原生化           | OS 版本（`SystemVersion.plist` / 注册表 / `/etc/os-release`）、网卡与地址（`if-addrs`）、macOS quarantine（`getxattr`）、DNS TXT/SRV（`hickory-resolver`）、崩溃报告与 WER 目录（文件系统）。路由表 / 服务定义 / 代码签名 / 防火墙仍调系统命令（无稳定原生 API），只隐藏窗口 |
| 统一日志超时         | `log show --last 10m` 给 30s；超时降级 `--last 2m` 重试一次（实测 12s 会整项丢失）                                                                                                                                                                                           |
| 普通模式日志         | 诊断包新增 `<log_dir>/normal/` 下最新 2 个文件、各取尾部 5MB（此前普通模式那段时间在 `exvlan.log` 里是空档）                                                                                                                                                                 |
| macOS 普通模式       | 解析 helper 的 `spawned pid=N` 用于探活；内核消失按 0/0/1/5 秒退避重启，60 秒内最多 3 次，超限提示切回服务模式                                                                                                                                                               |
| helper 竞态          | helper CLI 连接/响应重试 6 次（200ms 递增）；`apply` 前先 `verify_healthy(5s)`，消除「daemon 刚起 → EOF → 误判回退提权」。`HELPER_VERSION` 2 → 3 强制完整重装（否则首次升级仍执行旧二进制）                                                                                  |
| 模式切换             | 切换时清空当前网络选择，并按新内核的真实实例列表恢复（消除 `get_config: No such network instance` 反复重试）；首次切到普通模式弹一次确认                                                                                                                                     |
| Android 分享         | 分享前把文件复制到 `cache/`（FileProvider 只声明 `cache`/`files`/`external`）；失败时自动落盘 `Download/Exvlan/` 并回传路径                                                                                                                                                  |
| Android 上传失败     | 同样落盘到 `Download/Exvlan/`，错误文案里直接给出路径（不新增独立按钮）                                                                                                                                                                                                      |
| TURN                 | 关闭匿名 TURN：`--lt-cred-mech` + 长期凭证、`--no-tls`、`--no-tcp-relay`；UDP 3478 必须保留（STUN Binding 与 TURN 共用同一个 listener，`--no-udp-relay` 会让 coturn 因没有 relay endpoint 直接退出）。跨 NAT 兜底交给内核内置 peer relay                                     |
| Linux 打包（pre.12） | `.deb` / `.rpm` / `.AppImage`，不签名，AppImage 附 zsync                                                                                                                                                                                                                     |

---

## 11. pre.12 补充决策（helper 回归 / 上传幂等 / 诊断包自身）

| 项                    | 结论                                                                                                                                                                                                                                                                                                                                          |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS helper 回归     | pre.11 的 `HELPER_VERSION 2→3` 触发强制重装，`launchctl bootstrap` 三次 EIO，旧 helper 已被 bootout、新的没起来 → 机器上没有 helper、App 起不来。pre.12 升到 4 并在失败时输出 `install_diagnostics`（`launchctl print-disabled`、bin/plist/version 属性、`plutil -lint`、`codesign -dv`、quarantine），**安装算法本身不变**，先拿数据再定修法 |
| helper 失败不阻断启动 | `provision` 里 helper 安装失败只记 `provision.helper failed`，不再 `?` 中断；服务模式不需要 helper，普通模式 TUN 由 UI 提示                                                                                                                                                                                                                   |
| helper 自动重试       | 启动后 UI 就绪时检查一次：未就绪则后台自动重试一次（会弹一次系统授权），失败转手动；横幅 + 设置页各有「重试安装」                                                                                                                                                                                                                             |
| 上传幂等              | 客户端每次 `upload_diagnostics` 生成 `idem`（uuid），三条通道共用；Worker `kv.get("idem:<uuid>")` 命中直接返回原 issue（`deduped: true`），未命中才建并写回 1 小时。修复「A 已建、响应丢失、B 又建一个」的重复 issue                                                                                                                          |
| redact 乱码           | 命中 key 后逐字节 `as char` 是 UTF-8→Latin-1 解码，把「token（中文）」写成乱码；改为按 char 扫描分隔符 + 整段切片。分隔符支持全角 `：`/`＝`，KEYS 增补 `cookie` / `api_key` / `passwd`                                                                                                                                                        |
| 诊断包顺序            | 先跑 7 个采集器再抓日志，包里的 `error.log` 才包含本轮 `[diag.item]`；新增 `diag-timing.txt`，`manifest.json` 每项加 `ms`                                                                                                                                                                                                                     |
| 诊断平台裁剪          | 非 macOS 不再生成 `helper.log` / `unified-log.txt`（避免 `missing` / `not supported` 噪音）；Android 报 `os=android`、跳过 Linux 专属命令、去掉 busybox 不认的 `\b`                                                                                                                                                                           |
| Android 上传          | 移动端 connect 8s / request 12s，A→B→A 再试一轮（无 curl 兜底），外层看门狗 90s                                                                                                                                                                                                                                                               |

---

## 12. pre.13 决策（特权服务合并 / 运行方式 × 目标 / 自建上传服务）

目标：把「两种模式都要能真正组网（TUN）」做成事实，并且**首次授权一次，之后升级/切模式/切目标/换内核零弹窗**。

| 项                  | 结论                                                                                                                                                                                                                                                                                                               |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 特权组件合并        | `helper` + 「内核 daemon 服务」合并为唯一常驻特权进程 `exvlan-service`（macOS LaunchDaemon `era.exvlan.studio.service` / Windows Service，LocalSystem）。删除 `packages/studio/helper`、GUI 侧 `helper.rs`、Windows `schtasks` 计划任务 + `portal.json` + lease                                                    |
| 模式语义            | 模式只是**生命周期策略**：`service ↔ normal` 只发 `mode.set`，**不重启内核、不断网、配置不变**；只有 `off` 停内核。单一 config、固定 `127.0.0.1:15999`                                                                                                                                                             |
| 权限模型            | 首次 `exvlan-service --install <uid>` 一次提权；macOS unix socket + `getpeereid` 校验 owner uid；Windows 命名管道（LocalSystem）                                                                                                                                                                                   |
| 升级零弹窗          | 服务自更新走 IPC `service.update`：GUI 校验 `service.sig` → 服务校验 minisign → fork `--replace-updater` → `launchctl kickstart -k` / `sc stop+start`，失败回滚上一版                                                                                                                                              |
| 换内核零弹窗        | `apply`：staging 权限校验 + minisign → 备份 `exvlan.prev` → 换二进制 → 10s 内 RPC 不就绪自动回滚。`kernel::install_kernel`（提权）作为未签名时的兜底                                                                                                                                                               |
| 普通模式（mac/win） | 也由同一个服务拉起内核，靠**连接存活**判定：GUI 断开 session → 服务停内核。UI 关闭即停，满足「普通模式随 App 退出即停」                                                                                                                                                                                            |
| 普通模式（Linux）   | 不引入常驻服务：`setcap cap_net_admin,cap_net_raw` + `PR_SET_PDEATHSIG`（父进程崩溃内核即被 SIGKILL）。`supervisor.rs` 收敛为 Linux 专用                                                                                                                                                                           |
| RPC 暴露            | 固定 `0.0.0.0:15999`，白名单**由已配置的虚拟网段推导**（回环 + 各 `<config-dir>/*.toml` 的 `ipv4`/`ipv6` 掩码 + 可选额外网段），去掉「允许远程管理」开关。远程只走 mesh，无密钥、无 HTTPS。pre.13 写死 `10.254.0.0/16` 有两个坑：漏掉回环导致 GUI 连不上（`deadline has elapsed`），且用户改网段后远程管理静默失效 |
| 运行方式 × 目标     | 两者正交：`runtime ∈ {service, normal, off}` 决定本机内核怎么活；`target ∈ {local, <remote id>}` 决定界面连哪里。切 target **不停本机内核**、不向远端推本地配置；「远程 + 本机普通模式」允许且不提示                                                                                                               |
| 目标切换器          | 状态栏下拉 + 托盘菜单（目标 / 运行方式两个子菜单）；`RemoteManager` 增删改 + 单测速，移动端全屏对话框                                                                                                                                                                                                              |
| 诊断统一流水线      | `build_diagnostics_package` 是唯一入口；`deliver_upload` / `deliver_share` 只分叉交付方式。上传失败 → 追加 `upload-error.txt`（含每个通道的 HTTP 状态与响应体全文）→ 自动转分享 → 再失败落盘                                                                                                                       |
| 自建上传服务        | Cloudflare Worker 只**下线**（删 DNS/route），代码与 `deploy.sh` 保留作灾备。sgp 上 `diag` 容器（oven/bun，hostNetwork，127.0.0.1:22100）+ `diag.exvlan.era-x.cn`（灰云 A → 8.219.215.202）                                                                                                                        |
| 旧 label 清理       | 保留「发现 `era.exvlan.studio` / `com.exvlan.*` / `exvlan-studio` 就 bootout/删除」的幂等扫描；**清理列表绝不包含当前 label**（pre.12 的 `bootstrap EIO` 就是列表里含当前 label 导致刚写的 plist 被删）                                                                                                            |
| 诊断采集修正        | `join_collect` 先 join 再计时；DNS TXT/SRV 每查询 2s 预算；`list_dir_newest` 只扫 2000 条；Android/iOS 直接短路托盘/菜单调用；`unhandledrejection` 补合成 stack                                                                                                                                                    |

---

## 13. pre.14 决策与事故复盘（pre.13 的三个阻断级 bug）

pre.13 发布后经 macOS / Windows 真机实测，发现 **6 个阻断级问题**，全部在 pre.14 修复。

### 事故一：Windows 安装包里没有 `exvlan-service.exe`

`tauri.windows.conf.json` 的 `bundle.resources` 会**整体替换**基础配置，pre.13 只补了
`resources/kernel/*` 而漏了 `resources/service/*` → `bundled_service()` 返回 None →
安装必然失败（issue #47）。CI 只校验了构建机的 staging 目录，所以照常通过。

修：补全 resources；新增 `script/ci/check-tauri-resources.py`（平台覆盖必须是基础的超集，
PR 阶段拦截）；`Verify service bundled` 增加**真实产物**断言（macOS 查 `.app/Contents`，
Windows 用 `7z l *-setup.exe` 列 NSIS 内容）。

### 事故二：Windows 服务启动即 1067

`--daemon` 在 `StartServiceCtrlDispatcherW` **之前**就建 State、拉内核（含最多 10s 等 RPC），
SCM 判定未响应直接终止；`service_main` 从未执行，`service.log` 只剩 `daemon start`。
且 `service_main` 不注册控制句柄、不上报 `SERVICE_RUNNING`。

修：`run_daemon` 拆三平台版本，Windows 只调 `win::run(p)`（先注册 dispatcher）；
`service_main` 内注册控制句柄 + 上报 RUNNING + 单一 `State` + 退出前 `stop_kernel()`。

### 事故三：macOS socket 权限 `0660 root:wheel` → 普通用户 EACCES

服务 `running`、socket 也在，但 GUI `connect` 直接 EACCES（`events.log`
`Permission denied (os error 13)`），界面永远显示「特权服务未就绪」。
另外 launchd 的 umask 会把 `/var/run/<id>` 建成 0700 root，连目录穿越都被拒。

修：目录显式 0755；socket `chown(owner_uid)` + 0600；属主来源三级
（`service.json` → 0644 的 `.owner` 旁路文件 → 收紧为 0600 root 并告警，**不 fail-open**）。

### 事故四：RPC 白名单丢掉回环 → `deadline has elapsed`

`--rpc-portal-whitelist` 是**整体替换**而非追加。pre.13 只写 `10.254.0.0/16`，
GUI 从 127.0.0.1 连进来被 `on_new_client` 拒绝 —— TCP 能建连、请求无人应答，
表现为 `[cmd:list_network_instance_ids] Timeout: deadline has elapsed` 与
「无法连接至远程客户端」。macOS / Windows 同时中招。

修：白名单改为**由已配置的虚拟网段推导**（见 §12 的 RPC 暴露条目）。

### 事故五：`exvlan-service --version` 报 crate 版本 → 每次启动重装服务

`env!("CARGO_PKG_VERSION")` 永远不含 `-pre.N+sha`，GUI 用 `--version` 与
`BUILD_VERSION` 比较 → 恒不相等 → 每次启动都替换服务二进制并重启。
修：改用 `option_env!("EXVLAN_BUILD_VERSION")`（与 GUI 同源），并在二进制旁写
`service.version`（0644），GUI 优先读文件（零子进程），缺失才回退 `--version`。

### 事故六：`ensure_kernel` 持锁自死锁 → 服务 RUNNING 但没有内核

`ensure_kernel` 持 `inner` 锁时调用 `rpc_whitelist()` / `kernel_args()`，两者内部又
`self.inner.lock()`；`std::sync::Mutex` 不可重入 → 同线程永久阻塞。表现为
`sc query` RUNNING、日志停在 `windows service start`、内核从未 spawn，
且连 `rpc whitelist =` 那行都没有。macOS 更严重（`boot()` 在 `serve_unix` 之前，
连 socket 都不 bind）。

修（锁纪律）：

- 白名单计算改为**无锁纯函数** `compute_whitelist(config_dir, extra)`；
  `kernel_args(&whitelist)` 接收参数
- `ensure_kernel` 只在读写 child 字段时短暂持锁，`spawn` 与 10s 探活移到锁外
- `starting` 标志 + `await_kernel_start()`：并发不重复 spawn，等待方不持锁
- `stop_kernel` 先摘出 child 再等退出（等待期间不持锁）

看门狗：`BOOT_DONE` / `KERNEL_UP` 两个原子量 + 监视线程（**不碰 mutex**）。
`BOOT_DONE` 20s 未置位 → 写日志并 `exit(70)` 让 launchd/SCM 重启。
注意判据是「boot 是否走完」而不是「内核是否起来」—— 后者会让「内核缺失」
这类可解释的失败变成重启风暴。

CI 硬门禁：`service.yml` 的 Windows 作业真实 `--install` 一次，断言日志出现
`rpc whitelist =` 且（`kernel spawned` 或 `boot ensure_kernel failed`），随后清理。
编译与单测都拦不住「启动卡死」，只有真跑一遍才行。

### 顺带修掉的其它不一致

| 项                    | 结论                                                                                                                             |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| 闪 cmd 窗口           | `exvlan-service` 改 GUI 子系统（不分配控制台）+ 所有 `sc`/`schtasks` 走 `CREATE_NO_WINDOW`                                       |
| 命名管道 ACL          | 显式 SDDL `D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)`，不依赖 LocalSystem 默认 DACL                                               |
| 运行方式真源          | `runtime`（prefs）唯一权威；`app_mode` 停止写入，只读迁移                                                                        |
| `rpc_portal`          | 强制 `127.0.0.1:15999`（服务与 supervisor 都写死；旧数据里的自定义端口会连不上）                                                 |
| 模式 UI               | 删除 `ModeSwitcher` 与 6 个在 mac/win 不生效的字段；设置新增「服务」页；「模式」改只读摘要                                       |
| 假成功按钮            | 「停止服务」「卸载服务」原调旧 label 命令，对新服务无效 → 改为 `runtime_apply('off')` / `service_uninstall`                      |
| `service.json` 读不到 | Windows `install()` 从不调 `save_config`；`--diagnose` 用 `unwrap_or_default()` 伪造 `{owner_uid:0}` → 改为明确报 `<unreadable>` |
| 日志重复              | plist 的 `StandardOutPath` 与自写文件重叠（每行两遍）→ 去掉 StdOut，stderr 独立 `service.stderr.log`                             |
| 15999 被占用          | 新增 `port.owner` / `port.cleanup`（只终止路径等于 `<core>/exvlan` 的进程）                                                      |
| DHCP 实例             | 读不到静态地址时回退 `10.254.0.0/16` 并在日志注明                                                                                |
| 诊断域名              | `DIAG_UPLOAD_URL` / `DIAG_TOKEN_URL` → `https://diag.exvlan.era-x.cn`（灰云直连）；旧 Worker 保留待 pre.14 覆盖足够后再下线      |
| `toml` 解析           | `str::parse::<toml::Value>()` 解析的是「值」不是文档 → 每个网络配置都被当成失败、白名单退化只剩回环；改用 `toml::from_str`       |

## 14. pre.17 决策（控制面归位 / 端口重编号 / 无运行时迁移）

### 14.1 组件与数据归属

| 项       | 决策                                                                                                     |
| -------- | -------------------------------------------------------------------------------------------------------- |
| 部署形态 | 只有两种 backend：`core`（裸核，systemd/GUI 子进程监督）与 `service`（特权服务监督内核）                 |
| 生命周期 | 由 `policy` 表达：`resident`（默认，常驻）/ `session`（随界面）/ `stopped`；「随界面」只有桌面客户端能设 |
| 数据权威 | 内核持有网络配置（`config/*.toml`）；服务持有服务配置（`service.toml`）；**客户端只持有 UI/会话状态**    |
| 路由     | 本地管理走环回直连（服务 IPC + 内核 RPC）；远端统一走对端 gateway（HTTP/WS + token）                     |
| 编排     | 唯一编排者是 `exvlan-service`；GUI/CLI/gateway 只发指令，不再各自判断版本/ensure/端口                    |

### 14.2 端口重编号（全部可配置，客户端从 `/api/v1/system.endpoints` 发现）

| 端口              | 用途                                | 默认绑定                                                                               |
| ----------------- | ----------------------------------- | -------------------------------------------------------------------------------------- |
| 1995              | 内核 RPC portal                     | 127.0.0.1（`service.toml` 的 `rpc_portal` 可改成 `0.0.0.0:1995`，供 GUI 直连远端内核） |
| 1996              | `exvlan-api` REST                   | 127.0.0.1                                                                              |
| 2016              | gateway HTTP/WS                     | 127.0.0.1（mesh 仅 opt-in）                                                            |
| 2018              | `exvlan-api` config-server          | 0.0.0.0（客户端必需）                                                                  |
| 11010/11011/11012 | peer 监听（tcp+udp / wg+ws / quic） | 0.0.0.0，支持 `:0` 自动分配                                                            |

历史端口 15999/16000/11211/22020 只保留在「默认值定义/兼容解析/诊断探测」处，由
`script/ci/check-no-hardcoded-ports.py` 守卫。

### 14.3 无运行时迁移/兼容

- 运行时不实现任何数据迁移或旧格式兼容读取；
- 一次性迁移由独立工具 `exvlan-migrate` 完成（本版随安装包分发，首次启动弹窗确认后执行；
  下一版移除随包，仅保留独立工具）。移除方式：把仓库变量 `BUNDLE_MIGRATE` 设为 `0`
  （`studio.yml` 里构建/拷贝/包内断言都随它切换，无需改代码）；此时 App 的迁移页
  会提示「自己跑 `exvlan-migrate --yes` + 重新检测」，不再点出一个必然失败的按钮；
- 迁移工具写 `migrate.log` / `migrate-report.json`，备份在用户目录，支持 `--rollback`；
- 迁移失败停在专用错误页（回滚/重试），不写完成标记。

### 14.4 安全模型

- 本地：环回 + unix socket / 命名管道（OS 鉴权，不做自建鉴权）；
- 远端：gateway Bearer token（`exv_<id>_<secret>`，服务端存 hash），scope 分权
  （`kernel:read/write`、`service:read/control/admin`、`gateway:admin`、`net:probe`）；
- token **只能本地 IPC 签发**（远端不能自我续命），支持过期/吊销/`rotate --grace`，审计带 `token_id`；
- 远端 `service:admin`（装服务/换内核/chown）由 admin token 全权（已接受风险，记录在案）。

### 14.5 网络配置只有一份权威（pre.17 去存储化）

| 项         | 决策                                                                                                            |
| ---------- | --------------------------------------------------------------------------------------------------------------- |
| 唯一权威   | 内核 `--config-dir` 下的 `<id>.toml`；**不再有** `networks.toml` / `localStorage.networkList` 这类第二份存储    |
| 写入者     | `service` 后端：服务 IPC `config.write`（目录归 root，客户端无权直写）；`core` 后端：客户端直接原子写同一目录   |
| 禁用语义   | `<id>.toml.disabled` —— 内核只扫 `*.toml`，所以「禁用」既不丢配置也不需要第二份状态文件                         |
| 运行       | 运行实例走内核 RPC（不落盘）；持久化只发生在配置文件，二者解耦                                                  |
| 迁移       | 旧 `networks.toml` 由 `exvlan-migrate` 一次性转成 `<id>.toml`（下一版不再随包）                                 |
| Linux 特例 | Linux 没有特权服务进程（内核由 systemd 以 root 运行），安装时把配置目录 **chown 给属主**（0700，root 仍可读）   |
| 守卫       | `exvlan-cli config list/get/set/rm/enable/disable`；`doctor` 增加 `config.dir`（不可写 = 保存会失败，直接报错） |

会话计数同时修正：只有发过 `hello` 的连接才算会话。此前任何一次短连接
（`exvlan-cli doctor` / `config list`）在 `policy=session` 下都会把会话数抖到 0，
顺手停掉内核 —— 表现为「CLI 查一下，网络就断了」。

### 14.6 本地 IPC 的边界（pre.17 现状与后续）

| 平台    | 鉴权                                                                                                                                                                                                             | 现状                                                                                          | 后续                                                    |
| ------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| macOS   | unix socket `0600` + 属主 uid（`peercred` 校验）                                                                                                                                                                 | 已按属主收紧                                                                                  | —                                                       |
| Windows | 命名管道 DACL：`SY`/`BA` 全权 + **安装者 SID** 读写（`exvlan_service_client::current_user_sid()` → `--install <uid> <sid>` → `service.toml owner_sid`，服务启动据此构造 ACL）；已加 `PIPE_REJECT_REMOTE_CLIENTS` | 已收窄：只有装服务的那个用户能连。SID 取不到/为空（旧安装）时**回退 `IU`** 并写日志，绝不锁门 | 升级安装会刷新 SID；`--diagnose` 显示 `owner_sid(boot)` |
| Linux   | 无特权服务（core backend），systemd 以 root 跑内核                                                                                                                                                               | 配置目录 chown 给属主（0700），root 内核只读                                                  | 如需常驻服务再补 systemd 版 `exvlan-service`            |

### 14.7 pre.18 热修（真机验收抓出的问题）

pre.17 在 macOS 真机（10.254.1.6，pre.16 → pre.17 实测）验收时暴露 6 个问题，全部修在 pre.18：

| #   | 问题                                                                                                                                            | 修法                                                                                                                                    |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| F1  | `exvlan-migrate` 只找**用户目录**的 `service.json`，macOS/Windows 的在**系统目录** → 迁移后 `owner_uid/policy/rpc_extra_whitelist` 被重置为默认 | `pick_service_json()` 两处都找（用户优先）；`service.toml` 已存在时**不覆盖**（只告警），并补单测                                       |
| F2  | 迁移备份目录 / `migrate.done` 属主是 **root**（GUI 提权运行），用户读不了自己的备份                                                             | 新增 `--owner-uid <uid>`：备份与 marker `chown` 给该用户（含主组）；GUI 自动传入                                                        |
| F3  | 内核 RPC 实际绑 `0.0.0.0:1995`，与文档「127.0.0.1」不符                                                                                         | 默认改回 `127.0.0.1:1995`；`service.toml.rpc_portal` 可显式覆盖（GUI 直连远端内核的老用法），端口解析随之生效                           |
| F4  | 内核版本文件不一致：服务写 `core/version`，service-client 读 `core/exvlan.version` → `doctor` 永远 `kernel=-`                                   | `Paths::kernel_version_file()` 改为 `<core>/version`，与写入方对齐                                                                      |
| F5  | 全新安装时 `doctor` 对不存在的 WebKit 目录报 `!`                                                                                                | 目录不存在降级为 `info`（属正常），只有属主不对才报 error                                                                               |
| F6  | 工具打进安装包但**从不安装**到系统路径，agent/用户只能用 app 包内路径                                                                           | `--install --tools-dir <dir>`：服务把 `exvlan-cli`/`exvlan-gateway` 拷到 `paths.{cli,gateway}_bin`（0755）；Linux 由提权 provision 落盘 |
| F7  | `gateway` 的 `/system` 与 `migrate.done` 版本串缺 `-pre.N`                                                                                      | 统一用 `EXVLAN_BUILD_VERSION`（CI 注入），本地构建回落 crate 版本                                                                       |

验收脚本同步加了第 6 项：检查内核 `--rpc-portal` 是否只绑回环（`script/lab/pre17-acceptance.sh`）。

### 14.8 pre.19：Windows 控制面修复 + 移动端存储统一

**W1（阻断）Windows 命名管道只支持 1 个并发实例**

- 现象：GUI 启动后常驻一条会话连接，把唯一实例占死 → `exvlan-cli`/gateway/doctor/
  GUI 的 `apply`/`mode.set`/`config.write` 全部 `ERROR_PIPE_BUSY (os error 231)`。
- 根因：`pipe_loop()` 是「建 1 个 → 阻塞等连接 → 交线程 → 再建下一个」；
  `PIPE_UNLIMITED_INSTANCES` 只是上限，不会自动扩容（Unix socket 有 backlog 才没暴露）。
- 修法：启动时**预建 16 个实例**，每实例一个线程「等连接 → 处理 → 断开 → 再等」，
  句柄复用；停服时自连 N 次逐个唤醒。客户端遇 231 走 `WaitNamedPipe(600ms)` 重试 4 次；
  `doctor` 新增 `service.pipe_busy`。

**W2 自更新在 Windows 上反复失败**

- 现象：`service.apply` 三种失败（`服务返回空响应` = W1、`备份当前内核失败`、
  `RPC 端口被占`）→ 每次升级都退回 UAC 提权。
- 修法：`apply` 的 kernel → `.prev` 改名改为轮询重试（10×100ms，等 Windows 释放 exe 句柄）；
  仍失败则**不动原内核**并返回 `kernel_busy`（GUI 提示「关掉所有 Exvlan 后重试」）。

**W3 VS Code Remote-SSH 自动转发抢 1995**

- 不是 WSL：该机 `wsl` 未安装，VS Code 用 Remote-SSH，默认 `remote.autoForwardPorts`
  把对端 1995 绑到本机 `127.0.0.1:1995`。
- 缓解：VS Code 设置 `remote.portsAttributes: { "1995": { "onAutoForward": "ignore" } }`；
  产品侧保持「结构化错误 + GUI 卡片 + doctor 换端口」（不自动顺延）。

**M1 移动端存储统一（配置目录唯一权威）**

- 移动端也写 `<app_config_dir>/config/<id>.toml`（禁用 `.toml.disabled`），
  `get_networks`/`set_networks` 变成配置目录视图；FFI 只负责按 TOML 起实例。
- 旧 `networks.toml` 做**一次性转换**（`networks-migrated` 标记；任一条失败则保留源文件、
  不写标记、下次重试），下一版删除这段（`networks/legacy.rs`）。

### 14.9 端口冲突卡片 + NSIS 静默安装的实测结论

**端口冲突（pre.20）**

- 服务把「端口被占」返回成结构化错误 `code=port_in_use` +
  `data={port, occupant{pid,name,path}, kind, hint}`，`kind ∈ exvlan|forwarded|foreign`
  （判定：进程路径命中我们的内核 → exvlan；`Code.exe`/`wslrelay`/`openssh` → forwarded；其余 foreign）。
- GUI 把它渲染成卡片（谁占的 + 按 kind 给的操作指引 + 复制命令 + 诊断 + 重试），
  不再只甩一句红字。
- `doctor` 新增 `net.port_conflict`（走 IPC `port.owner`，内核没跑时也能看出端口被谁占）。

**NSIS `/S` 静默安装（实测，Windows 26xxx）**

| 场景                           | 结果                                                        |
| ------------------------------ | ----------------------------------------------------------- |
| `setup.exe /S /D=<空目录>`     | ✅ 正常安装                                                 |
| 同一目录**再装同版本**         | ⚠️ 退出码 0 但**什么都不做**（文件时间戳不变）              |
| 在已装同版本的默认目录装同版本 | ⚠️ 同上（这就是「静默升级看起来成功、实际没换文件」的原因） |

原因：预览版共用 `26.410.20712` 作为 bundle version，NSIS 认为「已是这个版本」→ 跳过；
而 MSI 侧用 `wix.version`（第 4 段 = 1000 + 预览序号，由 `script/ci/msi-version.py` 生成）
→ **MSI 的同版本重装是有效的**。

自动化/agent 升级因此统一走：

```powershell
# 推荐：MSI 静默重装（版本号已按预览序号递增）
msiexec /i EXVlan.Studio_<ver>_x64_en-US.msi /qn REINSTALL=ALL REINSTALLMODE=vomus
# 或：先卸载再装 NSIS
& "$env:PROGRAMFILES\EXVlan Studio\uninstall.exe" /S ; setup.exe /S
```

用户手动双击安装不受影响（有 UI，可确认重装）。

### 14.10 远端接入（pre.20）：对端出码 + **对端签发** token；gateway 由服务托管

方向与「远程协助类软件」一致：**谁被连，谁出码、谁签发**。
控制端（你）永远不签发 token，只拿着对端给的 token 去调对端的 gateway。

| 角色                 | 做什么                                                                                                           |
| -------------------- | ---------------------------------------------------------------------------------------------------------------- |
| 被管端（target）     | 本机生成一次性配对码（5 分钟）；**兑换时由对端进程签发 token** 并写进自己的 `gateway-tokens.json`（只存 sha256） |
| 控制端（controller） | 输入对端给的码 → 调对端的 `POST /api/v1/pair/redeem` → 拿回 token 明文（一次性）→ 之后带 Bearer 调对端           |

**流程**

1. 对端：`exvlan-cli auth pair --role readonly`（或对端 GUI 的「远程协助」按钮）
   → 本机调用 `POST /api/v1/pair/start`（**仅允许来自回环**）→ 返回 `{code, expires_at}` 并显示给你。
2. 控制端：GUI/CLI 输入该码 → `POST http://<target>:2016/api/v1/pair/redeem {code, client}`
   → 对端校验（存在/未过期/未使用/限速/首次绑定来源 IP）→ **由对端 gateway 进程 mint token**（scope 由码的 role 决定）
   → 返回 `{token, scopes, expires_at}`。
3. 控制端把该 token 存到本地（`<user>/remotes.json`，0600，按目标区分），后续请求带 `Authorization: Bearer`。

**安全约束**

- 出码必须发生在对端本机（`pair/start` 仅回环可调，或走 IPC）——远端无法给自己发码；
- 兑换：一次性 + 5 分钟 + 首次兑换绑定来源 IP + 每 IP 5 次/分钟限速；
- token 只存 sha256（对端）、可过期/吊销、带 scope；审计记录 `token_id + client + 来源 IP`；
- 控制端本地保存的是**明文 token**（它必须能用来发请求），文件 0600、可随时在对端吊销。

**对「单写者」的修订**：原决策是「只有 CLI 写 token 文件，gateway 只读」。
配对需要 gateway 在兑换时落一条 token 记录，因此规则改为：
**token 只能由「对端」的本机进程（CLI 或对端 gateway）签发/写入；控制端永远不签发、不写对端的 token 文件。**

**gateway 的托管**：不再由人工起进程 —— 由 `exvlan-service` 作为子进程监管
（macOS launchd / Windows SCM / Linux systemd 下由服务 fork），配置来自 `service.toml`：

```toml
[gateway]
enabled = true # 默认 true（本机回环）
bind = "127.0.0.1" # 需要被远端接入时改成 mesh 地址
port = 2016
token_file = "<data>/gateway-token"
```

服务负责：随服务启停、异常重启、`status` 里报告 gateway pid/地址。
