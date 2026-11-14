# `exvlan-migrate`：dev.2 覆盖范围与决策

> 铁律见 [`docs/decisions-7.3.md`](./decisions-7.3.md) §14.15：
> **运行时代码不保留任何「为了读旧形状」的逻辑**；历史残留一律由本工具一次性处理。
> **执行顺序硬约束**：先让 migrate 覆盖某个旧形状，**再**删运行时的兼容读取。
> 本轮（dev.2）只做「覆盖」，**不删**任何运行时兼容代码。

## 1. 契约

| 项       | 结论                                                                                       |
| -------- | ------------------------------------------------------------------------------------------ |
| 默认行为 | dry-run：打印每一步的计划/结论，**不落盘**（连内核 `config/*.toml` 都不写）                |
| 落地     | `--yes`（`-y`）                                                                            |
| 报告     | `--json` 打印单个 JSON；同时写 `<backup>/migrate-report.json`                              |
| 回滚     | `--rollback <backup-dir>`（按文件名回灌用户目录，并删 `migrate.done` / `migrate.done.v2`） |
| 提权属主 | `--owner-uid <uid>`（GUI 提权调用时传桌面用户 uid，备份/marker 会 chown 回去）             |
| 退出码   | `0` 成功 / `1` 无旧数据 / `2` 失败                                                         |
| 幂等     | 第二次运行 = 「没有旧版本数据，无需迁移」（`exit 1`），文件内容与 `mtime` 都不变           |

报告里每一步都有 `status` ∈ `planned | ok | skipped | partial | failed`，
`reason` 写明为什么跳过/失败；`completed` / `skipped` 是派生根，方便人直接读。
顶层保留旧键（`ok` / `converted_networks` / `removed` / `backup` / `ts`），不破坏既有消费方。

## 2. 执行顺序

```
0. sanitize     service.toml 里非法的 owner_sid（存量修复，标记 migrate.done.v2）
1. detect       exvlan_control::legacy::detect（GUI 闸门 / doctor 共用同一份）
2. backup       <user data>/exvlan-migrate-backup-<ts>/（目录 + manifest）
3. convert      networks.toml → config/<id>.toml；service.json → service.toml
3.5 service_toml 顶层 mode → policy，旧词汇归一（S8）
4. prefs        ui-state.json → prefs.toml，旧 app_mode 拆分，陈旧 [misc] 键清理
5. machine_id   旧落点 COPY 到新落点（不删旧文件）
6. cleanup      stale_paths → 旧服务定义 → 删源（ui-state.json 条件删除）
7. marker       migrate.done + 报告
```

顺序理由：

- **备份**必须早于任何**数据类**原地改写（`prefs.toml` / `service.toml` 归一 / `ui-state.json` 删除）；
  例外是第 0 步的 `owner_sid` 清洗 —— 它有自己的标记（`migrate.done.v2`）、只改一行值、
  且必须在「无旧数据即 `exit 1`」之前跑（否则全新安装机器的脏 `owner_sid` 永远清不掉）；
- **prefs 早于删源**：`ui-state.json` 只有在 prefs 步骤成功后才允许删（铁律：不许「删了源没写目标」）；
- **machine_id 早于 `stale_paths`**：`stale_paths` 里含 `/var/lib/exvlan` 这类可能装着旧 `machine_id`
  的目录，另有护栏「machine_id 未归位就不删包含源的目录」；
- **旧服务定义最后**：`launchctl bootout` / `sc delete` 与文件删除都在服务配置已就位之后。

## 3. 失败语义

| 场景                                      | 行为                                                                                                                    |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `networks.toml` / `service.json` 转换失败 | **bail**（退出码 2），源保留，**不写** `migrate.done`                                                                   |
| `prefs.toml` 写入失败                     | `prefs` 步骤 `failed` + `prefs_skipped: …`，**不 bail**，`ui-state.json` 保留；marker 照写但把步骤 id 记进 `incomplete` |
| `prefs.toml` 本身解析不了                 | 步骤 `skipped`（`prefs_unparsable`），**不动**该文件，源保留                                                            |
| `[app].mode` 解析失败                     | `app_mode_invalid`，**保留原值**（前端仍可工作），不 bail；此时不删 `ui-state.json`                                     |
| `machine_id` 旧文件非法/读不到            | 步骤 `skipped` 并列出候选与原因，不 bail                                                                                |
| 旧路径 / 旧服务定义删不掉                 | `partial` + 每条 `failed[{path,error}]`，继续下一条；无 root 时整体 `skipped`（提示需要管理员）                         |
| `service.toml` 写入失败                   | `service_toml` 步骤 `failed`，报告明确「运行时别名仍需保留」                                                            |

