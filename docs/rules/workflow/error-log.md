# GUI 错误日志（`error.log`）

> 用途：让用户/支持同学一条消息就能发出「出错现场」，不用翻几十 MB 的 `exvlan.log`。

## 位置

| 平台 | 路径 |
| --- | --- |
| macOS | `~/Library/Logs/<bundle-id>/error.log`（`app_log_dir`） |
| Windows | `%LOCALAPPDATA%\<bundle-id>\logs\error.log`（`app_log_dir`） |
| Linux | `~/.local/share/<bundle-id>/logs/error.log`（`app_log_dir`） |
| Android / iOS | `<app cache>/logs/error.log`（应用私有目录） |

设置菜单 / 启动失败横幅里的「分享错误日志」：

- Android：系统分享面板（微信/邮件/文件），走 FileProvider，无需存储权限；
- 桌面：返回路径并复制到剪贴板（配合「打开日志目录」）。

## 记录内容

| scope | 内容 |
| --- | --- |
| `boot` | 每次启动一行：版本、OS、架构；桌面额外记录内核是否安装/版本、服务状态 |
| `cmd:<命令名>` | 所有 `#[tauri::command]` 返回的错误（`anyhow` 自动展开因果链） |
| `service.install` / `service.install.diag` | 服务安装重试过程、`launchctl print` 输出、plist 内容 |
| `service.install.not_running` | 装上了但没进入 Running（含现场） |
| `service.ctl` / `kernel.install` / `update.apply` | 关键动作（启停服务、装内核、装应用更新） |
| `mode` | RPC 连接成功（桌面 url / 移动 FFI） |
| `elevate` | 提权子进程失败（含 `/tmp/exvlan-elevate.log` 尾部） |
| `panic` | Rust panic（Android release `panic=abort`，这是唯一的落盘机会） |
| `fe:*` | 前端 `onerror` / `unhandledrejection` / Vue `errorHandler` / 关键业务错误 |

格式：`RFC3339 [scope] message`，多行内容压成 `|` 分隔的一行，便于 grep。

## 目录不可写时的兜底（20710 起）

日志目录按以下顺序尝试，第一个可写即用，并记录降级原因：

1. `app_log_dir()`（平台默认）
2. `app_config_dir()/logs`
3. `std::env::temp_dir()/exvlan-logs`

启动时若检测到用户目录不可写（典型：被 root 守护进程写过，属主变成 root），
会**自动提权修复一次**（`chown -R <user>`），并把结果写进 `error.log`。
关于页「诊断信息」可查看路径、可写性、服务状态，并一键复制/分享。

> 20709 实测：服务模式的 `--config-dir` / `--file-log-dir` 曾指向用户目录，
> 而守护进程以 root 运行 → 目录属主变 root → GUI 既写不了 `error.log` 也写不了
> `ui-state.json`（表现为「日志尚未初始化」+ 语言/模式不持久化）。
> 20710 起服务默认目录改为系统路径：
> macOS `/Library/Application Support/exvlan/config.d` + `/Library/Logs/exvlan`；
> Linux `/var/lib/exvlan/config.d` + `/var/log/exvlan`；Windows `C:\ProgramData\exvlan`。
> 旧用户目录里的 `config.d` 会在安装服务时复制到新位置（不动源目录）。

## 轮转与脱敏

- 5 MB × 3 份：`error.log`、`error.log.1`、`error.log.2`（上限约 15 MB）。
- 默认脱敏：`network_secret` / `*_private_key` / `credential_secret` / `password` /
  `secret` / `token` / `authorization` 的值替换为 `***`；不写整份 `NetworkConfig`。
- 写入失败静默（绝不影响业务）；初始化前发生的记录进 200 条环形缓冲，初始化后回放。

## 代码入口

- `packages/studio/src-tauri/src/error_log.rs`：`init` / `record` / `other` / `tail` / `redact`
- 初始化：`run_gui()` 的 `setup` 里，先于任何命令与 FFI 调用
- 前端上报：`report_frontend_error(scope, message, stack)` → `error.log`

## 排查建议

1. 先看最后一条 `[boot]`：确认版本、模式（service/normal）、内核与服务状态；
2. 再找时间点附近的 `[cmd:*]` / `[service.*]` / `[panic]`；
3. 服务类问题看 `service.install.diag`（含 launchctl 与 plist 全文）；
4. 需要更细的上下文时再对照同目录的 `exvlan.log`。
