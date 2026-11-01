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
| ③ 移动端 RPC 换底层 transport → `call_json_rpc` | ⬜ 主要工作量 |
| ④ FFI 事件回调（回调 + 内部队列） | ⬜ 需改内核 FFI |
| ⑤ `mobile.yml` 下载 `.so` 注入 `jniLibs`，移除内核编译 | ⬜ |
| ⑥ 移动作业耗时 25min → ~12min | ⬜ 依赖 ③④⑤ |
| ⑦ iOS 无签名编译检查（`ios-check.yml`） | ✅ 新增，待 CI 验证 |

## ③ 的技术要点（下一步实施）

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

## ④ 的技术要点

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