> marker 在 `prefs` 失败时仍然写：否则闸门会在每次启动都弹（`pending` 永远为真）。
> 代价是「这一步没做完」只能靠 `incomplete` / 报告看见 —— 所以 `ui-state.json` 与备份必须都保留。

## 4. 各步骤决策

### S0 `ui-state.json` → `prefs.toml`

键映射与运行时 `packages/studio/src-tauri/src/prefs.rs`（`set_key` / `map_key`）同一张表：

| `ui-state.json` 键                                  | `prefs.toml` 落点                                                                      |
| --------------------------------------------------- | -------------------------------------------------------------------------------------- |
| `app_mode`                                          | `[app].mode`                                                                           |
| `lang`                                              | `[app].lang`                                                                           |
| `log_level`                                         | `[app].log_level`                                                                      |
| `last_network_instance_id`                          | `[ui].last_network_instance_id`                                                        |
| `diag_last_upload`                                  | `[diag].last_upload_url`（旧值是 `{at,urls}` 时拆成 `last_upload_url` + `updated_at`） |
| 其它（含 `networkList`、`upgrade_ignored_version`） | `[misc].<原键名>`                                                                      |

- **已存在的 `prefs.toml` 只补缺失键，绝不覆盖已有值**（`kept_existing` 会在报告里列出来）。
- 只有 `ui-state.json` 不存在时才看 `.bak`：**只记录、不恢复**（7.1 的损坏快照，回滚专用）。
- 写入方式与运行时一致：临时文件 + rename + 读回校验，权限 `0600`。

### S1 `app_mode` 拆分

规则在 `exvlan/control/src/legacy/app_mode.rs`，与前端 `target.ts`
（`migrateLegacyMode()` / `parseAddress()`）**逐字一致**：

| 旧 `mode`               | `[misc].runtime`                      | `[misc].target`           | `[misc].remotes`        |
| ----------------------- | ------------------------------------- | ------------------------- | ----------------------- |
| `service`               | `service`                             | —                         | —                       |
| `normal`                | `normal`                              | —                         | —                       |
| `remote` + 合法地址     | `service`（本机内核照跑，界面连别处） | `rm-legacy-<host>-<port>` | `[{id,name,host,port}]` |
| `remote` + 非法地址     | `service`                             | —                         | —                       |
| 未知 / 缺字段 / 非 JSON | **保留原值**（`app_mode_invalid`）    | —                         | —                       |

- `remotes` 的合成 id `rm-legacy-<host>-<port>` 与 `name`/`host`/`port` 字段必须与前端算出来的一致，
  否则老用户丢远端列表（`target` 指向不存在的 id）。
- **`config_server_url` 是现役设置，保留在瘦身后的 `[app].mode` 里**（`{"config_server_url": "…"}`）：
  前端 `loadMode()` 直接读 `app_mode.config_server_url`，放 `[misc]` 需要同时改前端读法 ——
  而本轮不动运行时。`config_dir` / `rpc_portal` / `file_log_dir` / `file_log_level` 等
  已被 `normalizeMode()` 从新路径推导，**删除**（保留它们反而会把界面带回旧目录）。
- 拆分成功后才删被消费的键；`[misc]` 里已有 `runtime` / `target` / `remotes` 时一律不覆盖。

### S2 陈旧 `[misc]` 键

`networkList`、`upgrade_ignored_version` 是前端 `localStorage` 写穿留下的界面缓存，删掉。
**不进迁移闸门的检测**：前端启动后又会写穿回来，放进检测会让闸门永远 pending。

### S3 `machine_id` 归位（高风险）

优先级（新路径已存在则**一律不动**）：

