# P6：移动端复用预编译内核库（设计 + 进度）

> 目标：Android/iOS 的 app 不再编译整颗内核，内核以**预编译库**（C ABI）随包分发。
> 决策基线（2026-11-01）：换底层 transport、事件用「回调 + 内部队列」、**不做回退开关**、iOS 先做无签名编译检查。

## 为什么必须用「库」而不是「可执行文件」

| 平台 | 内核运行位置 | 原因 |
|---|---|---|
| 桌面 | 独立 root 守护进程 | TUN 需要 root；GUI 保持普通用户 |
| Android | **app 进程内**（`.so`） | `VpnService` 的 TUN fd 只能在应用进程使用；Android 禁止从应用目录 exec 原生二进制 |
| iOS | **NetworkExtension 进程内**（`.a`） | Packet Tunnel Provider 由系统拉起，必须是随包签名的扩展 |

因此移动端不能像桌面那样「下载二进制 + 起进程」，只能复用**预编译库**。

## 现有可用的 C ABI

`exvlan/contrib/ffi`（`exvlan-ffi`，`crate-type = ["cdylib", "rlib"]`）已导出：

```
parse_config / run_network_instance / retain_network_instance / delete_network_instance
list_instance / collect_network_infos / set_tun_fd
call_json_rpc(service, method, domain, payload_json, out) 
start_config_server_client / stop_config_server_client / is_config_server_client_connected
get_error_msg / free_string
```

`packages/studio/src-tauri/src/ffi_kernel.rs`（仅 android）已封装这些调用。

## 进度

| 步骤 | 状态 |
|---|---|
| ① CI 产出 per-ABI `libexvlan_ffi.so`（`mobile-kernel.yml`） | ✅ 已验证（arm64-v8a，7 MB） |
| ①b pipeline 门控（develop/tag/dispatch） | ✅ |
| ② Rust 绑定层（`ffi_kernel.rs`） | ✅ |
| ③ 移动端 RPC 换底层 transport → `call_json_rpc` | ✅ `RpcTransport` + `TunnelTransport` / `FfiTransport` |
| ④ FFI 事件回调（回调 + 内部队列） | ✅ `event_api.rs` + Rust 转发线程 + 30s 比对兜底 |
| ⑤ `mobile.yml` 下载 `.so` 注入 `jniLibs`，移除内核编译 | ✅ 依赖已删除，构建不再编译内核 |
| ⑥ 移动作业耗时 25min → ~12min | ⏳ 待全量 CI 实测 |
| ⑦ iOS 无签名编译检查（`ios-check.yml`） | ✅ CI 已验证 |
| ⑧ `WebClientService` 方案（决策 C） | ✅ 结论：**无需新内核 API** |

### ⑧ WebClientService 的最终结论（重要）

内核显式拒绝 `api.manage.WebClientService` 走通用 FFI RPC。排查 GUI 的实际用法后发现：
它只在 `load_configs` 里用于「启动时把 enabled_networks 重新拉起」：

```rust
client.run_network_instance(BaseController::default(), RunNetworkInstanceRequest { ... })
```

而 FFI 已有等价能力：`run_network_instance(cfg_toml)` + `set_tun_fd(name, fd)`。
因此 **Android 侧直接逐个调用现有 FFI 接口即可**，不需要新增「专用对账接口」，
也不需要放开内核的拒绝分支 —— 内核零改动，API 面最小。

## ③ 的实际实现（已完成）

`exvlan/rpc` 里新增 `RpcTransport` trait，返回 **typed client**（不是裸 tunnel）：

| 实现 | 场景 | 说明 |
|---|---|---|
| `TunnelTransport` | 桌面 | `BidirectRpcManager::run_with_tunnel`（tcp 连本地内核守护进程） |
| `FfiTransport` | 移动 | `call_json_rpc`（feature `ffi-rpc`），仅实现 GUI 用到的 5 个方法 |

`GUIClientManager` 持有 `transport` 字段，业务代码只依赖 typed client，不再关心底层。

Android 侧同时删掉：ring tunnel、本地 `ApiRpcServer`、`NativeInstanceManager`、
`WebClient`（改 `ffi_kernel::start_config_server`）、`subscribe_native_instance_event`（改 ④），
最后删除 `[target.'cfg(android|ios)'] exvlan = {...}` —— **studio 依赖树中已无内核 crate**。

## ③ 的原始设计（存档）

GUI 现在通过 `BidirectRpcManager::run_with_tunnel(tunnel)` 走**二进制 RPC 协议**；
FFI 只有 `call_json_rpc`（protobuf **JSON** 请求/响应）。所以「换 transport」的实际含义是：

