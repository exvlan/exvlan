//! 普通模式内核子进程：由 GUI 拉起、随 GUI 退出。
//!
//! 自 7.3 起 macOS / Windows 的普通模式也由 `exvlan-service` 托管
//! （见 `service_client`），这里只剩 **Linux**：
//! 不引入常驻特权进程，用内核二进制的 `setcap cap_net_admin,cap_net_raw`
//! 获得建 TUN 的能力，靠 `PR_SET_PDEATHSIG` 保证 GUI 死亡（含崩溃）时
//! 内核被 SIGKILL，不留孤儿进程。
//!
//! 设计原则：**不改内核**。只使用内核已有的参数：
//! `--config-dir` / `--file-log-dir` / `--rpc-portal` / `--rpc-portal-whitelist`。

#![cfg(not(any(target_os = "macos", target_os = "windows")))]

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context as _, bail};

pub struct NormalKernel {
    child: std::process::Child,
    /// 拉起时刻与次数：用于退避重启与「反复退出」判定。
    spawned_at: std::time::Instant,
    pub rpc_portal: String,
    pub log_dir: PathBuf,
}

impl NormalKernel {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }
}

/// 60 秒内最多重启 3 次；返回本次是第几次（从 0 开始）。
fn allow_restart() -> Option<usize> {
    static RESTARTS: Mutex<Vec<std::time::Instant>> = Mutex::new(Vec::new());
    let now = std::time::Instant::now();
    let mut guard = RESTARTS.lock().unwrap_or_else(|e| e.into_inner());
    guard.retain(|at| now.duration_since(*at) < std::time::Duration::from_secs(60));
    if guard.len() >= 3 {
        return None;
    }
    guard.push(now);
    Some(guard.len() - 1)
}

static NORMAL: Mutex<Option<NormalKernel>> = Mutex::new(None);

/// 确保普通模式内核在跑，返回 RPC 地址（`127.0.0.1:<port>`）。
pub fn ensure(app: &tauri::AppHandle) -> anyhow::Result<String> {
    let mut guard = NORMAL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(kernel) = guard.as_mut() {
        if kernel.alive() {
            return Ok(kernel.rpc_portal.clone());
        }
        let lived = kernel.spawned_at.elapsed();
        let log_dir = kernel.log_dir.clone();
        crate::logger::record(
            "normal",
            format!(
                "kernel exited (pid={}, lived={}ms), restarting",
                kernel.pid(),
                lived.as_millis()
            ),
        );
        let Some(restarts) = allow_restart() else {
            *guard = None;
            bail!(
                "普通模式内核反复退出（60 秒内 3 次），已停止重启；请切回服务模式，日志见 {}",
                log_dir.display()
            );
        };
        // 退避：0 / 0 / 1 / 5 秒，避免瞬间连续拉起。
        let wait = match restarts {
            0 | 1 => 0,
            2 => 1,
            _ => 5,
        };
        if wait > 0 {
            std::thread::sleep(std::time::Duration::from_secs(wait));
        }
    }

    let kernel = spawn(app)?;
    let portal = kernel.rpc_portal.clone();
    crate::logger::record(
        "normal",
        format!(
            "spawned kernel pid={} rpc={} log={}",
            kernel.pid(),
            portal,
            kernel.log_dir.display()
        ),
    );
    *guard = Some(kernel);
    Ok(portal)
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

    // 与服务模式完全一致的参数集，只是目录（用户态）不同。
    // RPC 端口默认 1995（可在 service.toml 配置）：远程管理 / GUI / 服务都连这个地址。
    let rpc_portal = "127.0.0.1:1995".to_string();

    let mut cmd = crate::kernel::kernel_command(&kernel);
    cmd.arg("--config-dir")
        .arg(&config_dir)
        .arg("--file-log-dir")
        .arg(&log_dir)
        // 与特权服务保持一致：默认 1380 的 tun 在 mesh/relay 路径上会让
        // TCP 分段超 MTU（连得上、收大包就卡）。1300 是安全值。
        .arg("--mtu")
        .arg("1300")
        .arg("--rpc-portal")
        .arg(&rpc_portal)
        .arg("--rpc-portal-whitelist")
        .arg("127.0.0.1/32")
        .arg("--file-log-level")
        .arg("info")
        .arg("--file-log-size")
        .arg("16")
        .arg("--file-log-count")
        .arg("3")
        .arg("--console-log-level")
        .arg("off");

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
        spawned_at: std::time::Instant::now(),
        rpc_portal,
        log_dir,
    })
}

/// 停止普通模式内核（GUI 退出时调用）。
pub fn stop() {
    let mut guard = NORMAL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut kernel) = guard.take() {
        crate::logger::record("normal", format!("stopping kernel pid={}", kernel.pid()));
        let _ = kernel.child.kill();
        let _ = kernel.child.wait();
    }
}
