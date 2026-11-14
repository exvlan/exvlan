; EXVlan NSIS：特权服务（exvlan-service）的生命周期钩子。
;
; 与 MSI 侧同一套语义 —— 判定逻辑、孤儿清理的前置条件、失败容忍策略都照抄
; wix/service-lifecycle.wxs（根因链条与验证步骤见 docs/windows-service-upgrade.md）。
; MSI 只服务手工/企业部署；**自动更新走的是 NSIS**（script/gen_upgrade_manifests.py
; 的 windows 规则只匹配 *_x64_*setup.exe），所以这条钩子才是主路径。
;
; 接入方式（tauri v2）
; ===================
; · 配置字段：bundle.windows.nsis.installerHooks（tauri-utils `NsisConfig::installer_hooks`，
;   serde 别名 installer-hooks），指向一个 .nsh 文件；
; · 可用宏：NSIS_HOOK_PREINSTALL / NSIS_HOOK_POSTINSTALL / NSIS_HOOK_PREUNINSTALL /
;   NSIS_HOOK_POSTUNINSTALL（文档：https://v2.tauri.app/distribute/windows-installer/ 的
;   "Customizing the NSIS Installer → Extending the Installer"）；
; · 路径相对 **src-tauri/** 解析：tauri-cli 打包前 `set_current_dir(tauri_dir())`，
;   bundler 再 `dunce::canonicalize()` 这个相对路径（与本仓 wix.fragmentPaths 同一套）；
; · 本文件被 `!include` 进 installer.nsi 顶部，宏在 `Section Install` 里展开：
;     PREINSTALL  → 复制文件 / 写注册表 / 建快捷方式之前
;     POSTINSTALL → 上述全部完成之后
;   两者都只在安装/升级时展开，卸载走的是另一支（`Section Uninstall`），所以
;   「NOT REMOVE」语义由 hook 位置天然表达，不需要额外条件。
;
; 为什么必须有 installMode=perMachine（同目录的 tauri.windows.conf.json）
; =========================================================================
; 默认 installMode=currentUser → installer.nsi 里 `RequestExecutionLevel user`，
; 安装包不提升权限；而服务是 GUI 提权注册的 SYSTEM 服务，默认 SCM 安全描述符
; 只给 Interactive Users `SERVICE_QUERY_STATUS`（无 SERVICE_STOP/START），
; 非提权进程 Stop-Service / Start-Service 一律拒绝访问 → 钩子等于空转。
; perMachine 时 `RequestExecutionLevel admin`，钩子以**提权后的管理员令牌**
; 运行（UAC 同意，不是 SYSTEM —— MSI 侧的 in-script 动作才是 SYSTEM，
; 两者对 SCM 的能力等价）。
;
; 刻意不做的事（与 MSI 一致 + 一处刻意的差异）
; ===========================================
; · 不创建服务：没装过 GUI 的机器上 Get-Service 返回空 → 静默跳过；
;   注册仍归 GUI 首启自愈。
; · 不碰数据目录：C:\ProgramData\era.exvlan.studio 下的 config/logs/core/bin/token
;   在安装、升级、卸载任何路径都不删不移；本文件只有 Get-Service / Stop-Service /
;   Start-Service / Get-Process / Stop-Process。
; · 不中断安装：PowerShell 恒 `exit 0`，NSIS 侧只把退出码 DetailPrint 出来
;   （停不掉只是回到旧行为，不该让整机装不上）。
; · 卸载**不** `sc delete`（MSI 侧那条 uninstall 动作的对应物）：NSIS 的
;   「先卸载再装」路径与真卸载无法区分，删了会连带打掉升级后的服务注册；
;   而服务注册本来就是 GUI 拥有的（见 docs 的风险 R12）。
;
; 日志
; ====
; NSIS 没有 msiexec /l*v 那样的安装日志：/P、/S 下 DetailPrint 也看不到。
; 所以每条钩子自己往 %TEMP%\exvlan-nsis-hooks.log **追加**一行（带时间戳、账号、
; 是否提权、前后状态）。提权是 over-the-shoulder（填了另一个管理员账号）时
; TEMP 会落在那个账号下，同时在 C:\Windows\Temp 下也看一眼。
;
; 语法约定（本文件由 NSIS 直接解析，不经过 Handlebars 预渲染）
; ==========================================================
; · NSIS 字符串里的 `$` 是变量引用，PowerShell 的 `$xxx` 必须写成 `$$xxx`；
;   单引号在 NSIS 双引号字符串里是普通字符，所以 PowerShell 字面量统一用单引号，
;   只有 `-Command "..."` 那层用 `$\"`。
; · 不要在本文件里出现 `{{ }}`（bundler 会把片段当模板预渲染一遍来探测 extension）。
; · 不要用 `${If}` 之类 LogicLib 宏，也不要用标签：宏会展开在 Section Install 内部，
;   标签与 Section 内部已有的名字可能冲突。

; 停服务并等 STOPPED（≤30s），**确认停了之后**才清孤儿内核/gateway。
; 服务活着时它的看门狗（service spawn_gateway_watchdog）每 5s 把 gateway 拉回来，
; 抢在安装窗口里反复抖动反而放大崩溃 → 与 MSI 侧同一条前置条件。
!macro NSIS_HOOK_PREINSTALL
  DetailPrint "EXVlan: stopping era.exvlan.studio.service before install"
  ExecWait "$\"$WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe$\" -NoProfile -NonInteractive -Command $\"$$ErrorActionPreference='SilentlyContinue';$$log=Join-Path $$env:TEMP 'exvlan-nsis-hooks.log';$$t=(Get-Date).ToString('o');$$n='era.exvlan.studio.service';$$s=Get-Service -Name $$n -ErrorAction SilentlyContinue;$$stopped=$$false;if(-not $$s){$$stopped=$$true}elseif($$s.Status -eq 'Stopped'){$$stopped=$$true}else{Stop-Service -Name $$n -Force -ErrorAction SilentlyContinue;try{$$s.WaitForStatus('Stopped','00:00:30');$$stopped=$$true}catch{}};if($$stopped){Get-Process -Name 'exvlan','exvlan-gateway' -ErrorAction SilentlyContinue|Stop-Process -Force -ErrorAction SilentlyContinue};$$adm='?';try{$$adm=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)}catch{};Add-Content -Path $$log -Value ($$t+' [preinstall] user='+$$env:USERNAME+' elevated='+$$adm+' stopped='+$$stopped);exit 0$\"" $0
  DetailPrint "EXVlan: stop hook done (exit $0)"
!macroend

; 装完把服务拉回 RUNNING：升级窗口内它可能已被 SCM 放弃。
; 服务不存在（从没启动过 GUI，或刚从 MSI 迁移过来被 MSI 卸载动作 sc delete 过）
; 就静默跳过 —— 下一次启动 GUI 会走 not-installed 分支重新注册（一次 UAC）。
!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "EXVlan: restoring era.exvlan.studio.service after install"
  ExecWait "$\"$WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe$\" -NoProfile -NonInteractive -Command $\"$$ErrorActionPreference='SilentlyContinue';$$log=Join-Path $$env:TEMP 'exvlan-nsis-hooks.log';$$t=(Get-Date).ToString('o');$$n='era.exvlan.studio.service';$$s=Get-Service -Name $$n -ErrorAction SilentlyContinue;$$before='absent';if($$s){$$before=$$s.Status.ToString()};$$started=$$false;if($$s -and $$s.Status -ne 'Running'){Start-Service -Name $$n -ErrorAction SilentlyContinue;try{$$s.WaitForStatus('Running','00:00:30');$$started=$$true}catch{}};Add-Content -Path $$log -Value ($$t+' [postinstall] before='+$$before+' started='+$$started);exit 0$\"" $0
  DetailPrint "EXVlan: start hook done (exit $0)"
!macroend