1. 在 `manager` 层新增 `FfiRpcClient`，把每个调用编成
   `{service, method, domain, payload_json}` → `call_json_rpc` → 解析响应 JSON → protobuf。
2. Android 分支不再创建 ring tunnel / 本地 RPC server，`GUIClientManager` 改用 `FfiRpcClient`。
3. 需要覆盖的调用面（当前 GUI 用到）：
   - `ConfigRpc`（get/patch config）
   - `LoggerRpc`（set logger config）
   - `VpnPortalRpc`（get vpn portal info）
   - `WebClientService` / `LoggerRpc` 等 scoped client
4. 序列化：`exvlan-proto` 的 prost 消息 → `serde_json`（protobuf JSON 映射）。

## ④ 的实际实现（已完成）

1. 内核 FFI 新增 `register_instance_event_callback` / `poll_instance_events`（`contrib/ffi/src/event_api.rs`）。
2. 回调只把事件 JSON 入队，不在回调线程做重活。
3. Rust 侧 `spawn_kernel_event_forwarder`：500ms 轮询 → `app.emit("dhcp_ip_changed"|"proxy_cidrs_updated", 实例名)`。
4. 兜底：`spawn_network_info_watchdog` 每 30s 调 `collect_network_infos`，比对
   `virtual_ipv4` / `routes` / `running` 签名（忽略 `peers` 等抖动字段），变化即 emit。
   队列溢出丢事件时靠它收敛。

## ④ 的原始设计（存档）

1. 内核 FFI 增加：
   ```c
   int register_instance_event_callback(cb, user_data);   // 0 成功
   ```
2. 实现：后台订阅 `subscribe_native_instance_event`，把事件 JSON **只入队**（不在回调线程做重活）。
3. Rust 侧一个消费者线程取出队列 → `app.emit("dhcp_ip_changed" | "proxy_cidrs_updated", id)`。
4. 回调签名与 `ConfigServerEventCallback` 保持一致（`extern "C" fn(*const c_char, *mut c_void)`）。

## ⑤ 的技术要点

```yaml
- uses: actions/download-artifact
  with: { pattern: mobile-kernel-*, path: ./kernel-libs }
- run: |
    for abi in arm64-v8a armeabi-v7a x86 x86_64; do
      mkdir -p packages/studio/src-tauri/gen/android/app/src/main/jniLibs/$abi
      cp kernel-libs/mobile-kernel-$abi/*.so packages/studio/src-tauri/gen/android/app/src/main/jniLibs/$abi/
    done
```
同时从 `packages/studio/src-tauri/Cargo.toml` 移除 android 的 `exvlan` 依赖。

## 坑 1：只放 .so 不链接 → 真机启动闪退（2026-11 实际踩到）

APK 里带了 `libexvlan_ffi.so`，但 `libapp_lib.so` 的 `DT_NEEDED` 里没有它，
进程启动 `dlopen` 时符号解析失败，直接 abort（表现为「一打开就闪退」，无 Java 栈）。

原因：Rust 侧只写了 `unsafe extern "C" { ... }`，**没有 `#[link(name = "exvlan_ffi")]`**，
而 cdylib 默认允许未定义符号，`cargo check` 与桌面构建都发现不了。

修法与防线：

1. `ffi_kernel.rs`：extern 块加 `#[link(name = "exvlan_ffi")]`
2. `build.rs`：android 目标下 `cargo:rustc-link-search=native=gen/android/app/src/main/jniLibs/<abi>`
   + `cargo:rustc-link-lib=dylib=exvlan_ffi`；缺库时 **panic**
3. CI 断言（`mobile.yml` + `mobile-ffi-check.yml`）：
   `readelf -d libapp_lib.so | grep libexvlan_ffi.so`，且 `UND` 不得含内核符号
4. 快检必须用 `cargo build`（只有链接才写入 DT_NEEDED），`cargo check` 无效

## 坑 2：APK 每次发版签名都不同（2026-11 实际踩到）

`gen/android/keystore.properties` 里的密码与 `upload-keystore.jks` 不匹配 →
`build.gradle.kts` 静默回退到 `signingConfigs.debug` → AGP 在每个 runner 上
**随机生成** debug keystore → 每次构建签名不同 → 用户覆盖安装报「签名冲突」。

修法与防线：

1. CI 用 secrets 注入：`ANDROID_KEYSTORE_BASE64` → `$RUNNER_TEMP/*.jks`，
   导出 `KEYSTORE_PATH` / `KEYSTORE_ALIAS` / `KEYSTORE_PASSWORD`（gradle 优先级：env > properties）
2. `build.gradle.kts`：release 构建 keystore 不可用直接 `GradleException`（禁止回退 debug）
3. 仓库不再存 keystore / 明文密码（`.gitignore` 已忽略 `*.jks`、`keystore.properties`）
4. CI 断言：`apksigner verify --print-certs` 的 SHA-256 == 正式证书
   `2556EBD965FA0DEE2C4166011ECE0927E2859EB6F828499048D3A0D144F7FA04`

