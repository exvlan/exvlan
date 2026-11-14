# 路径规范（v3）

> 单一真源有两个（由 CI 守卫 `script/ci/check-paths.sh` 登记）：
>
> - **用户/系统路径**：`packages/studio/src-tauri/src/paths.rs`（GUI 侧）、`exvlan/control/src/paths.rs`（CLI / 服务 / gateway / migrate 共用）；
> - 两者必须保持一致（目录名、二进制名、服务 label）。
>
> **除这两个文件（及服务端 `exvlan/service/src/main.rs` 的 `paths()`）外，任何 Rust 源码不得出现绝对路径字面量。**

## 原则

1. 唯一标识 `era.exvlan.studio`（bundle id / applicationId / 服务 label / 目录名四者一致）
2. 每端最多两个根：用户根 + 系统根（Android 只有用户根）
3. 子结构统一：`config/` `core/` `logs/` `diagnostics/` `tmp/`
4. 文件名全小写：`prefs.toml` `networks.toml` `machine_id` `remote-tokens.json` `error.log` `events.log` `studio.log` `exvlan.log` `helper.log`
5. 不保留向后兼容：旧 `exvlan` 路径只用于一次性清理

## 落点

### macOS

```
~/Library/Application Support/era.exvlan.studio/   prefs.toml · networks.toml · machine_id · remote-tokens.json(0600)
~/Library/Logs/era.exvlan.studio/                  error.log · events.log · studio.log · diagnostics/
/Library/Application Support/era.exvlan.studio/    config/ · core/{exvlan, exvlan.sig, version}
/Library/Logs/era.exvlan.studio/                   exvlan.log · helper.log
/Library/LaunchDaemons/                            era.exvlan.studio.plist · era.exvlan.studio.helper.plist
/Library/PrivilegedHelperTools/                    era.exvlan.studio.helper · .helper.version
/var/run/era.exvlan.studio/                        helper.sock   （目录 0755，socket 0666）
$TMPDIR/era.exvlan.studio/                         elevate.log（0600）
```

### Windows

```
%APPDATA%\era.exvlan.studio\        prefs.toml · networks.toml · remote-tokens.json
%LOCALAPPDATA%\era.exvlan.studio\   machine_id · logs\（error.log · events.log · studio.log · diagnostics\）
%ProgramData%\era.exvlan.studio\    config\ · core\{exvlan.exe, version} · logs\exvlan.log
%TEMP%\era.exvlan.studio\           elevate.log
```

Windows 没有 POSIX 权限，依赖用户目录的默认 DACL（等价 0600）。

### Android

```
filesDir/   prefs.toml · networks.toml · machine_id · remote-tokens.json · logs/
cacheDir/   diagnostics/ · tmp/
```

## 标准出处

| 端        | 文档                                  | 关键规定                                                                                                                                           |
| --------- | ------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS     | Apple _File System Programming Guide_ | 用户数据 `~/Library/Application Support/<bundle-id>`；用户日志 `~/Library/Logs/<id>`；**守护进程日志 `/Library/Logs/<id>`**；运行时状态 `/var/run` |
| macOS     | _Authorization Services / SMJobBless_ | 特权 helper 放 `/Library/PrivilegedHelperTools`，launchd 描述放 `/Library/LaunchDaemons`                                                           |
| Windows   | Microsoft _KNOWNFOLDERID_             | 漫游配置 `%APPDATA%`；机器相关/大文件 `%LOCALAPPDATA%`；服务数据 `%ProgramData%`                                                                   |
| Android   | _App-specific storage_                | 持久数据 `filesDir`；可丢弃 `cacheDir`；禁止写共享存储                                                                                             |
| POSIX/FHS | FHS 3.0                               | `/var/run`（重启清空）= 运行时 socket                                                                                                              |

**有意识的偏离**：Windows 的 `%ProgramData%` 未采用 `<Publisher>\<Product>` 两层，而是与其他端一致的
`era.exvlan.studio` 单层 —— 一个标识符走遍三端，便于文档与排查脚本统一引用。

## 冻结项

以下不走“规范”，因为改动等于重新安装/重新授权：

- bundle id / applicationId / 服务 label：`era.exvlan.studio`
- 服务 plist 名、helper 二进制名（由上面的标识符派生）

## 迁移

见 `docs/decisions-7.3.md`：7.3 首次启动一次性重装系统组件，并保留一个幂等的「发现即删」扫描
（旧 `exvlan` 目录 / 旧 `exvlan-studio.plist`），7.4 再彻底删除。

旧路径清单（`stale_paths()`）、旧服务定义常量、`machine_id` 落点都只有一份，在
`exvlan/control/src/paths.rs`：GUI、`exvlan-migrate`、`doctor` 全部引用它（各写一遍必然漂）。
迁移步骤与决策见 `docs/migrate-7.3.md`。
