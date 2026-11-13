//! 网络配置落盘：唯一权威是内核的 `<config-dir>/<id>.toml`。
//!
//! pre.16 有两份权威（前端 `localStorage` + Rust `networks.toml`），两边漂移
//! 造成了「设置了但没生效 / 列表里多出幽灵网络」这类问题。pre.17 起只认内核
//! 真正加载的那批文件。
//!
//! 读写语义（服务 IPC 优先、裸核本地文件兜底）由
//! [`exvlan_control::config`] 统一实现；这里只做 GUI 侧的薄适配
//! （错误信息带上下文 + 挂到 `error.log`）。

#![cfg(not(any(target_os = "android", target_os = "ios")))]

pub use exvlan_control::config::Saved as SavedConfig;

use exvlan_control::config as svc;

/// 配置目录（诊断面板展示用）。
pub fn dir() -> String {
    match svc::dir() {
        Ok(dir) if !dir.is_empty() => dir,
        Ok(_) => String::new(),
        Err(err) => {
            crate::logger::record("config_store", format!("dir 失败: {err}"));
            String::new()
        }
    }
}

/// 列出所有已保存的网络（启用 + 禁用）。
pub fn list() -> Result<Vec<SavedConfig>, String> {
    svc::list().map_err(|e| {
        crate::logger::record("config_store", format!("list 失败: {e}"));
        e
    })
}

/// 读一条配置。
pub fn read(id: &str) -> Result<SavedConfig, String> {
    svc::read(id)
}

/// 原子写入（服务后端交给服务；core 后端本地写）。
pub fn write(id: &str, text: &str, enabled: bool) -> Result<(), String> {
    svc::write(id, text, enabled).map_err(|e| {
        crate::logger::record("config_store", format!("write {id} 失败: {e}"));
        e
    })
}

/// 删除（启用/禁用两种形态都删）。不存在不算错误。
pub fn delete(id: &str) -> Result<(), String> {
    svc::delete(id).map(|_| ()).map_err(|e| {
        crate::logger::record("config_store", format!("delete {id} 失败: {e}"));
        e
    })
}

/// 切换启用/禁用。
pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    svc::set_enabled(id, enabled).map_err(|e| {
        crate::logger::record(
            "config_store",
            format!("set_enabled {id}={enabled} 失败: {e}"),
        );
        e
    })
}
