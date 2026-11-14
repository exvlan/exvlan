# Windows 大版本升级：服务（exvlan-service）生命周期

> 适用：`EXVlan Studio` MSI（WiX v3）在 **ProductCode 变化**的大版本升级场景
> （`UpgradeCode` 不变 → MSI 走 `RemoveExistingProducts`）。
> 结论源自 dev.1（`26.410.20713-dev.1`）真机取证，修正过一版早期错误假设
> （「旧产品卸载时把服务注销了」已被证伪：`sc query` 与 System 日志 7045 均为 0 条，
> 服务注册自 2026-11-11 04:57Z 起从未丢失）。

## 1. 现场证据（dev.1 真机）

| 编号 | 现象 | 证据 |
| --- | --- | --- |
| M1 | `msiexec REINSTALL=ALL` 跨 ProductCode **静默空转**（返回 0，实际没装） | 手工复现 |
| M2 | 直接 `/i` 新 MSI：`RemoveExistingProducts` 崩 **1601**，Restart Manager 报 **351** | 手工复现；占用 `C:\Program Files\EXVlan Studio` 下的文件 |
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
   ⇒ 升级时没人停它，`InstallValidate` 的 Restart Manager 直接看到
   `EXVlan Studio.exe` / `resources\**` 被占用 → **351**；
   `RemoveExistingProducts` 阶段 → **1601**（M2）。

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

三个自定义动作（全部 `Execute="immediate"`、`Impersonate="no"`、`Return="ignore"`）：

| 动作 | 时序 | 条件 | 作用 |
| --- | --- | --- | --- |
| `ExvlanStopServiceForUpgrade` | `Before="InstallValidate"` | 恒真 | `Stop-Service` 等 STOPPED（≤30s），再 `Stop-Process` 清掉孤儿 `exvlan.exe` / `exvlan-gateway.exe`（它们占着 Program Files 里的文件） |
| `ExvlanStartServiceAfterInstall` | `After="InstallFinalize"` | `NOT REMOVE` | 把服务拉回 RUNNING（覆盖 B1 的「升级窗口内被 SCM 放弃」）；服务不存在则静默跳过 |
| `ExvlanRemoveServiceOnUninstall` | `Before="RemoveFiles"` | `REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE` | 真卸载才停 + `sc delete`（只删 SCM 项） |

关键取舍：

- **必须早于 `InstallValidate`**：MSI 的 Restart Manager 文件占用检查就在这一步，
  晚于它就没意义（这也是选 `Execute="immediate"` 的原因——deferred 动作只能排在
  `InstallInitialize` 之后）。带 UI 的安装里，`InstallValidate` 之前的动作会先在
  客户端跑一遍、再在服务端（perMachine 为 SYSTEM）重跑，`Impersonate="no"` 让
  服务端那一次以足够权限停服务。
- **不创建服务**：不写 `<ServiceInstall>`。服务二进制在 ProgramData，MSI 不拥有它；
  注册由 GUI 首启自愈负责（沿用现有模型）。
- **不依赖 `exvlan-service` 自己的命令行面**：升级源可能是任意旧版本，
  所以停/启/注销全走 `sc` / `Get-Service`。
- **失败不阻塞安装**：`Return="ignore"`。停不掉只是回到旧行为（RM 弹
  「文件正在使用」），不该让整机装不上。

### 3.2 服务端：幂等 + 语言无关 + 收尾清理

`exvlan/service/src/main.rs`：

