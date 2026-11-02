//! Android 端：通过 C ABI 调用**预编译**内核库（`libexvlan_ffi.so`）。
//!
//! 目的：让 studio 的移动构建不再编译整颗内核（内核以 core CI 产出的 .so 形式
//! 随 APK 打包）。桌面端不用这套（桌面走独立守护进程 + RPC）。
//!
//! 约定：
//! - 所有函数返回 `0` 成功 / `-1` 失败；失败时用 `get_error_msg` 取原因。
//! - 字符串均为 UTF-8 C 字符串；`call_json_rpc` 的输出需用 `free_string` 释放。
//!
//! 目前仅提供绑定与安全的 Rust 包装，尚未替换现有调用点（见 `P6` 计划）。

#![cfg(target_os = "android")]
#![allow(dead_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;

#[repr(C)]
pub struct KeyValuePair {
    pub key: *const c_char,
    pub value: *const c_char,
}

pub type ConfigServerEventCallback = Option<unsafe extern "C" fn(*const c_char, *mut c_void)>;

unsafe extern "C" {
    fn run_network_instance(cfg_str: *const c_char) -> c_int;
    fn retain_network_instance(inst_names: *const *const c_char, length: usize) -> c_int;
    fn delete_network_instance(inst_names: *const *const c_char, length: usize) -> c_int;
    fn list_instance(infos: *mut KeyValuePair, max_length: usize) -> c_int;
    fn collect_network_infos(infos: *mut KeyValuePair, max_length: usize) -> c_int;
    fn set_tun_fd(inst_name: *const c_char, fd: c_int) -> c_int;
    fn call_json_rpc(
        service_name: *const c_char,
        method_name: *const c_char,
        domain_name: *const c_char,
        payload_json: *const c_char,
        out_response_json: *mut *const c_char,
    ) -> c_int;
    fn start_config_server_client(
        config_server_url: *const c_char,
        hostname: *const c_char,
        machine_id: *const c_char,
        secure_mode: bool,
        callback: ConfigServerEventCallback,
        user_data: *mut c_void,
    ) -> c_int;
    fn stop_config_server_client() -> c_int;
    fn is_config_server_client_connected() -> c_int;
    fn get_error_msg(out: *mut *const c_char);
    fn free_string(s: *const c_char);
}

/// 读取上一次 FFI 调用的错误信息（同线程）。
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

fn to_cstring(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| "字符串包含 NUL 字节".to_string())
}

fn check(rc: c_int) -> Result<(), String> {
    if rc == 0 { Ok(()) } else { Err(last_error()) }
}

/// 启动一个网络实例（`cfg_str` 为 TOML 配置）。
pub fn run_instance(cfg_toml: &str) -> Result<(), String> {
    let cfg = to_cstring(cfg_toml)?;
    unsafe { check(run_network_instance(cfg.as_ptr())) }
}

/// 保留（停止）指定实例。
pub fn retain_instances(names: &[String]) -> Result<(), String> {
    let cs: Vec<CString> = names
        .iter()
        .map(|n| to_cstring(n))
        .collect::<Result<_, _>>()?;
    let ptrs: Vec<*const c_char> = cs.iter().map(|c| c.as_ptr()).collect();
    unsafe { check(retain_network_instance(ptrs.as_ptr(), ptrs.len())) }
}

/// 删除指定实例。
pub fn delete_instances(names: &[String]) -> Result<(), String> {
    let cs: Vec<CString> = names
        .iter()
        .map(|n| to_cstring(n))
        .collect::<Result<_, _>>()?;
    let ptrs: Vec<*const c_char> = cs.iter().map(|c| c.as_ptr()).collect();
    unsafe { check(delete_network_instance(ptrs.as_ptr(), ptrs.len())) }
}

