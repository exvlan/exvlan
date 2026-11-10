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
| TURN                 | 移除 coturn 的 TURN 中继，只保留 STUN；跨 NAT 兜底交给内核内置 peer relay（节点本身是网络里的 peer，打洞失败即经它转发）                                                                                                                                                     |
| Linux 打包（pre.12） | `.deb` / `.rpm` / `.AppImage`，不签名，AppImage 附 zsync                                                                                                                                                                                                                     |
