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

pub struct NormalKernel {
    /// `None` 表示由 macOS helper / Windows supervisor 托管，GUI 只负责通知停止。
    child: Option<std::process::Child>,
    /// Windows：与 supervisor 的 lease 连接；关闭即代表 GUI 退出。
    lease: Option<std::net::TcpStream>,
    /// macOS：helper 回报的内核 pid（用于探活；helper 托管时没有 Child 句柄）。
    helper_pid: Option<u32>,
    /// 拉起时刻与次数：用于退避重启与「反复退出」判定。
    spawned_at: std::time::Instant,
    pub rpc_portal: String,
    pub log_dir: PathBuf,
}

/// 进程是否还在（`kill(pid, 0)`，不发信号）。
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(windows)]
fn process_alive(_pid: u32) -> bool {
    // Windows 上有 Child / lease 两条路径，无需按 pid 探活。
    true
}

impl NormalKernel {
    fn alive(&mut self) -> bool {
        if let Some(child) = self.child.as_mut() {
            return matches!(child.try_wait(), Ok(None));
        }
        match self.helper_pid {
            Some(pid) => process_alive(pid),
            // 拿不到 pid（helper 老版本/响应解析失败）：无法探活，按存活处理，
            // 但 spawn 时会记一条明确的日志，避免静默。
            None => true,
        }
    }

    fn pid(&self) -> u32 {
        if let Some(child) = self.child.as_ref() {
            return child.id();
        }
        self.helper_pid.unwrap_or(0)
    }
}

/// 从 helper 的 `spawned pid=22441` 里取 pid。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_spawn_pid(message: &str) -> Option<u32> {
    message
        .split("pid=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|num| num.parse().ok())
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

    // 参数保持与 service 模式一致，只是目录/端口不同。
    let args: Vec<String> = vec![
        "--config-dir".into(),
        config_dir.display().to_string(),
        "--file-log-dir".into(),
        log_dir.display().to_string(),
        "--rpc-portal".into(),
        rpc_portal.clone(),
        "--rpc-portal-whitelist".into(),
        "127.0.0.1/32".into(),
        "--file-log-level".into(),
        "info".into(),
        "--console-log-level".into(),
        "off".into(),
    ];

    // macOS：普通模式要建 utun（需要 root），交给常驻 helper 代跑；
    // helper 用 kqueue 监听本进程，GUI 退出（含崩溃）即杀掉内核。
    #[cfg(target_os = "macos")]
    {
        let reply = crate::helper::spawn_kernel(&args, std::process::id())
            .map_err(|e| anyhow::anyhow!(e))?;
        let helper_pid = parse_spawn_pid(&reply);
        if helper_pid.is_none() {
            // 没有 pid 就没法探活/停止，必须留痕（曾经这里直接丢弃了返回值，
            // 日志里只剩 `spawned kernel pid=0`，出问题无从下手）。
            crate::logger::record(
                "normal",
                format!("helper 未返回内核 pid（reply={reply:?}）；本次无法探活"),
            );
        }
        return Ok(NormalKernel {
            child: None,
            lease: None,
            helper_pid,
            spawned_at: std::time::Instant::now(),
            rpc_portal,
            log_dir,
        });
    }

    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(windows)]
        {
            if let Some((portal, lease)) = start_windows_supervisor(&config_dir, &log_dir)? {
                return Ok(NormalKernel {
                    child: None,
                    lease: Some(lease),
                    helper_pid: None,
                    spawned_at: std::time::Instant::now(),
                    rpc_portal: portal,
                    log_dir,
                });
            }
            crate::logger::record(
                "normal",
                "supervisor 不可用，回退为普通用户进程（TUN 需要管理员）",
            );
        }

        let mut cmd = crate::kernel::kernel_command(&kernel);
        cmd.args(&args);

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
            child: Some(child),
            lease: None,
            helper_pid: None,
            spawned_at: std::time::Instant::now(),
            rpc_portal,
            log_dir,
        })
    }
}

/// 停止普通模式内核（GUI 退出时调用）。
pub fn stop() {
    let mut guard = NORMAL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut kernel) = guard.take() {
        crate::logger::record("normal", format!("stopping kernel pid={}", kernel.pid()));
        #[cfg(target_os = "macos")]
        {
            if let Err(err) = crate::helper::kill_kernel() {
                crate::logger::record("normal", format!("helper kill failed: {err}"));
            }
        }
        // Windows：关闭 lease → supervisor 收到 EOF → 杀掉内核并退出
        if let Some(lease) = kernel.lease.take() {
            let _ = lease.shutdown(std::net::Shutdown::Both);
        }
        if let Some(child) = kernel.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        #[cfg(windows)]
        {
            // 兜底：确保计划任务里没有残留实例
            let _ = crate::kernel::hidden_command("schtasks")
                .args(["/end", "/tn", SUPERVISOR_TASK])
                .output();
        }
    }
}

// ---------------------------------------------------------------------------
// Windows：计划任务 supervisor（以最高权限启动内核，GUI 通过 lease 维持生命周期）
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub const SUPERVISOR_TASK: &str = "EXVlan Kernel Supervisor";

