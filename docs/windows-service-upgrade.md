# Windows 大版本升级：服务（exvlan-service）生命周期

> 适用：`EXVlan Studio` Windows 安装包在 **ProductCode 变化**的大版本升级场景。
> 两条路径都要覆盖，否则等于没修：
>
> - **MSI（WiX v3）**：手工 / 企业部署（`UpgradeCode` 不变 → 走 `RemoveExistingProducts`）；
>   钩子在 `wix/service-lifecycle.wxs`。
> - **NSIS（`*_x64_*setup.exe`）**：`tauri-plugin-updater` 的**自动更新主路径**
>   （`script/gen_upgrade_manifests.py:64-65` 只匹配 `*setup.exe`）；
>   钩子在 `nsis/service-lifecycle.nsh`，见 §3.5。
>
> 结论源自 dev.1（`26.410.20713-dev.1`）真机取证，修正过一版早期错误假设
> （「旧产品卸载时把服务注销了」已被证伪：`sc query` 与 System 日志 7045 均为 0 条，
> 服务注册自 2026-11-11 04:57Z 起从未丢失）。

## 1. 现场证据（dev.1 真机）

| 编号 | 现象 | 证据 |
| --- | --- | --- |
| M1 | `msiexec REINSTALL=ALL` 跨 ProductCode **静默空转**（返回 0，实际没装） | 手工复现 |
| M2 | 直接 `/i` 新 MSI：`RemoveExistingProducts` 崩 **1601**，Restart Manager 报 **351** | 手工复现；RM 列出的占用方是 `C:\Program Files\EXVlan Studio` 下的进程/文件（**主要是 GUI 自身**，见 R9；内核/gateway/服务本体在 ProgramData） |
| B1 | 升级窗口内服务崩溃 → **SCM 放弃**（无 panic / WER） | `System` 日志 `09:23:58Z id=7034 The Exvlan Service service terminated unexpectedly. It has done this 3 time(s).`；`service.log` 09:19:49 kernel spawned → 09:20:15 `kernel not ready: rpc 超时 10s` + `kernel.restart failed: … (os error 32)` → 09:20:15–09:24:12 近 4 分钟无进程日志 |
| B2 | 首启自愈**静默失败** | 09:20:41Z 提权 `--install-exvlan-service` 未产出 `[service.reinstall] done`（同时刻 `[service.update] bad_request: staging`、`[service.apply] … (os error 2)`）——此刻 `resources/` 正被 MSI 改写 |
| B3 | `--install` 在服务运行中覆盖自身 exe → **os error 32** | 09:29:00Z `[service] cli --install failed: 复制服务二进制失败("…\bin\exvlan-service.exe"): 另一个程序正在使用此文件 (os error 32)`；`[elevate] exit code: 1` → UI 误报「安装失败」，而服务其实在跑 |
| B4 | 状态语义把「管道不通」当成「未安装」 | `[fe:diag] service_auto_retry 连接服务管道失败: os error 2` → 触发重装 |
| B7 | staging 残留 | `bin\exvlan-service.exe.new`(11/13)、`core\exvlan.exe.new`(11/14)、`core\v.txt` 仍是 `26.410.20712-pre.19+e27ee2` |
| S1 | 服务注册从未被 MSI 删除 | `sc query` = `RUNNING` / `AUTO_START` / `BINARY_PATH_NAME=C:\ProgramData\era.exvlan.studio\bin\exvlan-service.exe --daemon` |

## 2. 根因链条（file:line，基于 dev.1 = `33ed7f4c`）

1. **MSI 完全不认识这个服务**
   `packages/studio/src-tauri/tauri.windows.conf.json` 里只有
   `bundle.windows.wix.version`，没有 `fragmentPaths` / `componentRefs` /
   `featureRefs`。服务是 GUI 首次运行时提权注册的
   （`packages/studio/src-tauri/src/service_client.rs:311` → `lib.rs` 的
   `--install-exvlan-service` → `exvlan/service/src/main.rs` `install()`）。
   ⇒ 升级时没人停它：服务在安装窗口内仍活着（看门狗每 5s 拉起 gateway、内核按需
   重启），与 MSI 争抢 `C:\Program Files\EXVlan Studio\resources` 下的随包二进制
   → 读半截 / 复制失败（`os error 32`）→ 服务崩、SCM 三次后放弃（B1）；
   `RemoveExistingProducts` 阶段就是 **1601**（M2）。
   注意：内核 / gateway / 服务**跑在 `C:\ProgramData\era.exvlan.studio\{core,bin}`**
   （`service/main.rs:232-247`、`:1517`），它们并不常驻占用 Program Files；
   真正长期占用 Program Files 的是 **GUI 自身**（`EXVlan Studio.exe` + WebView2 +
   它正在读的 `resources`），这一项停服务解决不了 → 见风险 R9。

2. **停服务用的是被本地化的文本判断**
   `exvlan/service/src/main.rs:4653-4665`（dev.1）`stop_service_blocking()` 用
   `sc query` 的 **字符串** 判断：`text.contains("STOPPED")`。真机是中文
   Windows，状态名随系统语言走 → 判断可能永不成立，紧接着
   `install()` 在服务仍运行时覆盖 exe → **os error 32**（B3）。
   `replace_updater()` 同样问题（`main.rs:4728-4735`）。

3. **升级窗口内服务崩了没人管**
   MSI 铺文件期间 `core\exvlan.exe` 被替换 → 服务 `kernel.restart` 拿到
   `os error 32` → 服务退出 → SCM 三次后放弃（B1）。MSI 收尾没有任何
   「把服务拉回来」的动作。

4. **GUI 首启自愈把「没就绪」当「没安装」**
   - `service_client.rs:263`（dev.1）`state()` 只有 `installed` / `ready` 两个布尔；
   - `service_client.rs:319-325`：`install()` 只等 5s 就报「服务已安装但 IPC 未就绪」，
     且出错文本不可操作；
   - `lib.rs:5076-5078`：`ensure_service_and_kernel()` 在
     `installed && 版本一致 && 内核一致` 时**直接返回 `current`**，无视 `ready`；
   - `lib.rs:5230-5241`：`service_install` 命令**无条件**调用 `install`；
   - `pages/index.vue:145-155`：`!ready` → 自动 `onRetryService(true)` → 重装。
   ⇒ 服务其实是 RUNNING，却先被覆盖 exe（B3）再被误报失败（B2）。

5. **状态判据只看 SCM**
   `exvlan/rpc/src/service_manager/mod.rs:548`：只有 SCM 返回 `0x424`
   （服务不存在）才算 `NotInstalled`；而 UI 侧把「管道不通」也当成未安装（B4）。

