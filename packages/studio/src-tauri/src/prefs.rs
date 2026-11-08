//! 用户偏好：`prefs.toml`（唯一权威）。
//!
//! 背景：以前设置分散在 WebView `localStorage` 与 `ui-state.json` 两处，
//! 两边不同步时表现为「切了服务模式，重开又变普通模式」。现在：
//!
//! - 文件由 Rust 独占读写，前端只通过 `get_ui_state` / `set_ui_state` 访问
//! - 写入采用「临时文件 + rename」，写完立即读回校验
//! - 首次启动自动把旧的 `ui-state.json` 迁移过来
//!
//! 路径：
//! - macOS / Windows / Android：`<app_config_dir>/prefs.toml`
//! - Linux：`$XDG_STATE_HOME/era.exvlan.studio/prefs.toml`
//!   （缺省 `~/.local/state/era.exvlan.studio/prefs.toml`）

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager as _;

const APP_DIR: &str = crate::paths::APP_ID;
const FILE_NAME: &str = "prefs.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Prefs {
    #[serde(default)]
    app: AppSection,
    #[serde(default)]
    ui: UiSection,
    #[serde(default)]
    diag: DiagSection,
    /// 未知键原样保留，避免升级丢数据。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    misc: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AppSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lang: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    log_level: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UiSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_network_instance_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DiagSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_upload_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

/// Linux 的 state 目录（XDG）。Android/iOS 走 app_config_dir，不用这里。
#[cfg(target_os = "linux")]
fn linux_state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir).join(APP_DIR));
    }
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
    Some(
        PathBuf::from(home)
            .join(".local")
            .join("state")
            .join(APP_DIR),
    )
}

/// 偏好文件路径（不创建目录）。
pub fn prefs_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        linux_state_dir()
            .map(|d| d.join(FILE_NAME))
            .ok_or_else(|| "无法定位 $XDG_STATE_HOME / $HOME".to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(|e| format!("无法定位应用配置目录: {e}"))?;
        Ok(dir.join(FILE_NAME))
    }
}

fn read_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// 读取偏好；文件损坏时返回默认值（并保留原文件供排查）。
fn load(app: &tauri::AppHandle) -> Prefs {
    let Ok(path) = prefs_path(app) else {
        return Prefs::default();
    };
    let Some(text) = read_file(&path) else {
        return Prefs::default();
    };
    match toml::from_str::<Prefs>(&text) {
        Ok(prefs) => prefs,
        Err(err) => {
            crate::error_log::record("prefs", format!("解析 {} 失败: {err}", path.display()));
            Prefs::default()
        }
    }
}

/// 原子写入 + 读回校验。
fn save(app: &tauri::AppHandle, prefs: &Prefs) -> Result<(), String> {
    let path = prefs_path(app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("创建配置目录失败（{}）: {e}", dir.display()))?;
    }
    let text = toml::to_string_pretty(prefs).map_err(|e| format!("序列化 prefs 失败: {e}"))?;
    if text.trim().is_empty() {
        return Err("序列化 prefs 得到空内容，拒绝写入".to_string());
    }

    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text).map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("替换 {} 失败: {e}", path.display())
    })?;

    // 读回校验：写不进去（权限/只读盘）必须立刻暴露，而不是下次启动才发现。
    let back = read_file(&path).unwrap_or_default();
    if back != text {
        return Err(format!(
            "prefs 写入校验失败（{}）：读回内容与写入不一致",
            path.display()
        ));
    }
    Ok(())
}

fn map_key(prefs: &Prefs, key: &str) -> Option<String> {
    match key {
        "app_mode" => prefs.app.mode.clone(),
        "lang" => prefs.app.lang.clone(),
        "log_level" => prefs.app.log_level.clone(),
        "last_network_instance_id" => prefs.ui.last_network_instance_id.clone(),
        "diag_last_upload" => prefs.diag.last_upload_url.clone(),
        other => prefs.misc.get(other).cloned(),
    }
}

fn set_key(prefs: &mut Prefs, key: &str, value: Option<String>) {
    match key {
        "app_mode" => prefs.app.mode = value,
        "lang" => prefs.app.lang = value,
        "log_level" => prefs.app.log_level = value,
        "last_network_instance_id" => prefs.ui.last_network_instance_id = value,
        "diag_last_upload" => prefs.diag.last_upload_url = value,
        other => match value {
            Some(v) => {
                prefs.misc.insert(other.to_string(), v);
            }
            None => {
                prefs.misc.remove(other);
            }
        },
    }
}

