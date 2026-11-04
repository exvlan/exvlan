//! UI 状态的**后端持久化**。
//!
//! 背景：Android WebView 的 `localStorage` 不可靠（重启后丢失），导致语言、日志级别、
//! 模式等设置每次都被重置。这里把 UI 状态落到应用配置目录的 `ui-state.json`，
//! 启动时由前端回填到 localStorage。
//!
//! 采用「键值 map」而非固定结构：新增设置项无需改后端。

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use tauri::Manager as _;

fn state_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| {
        let msg = format!("无法定位应用配置目录: {e}");
        crate::error_log::record("ui_state", &msg);
        msg
    })?;
    std::fs::create_dir_all(&dir).map_err(|e| {
        let msg = format!("创建配置目录失败（{}）: {e}", dir.display());
        crate::error_log::record("ui_state", &msg);
        msg
    })?;
    Ok(dir.join("ui-state.json"))
}

fn load_map(app: &tauri::AppHandle) -> BTreeMap<String, String> {
    let Ok(path) = state_path(app) else {
        return BTreeMap::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => BTreeMap::new(),
    }
}

fn save_map(app: &tauri::AppHandle, map: &BTreeMap<String, String>) -> Result<(), String> {
    let path = state_path(app)?;
    let text = serde_json::to_string_pretty(map).map_err(|e| format!("序列化失败: {e}"))?;
    // 原子写：先写临时文件再 rename，避免半截文件
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, text) {
        let msg = format!("写入 {} 失败: {e}", tmp.display());
        crate::error_log::record("ui_state", &msg);
        return Err(msg);
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let msg = format!("替换 {} 失败: {e}", path.display());
        crate::error_log::record("ui_state", &msg);
        return Err(msg);
    }
    Ok(())
}

/// 读取全部 UI 状态（前端启动时回填 localStorage）。
#[tauri::command]
pub fn get_ui_state(app: tauri::AppHandle) -> BTreeMap<String, String> {
    load_map(&app)
}

/// 写入/删除单个键（`value == None` 表示删除）。
#[tauri::command]
pub fn set_ui_state(
    app: tauri::AppHandle,
    key: String,
    value: Option<String>,
) -> Result<(), String> {
    let mut map = load_map(&app);
    match value {
        Some(v) => {
            map.insert(key, v);
        }
        None => {
            map.remove(&key);
        }
    }
    save_map(&app, &map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_round_trip_is_json() {
        let mut map = BTreeMap::new();
        map.insert("lang".to_string(), "cn".to_string());
        let text = serde_json::to_string(&map).unwrap();
        let back: BTreeMap<String, String> = serde_json::from_str(&text).unwrap();
        assert_eq!(back.get("lang"), Some(&"cn".to_string()));
    }
}