1. `<data>/exvlan/machine_id`（`$XDG_DATA_HOME` / `%LOCALAPPDATA%` / `~/Library/Application Support`）；
2. Linux 兜底：`/var/lib/exvlan/machine_id`、`/var/lib/era.exvlan.studio/machine_id`
   （以 root 且没有 `HOME` 跑过的内核落点，都在 `stale_paths()` 的删除清单里）；
3. `<core>/et_machine_id`（旧内核把 id 写在可执行文件旁）。

- **COPY 到 `<machine_id_state_dir>/machine_id`，不删旧文件**（下次幂等再判；回滚不依赖它）。
- 目标目录与内核 `machine_id.rs::default_machine_id_state_dir()` **必须一致**，注意三端不同：
  macOS 与 `user_paths().data` 同目录；Windows 是 `%LOCALAPPDATA%`（data 在 `%APPDATA%`）；
  Linux 是 `$XDG_DATA_HOME`（data 在 `$XDG_STATE_HOME`）。
- **放弃的一支：旧 `machine_uid` 哈希**（`resolve_legacy_machine_uid_hash`）。
  理由：复算需要 `machine_uid` crate + 内核 `digest_uuid_from_str`（`std::hash::DefaultHasher`）
  逐位一致，Linux 还掺 hostname/MAC；而这一支只在「状态目录非空、但 `machine_id` 从未落盘」
  时生效 —— 那种机器旧版本每次启动也是现算，7.3 运行时同样现算（`resolve_new_machine_id`），
  **没有「已存在的身份」可保留**。强行复算反而可能写出与旧 `peer_id` 不同的值（Linux 上两者
  的种子不同）。报告里以 `machine_id_uid_hash: skipped(<reason>)` 显式记录。

### S4 旧服务定义清理

清单来源与 `exvlan/service/src/main.rs::cleanup_legacy_macos()` / `cleanup_legacy_windows()`
一致，常量落在 `exvlan/control/src/paths.rs`：

| 平台    | 清理对象                                                                                                                                                                                                        |
| ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS   | 5 个旧 label（`era.exvlan.studio`、`…​.helper`、`com.exvlan.studio`、`com.exvlan.studio.helper`、`exvlan-studio`）的 `launchctl bootout` + `LaunchDaemons`/`LaunchAgents` plist；旧 helper 二进制；`helper.sock` |
| Windows | 旧 SCM 服务（`sc stop` + `sc delete`）、计划任务 `era.exvlan.studio` / `ExvlanKernelSupervisor`（`schtasks /Delete`）                                                                                           |
| Linux   | 旧单元文件在 `stale_paths()` 里，无额外服务定义                                                                                                                                                                 |

逐条 best-effort：单条失败 → `service_cleanup_partial` 继续；非 root → 整体 `skipped` 并提示管理员。
**探测**（闸门用）只做「看得见」的部分：macOS 查 plist / helper 文件；Windows 查
`sc query` + `%SystemRoot%\System32\Tasks\<name>`（注册表里的服务没有文件，用 `sc` 探测）。

### S5 `stale_paths` 单一真源

清单从 `packages/studio/src-tauri/src/paths.rs` **搬到 `exvlan/control/src/paths.rs`**，
GUI 只做转发，migrate 用同一份。理由：

1. `exvlan-control` 已经是 GUI/CLI/gateway 共用的基础层，且**已在
   `script/ci/check-paths.sh` 的绝对路径白名单**里（不用新增白名单文件）；
2. `exvlan-migrate` 只有 `[[bin]]`，GUI 反过来依赖 bin crate 不成立；新建 `exvlan/paths`
   会多一个 workspace 成员 + 版本对齐三处；
3. 不产生循环依赖（`control` 不依赖 GUI/migrate）；
4. 迁移期「清单只改一处」比「抽屉更薄」更重要。

顺带修掉两处不一致：

- 清单里原来混进用户侧 `~/…/era.exvlan.studio/ui-state.json`（macOS）——它与「prefs 成功才能删源」
  冲突，已移出清单，由 S0 的条件删除统一管；
- `doctor` 的 `migrate.legacy` 检查原来自己抄了一份文件清单，已改为同一份 `detect()`。

### S6 闸门检测清单对齐

