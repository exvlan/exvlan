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

/// 调用一个 RPC 方法（protobuf JSON 编码）。
///
/// `domain` 为空字符串表示全局域；实例级方法需传实例 ID。
pub fn call(
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

/// 已确认可经 FFI 调用的服务名（内核会拒绝 `api.manage.WebClientService`）。
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
