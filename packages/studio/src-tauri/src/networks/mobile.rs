//! 移动端网络配置：与桌面**同一套语义**（配置目录唯一权威）。
//!
//! - 文件：`<app_config_dir>/config/<id>.toml`；禁用 = `<id>.toml.disabled`
//! - 弹核（FFI）只是执行者：启动时读目录 → 逐条 `run_network_instance(toml)`
//! - 旧 `config/networks.toml` 只在**首次升级**时做一次性转换：转换逻辑**不在
//!   这里**，而是 `exvlan-migrate` 的 lib（经 `exvlan_ffi` 的
//!   `exvlan_migrate_networks_once` 暴露），语义与桌面迁移一致：失败保留源文件、
//!   不写标记、下次重试（见 `docs/migrate-7.3.md` §7）。

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use exvlan_rpc::exvlan_proto::api::manage::NetworkConfig;

const CONFIG_DIR: &str = "config";
const ENABLED_SUFFIX: &str = ".toml";
const DISABLED_SUFFIX: &str = ".toml.disabled";

fn app_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager as _;
    app.path()
        .app_config_dir()
        .map_err(|e| format!("无法定位应用配置目录: {e}"))
}

fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app_dir(app)?.join(CONFIG_DIR))
}

/// 实例 id → 文件名。与服务/桌面同一套白名单（挡路径穿越）。
fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err("实例 id 长度必须在 1..=64 之间".into());
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("实例 id 只允许 [A-Za-z0-9._-]".into());
    }
    if id.starts_with('.') || id.contains("..") {
        return Err("实例 id 不能以 . 开头，也不能包含 ..".into());
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct Saved {
    id: String,
    enabled: bool,
    text: String,
}

fn list_in(dir: &Path) -> Result<Vec<Saved>, String> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(err) => return Err(format!("读取 {} 失败: {err}", dir.display())),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let (id, enabled) = if let Some(id) = name.strip_suffix(DISABLED_SUFFIX) {
            (id.to_string(), false)
        } else if let Some(id) = name.strip_suffix(ENABLED_SUFFIX) {
            (id.to_string(), true)
        } else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        match std::fs::read_to_string(entry.path()) {
            Ok(text) => out.push(Saved { id, enabled, text }),
            Err(err) => crate::logger::record(
                "networks.store",
                format!("读 {} 失败: {err}", entry.path().display()),
            ),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// 原子写：`.tmp` → 校验 → rename，再删掉另一种（启用/禁用）形态。
fn write_in(dir: &Path, id: &str, text: &str, enabled: bool) -> Result<(), String> {
    validate_id(id)?;
    // 与内核一致的最小校验：缺 `network_identity` 的配置内核加载不了。
    let doc: toml::Value = toml::from_str(text).map_err(|e| format!("配置不是合法 TOML: {e}"))?;
    let name = doc
        .get("network_identity")
        .and_then(|v| v.get("network_name"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if name.trim().is_empty() {
        return Err("缺少 [network_identity] network_name（内核无法加载该实例）".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{id}.toml.tmp"));
    std::fs::write(&tmp, text).map_err(|e| format!("写 {} 失败: {e}", tmp.display()))?;
    let suffix = if enabled {
        ENABLED_SUFFIX
    } else {
        DISABLED_SUFFIX
    };
    let dst = dir.join(format!("{id}{suffix}"));
    if let Err(err) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("替换 {} 失败: {err}", dst.display()));
    }
    let other = dir.join(format!(
        "{id}{}",
        if enabled {
            DISABLED_SUFFIX
        } else {
            ENABLED_SUFFIX
        }
    ));
    let _ = std::fs::remove_file(other);
    Ok(())
}

fn read_in(dir: &Path, id: &str) -> Result<Saved, String> {
    for (suffix, enabled) in [(ENABLED_SUFFIX, true), (DISABLED_SUFFIX, false)] {
        let path = dir.join(format!("{id}{suffix}"));
        if path.is_file() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
            return Ok(Saved {
                id: id.to_string(),
                enabled,
                text,
            });
        }
    }
    Err(format!("配置不存在: {id}"))
}

fn delete_in(dir: &Path, id: &str) -> Result<bool, String> {
    let mut removed = false;
    for suffix in [ENABLED_SUFFIX, DISABLED_SUFFIX] {
        let path = dir.join(format!("{id}{suffix}"));
        if path.is_file() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("删除 {} 失败: {e}", path.display()))?;
            removed = true;
        }
    }
    Ok(removed)
}

// 移动端**不能**链接 `exvlan-core`（CI 会断言 Android 依赖树里没有它），所以
// TOML ↔ NetworkConfig 的转换走 FFI 暴露的内部服务 `exvlan.internal.ConfigConvert`
// （与导入/导出、`generate_network_config` 同一份实现，语义与桌面完全一致）。

const CONFIG_CONVERT_SERVICE: &str = "exvlan.internal.ConfigConvert";

/// 与桌面同形：内核 TOML → `NetworkConfig`。
fn from_toml(text: &str) -> Result<NetworkConfig, String> {
    let value = exvlan_rpc::ffi_rpc::call(
        CONFIG_CONVERT_SERVICE,
        "toml_to_config",
        "",
        serde_json::json!({ "tomlConfig": text }),
    )?;
    serde_json::from_value(value).map_err(|e| format!("解析内核配置失败: {e}"))
}

/// 与桌面同形：`NetworkConfig` → 内核 TOML（走内核自己的 `gen_config`）。
fn to_toml(cfg: &NetworkConfig) -> Result<String, String> {
    let payload = serde_json::to_value(cfg).map_err(|e| format!("序列化网络配置失败: {e}"))?;
    let value = exvlan_rpc::ffi_rpc::call(CONFIG_CONVERT_SERVICE, "config_to_toml", "", payload)?;
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "config_to_toml 未返回字符串".to_string())
}