/// 旧 `ui-state.json` 迁移（只做一次；迁移后改名为 `.bak`）。
fn migrate_legacy(app: &tauri::AppHandle) -> Option<Prefs> {
    let dir = app.path().app_config_dir().ok()?;
    let legacy = dir.join("ui-state.json");
    if !legacy.is_file() && dir.join("ui-state.json.bak").is_file() {
        // 7.1 曾把它改名为 .bak；按约定只记录、不自动恢复。
        crate::error_log::record(
            "prefs",
            "found ui-state.json.bak (7.1 damage); snapshot not restored (rollback only)",
        );
    }
    let text = std::fs::read_to_string(&legacy).ok()?;
    let map: BTreeMap<String, String> = serde_json::from_str(&text).ok()?;
    if map.is_empty() {
        return None;
    }
    let mut prefs = Prefs::default();
    for (key, value) in map {
        set_key(&mut prefs, &key, Some(value));
    }
    // 诊断上传链接在旧版本是 JSON `{at, urls}`，这里转换一下。
    if let Some(raw) = prefs.diag.last_upload_url.clone()
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw)
    {
        let urls = v.get("urls").and_then(|u| u.as_str()).map(str::to_owned);
        let at = v.get("at").and_then(|u| u.as_str()).map(str::to_owned);
        prefs.diag.last_upload_url = urls;
        prefs.diag.updated_at = at;
    }
    if crate::ENABLE_LEGACY_CLEANUP {
        let _ = std::fs::rename(&legacy, dir.join("ui-state.json.bak"));
    } else {
        // preview 期间**保留** ui-state.json（回退到旧版本仍可读取）。
        crate::error_log::record("prefs", format!("snapshot kept: {}", legacy.display()));
    }
    Some(prefs)
}

fn load_or_migrate(app: &tauri::AppHandle) -> Prefs {
    let Some(path) = prefs_path(app).ok() else {
        return Prefs::default();
    };
    if path.is_file() {
        return load(app);
    }
    if let Some(prefs) = migrate_legacy(app) {
        match save(app, &prefs) {
            Ok(()) => {
                crate::error_log::record("prefs", "migrated ui-state.json → prefs.toml");
                return prefs;
            }
            Err(err) => crate::error_log::record("prefs", format!("迁移失败: {err}")),
        }
    }
    Prefs::default()
}

/// 读取全部偏好（前端启动时回填）。
#[tauri::command]
pub fn get_ui_state(app: tauri::AppHandle) -> BTreeMap<String, String> {
    let prefs = load_or_migrate(&app);
    let mut out = BTreeMap::new();
    for key in [
        "app_mode",
        "lang",
        "log_level",
        "last_network_instance_id",
        "diag_last_upload",
    ] {
        if let Some(value) = map_key(&prefs, key) {
            out.insert(key.to_string(), value);
        }
    }
    for (key, value) in &prefs.misc {
        out.insert(key.clone(), value.clone());
    }
    out
}

/// 写入/删除单个键（`value == None` 表示删除）。
#[tauri::command]
pub fn set_ui_state(
    app: tauri::AppHandle,
    key: String,
    value: Option<String>,
) -> Result<(), String> {
    let mut prefs = load_or_migrate(&app);
    set_key(&mut prefs, &key, value);
    if key == "diag_last_upload" {
        prefs.diag.updated_at = Some(crate::error_log::timestamp());
    }
    save(&app, &prefs)
}

/// 偏好文件路径（诊断用）。
#[tauri::command]
pub fn prefs_path_cmd(app: tauri::AppHandle) -> Result<String, String> {
    prefs_path(&app).map(|p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_round_trip_keeps_known_and_unknown_keys() {
        let mut prefs = Prefs::default();
        set_key(&mut prefs, "app_mode", Some(r#"{"mode":"service"}"#.into()));
        set_key(&mut prefs, "lang", Some("cn".into()));
        set_key(&mut prefs, "last_network_instance_id", Some("id-1".into()));
        set_key(&mut prefs, "custom_key", Some("custom".into()));

        let text = toml::to_string_pretty(&prefs).unwrap();
        let back: Prefs = toml::from_str(&text).unwrap();

        assert_eq!(
            map_key(&back, "app_mode").as_deref(),
            Some(r#"{"mode":"service"}"#)
        );
        assert_eq!(map_key(&back, "lang").as_deref(), Some("cn"));
        assert_eq!(
            map_key(&back, "last_network_instance_id").as_deref(),
            Some("id-1")
        );
        assert_eq!(map_key(&back, "custom_key").as_deref(), Some("custom"));
    }

    #[test]
    fn set_key_removes_value() {
        let mut prefs = Prefs::default();
        set_key(&mut prefs, "lang", Some("cn".into()));
        set_key(&mut prefs, "lang", None);
        assert!(map_key(&prefs, "lang").is_none());
    }
}
