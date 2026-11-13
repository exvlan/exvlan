//! 一次性迁移闸门（pre.17 专用）。
//!
//! 运行时**不做**迁移/兼容：检测到旧数据就阻塞启动，由用户确认后调用随包的
//! `exvlan-migrate`（它自己会备份/转换/清理/写标记）。下一版移除随包与自动执行。

use std::path::PathBuf;

use serde::Serialize;
use tauri::Manager as _;

#[derive(Debug, Clone, Serialize)]
pub struct MigrateStatus {
    /// 是否有旧数据需要迁移
    pub pending: bool,
    /// 检测到的遗留文件（给用户看清楚）
    pub items: Vec<String>,
    /// 随包迁移工具路径（None = 安装包不完整）
    pub tool: Option<String>,
    /// 是否已完成过迁移
    pub done: bool,
}

/// 旧版本遗留数据（用户侧 + 内核侧的 `.prev`）。
pub fn legacy_items() -> Vec<String> {
    let user = exvlan_control::paths::user_paths();
    let p = exvlan_control::paths::paths();
    let mut out = Vec::new();
    let mut push = |path: PathBuf| {
        if path.exists() {
            out.push(path.display().to_string());
        }
    };
    push(user.data.join("networks.toml"));
    push(user.data.join("service.json"));
    push(user.data.join("config.d"));
    push(user.data.join("ui-state.json"));
    push(user.data.join("ui-state.json.bak"));
    push(p.core.join("exvlan.prev"));
    push(p.core.join("exvlan.exe.prev"));
    out
}

/// 随包迁移工具（Tauri 会保留 resources 里的相对路径，两种形态都试）。
fn tool_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = app.path().resource_dir().ok()?;
    let name = if cfg!(windows) {
        "exvlan-migrate.exe"
    } else {
        "exvlan-migrate"
    };
    let candidates = [
        dir.join("tools").join(name),
        dir.join("resources").join("tools").join(name),
        dir.join(name),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

#[tauri::command]
pub fn migrate_status(app: tauri::AppHandle) -> MigrateStatus {
    let user = exvlan_control::paths::user_paths();
    let done = user.data.join("migrate.done").is_file();
    let items = legacy_items();
    MigrateStatus {
        pending: !done && !items.is_empty(),
        items,
        tool: tool_path(&app).map(|p| p.display().to_string()),
        done,
    }
}

/// 执行随包迁移工具（提权一次），返回它打印的 JSON 报告。
#[tauri::command]
pub async fn migrate_run(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let bin = tool_path(&app).ok_or_else(|| {
        "安装包里没有 exvlan-migrate（安装包不完整），请重新下载安装包".to_string()
    })?;
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let mut cmd = std::process::Command::new(&bin);
        // 提权运行 → 备份/marker 会变成 root 所有，用户读不了自己的备份；
        // 把当前桌面用户 uid 传进去，由工具 chown 回来。
        cmd.args([
            "--yes",
            "--json",
            "--owner-uid",
            &crate::service_client::current_uid().to_string(),
        ]);
        let out = crate::elevate::Command::new(cmd)
            .output()
            .map_err(|e| format!("调用迁移工具失败: {e}"))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        // 工具会写日志到 stderr；把最后一行 JSON 解析出来（前面可能有日志）。
        let json_line = stdout
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with('{'))
            .unwrap_or("");
        let value = if json_line.is_empty() {
            serde_json::json!({"ok": out.status.success()})
        } else {
            serde_json::from_str(json_line)
                .unwrap_or_else(|_| serde_json::json!({"ok": out.status.success()}))
        };
        if !out.status.success() {
            return Err(format!(
                "迁移失败（退出码 {:?}）：{}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("")
            ));
        }
        Ok(value)
    })
    .await
    .map_err(|e| format!("迁移任务失败: {e}"))??;

    crate::logger::record("migrate", format!("done: {result}"));
    Ok(result)
}