// ---------------------------------------------------------------------------
// 一次性转换（旧 `config/networks.toml` → `<config>/<id>.toml`）
// ---------------------------------------------------------------------------

/// 旧 `config/networks.toml` 的一次性转换。
///
/// 实现**不在运行时**：转换逻辑在 `exvlan-migrate` 的 lib（`mobile` 模块），
/// 经 Android 的 `libexvlan_ffi.so` 暴露成 `exvlan_migrate_networks_once`。
/// 这里只把两个目录传过去 + 记日志（失败保留源、下次调用重试）。
#[cfg(target_os = "android")]
fn migrate_legacy_once(app: &tauri::AppHandle) {
    let (Ok(root), Ok(dir)) = (app_dir(app), config_dir(app)) else {
        return;
    };
    match crate::ffi::migrate_networks_once(&root, &dir) {
        Ok(value) => {
            // `nothing_to_do` 是常态（绝大多数机器没有旧文件），不刷日志。
            let status = value
                .get("report")
                .and_then(|report| report.get("status"))
                .and_then(|status| status.as_str())
                .unwrap_or_default();
            if status != "nothing_to_do" {
                crate::logger::record("networks.migrate", value.to_string());
            }
        }
        // 失败 = 源文件已保留（下一次 get/set 调用会重试）。
        Err(err) => crate::logger::record("networks.migrate", err),
    }
}

/// iOS 没有随包的内核 FFI（无发货产物，CI 只做 `cargo check`）：不迁移，
/// 旧 `networks.toml` 原样保留。
#[cfg(not(target_os = "android"))]
fn migrate_legacy_once(_app: &tauri::AppHandle) {}

// ---------------------------------------------------------------------------
// 命令（与桌面同形，前端契约不变）
// ---------------------------------------------------------------------------

/// 配置目录路径（诊断用）。
#[tauri::command]
pub fn networks_path_cmd(app: tauri::AppHandle) -> Result<String, String> {
    config_dir(&app).map(|p| p.display().to_string())
}

/// 网络列表（JSON 字符串，前端直接 `JSON.parse`）。
#[tauri::command]
pub fn get_networks(app: tauri::AppHandle) -> Result<String, String> {
    migrate_legacy_once(&app);
    let dir = config_dir(&app)?;
    let mut list = Vec::new();
    for saved in list_in(&dir)? {
        match from_toml(&saved.text) {
            Ok(config) => list.push(serde_json::json!({
                "config": config,
                "source": "user",
            })),
            Err(err) => crate::logger::record(
                "networks.store",
                format!("解析 {} 失败，已跳过: {err}", saved.id),
            ),
        }
    }
    serde_json::to_string(&list).map_err(|e| format!("序列化网络列表失败: {e}"))
}

/// 写入网络列表：逐条落盘（已存在的保留启用/禁用状态）。
#[tauri::command]
pub fn set_networks(app: tauri::AppHandle, networks: String) -> Result<(), String> {
    migrate_legacy_once(&app);
    let dir = config_dir(&app)?;
    let value: serde_json::Value =
        serde_json::from_str(&networks).map_err(|e| format!("网络列表不是合法 JSON: {e}"))?;
    let items = value
        .as_array()
        .ok_or_else(|| "网络列表必须是数组".to_string())?;
    for item in items {
        let config: NetworkConfig = serde_json::from_value(
            item.get("config")
                .cloned()
                .ok_or_else(|| "网络条目缺少 config".to_string())?,
        )
        .map_err(|e| format!("网络配置不是合法 NetworkConfig: {e}"))?;
        let id = config
            .instance_id
            .clone()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "网络配置缺少 instance_id".to_string())?;
        let text = to_toml(&config)?;
        let enabled = read_in(&dir, &id).map(|s| s.enabled).unwrap_or(true);
        write_in(&dir, &id, &text, enabled)?;
    }
    Ok(())
}

/// 切换启用/禁用（禁用 = `<id>.toml.disabled`，内核不加载）。
///
/// 移动端的客户端管理器（`src/mobile.rs`）在用户「关闭网络」时会调用这里，
/// 否则「禁用」只存在于内存里，重启就回来了。
pub fn set_enabled(app: &tauri::AppHandle, id: &str, enabled: bool) -> Result<(), String> {
    let dir = config_dir(app)?;
    let saved = read_in(&dir, id)?;
    if saved.enabled == enabled {
        return Ok(());
    }
    write_in(&dir, id, &saved.text, enabled)
}

/// 删除一条配置（供内部/未来 command 使用）。
#[allow(dead_code)]
pub fn delete_network(app: &tauri::AppHandle, id: &str) -> Result<bool, String> {
    delete_in(&config_dir(app)?, id)
}
