# iOS 编译检查：现状与待办（2026-11-03）

## 结论

`ios-check.yml`（`cargo check -p exvlan-ios --target aarch64-apple-ios`）目前**失败**，
但**与 P7 / 移动端改造无关**：失败发生在第三方依赖 `nix 0.31.3` 编译期。

```
error[E0425]: cannot find value `AF_SYS_CONTROL` in crate `libc`
  --> nix-0.31.3/src/sys/socket/addr.rs:1793
error[E0425]: cannot find value `SYSPROTO_CONTROL` in crate `libc`
  --> nix-0.31.3/src/sys/socket/sockopt.rs:1289
error[E0425]: cannot find value `UTUN_OPT_IFNAME` in crate `libc`
  --> nix-0.31.3/src/sys/socket/sockopt.rs:1290
error: could not compile `nix` (lib) due to 11 previous errors
```

`AF_SYS_CONTROL` / `SYSPROTO_CONTROL` / `UTUN_OPT_IFNAME` 都是 **macOS-only** 的 libc 常量，
`nix` 的 utun / `KextControl` 代码在 iOS 目标下没有被正确 `cfg` 掉，导致整体编译失败。

## 依赖来源

```
atomic-write-file 0.3.1 → nix ^0.31   （exvlan 的 management feature）
os_info 3.15.0          → nix ^0.31
```

## 可选修法（按代价排序）

1. **等上游**：`nix` 修掉 iOS 的 cfg（最省事，但不可控）。
2. **降级 `atomic-write-file` / `os_info`**：需要确认是否存在不依赖 `nix 0.31` 的版本；
   属于依赖变更，需要走 audit 与全量 CI。
3. **给 `nix` 打 vendored patch**：在 `[patch.crates-io]` 里放一个 `cfg` 修正版，
   代价是维护补丁（与「rust 告警等上游、不自维护补丁」的项目约定冲突）。
4. **把 iOS 检查降级为 `continue-on-error`**（现状即是如此：`Check studio for iOS` 早就
   `continue-on-error: true`，但 `Check kernel for iOS` 是硬失败，所以 workflow 一直红）。

## 现状建议

- 该工作流**不在 `pipeline.yml` 门控内**，不影响发版；
- 保留硬失败，作为「上游修好后自动变绿」的哨兵；
- 需要 iOS 真机链路前必须解决（NetworkExtension 本身也只能真机验证）。

## 验证记录

- 2026-11-03：`ios-check` 两次手动触发（`021c14a1` / `bb7a2baa` 之后的 develop）均失败于上述
  `nix 0.31.3`；`P7` 的移动端代码（`mobile_manager.rs`、`RpcTransport::web_client`）
  在 Android 4 ABI 全绿，iOS 侧尚未获得有效编译结论。
