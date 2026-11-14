//! 网络列表命令：两个后端实现，前端契约一致（`get_networks` / `set_networks`）。
//!
//! - 桌面：内核配置目录是唯一权威（`<config-dir>/<id>.toml`），见 [`desktop`]；
//! - 移动：内核进程内运行，仍用 `networks.toml`，见 [`mobile`]（后续统一）。

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop;
#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use desktop::*;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::*;