> 教训：这类「构建仍绿但产物不可用」的问题只能靠**产物级断言**拦截，编译通过说明不了任何事。

## 坑 3：冷启动误报「无法连接至远程客户端」（2026-11 实际踩到）

B3b 把 android 的 `is_client_running` 实现改成「FFI 实例列表非空」，
但 UI 用它决定渲染主界面还是空状态（`client.not_running`）——
冷启动必然为空，于是永远卡在「无法连接至远程客户端」，用户也没有入口启动实例。

修法：移动端以「`GUIClientManager` 已初始化（FFI 就绪）」为准，
实例是否在跑由实例列表接口单独反映。已在 `ee088f6f` 修复。

## P7（未完成）：移动端彻底剥离 `exvlan-core`

现状：`exvlan-rpc` 把 `exvlan-core` 作为**非可选依赖**，移动端因此仍在编译内核
（`libexvlan_core.so` 775KB + 约 5 分钟编译）。移动作业 25min → 16.3min 的收益到此为止。

要真正剥离，缺口在**客户端管理面**（`RemoteClientManager` 的默认实现都在 core 里）：

1. `exvlan-rpc`：`exvlan-core` 改 optional；`rpc` / `gui-client` feature 依赖 core（仅桌面），
   `ffi-rpc` 不依赖
2. 日志：`log/file.rs` 现在用 core 的 `FileLoggerConfig`、`log/management.rs` 用 `LoggingConfigLoader`。
   在 rpc 内置一份最小配置类型（level/dir/file 名/滚动参数）直接喂 `tracing`，不再依赖 core
3. studio：新增 `cfg(mobile)` 的 `FfiClientManager`（`GUIStorage` + 全部命令走
   `ffi_kernel` 的 run/retain/delete/list/attach_tun_fd/collect 与 `call_json_rpc` 的
   config/patch/logger/vpnportal），替代 core 的 `RemoteClientManager` 默认实现
4. 移动 target features 收敛为 `logging, ffi-rpc`
5. CI 断言：`cargo tree -p exvlan-studio --target aarch64-linux-android` 不含 `exvlan-core`

工作量约 400 行，且只能靠 4-ABI 快检 + 真机验证。

### 落地顺序（下一步从这里开始）

1. `exvlan/rpc/Cargo.toml`：`exvlan-core` 改 optional，`rpc`/`gui-client` 依赖它，`ffi-rpc` 不依赖
2. `exvlan/rpc/src/log/{file,management}.rs`：把 core 的 `FileLoggerConfig`（level/file/dir/size_mb/count）
   与 `LoggingConfigLoader` 复制为 rpc 本地类型/ trait；studio 侧做一次转换
3. `exvlan/rpc/src/{rpc,transport}.rs`：`TunnelTransport` 与拨号器 gate 在 `rpc` feature 后
4. studio：新增 `cfg(mobile)` 的 `FfiClientManager`（GUIStorage + 命令走 ffi_kernel / call_json_rpc），
   桌面保留现有 `GUIClientManager`。需要覆盖的方法（桌面命令层实际调用的 12 个）：
   `handle_run_network_instance` / `handle_remove_network_instances` /
   `handle_list_network_instance_ids` / `handle_collect_network_info` /
   `handle_get_network_config` / `handle_get_network_config_with_source` /
   `handle_save_network_config` / `handle_validate_config` / `handle_update_network_state` /
   `handle_get_network_metas` / `handle_patch_vpn_portal_clients` /
   `post_run_network_instance_hook`（+ `set_logger_config` 走 transport.logger()）。
   其余命令在移动端返回「该功能仅桌面端支持」。

   **签名来源**：`exvlan/core/src/management/full/remote_client.rs::RemoteClientManager`
   （`T = AppHandle`，`E = anyhow::Error`）。桌面实现见 studio 的
   `impl RemoteClientManager<AppHandle, GUIConfig, anyhow::Error> for GUIClientManager`。

   移动端实现建议用**固有方法**（inherent impl）而非实现该 trait ——
   trait 在 core 里，实现它就必然依赖 core；而命令层是按名字调用
   `client_manager.handle_xxx(...)`，固有方法会优先匹配，无需 trait 在作用域。

   ```rust
   #[cfg(any(target_os = "android", target_os = "ios"))]
   impl GUIClientManager {
       pub async fn handle_run_network_instance(&self, _app: AppHandle, cfg: NetworkConfig, save: bool)
           -> Result<(), anyhow::Error> { /* toml + ffi_kernel::run_instance */ }
       // ... 其余 11 个
   }
   ```

   **类型不是问题（2026-11 重新核实）**：`exvlan_core::config::api_input::NetworkConfig`
   只是 `pub type NetworkConfig = exvlan_proto::api::manage::NetworkConfig;` 的别名 ——
   移动端直接用 `exvlan_proto` 即可，不需要 core 的 1016 行转换逻辑
   （配置→TOML 的转换发生在内核侧，移动端只是把 NetworkConfig 作为 RPC 载荷发过去）。

   真正需要补的两块：
   1. `RpcTransport` 增加 `web_client()`（`api.manage.WebClientService`，8 个 RPC：
      validate_config / run_network_instance / retain_network_instance /
      collect_network_info / list_network_instance / delete_network_instance /
      get_network_instance_config / list_network_instance_meta）。
       `FfiTransport` 每个方法都是 `call("api.manage.WebClientService", "<method>", &input)`，
       `TunnelTransport` 用 `scoped_client::<WebClientServiceClientFactory<..>>`。
   2. 移动端本地存储：桌面用 core 的 `Storage`/`PersistentConfig` trait + `GUIConfig`，
      移动端写一份最小的 JSON 存储（连接列表 + enabled_networks + 上次实例），
      不再依赖 core 的持久化 trait。
