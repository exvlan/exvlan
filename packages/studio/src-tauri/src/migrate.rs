//! 一次性迁移（pre.17）：桌面端真正执行，移动端没有这段历史（无旧数据）。
//!
//! 运行时**不做**迁移/兼容：检测到旧数据就阻塞启动，由用户确认后调用随包的
//! `exvlan-migrate`（它自己会备份/转换/清理/写标记）。下一版移除随包与自动执行。

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop;
#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use desktop::*;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::*;