`packages/studio/src-tauri/src/migrate/desktop.rs` 与 `exvlan-control` 的 `doctor`、
`exvlan-migrate` 现在共用 `exvlan_control::legacy::detect()`，覆盖：
`networks.toml`、`config.d/`、`ui-state.json(.bak)`、内核 `.prev`、**系统目录 `service.json`**、
**`[app].mode`**、**旧 `machine_id`**、**旧服务定义**、**`stale_paths`**、**`service.toml` 旧字段名**。

`[app].mode` 的判定不是「有没有 `mode` 键」—— 7.3 前端仍会把完整 `app_mode` 写进 `[app].mode`，
那样会永远 pending。判据是「拆分目标还没落地」：

- 没有 `[misc].runtime` 且旧 JSON 里还有被消费的键；或
- 旧 JSON 是 `remote` 且 `[misc].remotes` 还没写入（否则丢远端列表）。

### S7 dry-run 可见

dry-run 会走完整条检测 + 计划：`prefs` / `app_mode` / `service_toml` / `machine_id` /
`networks`（解析出要转几个网络，但不写 `config/`）/ `stale_paths` / `service_cleanup` / `cleanup`
都以 `planned` 或 `skipped(reason)` 出现在 `steps` 里。

### S8 `service.toml` 旧字段名归一

`exvlan/control/src/legacy/service_toml.rs`（判定与改写同一份实现，migrate 只负责读写）：

- 顶层 `mode = <值>` → `policy = <值>`；
- 值归一：`service|Service` → `resident`、`normal|Normal` → `session`、`off|Off` → `stopped`，
  已是 `resident|session|stopped` 的原样保留，认不出的值**不改**、只在报告里列 `unknown_values`；
- 同时存在 `mode` 与 `policy` 时**丢掉 `mode` 行**：`#[serde(alias = "mode")]` 会让
  `load_config()` 报重复字段 → `.ok().unwrap_or_default()` → 静默回落 `resident`；
- **逐行文本改写**（不是解析成 `toml::Value` 再序列化）：重新序列化会丢注释与字段顺序，
  而这份文件是 root 手工排查时唯一的现场。`[section]` 里的同名键（如 `[gateway] mode`）不动；
- 幂等：第二次跑 `changed == false`，不写文件（`mtime` 不变）；dry-run 只报告；
- 报告额外一条 `service_toml_aliases`：归一成功时明确写「可安全删除运行时别名」，
  失败/跳过时写「**不要**删别名」。

> **残余风险（下一轮删别名前必须处理）**：`migrate.done` 存在时闸门不再触发，
> 所以「migrate 跑过之后又有更早的 pre.17 构建写入 `mode = …`」的机器不会再被自动覆盖。
> 建议在删别名的版本里把 marker 版本升到 `migrate.done.v3`（或让闸门对
> `service_toml` 一项忽略 `done`），确保这类机器至少再跑一次。

## 5. 回滚语义

- 备份目录按**文件名**回灌到用户目录（`networks.toml` / `service.json` / `config.d` /
  `ui-state.json(.bak)` / `*.prev` / `prefs.toml`），并删 `migrate.done` 与 `migrate.done.v2`；
- `prefs.toml` 若迁移前不存在，回滚后仍会留着（内容等同旧运行时会生成的值）；
- **系统目录**的 `service.toml` / `service.json` 只做了转发与归一，回滚不还原（`service.toml`
  归一前后 serde 语义相同；`service.json` 原件在备份目录里）。

## 6. 下一轮（删运行时兼容）的前置条件与清单

前置：本版 migrate 已随包分发并在目标机器上跑过（`migrate.done` 存在，或人工 `--yes`）。
**删之前逐项确认**：

