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
    pub rpc_portal: String,
    pub log_dir: PathBuf,
}

impl NormalKernel {
    fn alive(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            // 由 helper / supervisor 托管：对方会在 GUI 退出时清理，这里视为存活
            None => true,
        }
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().map(|c| c.id()).unwrap_or(0)
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
        crate::helper::spawn_kernel(&args, std::process::id()).map_err(|e| anyhow::anyhow!(e))?;
        return Ok(NormalKernel {
            child: None,
            lease: None,
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
            let _ = std::process::Command::new("schtasks")
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
    let out = std::process::Command::new("schtasks")
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

    let out = std::process::Command::new("schtasks")
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