6. **UpgradeCode 派生自 `productName`，因此它其实一直是稳定的**
   `tauri-bundler` 在未显式配置时用
   `Uuid::new_v5(NAMESPACE_DNS, "<productName>.exe.app.x64")`。
   `packages/studio/src-tauri/tauri.conf.json` 的 `productName` 是
   `EXVlan Studio` ⇒ UpgradeCode = `42C08A88-9ABD-5406-B191-2A8543B12B98`。
   **`productName` 不得修改**（它同时决定 MSI 文件名与 UpgradeCode）。
   本次把它**显式写进配置**，避免以后有人改 `productName` 时静默换掉 UpgradeCode
   （那样升级会变成「并存安装」，比丢服务更难查）。

## 3. 改动清单

### 3.1 MSI：升级前停服务、装完拉起（方案 A，已实施）

新增 `packages/studio/src-tauri/wix/service-lifecycle.wxs`（WiX v3 fragment），
并在 `packages/studio/src-tauri/tauri.windows.conf.json` 注册：

```json
"wix": {
  "version": "26.104.20713.50013",
  "upgradeCode": "42C08A88-9ABD-5406-B191-2A8543B12B98",
  "fragmentPaths": ["wix/service-lifecycle.wxs"]
}
```

> `fragmentPaths` 相对 **`src-tauri/`** 解析：tauri-cli 在打包前
> `set_current_dir(tauri_dir())`，bundler 用 `current_dir().join(path)`。
> 片段按**原文件**交给 `candle`（bundler 只用 Handlebars 预渲染一遍来探测
> extension，不写回），因此片段里**不能出现 `{{ }}`**。

四个自定义动作（全部 `Return="ignore"`；可执行文件用
`[SystemFolder]WindowsPowerShell\v1.0\powershell.exe` 全路径，且不用
`-ExecutionPolicy Bypass`——`-Command` 内联脚本不受执行策略限制，去掉这一串能少
一个 EDR 高危特征）：

| 动作 | Execute | 时序 | 条件 | 作用 |
| --- | --- | --- | --- | --- |
| `ExvlanStopServiceForUpgrade` | immediate | `Before="InstallValidate"` | 恒真 | `Stop-Service` 等 STOPPED（≤30s）；**确认已停/不存在之后**才 `Stop-Process` 清孤儿 `exvlan.exe` / `exvlan-gateway.exe` |
| `ExvlanStartServiceAfterInstall` | immediate | `After="InstallFinalize"` | `NOT REMOVE` | 把服务拉回 RUNNING（覆盖 B1 的「升级窗口内被 SCM 放弃」）；服务不存在则静默跳过 |
| `ExvlanStartServiceOnRollback` | rollback | `After="InstallInitialize"` | `NOT REMOVE` | 安装脚本中途失败回滚时把服务拉回来 |
| `ExvlanRemoveServiceOnUninstall` | immediate | `Before="RemoveFiles"` | `REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE` | 真卸载才停 + `sc delete`（只删 SCM 项） |

**为什么 immediate 动作不写 `Impersonate`**：`msidbCustomActionTypeNoImpersonate`
必须与 `msidbCustomActionTypeInScript`（deferred/rollback/commit）同时出现才有意义，
在 immediate 动作（type 34）上引擎直接忽略该位——写了只是噪声（WiX 也不会报错，
`Compiler.cs` 对 `Impersonate` 没有 Execute 交叉校验）。**唯一写 `Impersonate="no"`
的是 rollback 那条**：它属于 in-script 动作，该位在这里真实生效（保证回滚时以服务端
身份启动服务）。

**为什么孤儿清理要加前置条件**：服务正常停止时 `service_main` 已经先
`stop_kernel()` → `stop_gateway()` 再报 STOPPED（`service/main.rs:3953-3959`），
所以清理只为收「孤儿」。而 `spawn_gateway_watchdog`（`main.rs:2175-2205`）每 5s
探活、按 1/2/5/10s 退避重启：**服务还活着时**去 kill gateway，会在 1–6s 内被拉回来，
等于在安装窗口里制造反复抖动、放大 B1。因此 PowerShell 里先用
`$stopped` 记住「已停 or 本来没有」，只有它为真才清理。

关键取舍：

- **必须早于 `InstallValidate`**：MSI 的 Restart Manager 文件占用检查就在这一步，
  晚于它就没意义（这也是选 `Execute="immediate"` 的原因——deferred 动作只能排在
  `InstallInitialize` 之后）。带 UI 的安装里，`InstallValidate` 之前的动作会先在
  客户端跑一遍、再在服务端（perMachine 为 SYSTEM）重跑；能停服务靠的是**服务端
  那一遍**的上下文，与 `Impersonate` 位无关（见上文）。
- **不创建服务**：不写 `<ServiceInstall>`。服务二进制在 ProgramData，MSI 不拥有它；
  注册由 GUI 首启自愈负责（沿用现有模型）。
- **不依赖 `exvlan-service` 自己的命令行面**：升级源可能是任意旧版本，
  所以停/启/注销全走 `sc` / `Get-Service`。
- **失败不阻塞安装**：`Return="ignore"`。停不掉只是回到旧行为（RM 弹
  「文件正在使用」），不该让整机装不上。

### 3.2 服务端：语言无关 + 明确报错 + 收尾清理

`exvlan/service/src/main.rs`：

