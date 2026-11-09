//! 普通模式内核子进程：由 GUI 拉起、随 GUI 退出。
//!
//! 设计原则：**不改内核**。只使用内核已有的参数：
//! `--config-dir` / `--file-log-dir` / `--rpc-portal` / `--rpc-portal-whitelist`。
//!
//! 生命周期：
//! - Linux：`PR_SET_PDEATHSIG`（父进程死亡——含崩溃——内核收到 SIGKILL）
//! - macOS / Windows：GUI 正常退出时主动 kill；崩溃场景由 Phase 2 的
//!   helper / supervisor 兜底（见 docs/decisions-7.3.md）。
//!
//! 权限：普通模式要建 TUN，Linux 用 `setcap`（安装内核时授予
//! `cap_net_admin,cap_net_raw`）即可零弹窗；macOS/Windows 需要特权组件，
//! 未就绪时这里会拉起用户态进程（代理模式可用，TUN 会失败并提示）。

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context as _, bail};
use tauri::Manager as _;

pub struct NormalKernel {
    child: std::process::Child,
    pub rpc_portal: String,
    pub log_dir: PathBuf,
}

impl NormalKernel {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

static NORMAL: Mutex<Option<NormalKernel>> = Mutex::new(None);

/// 确保普通模式内核在跑，返回 RPC 地址（`127.0.0.1:<port>`）。
pub fn ensure(app: &tauri::AppHandle) -> anyhow::Result<String> {
    let mut guard = NORMAL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(kernel) = guard.as_mut() {
        if kernel.alive() {
            return Ok(kernel.rpc_portal.clone());
        }
        crate::logger::record("normal", "child exited, restarting");
    }

    let kernel = spawn(app)?;
    let portal = kernel.rpc_portal.clone();
    crate::logger::record(
        "normal",
        format!(
            "spawned kernel pid={} rpc={} log={}",
            kernel.child.id(),
            portal,
            kernel.log_dir.display()
        ),
    );
    *guard = Some(kernel);
    Ok(portal)
}

fn free_port() -> anyhow::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").context("分配本地端口失败")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn spawn(app: &tauri::AppHandle) -> anyhow::Result<NormalKernel> {
    let kernel = crate::kernel::kernel_path();
    if !kernel.is_file() {
        bail!("内核未安装（{}）", kernel.display());
    }

    let config_dir = crate::paths::normal_config_dir(app).map_err(|e| anyhow::anyhow!(e))?;
    let log_dir = crate::paths::normal_log_dir(app).map_err(|e| anyhow::anyhow!(e))?;
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("创建普通模式配置目录 {} 失败", config_dir.display()))?;
    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("创建普通模式日志目录 {} 失败", log_dir.display()))?;

    let port = free_port()?;
    let rpc_portal = format!("127.0.0.1:{port}");

    let mut cmd = crate::kernel::kernel_command(&kernel);
    cmd.arg("--config-dir")
        .arg(&config_dir)
        .arg("--file-log-dir")
        .arg(&log_dir)
        .arg("--rpc-portal")
        .arg(&rpc_portal)
        .arg("--rpc-portal-whitelist")
        .arg("127.0.0.1/32")
        .arg("--file-log-level")
        .arg("info")
        .arg("--console-log-level")
        .arg("off");

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt as _;
        // 父进程死亡（正常退出或崩溃）→ 内核收到 SIGKILL，不会留孤儿进程。
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("启动普通模式内核失败（{}）", kernel.display()))?;

    Ok(NormalKernel {
        child,
        rpc_portal,
        log_dir,
    })
}

/// 停止普通模式内核（GUI 退出时调用）。
pub fn stop() {
    let mut guard = NORMAL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut kernel) = guard.take() {
        crate::logger::record(
            "normal",
            format!("stopping kernel pid={}", kernel.child.id()),
        );
        let _ = kernel.child.kill();
        let _ = kernel.child.wait();
    }
}

/// 普通模式是否已启动（供诊断）。
pub fn is_running() -> bool {
    let mut guard = NORMAL.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_mut().is_some_and(NormalKernel::alive)
}

/// 普通模式日志目录（供诊断引用）。
pub fn log_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    crate::paths::normal_log_dir(app).ok()
}