| 位置 | 改动 |
| --- | --- |
| `install()`（Windows） | 新增幂等短路：**目标版本已一致 + SCM 报告 RUNNING** → 直接返回，不覆盖 exe（消灭 B3 的 os error 32） |
| `install()`（Windows） | 停不下来时**明确报错**（`服务仍在运行，未覆盖 …；请先 sc stop`），不再让 `copy` 抛裸 os error 32 |
| `stop_service_blocking(timeout)` | 改为 SCM API（`windows-service`）轮询 `ServiceState::Stopped`，**与系统语言无关**（原实现匹配 `"STOPPED"` 文本） |
| `service_is_running()` / `start_service_scm()` | 新增；同样走 SCM API |
| `restart_service()` / `replace_updater()`（Windows） | 改用上面三个函数（去掉 `sc query` 文本匹配） |
| `cleanup_stale_staging()` | 新增，`boot()` 时调用：删除 `bin\exvlan-service.exe.new`、`core\exvlan.exe.new`、`core\tools.new\`、`core\v.txt`（B7），**要求 mtime > 10 分钟**（避免删掉正在进行的替换）；不碰 `config/`、`logs/`、`*.prev`、token |
| `replace_updater()`（Windows） | 替换成功后删除 `.new` 暂存 |
| `uninstall()`（Windows） | 改为 `stop_service_blocking` + `sc delete`；**只删 SCM 项**，不删二进制、不删数据 |

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
| **自动更新走的是 NSIS 而不是 MSI** | `bundle.windows.nsis.installerHooks` + `!macro NSIS_HOOK_PREINSTALL`（同样先停服务） | 见 §5 风险 R1：`script/gen_upgrade_manifests.py` 里官方 updater 的 Windows 规则匹配 `*_x64_*setup.exe`（NSIS），MSI 只服务手工/企业部署 |
| MSI 若仍报 351 | 追加 `<ServiceControl Id="…" Name="era.exvlan.studio.service" Stop="both" Wait="yes" Remove="uninstall"/>`（需要挂在一个 Component 上，即 `componentGroupRefs`/`componentRefs`） | MSI 原生停服务（SYSTEM 上下文），但 `StopServices` 排在 `RemoveExistingProducts`（`afterInstallInitialize` = 1501）**之后**，对升级窗口来得太晚，所以只作兜底 |
| 让 GUI 在 RM 关它之前自己收尾 | Tauri `RunEvent::ExitRequested` 里 `service_client::stop()` | 只对**新版本**有用；老版本 GUI 没有这段代码，救不了当前这批用户 |

## 4. 数据安全红线（已核对的代码路径）

- 任何 MSI 动作都**不触碰** `C:\ProgramData\era.exvlan.studio`：fragment 里只有
  `sc`/`Get-Service`/`Stop-Process`。
- `--uninstall`（`main.rs` `uninstall()`）只 `sc stop` + `sc delete`，**不删文件**。
- `--purge` 只在 Linux 实现（`main.rs`），Windows 直接 `bail!`，且**没有**挂到
  任何 MSI 卸载路径上。
- 新增的 `cleanup_stale_staging()` 白名单只有 4 个名字（见 3.2），并有 10 分钟
  mtime 门槛；不删 `*.prev`（回滚备份）、不删 `config/`、`logs/`、token。

## 5. 未覆盖风险

- **R1（最重要）**：`tauri-plugin-updater` 在 Windows 上走的是 **NSIS**
  （`script/gen_upgrade_manifests.py:64-65` 匹配 `*setup.exe`），本次只改 MSI。
  也就是说**自动更新**路径尚未停服务；需要按 §3.4 方案 B 补
  `NSIS_HOOK_PREINSTALL`。手工/企业 MSI 升级已覆盖。
- **R2（老 MSI 用户）**：`pre.19` / `dev.1` 的 MSI 里没有这些动作，
  从它们升级到 dev.2 **第一次**运行时，靠的是 dev.2 自己的
  `ExvlanStopServiceForUpgrade`（它在新 MSI 里，所以仍然生效——停服务用的是
  `sc`，与旧版本无关）。但**反向**（dev.2 → 更早版本）不保证。
- **R3**：`Impersonate="no"` 的立即动作在「标准用户 + 管理员凭据代填」的
  UAC 场景下可能拿不到足够权限（此时 `Stop-Service` 失败，动作 `ignore` 后
  仍会回到 351）。验证清单 C 步专门检查这一点。
- **R4**：`Stop-Process exvlan/exvlan-gateway` 会强杀**任何**同名进程
  （包括用户手工跑的 CLI 内核会话）。升级期间可接受，但属于行为变化。
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
- **R8**：fragment 里三个动作都是 `Execute="immediate"` + `Impersonate="no"`。
  若 `candle`/`light` 对此组合报错（本机无法验证），**最小改法**是删掉
  `Impersonate="no"`（此时以提权用户身份执行，管理员场景仍可停服务；
  「标准用户 + 代填管理员凭据」场景会失败，即 R3）。验证清单 A1/A2 会在
  构建期暴露这个问题。
- **不在本任务范围**（另派）：B5 doctor `net.ports` 检查地址错、B6 gateway 首启
  `bind 失败 os error 10049`（`exvlan/control/src/doctor.rs`、`exvlan/gateway/**`）。

## 6. Linux 侧已做的验证

```bash
cargo check -p exvlan-service                                   # 通过
cargo check -p exvlan-studio                                    # 通过
xmllint --noout packages/studio/src-tauri/wix/service-lifecycle.wxs   # 通过
python3 -c "import json;json.load(open('packages/studio/src-tauri/tauri.windows.conf.json'))"  # 通过
# Windows 专用 SCM 代码单独抽取到临时 crate，用 windows-service 0.8.1 交叉类型检查
cargo check --target x86_64-pc-windows-msvc   # 通过（整包交叉编译会先卡在 aws-lc-sys 的 C 依赖）
```

**未做**：WiX `candle`/`light` 编译、MSI 安装/升级实跑（均为 Windows 专属）。

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
| A2 | `dark.exe -x out EXVlan_Studio_x64_en-US.msi` 后看 `out\CustomAction.wxs` | 有 `ExvlanStopServiceForUpgrade` / `ExvlanStartServiceAfterInstall` / `ExvlanRemoveServiceOnUninstall` 三条 |
| A3 | 同上 `out\Property.wxs` | `UpgradeCode` = `42C08A88-9ABD-5406-B191-2A8543B12B98`，与**上一版已发布 MSI** 一致（不一致会导致并存安装） |
| A4 | `out\InstallExecuteSequence.wxs` | 三条动作的 Sequence：停 < 100（InstallValidate），启 > InstallFinalize，注销在 RemoveFiles 之前 |

### B. 升级实测（主场景：dev.1 → dev.2）

```powershell
msiexec /i "EXVlan_Studio_26.410.20713-dev.2_x64_en-US.msi" /l*v $env:TEMP\upgrade.log /passive
```

| # | 期望 |
| --- | --- |
| B1 | 退出码 `0`；**没有** `1601`；日志里**没有** Restart Manager `351` 提示 |
| B2 | 日志中含 `exvlan: service stopped before upgrade`（说明停服务动作执行了） |
| B3 | 日志中含 `exvlan: service start requested` |
| B4 | `sc.exe query era.exvlan.studio.service` = `RUNNING`（不需要重启机器） |
| B5 | 启动 GUI：**不弹 UAC**、顶部无「服务未就绪」横幅；`error.log` 里没有 `service.repair` / `service.install`（说明 health = installed-running） |
| B6 | 步骤 0 的数据快照 `Get-FileHash` 与 `before.csv` **逐条一致** |
| B7 | `Get-ChildItem C:\ProgramData\era.exvlan.studio\bin\*.new, C:\ProgramData\era.exvlan.studio\core\*.new, C:\ProgramData\era.exvlan.studio\core\v.txt` → 空（或只剩 mtime < 10 分钟的） |

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
