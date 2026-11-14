//! 移动端占位：移动端没有 `networks.toml`/`service.json` 时代的数据，
//! 也没有特权服务与随包迁移工具，所以永远「无事可做」。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct MigrateStatus {
    pub pending: bool,
    pub items: Vec<String>,
    pub tool: Option<String>,
    pub done: bool,
    /// 桌面端的失败指引；移动端永不 pending，因此没有可指引的内容。
    pub guide: Option<super::MigrateGuide>,
}

#[tauri::command]
pub fn migrate_status() -> MigrateStatus {
    MigrateStatus {
        pending: false,
        items: Vec::new(),
        tool: None,
        done: true,
        guide: None,
    }
}

#[tauri::command]
pub async fn migrate_run() -> Result<serde_json::Value, String> {
    Err("移动端没有一次性迁移".to_string())
}
