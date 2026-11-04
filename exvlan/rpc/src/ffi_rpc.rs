//! 通过 C ABI 调用内核 RPC（移动端）。
//!
//! 桌面走独立守护进程的 TCP RPC；移动端内核在应用进程内，只有 `call_json_rpc`
//! 这一条通道（protobuf JSON 请求/响应）。本模块把它包装成 Rust 友好的形态，
//! 供 `exvlan-studio` 的 Android 分支替换掉 `BidirectRpcManager` 的 tunnel 传输。
//!
//! 启用方式：`exvlan-rpc` 开启 feature `ffi-rpc`（默认关闭，桌面不编译）。
//!
//! 注意：`call_json_rpc` 的符号由 `libexvlan_ffi.so` 提供，必须在运行时加载
//! （Android 通过 jniLibs 打包；其他平台链接期会缺符号，故默认不启用）。

#![cfg(feature = "ffi-rpc")]

use std::ffi::{CStr, CString, c_char, c_int};
use std::ptr;

unsafe extern "C" {
    fn call_json_rpc(
        service_name: *const c_char,
        method_name: *const c_char,
        domain_name: *const c_char,
        payload_json: *const c_char,
        out_response_json: *mut *const c_char,
    ) -> c_int;
    fn get_error_msg(out: *mut *const c_char);
    fn free_string(s: *const c_char);
}

/// 读取上一次 FFI 调用的错误信息（同一线程）。
pub fn last_error() -> String {
    unsafe {
        let mut out: *const c_char = ptr::null();
        get_error_msg(&mut out);
        if out.is_null() {
            return "(unknown)".to_string();
        }
        let msg = CStr::from_ptr(out).to_string_lossy().into_owned();
        free_string(out);
        msg
    }
}

fn cstring(s: &str, field: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| format!("{field} 含 NUL 字节"))
}

struct Job {
    service: String,
    method: String,
    domain: String,
    payload: serde_json::Value,
    reply: std::sync::mpsc::Sender<Result<serde_json::Value, String>>,
}

/// 专用的 FFI 调用线程。
///
/// 内核侧的 `call_json_rpc` 内部会用 `Runtime::block_on` 驱动异步 RPC；
/// 如果直接在当前线程调用，而当前线程正处于 Tauri 的 tokio 运行时上下文里
/// （所有 `#[tauri::command] async fn` 都是），tokio 会 panic：
/// `Cannot start a runtime from within a runtime`。移动端 release 又是
/// `panic = "abort"`，于是表现为**进入页面即闪退**。
///
/// 因此凡是「在运行时上下文里」调用，一律转交这个普通线程执行。
fn worker() -> Option<&'static std::sync::mpsc::Sender<Job>> {
    static WORKER: std::sync::OnceLock<Option<std::sync::mpsc::Sender<Job>>> =
        std::sync::OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("ffi-rpc".to_string())
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        let result = call_raw(&job.service, &job.method, &job.domain, job.payload);
                        let _ = job.reply.send(result);
                    }
                })
                .ok()?;
            Some(tx)
        })
        .as_ref()
}

/// 调用一个 RPC 方法（protobuf JSON 编码）。
///
/// `domain` 为空字符串表示全局域；实例级方法需传实例 ID。
///
/// 线程语义：在 tokio 运行时上下文里会自动转交 `ffi-rpc` 专用线程，
/// 因为内核侧会嵌套 `block_on`（见 [`worker`]）。
pub fn call(
    service: &str,
    method: &str,
    domain: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if tokio::runtime::Handle::try_current().is_ok()
        && let Some(tx) = worker()
    {
        let (reply, rx) = std::sync::mpsc::channel();
        tx.send(Job {
            service: service.to_string(),
            method: method.to_string(),
            domain: domain.to_string(),
            payload,
            reply,
        })
        .map_err(|_| "FFI 调用线程不可用".to_string())?;
        // 超时兜底：内核侧若卡住（例如等锁），不能让 GUI 命令无限等待（Android 会 ANR）。
        return rx
            .recv_timeout(std::time::Duration::from_secs(15))
            .unwrap_or_else(|err| match err {
                std::sync::mpsc::RecvTimeoutError::Timeout => {
                    Err("FFI 调用超时（15s），内核可能无响应".to_string())
                }
                std::sync::mpsc::RecvTimeoutError::Disconnected => {
                    Err("FFI 调用线程已退出".to_string())
                }
            });
    }

    call_raw(service, method, domain, payload)
}

fn call_raw(
    service: &str,
    method: &str,
    domain: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let service = cstring(service, "service")?;
    let method = cstring(method, "method")?;
    let domain = cstring(domain, "domain")?;
    let payload = cstring(&payload.to_string(), "payload")?;

    let mut out: *const c_char = ptr::null();
    let rc = unsafe {
        call_json_rpc(
            service.as_ptr(),
            method.as_ptr(),
            domain.as_ptr(),
            payload.as_ptr(),
            &mut out,
        )
    };
    if rc != 0 {
        return Err(last_error());
    }
    if out.is_null() {
        return Ok(serde_json::Value::Null);
    }
    let text = unsafe { CStr::from_ptr(out) }
        .to_string_lossy()
        .into_owned();
    unsafe { free_string(out) };
    serde_json::from_str(&text).map_err(|e| format!("解析 RPC 响应失败: {e}"))
}

/// 便捷封装：请求体用 `Serialize`，响应反序列化为 `T`。
pub fn call_as<T: serde::de::DeserializeOwned>(
    service: &str,
    method: &str,
    domain: &str,
    payload: impl serde::Serialize,
) -> Result<T, String> {
    let payload = serde_json::to_value(payload).map_err(|e| format!("序列化请求失败: {e}"))?;
    let value = call(service, method, domain, payload)?;
    serde_json::from_value(value).map_err(|e| format!("反序列化响应失败: {e}"))
}

/// 常用的服务名。
pub const SVC_LOGGER: &str = "api.logger.LoggerRpcService";
pub const SVC_CONFIG: &str = "api.config.ConfigRpc";
pub const SVC_VPN_PORTAL: &str = "api.instance.VpnPortalRpc";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cstring_rejects_nul() {
        assert!(cstring("a\0b", "x").is_err());
        assert!(cstring("ok", "x").is_ok());
    }

    #[test]
    fn service_names_are_stable() {
        assert_eq!(SVC_LOGGER, "api.logger.LoggerRpcService");
        assert_eq!(SVC_CONFIG, "api.config.ConfigRpc");
    }
}
