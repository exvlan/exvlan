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

use serde::Serialize;

/// 迁移失败时用户能自己动手的全部信息。
///
/// **路径与命令只由后端给出**：备份目录（`<数据目录>/exvlan-migrate-backup-<ts>`）、
/// 日志（`<日志目录>/migrate.log`）、报告（备份目录里的 `migrate-report.json`）的
/// 布局三平台不同，前端拼不出来也不该拼。
// 移动端永不 pending，不会构造它；但前端用同一份 TS 类型，字段必须保持一致。
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
#[derive(Debug, Clone, Serialize)]
pub struct MigrateGuide {
    /// 用户数据目录（备份目录建在这里）
    pub data_dir: String,
    /// 迁移日志绝对路径（`exvlan-migrate` 默认 `--log` 目标）
    pub log_file: String,
    /// 最近一次备份目录；还没开始备份（失败在检测/备份之前）时为 None
    pub backup_dir: Option<String>,
    /// 备份里的报告文件（`migrate-report.json`，跑完收尾才有）
    pub report_file: Option<String>,
    /// 随包工具应放的位置（`<安装目录>/tools`）：工具被删/被杀软拦时引导用户
    pub tool_dir: Option<String>,
    /// 可直接粘贴到终端的命令（绝对路径 + 提权）；工具缺失时为 None
    pub command: Option<String>,
    /// `migrate.log` 尾部（最近一次运行的原始输出）
    pub log_tail: String,
}