| 位置 | 改动 |
| --- | --- |
| `install()`（Windows） | 停不下来时**明确报错**（`服务仍在运行，未覆盖 …；请先 sc stop`），不再让 `copy` 抛裸 os error 32 |
| `stop_service_blocking(timeout)` | 改为 SCM API（`windows-service`）轮询 `ServiceState::Stopped`，**与系统语言无关**（原实现匹配 `"STOPPED"` 文本） |
| `start_service_scm()` | 新增；同样走 SCM API |
| `restart_service()` / `replace_updater()`（Windows） | 改用上面两个函数（去掉 `sc query` 文本匹配）；`replace_updater` 回滚分支的两次 `start_service_scm()` 结果都写日志 |
| `cleanup_stale_staging()` | 新增，`boot()` 时调用：删除 `bin\exvlan-service.exe.new`、`core\exvlan.exe.new`、`core\tools.new\`、`core\v.txt`（B7），**要求 mtime > 10 分钟**（避免删掉正在进行的替换）；不碰 `config/`、`logs/`、`*.prev`、token |
| `replace_updater()`（Windows） | 替换成功后删除 `.new` 暂存 |
| `uninstall()`（Windows） | `stop_service_blocking` + `sc delete`；按**退出码**区分（0 成功 / 1060 幂等 / 其它记日志并返回错误）；**只删 SCM 项**，不删二进制、不删数据 |

**不做「版本一致就跳过覆盖」的短路（R11）**：一度加过，但正常自愈路径进不到那里
（未安装时服务本来就没在跑，版本不一致时也该覆盖），它唯一能命中的场景恰恰是
**强制重装**（`service_reinstall` → `service_client::install` → 服务端直接 return），
于是 UI 报成功而实际什么都没做。B3 的重复覆盖已由「GUI 三态分流」+「等 STOPPED
再复制」解决，故删除短路，而不是再加一个 `--force-install` 旁路。

### 3.3 GUI：三态判据 + 只在该重装时重装

`packages/studio/src-tauri/src/service_client.rs`：

- 新增 `ServiceHealth { NotInstalled, InstalledNotReady, InstalledRunning }`，
  随 `ServiceState` 一起序列化（`health` 字段，kebab-case）。
- `NotInstalled` 的判据是「二进制不在 **或**（Windows）SCM 里没有注册项」——
  MSI 卸载现在会 `sc delete` 而保留 ProgramData 里的二进制，这种机器必须走
  `install` 重新注册（`--restart` 对不存在的服务只会报错）。
- 新增 `repair()`：已安装且已注册 → 提权 `--exvlan-service-ctl restart`，
  等 IPC 就绪（≤30s），失败给**可操作**提示（`sc query` / 日志路径 / 「强制重装」）。
- `install()` 的等待从 5s 提到 30s（内核冷启动最长 10s），错误文本换成上面的提示。

`packages/studio/src-tauri/src/lib.rs`：

- `ensure_service_and_kernel()`：早退条件加上 `ready`；新增
  `installed && 版本一致 && !ready` → `repair()`（**不重装**）；`ensure_kernel`
  失败时附带服务状态 / 二进制路径 / 日志路径。
- `service_install` 命令（界面「重试」按钮 + 启动自动重试都走它）按 `health` 分流：
  `InstalledRunning` → `current`（幂等，不重启）；`InstalledNotReady` → `repair`；
  `NotInstalled` → `install`。

`packages/studio/src`：`ServiceState` 加 `health` 字段；启动自动重试日志带上
`health`（`service_auto_retry: installed-not-ready: …`），便于日后取证。

### 3.4 方案 B（备选 / 后续）

| 场景 | 做法 | 覆盖面 |
| --- | --- | --- |
| **自动更新走的是 NSIS 而不是 MSI** | `bundle.windows.nsis.installerHooks` + `!macro NSIS_HOOK_PREINSTALL`（同样先停服务） | **已实施**（dev.2，见 §3.5）：`script/gen_upgrade_manifests.py` 里官方 updater 的 Windows 规则匹配 `*_x64_*setup.exe`（NSIS），MSI 只服务手工/企业部署 |
| MSI 若仍报 351 | 追加 `<ServiceControl Id="…" Name="era.exvlan.studio.service" Stop="both" Wait="yes" Remove="uninstall"/>`（需要挂在一个 Component 上，即 `componentGroupRefs`/`componentRefs`） | MSI 原生停服务（SYSTEM 上下文），但 `StopServices` 排在 `RemoveExistingProducts`（`afterInstallInitialize` = 1501）**之后**，对升级窗口来得太晚，所以只作兜底 |
| 让 GUI 在 RM 关它之前自己收尾 | Tauri `RunEvent::ExitRequested` 里 `service_client::stop()` | 只对**新版本**有用；老版本 GUI 没有这段代码，救不了当前这批用户 |

### 3.5 NSIS（自动更新主路径）：停服务、装完拉起（已实施）

**为什么必须补这一条**：官方 updater 在 Windows 上下载的是 `…_x64-setup.exe`，
不是 MSI（`script/gen_upgrade_manifests.py:64-65` 的 `^.*_x64.*setup\.exe$`）。
MSI 里那四条自定义动作对自动更新**一行都不生效** —— 也就是 R1 的原始结论。

新增 `packages/studio/src-tauri/nsis/service-lifecycle.nsh`，并在
`packages/studio/src-tauri/tauri.windows.conf.json` 注册：

```json
"windows": {
  "wix": { "…": "…", "fragmentPaths": ["wix/service-lifecycle.wxs"] },
  "nsis": {
    "installMode": "perMachine",
    "installerHooks": "nsis/service-lifecycle.nsh"
  }
}
```

**tauri v2 的准确接入面**（`tauri-utils 2.10.1` 的 `config.rs` + `tauri-bundler` 的
`nsis/installer.nsi`，对照 tag `tauri-v2.12.2` 与 `tauri-v2.12.4` 钩子结构完全一致；
不是凭记忆）：

| 项 | 值 | 依据 |
| --- | --- | --- |
| 配置字段 | `bundle.windows.nsis.installerHooks`（serde 别名 `installer-hooks`），类型 = 单个 `.nsh` 路径 | `tauri-utils/src/config.rs` `NsisConfig::installer_hooks` |
| 可用宏 | `NSIS_HOOK_PREINSTALL` / `NSIS_HOOK_POSTINSTALL` / `NSIS_HOOK_PREUNINSTALL` / `NSIS_HOOK_POSTUNINSTALL` | 同上 doc comment；官方文档「Customizing the NSIS Installer → Extending the Installer」 |
| 路径解析 | 相对 **`src-tauri/`**：tauri-cli 打包前 `set_current_dir(dirs.tauri)`（`tauri-cli/src/bundle.rs`），bundler 再 `dunce::canonicalize(installer_hooks)`；与 `wix.fragmentPaths` 同一套 | 同左 |
| 插入位置 | 钩子文件在 `installer.nsi` 顶部被 `!include`；`NSIS_HOOK_PREINSTALL` 是 `Section Install` 的**第一条**语句（早于 `File` 复制与注册表写入），`NSIS_HOOK_POSTINSTALL` 是 `Section Install` 的**最后一条**（晚于文件/注册表/快捷方式） | tauri-bundler `nsis/installer.nsi` 的 `Section Install` |
| 「NOT REMOVE」 | NSIS 的卸载是另一支（`Section Uninstall`），安装钩子只在安装/升级时展开，**位置天然表达 NOT REMOVE**，不需要额外条件 | 同上 |
| 执行身份 | `perMachine` → `RequestExecutionLevel admin` → 钩子以**提权后的管理员令牌**执行（UAC 同意；仍是当前登录账号，不是 SYSTEM）。`MSI` 侧 `Execute="rollback"` 那种 in-script 动作才是 SYSTEM | `installer.nsi` 顶部 `!if "${INSTALLMODE}" == "perMachine" … RequestExecutionLevel admin` |

**为什么顺带把 `installMode` 改成 `perMachine`**（本次唯一的行为变更，务必知情）：

- 默认 `installMode` 是 `currentUser` → `RequestExecutionLevel user`，安装包**不提升权限**
  （updater 用 `ShellExecuteW("open", …)` 拉起 setup.exe，没有 `runas` 动词，UAC 与否
  完全由 setup.exe 自己的 manifest 决定）。
- 而服务是 GUI 提权注册的 SYSTEM 服务，SCM 默认安全描述符只给 Interactive Users
  `SERVICE_QUERY_STATUS`（**没有** `SERVICE_STOP`/`SERVICE_START`）⇒ 非提权进程
  `Stop-Service` / `Start-Service` 一律 `拒绝访问`，钩子等于空转。
- 所以 **`installerHooks` 必须与 `installMode=perMachine` 成对出现**；只加钩子不加
  `installMode` 会得到一个「日志里写着 `stopped=False`」的假修复。
- 附带好处：NSIS 安装目录从 `%LOCALAPPDATA%\EXVlan Studio` 变成
  `$PROGRAMFILES64\EXVlan Studio`，与 MSI 一致（受影响的迁移与残留见 R13）。

钩子做什么（**与 MSI 逐条对齐**，故意不做的事写在注释里）：

| 钩子 | 作用 | 失败策略 |
| --- | --- | --- |
| `NSIS_HOOK_PREINSTALL` | `Stop-Service … -Force` 并 `WaitForStatus('Stopped','00:00:30')`；**只有确认服务已 Stopped 或本来不存在**（`$stopped` 为真）才 `Stop-Process exvlan, exvlan-gateway` 收孤儿 | `exit 0`，只把 `stopped=` 与退出码写进日志；停不掉 = 回到旧行为 |
| `NSIS_HOOK_POSTINSTALL` | 服务存在且不是 `Running` → `Start-Service` + `WaitForStatus('Running','00:00:30')` | 同上，`before=` / `started=` 进日志；服务不存在（从没启动过 GUI、或刚从 MSI 迁移）就静默跳过 |

**为什么孤儿清理要判断 `$stopped`**：与 MSI 侧同一条坑。服务活着时看门狗
（`service spawn_gateway_watchdog`）每 5s 把 gateway 拉回来，1–6s 内会重建，等于在
安装窗口里制造抖动、放大 B1。服务正常停下时它自己已经先
`stop_kernel()` → `stop_gateway()`，所以清理只为收孤儿。

**日志在哪**：NSIS 没有 `msiexec /l*v` 那种安装日志（`/P`、`/S` 下 `DetailPrint`
也看不到），所以钩子自己往 `%TEMP%\exvlan-nsis-hooks.log` **追加**一行：

```
2026-11-14T18:33:34.3462100+08:00 [preinstall] user=<账号> elevated=True stopped=True
2026-11-14T18:33:38.5661664+08:00 [postinstall] before=Stopped started=True
```

over-the-shoulder 提权（填的是另一个管理员账号）时 TEMP 落在那个账号下，
同时在 `C:\Windows\Temp\exvlan-nsis-hooks.log` 看一眼。

**与 MSI 的语义对照**

| 语义 | MSI（`wix/service-lifecycle.wxs`） | NSIS（`nsis/service-lifecycle.nsh`） |
| --- | --- | --- |
| 停服务 + 等 STOPPED ≤30s | `ExvlanStopServiceForUpgrade`，`Before="InstallValidate"` | `NSIS_HOOK_PREINSTALL`，`Section Install` 第一条 |
| 孤儿清理前置条件 | 仅当 `$stopped`（已停 or 不存在） | 同（同一段 PowerShell） |
| 装完拉起 | `ExvlanStartServiceAfterInstall`，`After="InstallFinalize"`，`NOT REMOVE` | `NSIS_HOOK_POSTINSTALL`，`Section Install` 末尾 |
| 失败容忍 | 全部 `Return="ignore"` | 全部 `exit 0`（NSIS 侧根本不看退出码，只 `DetailPrint`） |
| 创建服务 | 不创建（无 `ServiceInstall`） | 不创建 |
| 数据目录 | 不触碰 `C:\ProgramData\era.exvlan.studio` | 同（只有 `Get-Service`/`Stop-Service`/`Start-Service`/`Get-Process`/`Stop-Process`） |
| 卸载注销 SCM | `ExvlanRemoveServiceOnUninstall`（`REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE`）→ `sc delete` | **刻意不做**（见 R12） |
| 回滚拉起 | `ExvlanStartServiceOnRollback`（`Execute="rollback"`） | 没有对应物：NSIS 装到一半失败时服务仍停着，靠下次开机（`AUTO_START`）或 GUI 首启 `repair` 恢复（R16） |
| 执行身份 | 服务端序列 = SYSTEM；rollback 动作 `Impersonate="no"` | 提权管理员令牌（perMachine + UAC） |

**`/UPDATE` 与「MSI → NSIS 迁移」的实际时序**（读 `installer.nsi` 得到，不是推测）：

1. 已经在用 NSIS 的用户：updater 传 `/UPDATE` ⇒ `PageLeaveReinstall` 直接
   `Goto reinst_done`（**不跑旧卸载器**）⇒ 只有我们的 PRE/POST 两个钩子。
2. 还在 MSI 上的用户：`PageReinstall` 扫 `HKLM\…\Uninstall` 命中
   `DisplayName=EXVlan Studio` + `msiexec` 的 `UninstallString` ⇒ `$WixMode=1` ⇒
   **即使带 `/UPDATE` 也强制先卸载 MSI**（`PageLeaveReinstall` 里 `$WixMode = 1`
   的判断在 `$UpdateMode` 之前）。这一步会触发 MSI 的
   `ExvlanRemoveServiceOnUninstall` → `sc delete`，于是 NSIS 的 POSTINSTALL 看到的是
   「服务不存在」→ 静默跳过 → **服务由升级后自动启动的 GUI 重新注册（一次 UAC）**。
   也就是说：MSI→NSIS 这一次切换后服务会短暂回到 `not-installed`，这是既有模型的
   正常路径（GUI 三态自愈的 F4 用例），不是新缺陷；发布说明里要写清。
   注意这一整套都在 `PageReinstall` 里，**`/S`（全静默）会跳过页面** ⇒ 不会卸 MSI、
   也就不会 `sc delete`。我们的 updater 用默认 `BasicUi`（或 `/P`），页面会跑；
   若有人把 `plugins.updater.windows.installMode` 改成 `quiet`，MSI→NSIS 迁移就不成立
   （旧 MSI 与新 NSIS 并存），这一点记在这里备查。

**产物命名不受影响**：`gen_upgrade_manifests.py` 的规则不用改。
`installerHooks`/`installMode` 都不参与命名，installer 名仍是
`{productName}_{version}_{arch}-setup.exe`（tauri-bundler `nsis/mod.rs`
`package_base_name`），`^.*_x64.*setup\.exe$` 照样命中；`.sig` 也没变。


## 4. 数据安全红线（已核对的代码路径）

- 任何 MSI 动作都**不触碰** `C:\ProgramData\era.exvlan.studio`：fragment 里只有
  `sc`/`Get-Service`/`Stop-Process`。
- 任何 NSIS 钩子同样**不触碰**该目录：`nsis/service-lifecycle.nsh` 里只有
  `Get-Service`/`Stop-Service`/`Start-Service`/`Get-Process`/`Stop-Process`
  （日志写在 `%TEMP%\exvlan-nsis-hooks.log`，不写进应用数据目录）。
- `--uninstall`（`main.rs` `uninstall()`）只 `sc stop` + `sc delete`，**不删文件**。
- `--purge` 只在 Linux 实现（`main.rs`），Windows 直接 `bail!`，且**没有**挂到
  任何 MSI 卸载路径上。
- 新增的 `cleanup_stale_staging()` 白名单只有 4 个名字（见 3.2），并有 10 分钟
  mtime 门槛；不删 `*.prev`（回滚备份）、不删 `config/`、`logs/`、token。

## 5. 未覆盖风险

- **R1（已关闭，dev.2）**：`tauri-plugin-updater` 在 Windows 上走的是 **NSIS**
  （`script/gen_upgrade_manifests.py:64-65` 匹配 `*setup.exe`）。dev.2 补了
  `packages/studio/src-tauri/nsis/service-lifecycle.nsh` + `nsis.installerHooks`
  （§3.5），**自动更新路径现在也停/启服务**。前提是 `installMode=perMachine` 一并生效
  —— 见 R13；两者拆开就退回假修复。
- **R2（老 MSI 用户）**：`pre.19` / `dev.1` 的 MSI 里没有这些动作，
  从它们升级到 dev.2 **第一次**运行时，靠的是 dev.2 自己的
  `ExvlanStopServiceForUpgrade`（它在新 MSI 里，所以仍然生效——停服务用的是
  `sc`，与旧版本无关）。但**反向**（dev.2 → 更早版本）不保证。
- **R3（权限，与 Impersonate 无关）**：immediate 动作由「客户端序列」先跑一遍
  （非提权）、再由服务端重跑一遍。若安装是「标准用户 + 代填管理员凭据」的
  越权（over-the-shoulder）形式，客户端那一遍必然失败（`Return="ignore"` 吞掉），
  只能指望服务端那一遍；若服务端那一遍也没拿到足够权限（例如 MSI 被以
  `MSIINSTALLPERUSER` 之类方式拉起），`Stop-Service` 会失败 → 仍回到 351。
  验证清单 C 步专门检查这一点。
- **R4**：`Stop-Process exvlan/exvlan-gateway` 会强杀**任何**同名进程
  （包括用户手工跑的 CLI 内核会话）。升级期间可接受，但属于行为变化。
  另一面：现在**只在确认服务已停/不存在时才清理**（M2），服务还活着时不会
  与 5s 看门狗（`main.rs:2175-2205`）互相打架。
- **R5**：Linux 上**无法**构建/验证 MSI（需要 Windows runner + WiX 3.14）。
  本次已做的验证见 §6。
- **R6**：`productName` 与 `identifier` 未改；`upgradeCode` 与历史包一致性
  只能靠「同一 `productName` + 同一派生公式」推断，没有在真机上比对过已发布
  MSI 的 Property 表（验证清单 A3 补这一步）。
- **R7（B2 未根治）**：GUI 首启自愈是在**安装窗口内**跑的，此时
  `C:\Program Files\EXVlan Studio\resources\` 可能仍在被 MSI 改写，
  `install` 仍可能因「随包二进制读不全」失败。本次的缓解是：MSI 先停服务
  （缩小窗口）+ 失败给可操作错误而不是静默（B2 原来的表现）。要根治需在
  GUI 侧加「等安装完成」的判据（例如 `msiexec` 仍在跑就不启动自愈）。
- **R8（已修正）**：immediate 动作**不再**写 `Impersonate`（该位在 type 34 上被
  引擎忽略，见 §3.1）；只有 rollback 动作保留 `Impersonate="no"`，那里它真实生效。
  因此不存在「WiX 拒绝该组合导致构建失败」的问题。
- **R9（停服务解决不了的部分）**：长期占用 `C:\Program Files\EXVlan Studio` 的是
  **GUI 自身**（`EXVlan Studio.exe` + WebView2 + 它正在读的 `resources`）。
  GUI 运行中手工跑 MSI，Restart Manager 仍会报「文件正在使用」并要求关闭界面——
  这是 MSI 的正常交互，不是本改动的缺陷。请**先退出 GUI 再装 MSI**（验证清单 B0）。
- **R10（Cancel 路径）**：`ExvlanStartServiceOnRollback` 只覆盖「执行序列中途失败
  回滚」。用户在 `CostFinalize` 之后、执行序列开始**之前**点 Cancel，MSI 不会执行
  任何回滚动作 → 服务会保持「已被停掉」的状态，直到下次开机（`AUTO_START`）或
  GUI 首启（会判为 `installed-not-ready` → `repair` 重启）。验证清单 B8 覆盖这个
  用例；要彻底避免只能在 `InstallUISequence` 里加对称动作，代价大于收益。
- **R11（强制重装语义）**：「版本一致就跳过覆盖」的短路已删除（见 §3.2），
  `service_reinstall`（设置里的「强制重装」）现在会真的重跑一次安装。
- **R12（NSIS 卸载不注销服务 = 刻意差异）**：NSIS 钩子只做「停 / 启」，不做
  `sc delete`。原因：NSIS 的「先卸载再装」路径（用户手动双击、选了 uninstall
  before installing）与真卸载在执行期无法区分，删了会连带打掉升级后的服务注册；
  而服务注册本来就归 GUI 首启自愈（沿用既有模型）。后果：**真卸载后 SCM 里仍留着
  `era.exvlan.studio.service`**（二进制在 ProgramData，服务继续跑），与 MSI 卸载
  （会 `sc delete`）不一致。若要做到一致，需要 `NSIS_HOOK_PREUNINSTALL` 里判
  `$UpdateMode`（只在卸载器被带 `/UPDATE` 调用时才跳过删除）——本轮不做，见 §7 的 I 步。
- **R13（`installMode=perMachine` 的迁移代价）**：dev.2 起 NSIS 安装目录从
  `%LOCALAPPDATA%\EXVlan Studio` 变成 `$PROGRAMFILES64\EXVlan Studio`（与 MSI 一致）。
  但 `SHCTX` 随之从 HKCU 变 HKLM，`RestorePreviousInstallLocation` /
  `PageReinstall` 都读不到老 per-user 记录的安装路径 ⇒ **老 per-user 副本不会被自动
  迁移或卸载**，会留下 `%LOCALAPPDATA%\EXVlan Studio` 目录 + HKCU 卸载项（「应用和
  功能」里两条）。清理办法见 §7 的 I 步。反过来，`perMachine` 也让 updater 每次
  更新都会弹一次 UAC（以前静默）——拒绝提权时 `ShellExecuteW` 返回 1223，GUI 报
  更新失败并回退自建流程。
- **R14（NSIS 没有安装日志）**：`/P`、`/S` 下 `DetailPrint` 看不到，钩子的唯一痕迹
  是 `%TEMP%\exvlan-nsis-hooks.log`（over-the-shoulder 提权时在
  `C:\Windows\Temp\exvlan-nsis-hooks.log`）。取证时先看这里。
- **R15（NSIS 侧执行身份不是 SYSTEM）**：perMachine 只是让安装包以提权管理员令牌
  运行（UAC 同意），与 MSI 的 in-script 动作（SYSTEM）不同。对 SCM 而言两者能力等价
  （都是管理员），但若机器策略禁止该账号管理服务（极少见），钩子会失败且只写日志。
- **R16（NSIS 安装中途失败无回滚拉起）**：NSIS 没有 MSI 那种 rollback 动作。
  用户在文件复制阶段取消/失败时，服务可能保持 STOPPED，直到下次开机（`AUTO_START`）
  或 GUI 首启（`installed-not-ready` → `repair`）。验证清单 I5 覆盖。
- **不在本任务范围**（另派）：B5 doctor `net.ports` 检查地址错、B6 gateway 首启
  `bind 失败 os error 10049`（`exvlan/control/src/doctor.rs`、`exvlan/gateway/**`）。

## 6. Linux 侧已做的验证

```bash
cargo check -p exvlan-service -p exvlan-studio                  # 通过
cargo test  -p exvlan-studio --lib service_client                # 2 passed（health_of 三态 + 序列化形状）
cargo fmt   -p exvlan-service -p exvlan-studio -- --check        # 无 diff
xmllint --noout packages/studio/src-tauri/wix/service-lifecycle.wxs          # 通过
python3 -c "import json;json.load(open('packages/studio/src-tauri/tauri.windows.conf.json'))"  # 通过
# Windows 专用 SCM 代码单独抽取到临时 crate，用 windows-service 0.8.1 交叉类型检查
cargo check --target x86_64-pc-windows-msvc   # 通过（整包交叉编译会先卡在 aws-lc-sys 的 C 依赖）
```

自定义动作的 PowerShell 脚本另外用 **PowerShell 7.4.6（Linux 版）**做了两道验证：

1. 语法：4 条 `-Command` 载荷逐条过
   `[System.Management.Automation.Language.Parser]::ParseInput` → 全部 `PARSE-OK`
   （也顺带确认脚本内不含会被 MSI 当属性展开的 `[` `]`）；
2. 控制流：把 `Get-Service`/`Stop-Service`/`Get-Process`/`Stop-Process`/`Start-Service`
   换成桩（`Stop-Process` 用 `[Parameter(ValueFromPipeline)]` + `process{}` 对齐真实
   空管道语义），跑 9 个场景，逐条比对实际调用序列：

   | 场景 | 期望 | 实测 |
   | --- | --- | --- |
   | 服务不存在 | 只清孤儿（`Get-Process`） | ✅ |
   | 服务已停止 | 只清孤儿，不重复 `Stop-Service` | ✅ |
   | 服务运行 → 按时停下 | `Stop-Service` + 清孤儿 | ✅ |
   | 服务运行 → 30s 超时 | **只 `Stop-Service`，不清孤儿** | ✅ |
   | start：未安装 / 已在跑 / 已停 | 无动作 / 无动作 / `Start-Service` | ✅✅✅ |
   | rollback start：已停 | `Start-Service` | ✅ |
   | remove：服务不存在 | 不调用 `sc.exe` | ✅ |

**未做**：WiX `candle`/`light` 编译、MSI 安装/升级实跑（均为 Windows 专属）。

### 6.1 NSIS 钩子（dev.2 新增）本机验证

本机有 `makensis` 3.11（Debian `nsis` 包），所以**钩子是真编译过的**，不是只做人工核对。
仓库里带了可复跑脚本 `packages/studio/src-tauri/nsis/verify.sh`：

```bash
bash packages/studio/src-tauri/nsis/verify.sh
# PASS  编译：makensis -WX 无 warning（nsis 3.11-1）
# PASS  抽取：preinstall.ps1（860 字符，含 exit 0）
# PASS  抽取：postinstall.ps1（525 字符，含 exit 0）
# PASS  语法：preinstall.ps1 PARSE-OK
# PASS  语法：postinstall.ps1 PARSE-OK
# PASS  控制流：preinstall.ps1/svc-absent → stopped=True
# …（共 7 个场景）
```

它做三层（没有 `makensis` 时第 1 层直接 SKIP，没有 `pwsh` 时只跳过第 3 层）：

1. **真编译**：生成一个只做 `!include` + `!insertmacro` 的 harness，
   `makensis -V4 -WX`（warning 当 error）——任何漏写的 `$$`、写错的 `$\"`、
   多余/缺失的 `!macroend` 都会在这里失败；
2. **抽实际命令行**：harness 用 `SetCompress off`，Unicode 字符串原样躺在 exe 里，
   用正则把 `powershell.exe" -NoProfile … -Command "<载荷>"` 抠出来 ⇒ 校验的对象
   **就是安装包会执行的那串字符**（顺带断言没有残留 `$$`、一定带 `exit 0`）；
3. **PowerShell 层**：`[Parser]::ParseInput` 语法 + 桩（`Get-Service`/`Stop-Service`/
   `Start-Service`/`Get-Process`/`Stop-Process` 全换成记录调用序列的假实现）跑 7 个场景：

| 场景 | 期望调用序列 | 实测 |
| --- | --- | --- |
| preinstall / 服务不存在 | `Get-Service` → 清孤儿（`Get-Process` + 2×`Stop-Process`），不 `Stop-Service` | ✅ `stopped=True` |
| preinstall / 服务已停 | 同上（不重复 `Stop-Service`） | ✅ `stopped=True` |
| preinstall / 服务运行 → 按时停 | `Stop-Service` + `WaitForStatus:Stopped` → 清孤儿 | ✅ `stopped=True` |
| preinstall / 服务运行 → 30s 超时 | **只 `Stop-Service` + wait，不清孤儿** | ✅ `stopped=False` |
| postinstall / 服务不存在 | 只 `Get-Service`，不 `Start-Service` | ✅ `before=absent started=False` |
| postinstall / 已在跑 | 只 `Get-Service`（幂等） | ✅ `before=Running started=False` |
| postinstall / 已停 | `Start-Service` + `WaitForStatus:Running` | ✅ `before=Stopped started=True` |

7 个场景进程退出码都是 0（`exit 0` 生效，且 `finally` 仍能写出结果），说明
「停不掉/起不来也不阻塞安装」这条成立。

**未做**：`tauri build` 打真包（需要 Windows runner + NSIS 3.11 + WebView2 下载）、
`setup.exe` 实跑。「`!include` 位置 / 宏展开点 / 路径解析」是按 tauri-v2.12.2 的
`installer.nsi` 与 `tauri-cli/src/bundle.rs:149` 反查确认的，验证清单 I0 在构建机复核。


## 7. Windows 手工验证清单

> 全程用**管理员 PowerShell**。提前记下回滚点：旧安装包 + 数据目录清单。

### 0. 准备

```powershell
# 数据快照（升级前后必须完全一致）
Get-ChildItem -Recurse C:\ProgramData\era.exvlan.studio\config, C:\ProgramData\era.exvlan.studio\logs |
  Get-FileHash | Export-Csv $env:TEMP\before.csv
sc.exe query era.exvlan.studio.service      # 期望 RUNNING
```

### A. 产物自检（构建机上，不用装）

| # | 步骤 | 期望 |
| --- | --- | --- |
| A1 | 看 `tauri build` 日志里 `candle` 的输入列表 | 出现 `…\src-tauri\wix\service-lifecycle.wxs`；没有 `file not found` |
| A2 | `dark.exe -x out EXVlan_Studio_x64_en-US.msi` 后看 `out\CustomAction.wxs` | 有 `ExvlanStopServiceForUpgrade` / `ExvlanStartServiceAfterInstall` / `ExvlanStartServiceOnRollback` / `ExvlanRemoveServiceOnUninstall` 四条；命令首 token 是 `[SystemFolder]WindowsPowerShell\v1.0\powershell.exe`，且**不含** `-ExecutionPolicy Bypass`；immediate 三条**不带** `Impersonate`，rollback 那条带 `Impersonate="no"` |
| A3 | 同上 `out\Property.wxs` | `UpgradeCode` = `42C08A88-9ABD-5406-B191-2A8543B12B98`，与**上一版已发布 MSI** 一致（不一致会导致并存安装） |
| A4 | `out\InstallExecuteSequence.wxs` | 四条动作的 Sequence：停 < 100（InstallValidate）；rollback 启在 InstallInitialize 之后；启 > InstallFinalize；注销在 RemoveFiles 之前 |
| A5 | `out\CustomAction.wxs` 里 Type 列 | 三条是 `34`（Directory + command line），rollback 那条是 `34` + in-script/no-impersonate 位（`dark.exe` 显示为 `34` + `Execute=rollback`）；若看到 `Impersonate` 位出现在 immediate 三条上，说明打包配置回退到了旧版 |

### B. 升级实测（主场景：dev.1 → dev.2）

| # | 步骤 | 期望 |
| --- | --- | --- |
| B0 | **先退出 GUI**（托盘 → 退出），确认 `Get-Process 'EXVlan Studio'` 为空 | 这是 R9 的前提：GUI 自身占着 Program Files，运行中装 MSI 会弹「文件正在使用」 |
| B1 | `msiexec /i "EXVlan_Studio_26.410.20713-dev.2_x64_en-US.msi" /l*v $env:TEMP\upgrade.log /passive` | 退出码 `0`；**没有** `1601`；日志里**没有** Restart Manager `351` 提示 |
| B2 | 日志里搜 `exvlan: pre-upgrade stop done` | 说明停服务动作执行了 |
| B3 | 日志里搜 `exvlan: service start requested` | 说明装完拉起动作执行了 |
| B4 | `sc.exe query era.exvlan.studio.service` = `RUNNING` | 不需要重启机器 |
| B5 | 启动 GUI：**不弹 UAC**、顶部无「服务未就绪」横幅；`error.log` 里没有 `service.repair` / `service.install`（说明 health = installed-running） |
| B6 | 步骤 0 的数据快照 `Get-FileHash` 与 `before.csv` **逐条一致** |
| B7 | `Get-ChildItem C:\ProgramData\era.exvlan.studio\bin\*.new, C:\ProgramData\era.exvlan.studio\core\*.new, C:\ProgramData\era.exvlan.studio\core\v.txt` → 空（或只剩 mtime < 10 分钟的） |
| B8 | **Cancel 用例（R10）**：再跑一次 `/i`，在「准备安装」页点 Cancel，然后 `sc query` | 服务可能停在 STOPPED（回滚动作不覆盖 UI 取消）；**下次开机或启动 GUI 必须自行恢复**：GUI 应在 `error.log` 打出 `service.repair` 并把服务拉起 |
| B9 | B8 之后按 F1 的判据复核 | 不允许出现 `cli --install failed … os error 32` |

**回滚**：`msiexec /x <新 ProductCode>` → `msiexec /i <旧 MSI> /passive`；
数据不动。服务若不在：`Set-Location C:\ProgramData\era.exvlan.studio\bin; .\exvlan-service.exe --uninstall`
后重开 GUI（会重新注册）。

### C. 服务停止态升级（覆盖 R3：权限不足的情况）

```powershell
sc.exe stop era.exvlan.studio.service
msiexec /i "…dev.2….msi" /l*v $env:TEMP\upgrade-stopped.log /passive
sc.exe query era.exvlan.studio.service
```

期望：安装成功 + 服务回到 `RUNNING`（`ExvlanStartServiceAfterInstall`）。
若服务仍 `STOPPED`，看日志里该动作的退出码；这即 R3 命中的信号。

### D. 从未启动过 GUI 的机器（无服务）

全新 Windows 上直接装 MSI，**不启动** GUI：

期望：安装成功；日志里停/启两个动作都执行但**静默跳过**（`Get-Service` 返回 null，
不报错）；`sc query` = `1060`（服务不存在）。随后启动 GUI → 一次 UAC → 服务被注册。

### E. 真卸载（数据红线）

```powershell
msiexec /x "EXVlan_Studio_*.msi" /l*v $env:TEMP\uninstall.log /passive
sc.exe query era.exvlan.studio.service      # 期望 1060（已注销）
Test-Path C:\ProgramData\era.exvlan.studio\config   # 期望 True
Test-Path C:\ProgramData\era.exvlan.studio\logs     # 期望 True
Test-Path C:\ProgramData\era.exvlan.studio\bin\exvlan-service.exe  # 期望 True
```

### F. GUI 自愈（B3/B4 回归）

| 场景 | 步骤 | 期望 |
| --- | --- | --- |
| F1 服务在跑、管道先断 | `sc.exe stop era.exvlan.studio.service` → 启动 GUI | `error.log` 出现 `service.repair`，**没有** `cli --install failed … os error 32`；服务回到 RUNNING；无「安装失败」提示 |
| F2 真未安装 | `sc.exe delete era.exvlan.studio.service` + 删 `bin\exvlan-service.exe` → 启动 GUI | 走一次安装（一次 UAC），`health` 变 `installed-running` |
| F3 界面重试按钮幂等 | 服务正常时点「重试」 | 立即成功、服务**不重启**（`sc query` 的 PID 不变） |
| F4 注销但二进制在 | `sc.exe delete era.exvlan.studio.service`（**不删** `bin\exvlan-service.exe`）→ 启动 GUI | `health` 判为 `not-installed` → 重新注册（一次 UAC），`sc query` = RUNNING；**不是** `repair` 失败 |
| F5 强制重装真的重装（R11） | 设置里点「强制重装」 | 服务被停→覆盖→重启，`sc query` 的 PID **变化**；`service.log` 有本轮 install 记录（不再出现「什么都没做就返回成功」） |

### G. B7 残留清理

```powershell
$f = "C:\ProgramData\era.exvlan.studio\bin\exvlan-service.exe.new"
New-Item -ItemType File -Force $f | Out-Null
(Get-Item $f).LastWriteTime = (Get-Date).AddHours(-1)
sc.exe stop era.exvlan.studio.service; sc.exe start era.exvlan.studio.service
Select-String -Path C:\ProgramData\era.exvlan.studio\logs\service.log -Pattern "staging cleanup"
Test-Path $f    # 期望 False
```

### H. 回归不变量

- `C:\Program Files\EXVlan Studio\EXVlan Studio.exe --version` 与安装包版本一致。
- `sc.exe qc era.exvlan.studio.service` 的 `BINARY_PATH_NAME` 仍是
  `C:\ProgramData\era.exvlan.studio\bin\exvlan-service.exe --daemon`
  （升级不应改写服务路径）。
- 服务日志里升级后不再出现 `kernel.restart failed … os error 32`。

### I. NSIS（自动更新主路径，§3.5）

> 这条路径**不能**用 MSI 的步骤代替：自动更新下载的是 `*_x64_*setup.exe`。
> 全程管理员 PowerShell；`$setup` 指新版 `EXVlan Studio_<ver>_x64-setup.exe`。

| # | 步骤 | 期望 |
| --- | --- | --- |
| I0 | 构建机上：`tauri build` 日志里搜 `installerHooks`/`service-lifecycle.nsh`；或 `7z l` 看 `bundle\nsis\*.exe` 旁边 `bundle\nsis-updater\…-setup.exe.sig` 仍在 | makensis 没报 `!include … file not found`；产物名仍是 `…_x64-setup.exe`（`gen_upgrade_manifests.py` 不用改） |
| I1 | `(Get-Item "$setup").VersionInfo` / 直接看 `$setup` 的 manifest：`RequestedExecutionLevel` = `requireAdministrator` | 说明 `installMode=perMachine` 生效（`asInvoker`/`highestAvailable` = 配置没吃到，钩子会空转，见 R13） |
| I2 | 记录基线：`sc.exe query era.exvlan.studio.service`（RUNNING）+ `Get-Item <bin>\exvlan-service.exe` 的 PID；`Get-ChildItem -Recurse C:\ProgramData\era.exvlan.studio\config,C:\ProgramData\era.exvlan.studio\logs \| Get-FileHash \| Export-Csv $env:TEMP\before.csv` | 服务 RUNNING |
| I3 | **NSIS→NSIS 覆盖安装**（已是 per-user/perMachine NSIS 的机器）：`& $setup /P`（passive，有进度条不说话）→ 接受 UAC | 安装成功；`sc.exe query` = RUNNING 且 **PID 变化**；`%TEMP%\exvlan-nsis-hooks.log` 出现 `[preinstall] … elevated=True stopped=True` 与 `[postinstall] before=Stopped started=True` |
| I4 | `Get-FileHash` 与 `before.csv` 逐条比对 | 完全一致（数据红线） |
| I5 | **Cancel 用例**（R16）：再跑一次 `$setup`，在文件复制阶段点 Cancel → 稍等 → `sc query` | 服务可能停在 STOPPED（NSIS 无回滚动作）；**启动 GUI 必须自愈**：`error.log` 出现 `service.repair`，`sc query` 回 RUNNING，且**没有** `os error 32` |
| I6 | **自动更新实跑**：GUI 里「检查更新 → 下载并安装」（走官方 updater，`/UPDATE`） | 一次 UAC；`sc query` = RUNNING、PID 变化；`%TEMP%\exvlan-nsis-hooks.log` 有本轮两行；重开 GUI 后无「服务未就绪」横幅、`error.log` 无 `service.install` |
| I7 | **从 MSI 迁移**（dev.1 MSI 机器上跑 NSIS setup）：`& $setup /P` | 会先弹 msiexec 的 UAC 卸掉 MSI（`$WixMode`）；装完后 `sc query` 可能是 **1060**（MSI 卸载动作 `sc delete` 过，见 §3.5 时序）；**启动 GUI 后**自动重新注册（一次 UAC）→ RUNNING。这是预期路径，不是缺陷 |
| I8 | **真卸载**（不删数据）：`& "$env:PROGRAMFILES\EXVlan Studio\uninstall.exe"`（或「应用和功能」） | 文件删掉；`sc.exe query era.exvlan.studio.service` **仍是 RUNNING**（R12：NSIS 故意不 `sc delete`）；`Test-Path C:\ProgramData\era.exvlan.studio\{config,logs,bin\exvlan-service.exe}` 全 `True` |
| I9 | **老 per-user 残留清理**（R13）：升级后 `Get-ChildItem "$env:LOCALAPPDATA\EXVlan Studio"` 非空 ⇒ 手工 `& "$env:LOCALAPPDATA\EXVlan Studio\uninstall.exe" /S` 清掉旧副本 | 「应用和功能」里只剩一条 `EXVlan Studio`；`%LOCALAPPDATA%\EXVlan Studio` 消失 |
