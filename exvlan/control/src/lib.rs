//! EXVlan 本机控制面客户端的基础层。
//!
//! 只解决「怎么连、对面是谁」，**不含任何策略**：
//!
//! - [`protocol`]：服务 IPC 的请求/响应类型（wire v2，`{proto,op,params}`）——**唯一真源**；
//! - [`ipc`]：特权服务 IPC 传输（unix socket / Windows 命名管道），行式 JSON；
//! - [`backend`]：探测当前是 `core`（裸核，无服务）还是 `service`（有特权服务）；
//! - [`config`]：网络配置（内核 `config-dir/*.toml`）唯一写者入口；
//! - [`paths`]：三平台统一的目录/二进制/凭据位置；
//! - [`legacy`]：旧版本（≤ 7.2）遗留数据的**唯一**检测来源（迁移闸门 / doctor / exvlan-migrate）。
//!
//! 使用方：GUI（`packages/studio/src-tauri`）、`exvlan-cli`、`exvlan-gateway`。
//! 编排逻辑（装服务 / 起停内核 / 策略 / 升级回滚）**不在这里** —— 那是
//! `exvlan-service` 的职责。

#![allow(clippy::result_large_err)]

pub mod backend;
pub mod config;
pub mod doctor;
pub mod identity;
pub mod ipc;
pub mod legacy;
pub mod paths;
pub mod protocol;
pub mod tokens;

pub use backend::{Backend, detect};
pub use identity::{current_uid, current_user_sid};
pub use ipc::{Client, call};
pub use paths::{APP_ID, PRIVILEGED_SERVICE_LABEL, Paths, paths};

/// 内核 RPC portal 默认端口（`service.toml [kernel] rpc_port` 可覆盖）。
pub const DEFAULT_RPC_PORT: u16 = 1995;
/// gateway 默认端口（`service.toml [gateway] port` 可覆盖）。
pub const DEFAULT_GATEWAY_PORT: u16 = 2016;
/// 本地 RPC 地址（回环）。
pub fn local_rpc_addr() -> String {
    format!("127.0.0.1:{DEFAULT_RPC_PORT}")
}
/// 本地 gateway 地址（回环）。
pub fn local_gateway_addr() -> String {
    format!("127.0.0.1:{DEFAULT_GATEWAY_PORT}")
}

/// 当前平台在镜像 `manifest.json` 里的键名（`assets` 的键）。
///
/// 镜像只托管 6 个平台：`macos-x86_64` / `macos-aarch64` / `windows-x86_64` /
/// `windows-arm64` / `linux-x86_64` / `linux-aarch64`。注意 Windows 的 arm64 键
/// 是 `arm64`（与 `core-<ARTIFACT_NAME>` 产物名一致），不是 `aarch64`。
///
/// 服务端下载内核、客户端查更新都从这里取，避免两边各写一份拼错的键。
pub fn platform_key() -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "freebsd") {
        // FreeBSD 有产物但不在镜像托管范围内（在线升级会明确报「无此平台包」）。
        "freebsd"
    } else {
        "linux"
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" if cfg!(target_os = "windows") => "arm64",
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        // 镜像产物名是 `windows-i686`；32 位 x86 在其它平台无镜像资产。
        "x86" => "i686",
        // 镜像不托管 arm 变体（armv7hf/armv7/armhf 三套 eabi 无法在 cfg 层区分）。
        "arm" => "armv7hf",
        other => other,
    };
    format!("{os}-{arch}")
}

#[cfg(test)]
mod tests {
    use super::platform_key;

    #[test]
    fn platform_key_uses_mirror_names() {
        let key = platform_key();
        let (os, arch) = key.split_once('-').expect("键形如 <os>-<arch>");
        assert!(
            matches!(os, "macos" | "windows" | "linux" | "freebsd"),
            "{key}"
        );
        // Windows 的 arm64 键在镜像里写作 `arm64`（产物名 windows-arm64）
        if cfg!(target_os = "windows") && cfg!(target_arch = "aarch64") {
            assert_eq!(arch, "arm64");
        } else {
            assert_ne!(arch, "arm64", "{key}");
        }
        // Linux/macOS x86_64 是最常见的验证机，键必须与镜像完全一致
        if cfg!(target_arch = "x86_64") {
            assert!(key.ends_with("-x86_64"), "{key}");
        }
    }
}