5. 移动 target features 收敛为 `logging, ffi-rpc`；CI 断言 `cargo tree --target aarch64-linux-android` 无 exvlan-core

验收：4 ABI 快检（真实 build + DT_NEEDED 断言）+ 真机回归（启动/建连/TUN/路由/关于页）。

## 风险

| 风险 | 缓解 |
|---|---|
| 移动作业无法本地编译（缺 NDK） | 只能靠 CI 迭代；每轮用 `p4` 临时分支 + 单 ABI 快速验证 |
| `call_json_rpc` 与二进制 RPC 语义差异（流式/订阅） | 先把 GUI 用到的请求/响应方法覆盖，事件用 ④ 的回调补 |
| APK 体积 | `.so` 约 7 MB/ABI（split-per-abi 后每包只含一个） |
| 总 CI 时间不降 | 只是把内核编译从 4 个移动作业挪到 1 个共享作业；单移动作业耗时下降 |

## iOS 现状

- 无 `gen/apple`、无签名配置、**不在原 CI 矩阵**。
- 已新增 `ios-check.yml`：macOS runner 上 `cargo check` 设备与模拟器目标（无签名）。
- 真机/商店包需要：Apple 开发者账号 + 证书 + provisioning profile；NetworkExtension 只能真机验证。

## 落地结果（2026-11，P7 步骤 4-5 已完成，随 v26.410.20708 发布）

| 步骤 | 落地内容 |
|---|---|
| 4a | `RpcTransport::web_client()`（`api.manage.WebClientService` 8 个 RPC）；`FfiTransport` 走 `call_json_rpc`，`TunnelTransport` 走 `scoped_client` |
| 4b | studio 新增 `src/mobile_manager.rs`（`#[path] mod manager`）：内存存储 + 事件落盘（`save_configs` / `save_enabled_networks`），12 个管理方法为**固有方法** |
| 4c | 内核侧补 `call_web_client_json_rpc`，FFI 的 `call_json_rpc` 现在把 `api.manage.WebClientService` 分发到 `ProcessManagementRpc`（此前被显式拒绝） |
| 5a | studio `Cargo.toml`：`exvlan-rpc` 按平台声明；android 只用 `logging,management,ffi-rpc` |
| 5b | CI（`mobile.yml` / `mobile-ffi-check.yml`）新增 `cargo tree --target <android> -p exvlan-studio` 不含 `exvlan-core` 的硬断言 |

**踩到的坑（重要）**：

1. `exvlan-rpc` 若写在 `[dependencies]`（而非 target 段），其 `gui-client` feature 会**统一**到 Android 目标，
   `cargo tree` 仍会看到 `exvlan-core`。必须按平台拆开声明。
2. `exvlan-proto` 需显式启用 `json-rpc`：桌面端此前靠内核 feature 统一带进来，移动端剥掉内核后就没有了。
3. 内核 `call_management_json_rpc` 原本对 `api.manage.WebClientService` 直接返回错误，
   不补 FFI 分发的话，移动端所有实例管理调用都会失败。
4. 日志配置类型下沉到 `exvlan-rpc` 后，内核（`exvlan`）与 `exvlan-api` 需要补各自 CLI 配置的 rpc 侧适配。

**发布验证（v26.410.20708）**：全量门禁 50 作业全绿；APK 体积
arm64 31.7→30.7MB、armeabi-v7a 22.8→22.2MB、x86_64 36.3→35.2MB、x86 35.9→34.6MB。
