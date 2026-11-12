//! 本地内核看门狗（O 项）。
//!
//! 背景：内核可能卡死或被杀（改配置重建、异常退出、端口被残留进程占住）。
//! 以前只能靠用户点「重装服务」才能恢复 —— 那是把内部实现当成了用户操作。
//!
//! 这里对**本机**内核做轻量探活：连续 `TIMEOUTS_BEFORE_RESTART` 次失败
//! 就通过服务 IPC 做一次原子重启（`kernel.restart`）。
//!
//! 注意：
//! - 只在「目标是本机」时启用（远端目标挂了不该重启本机内核）；
//! - 只在特权服务存在时启用（core 模式下内核由 systemd/GUI supervisor 管）；
//! - 全局只允许一个看门狗线程。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::AppHandle;

/// 连续失败多少次后重启内核（1s 一次探活 → 约 6s）。
const TIMEOUTS_BEFORE_RESTART: u32 = 6;
/// 探活间隔。
const INTERVAL: Duration = Duration::from_secs(1);

static ARMED: AtomicBool = AtomicBool::new(false);

/// 启动看门狗（幂等）。`is_normal_mode` 目前只用于日志。
pub fn arm(app: AppHandle, is_normal_mode: bool) {
    if ARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    if !crate::service_client::is_installed() {
        // core 模式（Linux）：由 systemd/GUI supervisor 负责，别抢别人的活。
        ARMED.store(false, Ordering::SeqCst);
        return;
    }
    std::thread::spawn(move || {
        let mode = if is_normal_mode { "normal" } else { "service" };
        crate::logger::record("watchdog", format!("armed (mode={mode})"));
        let mut failures = 0u32;
        loop {
            std::thread::sleep(INTERVAL);
            if !crate::service_client::is_installed() {
                // 服务被卸载：看门狗退休。
                ARMED.store(false, Ordering::SeqCst);
                crate::logger::record("watchdog", "service gone, disarmed");
                return;
            }
            let ok = probe(&app);
            if ok {
                if failures > 0 {
                    crate::logger::record(
                        "watchdog",
                        format!("kernel responded again after {failures} failure(s)"),
                    );
                }
                failures = 0;
                continue;
            }
            failures += 1;
            if failures < TIMEOUTS_BEFORE_RESTART {
                continue;
            }
            crate::logger::record(
                "watchdog",
                format!("kernel unresponsive x{failures}, restarting"),
            );
            match crate::service_client::restart_kernel() {
                Ok(_) => {
                    crate::logger::record("watchdog", "kernel restarted");
                    failures = 0;
                }
                Err(err) => {
                    crate::logger::record("watchdog", format!("restart failed: {err}"));
                    // 退避，避免在最坏情况下刷屏/打爆服务。
                    std::thread::sleep(Duration::from_secs(15));
                    failures = 0;
                }
            }
        }
    });
}

/// 一次真实 RPC 探活（3s 超时）。用阻塞的 tokio runtime，避免依赖主线程上下文。
fn probe(app: &AppHandle) -> bool {
    let app = app.clone();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return true, // 建不出 runtime 就别误判
    };
    rt.block_on(async move {
        tokio::time::timeout(
            Duration::from_secs(3),
            crate::list_network_instance_ids(app),
        )
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
    })
}