/// 创建/更新计划任务（只应在提权进程里调用）。
#[cfg(windows)]
pub fn ensure_supervisor_task() -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("定位自身可执行文件失败")?;
    let tr = format!("\"{}\" --kernel-supervisor", exe.display());
    let out = crate::kernel::hidden_command("schtasks")
        .args([
            "/create",
            "/tn",
            SUPERVISOR_TASK,
            "/tr",
            tr.as_str(),
            "/sc",
            "once",
            "/st",
            "00:00",
            "/rl",
            "highest",
            "/f",
        ])
        .output()
        .context("执行 schtasks 失败")?;
    if !out.status.success() {
        bail!(
            "创建计划任务失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    crate::logger::record("normal.task", "supervisor task created/updated");
    Ok(())
}

#[cfg(windows)]
fn portal_path(log_dir: &std::path::Path) -> PathBuf {
    log_dir.join("portal.json")
}

#[cfg(windows)]
fn start_windows_supervisor(
    _config_dir: &std::path::Path,
    log_dir: &std::path::Path,
) -> anyhow::Result<Option<(String, std::net::TcpStream)>> {
    let portal = portal_path(log_dir);
    let _ = std::fs::remove_file(&portal);

    let out = crate::kernel::hidden_command("schtasks")
        .args(["/run", "/tn", SUPERVISOR_TASK])
        .output()
        .context("执行 schtasks /run 失败")?;
    if !out.status.success() {
        return Ok(None);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&portal)
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            && let (Some(rpc), Some(lease)) = (
                value.get("rpc").and_then(|v| v.as_str()),
                value.get("lease").and_then(|v| v.as_u64()),
            )
        {
            match std::net::TcpStream::connect(("127.0.0.1", lease as u16)) {
                Ok(stream) => return Ok(Some((rpc.to_string(), stream))),
                Err(err) => {
                    crate::logger::record("normal", format!("lease connect failed: {err}"));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Ok(None)
}

/// supervisor 主循环。由计划任务以最高权限启动，argv 含 `--kernel-supervisor`。
pub fn run_supervisor() -> i32 {
    let result = (|| -> anyhow::Result<()> {
        let app = std::env::var_os("APPDATA")
            .map(|p| {
                PathBuf::from(p)
                    .join("era.exvlan.studio")
                    .join("normal")
                    .join("config")
            })
            .context("缺少 APPDATA")?;
        let local = std::env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("era.exvlan.studio").join("normal"))
            .context("缺少 LOCALAPPDATA")?;
        std::fs::create_dir_all(&app)?;
        std::fs::create_dir_all(&local)?;

        let rpc_port = free_port()?;
        let lease_port = free_port()?;
        let portal = local.join("portal.json");
        std::fs::write(
            &portal,
            serde_json::json!({
                "rpc": format!("127.0.0.1:{rpc_port}"),
                "lease": lease_port,
                "pid": std::process::id(),
            })
            .to_string(),
        )?;

        let listener = std::net::TcpListener::bind(("127.0.0.1", lease_port))
            .context("绑定 lease 端口失败")?;

        let mut child = crate::kernel::kernel_command(crate::kernel::kernel_path())
            .arg("--config-dir")
            .arg(&app)
            .arg("--file-log-dir")
            .arg(&local)
            .arg("--rpc-portal")
            .arg(format!("127.0.0.1:{rpc_port}"))
            .arg("--rpc-portal-whitelist")
            .arg("127.0.0.1/32")
            .arg("--file-log-level")
            .arg("info")
            .arg("--console-log-level")
            .arg("off")
            .spawn()
            .context("supervisor 启动内核失败")?;
        crate::logger::record(
            "normal.supervisor",
            format!("kernel pid={} rpc=127.0.0.1:{rpc_port}", child.id()),
        );

        // 等 GUI 连上来；连接断开（GUI 退出/崩溃）即清理
        if let Ok((mut lease, _)) = listener.accept() {
            let mut buf = [0u8; 64];
            loop {
                match std::io::Read::read(&mut lease, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&portal);
        crate::logger::record("normal.supervisor", "lease closed, kernel stopped");
        Ok(())
    })();

    match result {
        Ok(()) => 0,
        Err(err) => {
            crate::logger::record("normal.supervisor", format!("{err:#}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_spawn_pid;

    #[test]
    fn parses_helper_spawn_reply() {
        // helper 的真实返回（曾在这里丢弃 pid，日志只剩 `spawned kernel pid=0`）。
        assert_eq!(parse_spawn_pid("spawned pid=22441"), Some(22441));
        assert_eq!(parse_spawn_pid("spawned pid=7 trailing"), Some(7));
    }

    #[test]
    fn missing_or_malformed_pid_is_none() {
        assert_eq!(parse_spawn_pid("killed"), None);
        assert_eq!(parse_spawn_pid("spawned pid="), None);
        assert_eq!(parse_spawn_pid("spawned pid=abc"), None);
    }
}
