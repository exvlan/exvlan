//! 一次性迁移入口（Android App 首启调用）。
//!
//! 桌面有随包的 `exvlan-migrate` 二进制，Android 跑不了；迁移核心抽在
//! `exvlan-migrate` 的 lib 里，这里只做 C ABI 包装（`docs/decisions-7.3.md` §14.15）。
//!
//! 约定：
//! - 入参是两个 NUL 结尾的 UTF-8 路径；返回值是**本库分配的** JSON 字符串，
//!   调用方必须用 `free_string` 释放；
//! - 返回值：`{"ok":true,"report":{…}}` 或 `{"ok":false,"error":"…"}`；
//! - **跨 FFI 不得 panic**：内部 `catch_unwind`，panic 也折成
//!   `{"ok":false,"error":"… panicked …"}`（panic payload 只取字符串内容）。

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use serde_json::{Value, json};

/// 一次性把旧 `<config_dir>/networks.toml` 迁移成 `<config_dir>/<id>.toml`。
///
/// 语义与桌面一致：一次性、失败保留源、幂等；「先算后写、失败回滚」就是移动端
/// 内置的 dry-run（见 `exvlan_migrate::mobile`）。
///
/// # Safety
/// `app_config_dir` 与 `config_dir` 必须是非空、以 NUL 结尾的 UTF-8 路径。
/// 返回的指针由本库分配，调用方必须用 `free_string` 释放（可传 `null`）。
///
/// # Return
/// 返回 JSON 字符串；本函数不会 panic。极端情况下（JSON 含 NUL 等）返回
/// `null`，调用方按失败处理即可。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exvlan_migrate_networks_once(
    app_config_dir: *const c_char,
    config_dir: *const c_char,
) -> *mut c_char {
    let value = guard(|| {
        // SAFETY: 调用方保证指针有效；null 在这里先挡掉（解引用 null 是 UB）。
        let app = unsafe { read_path(app_config_dir, "app_config_dir") }?;
        let config = unsafe { read_path(config_dir, "config_dir") }?;
        let report = exvlan_migrate::mobile::migrate_networks_once(&app, &config)?;
        Ok(json!({"ok": true, "report": report}))
    });
    to_c_string(&value)
}

/// 把「算出来的结果 / 失败 / panic」统一折成 JSON（**永远不抛**）。
fn guard<F>(f: F) -> Value
where
    F: FnOnce() -> Result<Value, String>,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => json!({"ok": false, "error": error}),
        Err(payload) => json!({
            "ok": false,
            "error": format!("migrate panicked: {}", panic_message(payload.as_ref())),
        }),
    }
}

fn to_c_string(value: &Value) -> *mut c_char {
    match CString::new(value.to_string()) {
        Ok(text) => text.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

unsafe fn read_path(ptr: *const c_char, name: &str) -> Result<PathBuf, String> {
    if ptr.is_null() {
        return Err(format!("{name} is null"));
    }
    // SAFETY: 由调用方保证是 NUL 结尾的 C 字符串（见函数文档的 Safety 段）。
    let raw = unsafe { CStr::from_ptr(ptr) };
    raw.to_str()
        .map(PathBuf::from)
        .map_err(|err| format!("{name} is not valid UTF-8: {err}"))
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn temp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-ffi-migrate-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    const LEGACY: &str = r#"schema = 1

[[network]]
[network.config]
instance_id = "1467ec85-9cf8-47bc-8c06-8dc320e9b9c5"
network_name = "exvlan"
network_secret = "s3cret"
peer_urls = ["tcp://192.0.2.10:11010"]
networking_method = 1
"#;

    /// 调一次 C ABI 入口并取回 JSON（调用方视角：`free_string` 释放）。
    fn call(app: &Path, config: &Path) -> Value {
        let app = CString::new(app.display().to_string()).unwrap();
        let config = CString::new(config.display().to_string()).unwrap();
        unsafe {
            let out = exvlan_migrate_networks_once(app.as_ptr(), config.as_ptr());
            assert!(!out.is_null(), "必须返回字符串（不得返回 null）");
            let text = CStr::from_ptr(out).to_string_lossy().into_owned();
            crate::error::free_string(out);
            serde_json::from_str(&text).expect("返回值必须是 JSON")
        }
    }

    #[test]
    fn returns_ok_report_after_converting() {
        let root = temp("ok");
        let app = root.join("app");
        let config = app.join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("networks.toml"), LEGACY).unwrap();

        let value = call(&app, &config);
        assert_eq!(value["ok"], true, "{value}");
        assert_eq!(value["report"]["status"], "converted", "{value}");
        assert_eq!(value["report"]["converted"], 1);
        assert!(
            config
                .join("1467ec85-9cf8-47bc-8c06-8dc320e9b9c5.toml")
                .is_file()
        );
        assert!(!config.join("networks.toml").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_to_do_is_ok() {
        let root = temp("idle");
        let value = call(&root.join("app"), &root.join("app").join("config"));
        assert_eq!(value["ok"], true, "{value}");
        assert_eq!(value["report"]["status"], "nothing_to_do", "{value}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failure_keeps_source_and_reports_error() {
        let root = temp("fail");
        let app = root.join("app");
        let config = app.join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("networks.toml"), "[[network]\n").unwrap();

        let value = call(&app, &config);
        assert_eq!(value["ok"], false, "{value}");
        assert!(
            value["error"].as_str().unwrap_or_default().contains("解析"),
            "{value}"
        );
        assert!(config.join("networks.toml").is_file(), "失败必须保留源");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn null_paths_are_an_error_not_a_panic() {
        unsafe {
            let out = exvlan_migrate_networks_once(std::ptr::null(), std::ptr::null());
            assert!(!out.is_null());
            let text = CStr::from_ptr(out).to_string_lossy().into_owned();
            crate::error::free_string(out);
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["ok"], false, "{value}");
            assert!(
                value["error"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("is null"),
                "{value}"
            );
        }
    }

    #[test]
    fn panic_is_folded_into_json() {
        let value = guard(|| panic!("boom"));
        assert_eq!(value["ok"], false, "{value}");
        let error = value["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("panicked") && error.contains("boom"),
            "{value}"
        );
    }
}