/// 列出实例名。
pub fn list_instances(max: usize) -> Result<Vec<String>, String> {
    let mut buf: Vec<KeyValuePair> = (0..max)
        .map(|_| KeyValuePair {
            key: ptr::null(),
            value: ptr::null(),
        })
        .collect();
    let n = unsafe { list_instance(buf.as_mut_ptr(), buf.len()) };
    if n < 0 {
        return Err(last_error());
    }
    let mut out = Vec::with_capacity(n as usize);
    for item in buf.iter().take(n as usize) {
        // key（实例名）与 value（实例 id）都由内核分配，取出后必须 free_string。
        if !item.key.is_null() {
            out.push(
                unsafe { CStr::from_ptr(item.key) }
                    .to_string_lossy()
                    .into_owned(),
            );
            unsafe { free_string(item.key) };
        }
        if !item.value.is_null() {
            unsafe { free_string(item.value) };
        }
    }
    Ok(out)
}

/// 采集运行中实例的网络信息（实例名, JSON）。
pub fn collect_running_network_infos(max: usize) -> Result<Vec<(String, String)>, String> {
    let mut buf: Vec<KeyValuePair> = (0..max)
        .map(|_| KeyValuePair {
            key: ptr::null(),
            value: ptr::null(),
        })
        .collect();
    let n = unsafe { collect_network_infos(buf.as_mut_ptr(), buf.len()) };
    if n < 0 {
        return Err(last_error());
    }
    let mut out = Vec::with_capacity(n as usize);
    for item in buf.iter().take(n as usize) {
        if item.key.is_null() || item.value.is_null() {
            continue;
        }
        let name = unsafe { CStr::from_ptr(item.key) }
            .to_string_lossy()
            .into_owned();
        let info = unsafe { CStr::from_ptr(item.value) }
            .to_string_lossy()
            .into_owned();
        unsafe {
            free_string(item.key);
            free_string(item.value);
        }
        out.push((name, info));
    }
    Ok(out)
}

/// 把 TUN fd 交给指定实例。
pub fn attach_tun_fd(inst_name: &str, fd: i32) -> Result<(), String> {
    let name = to_cstring(inst_name)?;
    unsafe { check(set_tun_fd(name.as_ptr(), fd)) }
}

/// 取一个内核事件（`{"type":"dhcp_ip_changed"|"proxy_cidrs_updated","instance":"..."}`）。
pub fn poll_event() -> Option<String> {
    unsafe extern "C" {
        fn poll_instance_events(out: *mut *const c_char) -> c_int;
    }
    let mut out: *const c_char = ptr::null();
    let rc = unsafe { poll_instance_events(&mut out) };
    if rc != 1 || out.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(out) }
        .to_string_lossy()
        .into_owned();
    unsafe { free_string(out) };
    Some(text)
}

/// 通过内核的 JSON-RPC 桥调用方法（移动端的 RPC 客户端将基于此实现）。
pub fn json_rpc(
    service: &str,
    method: &str,
    domain: &str,
    payload_json: &str,
) -> Result<String, String> {
    let service = to_cstring(service)?;
    let method = to_cstring(method)?;
    let domain = to_cstring(domain)?;
    let payload = to_cstring(payload_json)?;
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
        return Ok(String::new());
    }
    let resp = unsafe { CStr::from_ptr(out) }
        .to_string_lossy()
        .into_owned();
    unsafe { free_string(out) };
    Ok(resp)
}

/// 启动 config-server 客户端（P6 之后用于替代桌面端的 web_client）。
pub fn start_config_server(
    url: &str,
    hostname: &str,
    machine_id: &str,
    secure_mode: bool,
    callback: ConfigServerEventCallback,
    user_data: *mut c_void,
) -> Result<(), String> {
    let url = to_cstring(url)?;
    let hostname = to_cstring(hostname)?;
    let machine_id = to_cstring(machine_id)?;
    unsafe {
        check(start_config_server_client(
            url.as_ptr(),
            hostname.as_ptr(),
            machine_id.as_ptr(),
            secure_mode,
            callback,
            user_data,
        ))
    }
}

pub fn stop_config_server() -> Result<(), String> {
    unsafe { check(stop_config_server_client()) }
}

pub fn config_server_connected() -> bool {
    unsafe { is_config_server_client_connected() == 1 }
}