| 运行时兼容项                                                    | 位置                                                                   | migrate 覆盖步骤    | 删的前置                                                    |
| --------------------------------------------------------------- | ---------------------------------------------------------------------- | ------------------- | ----------------------------------------------------------- |
| `prefs.rs::migrate_legacy` / `load_or_migrate` 的整段旧数据导入 | `packages/studio/src-tauri/src/prefs.rs:209-263`                       | S0                  | 真机确认 prefs 步骤 `ok`（macOS/Windows 各 1 台老数据机器） |
| `app_mode` 旧 JSON 读取                                         | `prefs.rs::map_key("app_mode")` + `target.ts::migrateLegacyMode`       | S1                  | 确认 `[misc].runtime/target/remotes` 已落地                 |
| 旧 `machine_id` 搬家逻辑                                        | `exvlan/exvlan/src/common/machine_id.rs:74-88,205-241`                 | S3（不含 uid 哈希） | 真机确认目标文件存在且 `peer_id` 未变                       |
| `stale_paths` 清单本身                                          | `exvlan/control/src/paths.rs`                                          | S5                  | 7.4 再删（保留一个版本用于降级回收）                        |
| 旧服务定义清理                                                  | `service/src/main.rs::cleanup_legacy_*`、`lib.rs::stop_legacy_service` | S4                  | 确认两平台真机清理成功                                      |
| `service.toml` serde 别名                                       | `service/src/main.rs:270-283, 309-311`                                 | S8                  | **先处理残余风险（marker 升 v3）**，再删                    |

## 7. 移动端复用评估（只评估，本轮不拆 crate）

| 逻辑                                                                        | 结论                                                                                                                                                                         |
| --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `exvlan_control::legacy::*`（检测 / `app_mode` 拆分 / `service_toml` 归一） | **已经是 lib**，移动端可直接用                                                                                                                                               |
| `prefs::plan` / `PrefsPlan::apply`                                          | **可抽**：不依赖 CLI，只依赖「用户数据目录」；抽成 `lib` 后移动端传 `filesDir` 即可                                                                                          |
| `machine_id::plan/select_source`                                            | **可抽**（纯 fs），但目标目录解析依赖 `exvlan_control::paths::machine_id_state_dir()`（桌面三端）；移动端需要显式传 state dir → 需把 `plan()` 改成收 `state_dir: &Path` 参数 |
| `service_toml::normalize_step`                                              | **可抽**（纯 fs + 文本）；服务配置在移动端不存在，可复用价值低                                                                                                               |
| `cleanup::stale_paths` / `remove_sources`                                   | **可抽**（纯 fs + 清单参数）                                                                                                                                                 |
| `cleanup::legacy_services`                                                  | **只能留 bin**：`launchctl` / `sc` / `schtasks` 是桌面特权操作，移动端没有对应物                                                                                             |
| `report::*`                                                                 | **可抽**（纯数据结构），但 `main` 的 stdout/退出码契约（`--json`、`exit 1`）只能留 bin                                                                                       |
| `sanitize_service_config` / `convert_service` / `convert_networks`          | 需改造：`convert_networks` 依赖 `exvlan-core`（移动端已用）。`sanitize`/`convert_service` 只服务桌面的 `service.*`，移动端用不到                                             |
| 参数解析 / `Logger` / `chown*` / 备份目录布局                               | **只能留 bin**（CLI 契约、提权属主、备份 UX）                                                                                                                                |

抽 lib 的最小切法：`lib.rs` 暴露 `plan(user_data, paths) -> Plan` 与
`Plan::apply(&self, dry_run) -> Report`，`main.rs` 只保留参数解析、日志、退出码。

## 8. 待真机验证

1. macOS：「只有系统目录 `service.json` 脏」的机器 → `migrate_status.pending == true`（S6 的核心回归）。
2. macOS/Windows：`remote` 老用户 → 升级后远端列表与 `target` 仍在（S1 的 `rm-legacy-*` 一致性）。
3. Windows：`%LOCALAPPDATA%\exvlan\machine_id` → `%LOCALAPPDATA%\era.exvlan.studio\machine_id`，
   且设备登记/已配对 token 仍认得这台机器（S3）。
4. 两平台：GUI 提权调用（`--owner-uid`）后备份目录/`prefs.toml`/`machine_id` 属主是桌面用户。
5. 两平台：旧 launchd/SCM 定义清理后，新服务仍能启动（S4 的 `bootout` 顺序）。
6. 提权运行下 `user_paths()` / `machine_id_state_dir()` 的 `HOME` / `%LOCALAPPDATA%` 是**桌面用户**的
   （`elevate` 是否传递环境变量），否则备份与 marker 会落到 root 的目录。
