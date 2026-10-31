//! EXVlan GUI 侧的**轻量**依赖层。
//!
//! 目的：让 `packages/studio/src-tauri` 不再静态链接整颗内核（`exvlan` crate），
//! 从而把 GUI 的构建时间与内核解耦，并支持内核以独立二进制/独立版本运行。
//!
//! 本 crate 只允许依赖 proto 与「纯管理/客户端」代码，**不得**引入
//! tun / smoltcp / quinn / websocket 等内核运行时依赖。
//!
//! 目前包含：
//! - [`service_manager`]：系统服务（launchd / systemd / Windows SCM）的安装、启停与查询；
//! - [`rpc`]：桌面端 RPC 拨号（最小 host runtime，不依赖内核 crate）；
//! - [`log`]：日志初始化（控制台 + 滚动文件），内核与 GUI 共用同一实现。

#[cfg(feature = "logging")]
pub mod log;
#[cfg(feature = "logging")]
pub mod tracing_rolling_appender;

pub mod rpc;
pub mod service_manager;
