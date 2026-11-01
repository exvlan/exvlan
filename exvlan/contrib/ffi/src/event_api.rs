//! 实例事件转发（P6）：内核事件 → FFI 队列 → 调用方轮询。
//!
//! 设计要点：
//! - 回调线程只做「入队」，不在回调里做重活（避免跨语言回调线程被阻塞/死锁）；
//! - 调用方用 [`poll_instance_events`] 拉取（返回 1 有事件 / 0 空 / -1 错误），
//!   拿到的字符串必须用 `free_string` 释放；
//! - 未注册回调时事件只入队（有上限），由轮询消费。

use std::collections::VecDeque;
use std::ffi::c_char;
use std::sync::{Mutex, OnceLock};

pub type InstanceEventCallback = Option<unsafe extern "C" fn(*const c_char, *mut std::ffi::c_void)>;

const MAX_QUEUE: usize = 256;

struct Registry {
    callback: InstanceEventCallback,
    user_data: usize,
}

fn registry() -> &'static Mutex<Option<Registry>> {
    static REG: OnceLock<Mutex<Option<Registry>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(None))
}

fn queue() -> &'static Mutex<VecDeque<String>> {
    static Q: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// 注册实例事件回调。`user_data` 原样回传给回调。
///
/// # Return
/// `0` 成功；`-1` 失败（当前实现在无锁竞争时不会失败，保留位）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn register_instance_event_callback(
    callback: InstanceEventCallback,
    user_data: *mut std::ffi::c_void,
) -> std::ffi::c_int {
    *registry().lock().unwrap() = Some(Registry {
        callback,
        user_data: user_data as usize,
    });
    0
}

/// 阻塞取出一个待处理事件（JSON 字符串）。
///
/// # Return
/// `1` 写出了一个事件（`*out` 需 `free_string`）；`0` 当前没有事件；`-1` 参数非法。
///
/// # Safety
/// `out` 必须是非空指针。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn poll_instance_events(out: *mut *const c_char) -> std::ffi::c_int {
    if out.is_null() {
        return -1;
    }
    let next = queue().lock().unwrap().pop_front();
    match next {
        Some(json) => match std::ffi::CString::new(json) {
            Ok(c) => {
                unsafe { *out = c.into_raw() };
                1
            }
            Err(_) => -1,
        },
        None => 0,
    }
}

/// 把事件推入队列并（若已注册）触发回调。供内核侧调用。
pub(crate) fn push_event(json: String) {
    {
        let mut q = queue().lock().unwrap();
        if q.len() >= MAX_QUEUE {
            q.pop_front();
        }
        q.push_back(json.clone());
    }

    let (cb, user_data) = {
        let reg = registry().lock().unwrap();
        match reg.as_ref() {
            Some(r) => (r.callback, r.user_data),
            None => (None, 0),
        }
    };
    if let Some(cb) = cb {
        if let Ok(c) = std::ffi::CString::new(json) {
            unsafe { cb(c.as_ptr(), user_data as *mut std::ffi::c_void) };
        }
    }
}

/// 为某个实例启动事件转发任务（DHCP IP / 代理 CIDR 变化）。
pub(crate) fn spawn_forwarder(
    handle: tokio::runtime::Handle,
    instance: std::sync::Arc<exvlan::instance::factory::NativeCoreInstance>,
    inst_id: String,
) {
    handle.spawn(async move {
        let Some(mut rx) = exvlan::instance::factory::subscribe_native_instance_event(&instance)
        else {
            return;
        };
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some(kind) = map_event(&event) {
                        push_event(format!(
                            "{{\"type\":\"{kind}\",\"instance\":\"{inst_id}\"}}"
                        ));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    });
}

fn map_event(event: &exvlan::common::global_ctx::GlobalCtxEvent) -> Option<&'static str> {
    use exvlan::common::global_ctx::GlobalCtxEvent;
    match event {
        GlobalCtxEvent::DhcpIpv4Changed(_, _) => Some("dhcp_ip_changed"),
        GlobalCtxEvent::ProxyCidrsUpdated(_, _) => Some("proxy_cidrs_updated"),
        _ => None,
    }
}
