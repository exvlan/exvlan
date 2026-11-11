//! EXVlan 特权服务（macOS LaunchDaemon / Windows Service）。
//!
//! 职责（合并了原 helper 与内核服务）：
//! 1. 持有内核：service 模式常驻；normal 模式随 GUI session 起停
//! 2. 应用内核更新：minisign 校验 → 保留上一版 → 失败自动回滚
//! 3. 通过 IPC 暴露 status / mode / kernel / apply
//!
//! 设计要点：模式只是「生命周期策略」，**切换模式不重启内核**；
//! 配置与端口两模式一致（单一 config、固定 15999）。

#![cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(dead_code, unused_imports)
)]
// Windows 上用 GUI 子系统：进程根本不分配控制台，从根上不会闪 cmd 窗口。
// `Command::output()` 通过管道仍能拿到 stdout（`--version` 的兜底路径），
// 只是用户手动在 cmd 里跑时看不到输出（`--diagnose` 本来就写临时文件）。
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

const SERVICE_LABEL: &str = "era.exvlan.studio.service";

/// 构建版本串：`<标准版本>[-<channel>.<n>][+<sha6>]`。
///
/// 由 CI 的 `script/ci/build-version.sh` 通过 `EXVLAN_BUILD_VERSION` 注入，
/// 本地构建没有该变量时回落到 crate 版本。
///
/// **必须与 GUI 的 `BUILD_VERSION` 同源**：GUI 用 `--version` 的输出来判断
/// 「服务是否需要自更新」。若这里报 crate 版本（永远不含 `-pre.N+sha`），
/// 每次启动都会被判定为需要升级 → 反复替换二进制 + 重启服务。
const BUILD_VERSION: &str = match option_env!("EXVLAN_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
const MINISIGN_PUBKEY: &str = "RWSFKUboUUaFQYx/ap73a7ySNbuI5kjDGsiVJBtceshu+Ml99YPMFqe9";
const PROTO: u32 = 1;
const RPC_PORTAL: &str = "0.0.0.0:15999";
const RPC_WHITELIST: &str = "10.254.0.0/16";
const READY_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// 路径
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Paths {
    pub config: PathBuf,
    pub core: PathBuf,
    pub logs: PathBuf,
    pub kernel: PathBuf,
    pub kernel_prev: PathBuf,
    pub service_bin: PathBuf,
    pub service_json: PathBuf,
    pub service_log: PathBuf,
}

/// Windows：拉起控制台程序（`sc` / `schtasks`）时不要弹 cmd 窗口。
///
/// `Command::output()` 只重定向 stdio，**不会**阻止 Windows 分配 conhost；
/// 必须显式 `CREATE_NO_WINDOW`。pre.11 修过一轮，pre.13 的服务改造又漏回来了。
#[cfg(target_os = "windows")]
fn hidden(program: impl AsRef<std::ffi::OsStr>) -> Command {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// 二进制旁边的版本文件（0644，GUI 直接读，避免每次启动 fork 一次 `--version`）。
fn version_path(bin: &Path) -> PathBuf {
    let mut raw = bin.as_os_str().to_os_string();
    raw.push(".version");
    PathBuf::from(raw)
}

fn write_version_file(p: &Paths) -> std::io::Result<()> {
    let path = version_path(&p.service_bin);
    std::fs::write(&path, format!("{BUILD_VERSION}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
    }
    Ok(())
}

fn paths() -> Paths {
    #[cfg(target_os = "macos")]
    {
        let data = PathBuf::from("/Library/Application Support/era.exvlan.studio");
        let logs = PathBuf::from("/Library/Logs/era.exvlan.studio");
        let core = data.join("core");
        Paths {
            config: data.join("config"),
            kernel: core.join("exvlan"),
            kernel_prev: core.join("exvlan.prev"),
            core,
            service_bin: PathBuf::from("/Library/PrivilegedHelperTools/era.exvlan.studio.service"),
            service_json: data.join("service.json"),
            service_log: logs.join("service.log"),
            logs,
        }
    }
    #[cfg(target_os = "windows")]
    {
        let data = PathBuf::from(r"C:\ProgramData\era.exvlan.studio");
        let core = data.join("core");
        Paths {
            config: data.join("config"),
            kernel: core.join("exvlan.exe"),
            kernel_prev: core.join("exvlan.exe.prev"),
            core,
            logs: data.join("logs"),
            service_bin: data.join("bin").join("exvlan-service.exe"),
            service_json: data.join("service.json"),
            service_log: data.join("logs").join("service.log"),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let data = PathBuf::from("/var/lib/era.exvlan.studio");
        let core = data.join("core");
        Paths {
            config: data.join("config"),
            kernel: core.join("exvlan"),
            kernel_prev: core.join("exvlan.prev"),
            core,
            logs: PathBuf::from("/var/log/era.exvlan.studio"),
            service_bin: PathBuf::from("/usr/local/bin/exvlan-service"),
            service_json: data.join("service.json"),
            service_log: PathBuf::from("/var/log/era.exvlan.studio/service.log"),
        }
    }
}

// ---------------------------------------------------------------------------
// 配置与状态
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Service,
    Normal,
    Off,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceConfig {
    #[serde(default)]
    pub owner_uid: u32,
    #[serde(default)]
    pub mode: Mode,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            owner_uid: 0,
            mode: Mode::Service,
        }
    }
}

fn load_config(p: &Paths) -> ServiceConfig {
    std::fs::read_to_string(&p.service_json)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_config(p: &Paths, cfg: &ServiceConfig) -> anyhow::Result<()> {
    if let Some(dir) = p.service_json.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let text = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&p.service_json, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&p.service_json, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 日志（自轮转 8MB × 2）
// ---------------------------------------------------------------------------

fn log_line(p: &Paths, msg: &str) {
    use std::io::Write as _;
    let line = format!("{}\n", format_line(msg));
    if let Some(dir) = p.service_log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(meta) = std::fs::metadata(&p.service_log)
        && meta.len() > 8 * 1024 * 1024
    {
        let backup = p.service_log.with_extension("log.1");
        let _ = std::fs::remove_file(&backup);
        let _ = std::fs::rename(&p.service_log, &backup);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p.service_log)
    {
        let _ = f.write_all(line.as_bytes());
    }
    eprint!("{line}");
}

fn format_line(msg: &str) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 只保留到秒的 UTC 时间，够用且不引依赖。
    format!("{secs} [service] {msg}")
}

// ---------------------------------------------------------------------------
// 协议
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Request {
    // 只有 unix 分支读它（Windows 管道由 ACL 限制），但两端都要反序列化。
    #[cfg_attr(not(unix), allow(dead_code))]
    #[serde(default)]
    proto: u32,
    op: String,
    #[serde(default)]
    mode: Option<Mode>,
    #[serde(default)]
    staging: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Serialize)]
struct KernelInfo {
    pid: u32,
    running: bool,
    rpc: String,
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    proto: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<Mode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kernel: Option<KernelInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl Response {
    fn ok() -> Self {
        Self {
            ok: true,
            proto: None,
            service: None,
            mode: None,
            kernel: None,
            code: None,
            message: None,
        }
    }
    fn err(code: &str, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            proto: None,
            service: None,
            mode: None,
            kernel: None,
            code: Some(code.to_string()),
            message: Some(message.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// 内核生命周期
// ---------------------------------------------------------------------------

struct Inner {
    child: Option<Child>,
    mode: Mode,
    config: ServiceConfig,
    sessions: usize,
}

pub struct State {
    paths: Paths,
    inner: Mutex<Inner>,
}

impl State {
    fn new(paths: Paths) -> Self {
        let config = load_config(&paths);
        let mode = config.mode;
        Self {
            paths,
            inner: Mutex::new(Inner {
                child: None,
                mode,
                config,
                sessions: 0,
            }),
        }
    }

    fn log(&self, msg: impl AsRef<str>) {
        log_line(&self.paths, msg.as_ref());
    }

    fn kernel_pid(&self) -> u32 {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        take_pid(&mut guard)
    }

    fn kernel_args(&self) -> Vec<String> {
        vec![
            "--config-dir".into(),
            self.paths.config.display().to_string(),
            "--rpc-portal".into(),
            RPC_PORTAL.into(),
            "--rpc-portal-whitelist".into(),
            RPC_WHITELIST.into(),
            "--file-log-dir".into(),
            self.paths.logs.display().to_string(),
            "--file-log-level".into(),
            "info".into(),
            "--file-log-size".into(),
            "16".into(),
            "--file-log-count".into(),
            "3".into(),
            "--console-log-level".into(),
            "off".into(),
        ]
    }

    fn ensure_kernel(&self) -> anyhow::Result<KernelInfo> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(child) = guard.child.as_mut() {
            match child.try_wait() {
                Ok(None) => {
                    return Ok(KernelInfo {
                        pid: child.id(),
                        running: true,
                        rpc: "127.0.0.1:15999".into(),
                    });
                }
                _ => {
                    guard.child = None;
                }
            }
        }

        if !self.paths.kernel.is_file() {
            bail!("内核不存在: {}", self.paths.kernel.display());
        }
        std::fs::create_dir_all(&self.paths.config).ok();
        std::fs::create_dir_all(&self.paths.logs).ok();

        let mut cmd = Command::new(&self.paths.kernel);
        cmd.args(self.kernel_args())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let child = cmd
            .spawn()
            .with_context(|| format!("启动内核失败: {}", self.paths.kernel.display()))?;
        let pid = child.id();
        guard.child = Some(child);
        drop(guard);
        self.log(format!("kernel spawned pid={pid}"));

        if let Err(err) = wait_rpc_ready(READY_TIMEOUT) {
            self.log(format!("kernel not ready: {err}"));
            self.stop_kernel().ok();
            bail!("kernel start failed: {err}");
        }

        Ok(KernelInfo {
            pid,
            running: true,
            rpc: "127.0.0.1:15999".into(),
        })
    }

    fn stop_kernel(&self) -> anyhow::Result<()> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut child) = guard.child.take() else {
            return Ok(());
        };
        let pid = child.id();
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        #[cfg(windows)]
        {
            let _ = child.kill();
        }
        for _ in 0..50 {
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
        drop(guard);
        self.log(format!("kernel stopped pid={pid}"));
        Ok(())
    }

    fn set_mode(&self, mode: Mode) -> anyhow::Result<()> {
        {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if guard.mode == mode {
                // 模式没变：只确保该在跑的在跑。
                if mode != Mode::Off && guard.child.is_none() {
                    drop(guard);
                    self.ensure_kernel()?;
                }
                return Ok(());
            }
            guard.mode = mode;
            guard.config.mode = mode;
            let cfg = guard.config.clone();
            // 持久化在锁外做，避免阻塞其它请求。
            drop(guard);
            save_config(&self.paths, &cfg)?;
        }
        self.log(format!("mode set to {mode:?}"));
        match mode {
            Mode::Off => self.stop_kernel()?,
            // service ↔ normal：**不重启内核**，只改变“断开是否停”的策略。
            Mode::Service | Mode::Normal => {
                if self.kernel_pid() == 0 {
                    self.ensure_kernel()?;
                }
            }
        }
        Ok(())
    }

    fn apply(&self, staging: &Path) -> anyhow::Result<()> {
        verify_staging(staging)?;
        let new_kernel = staging.join("kernel");
        let new_sig = staging.join("kernel.sig");
        verify_minisign(&new_kernel, &new_sig)?;

        let version = std::fs::read_to_string(staging.join("version"))
            .unwrap_or_default()
            .trim()
            .to_string();

        if let Some(dir) = self.paths.kernel.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        // 保留上一版，失败可回滚。
        if self.paths.kernel.is_file() {
            let _ = std::fs::remove_file(&self.paths.kernel_prev);
            std::fs::rename(&self.paths.kernel, &self.paths.kernel_prev)
                .context("备份当前内核失败")?;
        }
        std::fs::copy(&new_kernel, &self.paths.kernel).context("安装新内核失败")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(
                &self.paths.kernel,
                std::fs::Permissions::from_mode(0o755),
            );
            chown_root(&self.paths.kernel);
        }
        if !version.is_empty() {
            let _ = std::fs::write(self.paths.kernel.with_file_name("version"), &version);
        }
        self.log(format!("kernel installed {version}"));

        let was_running = self.kernel_pid() != 0;
        if was_running {
            self.stop_kernel()?;
            if let Err(err) = self.ensure_kernel() {
                self.log(format!("new kernel failed ({err}), rolling back"));
                let _ = std::fs::remove_file(&self.paths.kernel);
                if self.paths.kernel_prev.is_file() {
                    std::fs::rename(&self.paths.kernel_prev, &self.paths.kernel).ok();
                }
                self.ensure_kernel().ok();
                return Err(anyhow::anyhow!("kernel_rollback: {err}"));
            }
        }
        Ok(())
    }

    fn status(&self) -> Response {
        let pid = self.kernel_pid();
        let mode = self.inner.lock().unwrap_or_else(|e| e.into_inner()).mode;
        let mut resp = Response::ok();
        resp.mode = Some(mode);
        resp.kernel = Some(KernelInfo {
            pid,
            running: pid != 0,
            rpc: "127.0.0.1:15999".into(),
        });
        resp
    }

    fn handle(&self, req: Request) -> Response {
        match req.op.as_str() {
            "hello" => {
                let mut r = Response::ok();
                r.proto = Some(PROTO);
                r.service = Some(BUILD_VERSION.to_string());
                r
            }
            "ping" => Response::ok(),
            "status" => self.status(),
            "mode.get" => {
                let mut r = Response::ok();
                r.mode = Some(self.inner.lock().unwrap_or_else(|e| e.into_inner()).mode);
                r
            }
            "mode.set" => match req.mode {
                Some(mode) => match self.set_mode(mode) {
                    Ok(()) => self.status(),
                    Err(err) => Response::err("mode_failed", err.to_string()),
                },
                None => Response::err("bad_request", "mode 缺失"),
            },
            "kernel.ensure" => match self.ensure_kernel() {
                Ok(info) => {
                    let mut r = self.status();
                    r.kernel = Some(info);
                    r
                }
                Err(err) => Response::err("kernel_start_failed", err.to_string()),
            },
            "kernel.stop" => match self.stop_kernel() {
                Ok(()) => self.status(),
                Err(err) => Response::err("internal", err.to_string()),
            },
            "apply" => match req.staging.as_deref() {
                Some(dir) => match self.apply(Path::new(dir)) {
                    Ok(()) => self.status(),
                    Err(err) => {
                        let msg = err.to_string();
                        let code = if msg.contains("kernel_rollback") {
                            "kernel_rollback"
                        } else {
                            "apply_failed"
                        };
                        Response::err(code, msg)
                    }
                },
                None => Response::err("bad_request", "staging 缺失"),
            },
            "service.update" => match req.staging.as_deref() {
                Some(dir) => match self.update_self(Path::new(dir), req.version.as_deref()) {
                    Ok(()) => Response::ok(),
                    Err(err) => Response::err("update_failed", err.to_string()),
                },
                None => Response::err("bad_request", "staging 缺失"),
            },
            other => Response::err("bad_request", format!("未知操作: {other}")),
        }
    }

    /// 免密自更新：校验签名 → 拉起独立替换进程 → 由它替换并重启。
    ///
    /// 这里**不做替换动作本身**：Windows 上运行中的 exe 被锁，必须先由新进程
    /// 停服务；macOS 上 kickstart 会杀掉本进程，同样不能由自己做。
    fn update_self(&self, staging: &Path, version: Option<&str>) -> anyhow::Result<()> {
        verify_staging_dir(staging)?;
        let bin_name = if cfg!(windows) {
            "exvlan-service.exe"
        } else {
            "exvlan-service"
        };
        let new_bin = staging.join(bin_name);
        let sig = staging.join("service.sig");
        if !new_bin.is_file() {
            bail!("staging 缺少服务二进制");
        }
        if !sig.is_file() {
            bail!("staging 缺少服务签名");
        }
        verify_minisign(&new_bin, &sig)?;

        let exe = std::env::current_exe().context("定位自身可执行文件失败")?;
        let mut cmd = Command::new(&exe);
        cmd.arg("--replace-updater")
            .arg(&new_bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            cmd.creation_flags(DETACHED_PROCESS);
        }
        let child = cmd.spawn().context("拉起替换进程失败")?;
        self.log(format!(
            "service.update requested → {} (updater pid={})",
            version.unwrap_or("-"),
            child.id()
        ));
        Ok(())
    }

    // ---- session（normal 模式的存活锚点） ----
    fn session_open(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions += 1;
    }

    fn session_close(&self) {
        let stop = {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.sessions = guard.sessions.saturating_sub(1);
            guard.sessions == 0 && guard.mode == Mode::Normal && guard.child.is_some()
        };
        if stop {
            self.log("last session closed in normal mode → stop kernel");
            let _ = self.stop_kernel();
        }
    }
}

fn take_pid(inner: &mut Inner) -> u32 {
    match inner.child.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(None) => child.id(),
            _ => {
                inner.child = None;
                0
            }
        },
        None => 0,
    }
}

fn wait_rpc_ready(timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(
            &"127.0.0.1:15999".parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    bail!("rpc 端口 {timeout:?} 内未就绪")
}

#[cfg(unix)]
fn chown_root(path: &Path) {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    unsafe {
        libc::chown(c.as_ptr(), 0, 0);
    }
}
#[cfg(not(unix))]
#[allow(dead_code)]
fn chown_root(_path: &Path) {}

// ---------------------------------------------------------------------------
// staging / 签名
// ---------------------------------------------------------------------------

fn verify_staging(dir: &Path) -> anyhow::Result<()> {
    let meta = std::fs::metadata(dir).context("staging 不存在")?;
    if !meta.is_dir() {
        bail!("staging 不是目录");
    }
    for name in ["kernel", "kernel.sig"] {
        if !dir.join(name).is_file() {
            bail!("staging 缺少 {name}");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if meta.mode() & 0o022 != 0 {
            bail!("staging 可被 group/other 写，拒绝执行");
        }
    }
    Ok(())
}

/// staging 目录的基本校验（只查权限，不查具体文件名）。
fn verify_staging_dir(dir: &Path) -> anyhow::Result<()> {
    let meta = std::fs::metadata(dir).context("staging 不存在")?;
    if !meta.is_dir() {
        bail!("staging 不是目录");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if meta.mode() & 0o022 != 0 {
            bail!("staging 可被 group/other 写，拒绝执行");
        }
    }
    Ok(())
}

fn verify_minisign(file: &Path, signature: &Path) -> anyhow::Result<()> {
    let public_key =
        minisign_verify::PublicKey::from_base64(MINISIGN_PUBKEY).context("解析内置公钥失败")?;
    let signature_text = std::fs::read_to_string(signature)?;
    let decoded = decode_signature_text(&signature_text);
    let signature = minisign_verify::Signature::decode(&decoded).context("解析签名文件失败")?;
    let data = std::fs::read(file)?;
    public_key
        .verify(&data, &signature, false)
        .or_else(|_| public_key.verify(&data, &signature, true))
        .context("签名不匹配")?;
    Ok(())
}

fn decode_signature_text(text: &str) -> String {
    use base64::Engine as _;
    let trimmed = text.trim();
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(trimmed) else {
        return trimmed.to_string();
    };
    String::from_utf8(bytes).unwrap_or_else(|_| trimmed.to_string())
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    let p = paths();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("exvlan-service {BUILD_VERSION}");
            Ok(())
        }
        Some("--daemon") => run_daemon(p),
        Some("--install") => {
            let uid = args.get(1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
            install(&p, uid)
        }
        Some("--uninstall") => uninstall(&p),
        Some("--status") => {
            println!("{}", serde_json::to_string_pretty(&load_config(&p))?);
            Ok(())
        }
        Some("--diagnose") => diagnose(&p),
        // 提权入口：重启服务本体（不改配置、不换二进制）。
        Some("--restart") => restart_service(),
        // 自更新落点：由运行中的服务 fork 出来，等它退出后替换二进制并拉起。
        Some("--replace-updater") => replace_updater(&p, args.get(1).map(String::as_str)),
        _ => {
            eprintln!(
                "usage: exvlan-service --daemon | --install <uid> | --uninstall | --status | --diagnose | --restart"
            );
            std::process::exit(2);
        }
    }
}

#[cfg(target_os = "macos")]
fn restart_service() -> anyhow::Result<()> {
    let out = Command::new("/bin/launchctl")
        .args(["kickstart", "-k", &format!("system/{SERVICE_LABEL}")])
        .output()?;
    if !out.status.success() {
        bail!(
            "launchctl kickstart 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn restart_service() -> anyhow::Result<()> {
    let _ = hidden("sc").args(["stop", SERVICE_LABEL]).output();
    for _ in 0..150 {
        let out = hidden("sc").args(["query", SERVICE_LABEL]).output()?;
        if String::from_utf8_lossy(&out.stdout).contains("STOPPED") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let out = hidden("sc").args(["start", SERVICE_LABEL]).output()?;
    if !out.status.success() {
        bail!(
            "sc start 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn restart_service() -> anyhow::Result<()> {
    bail!("exvlan-service 只在 macOS/Windows 上运行")
}

/// Windows：**先** `StartServiceCtrlDispatcherW`，再由 `service_main` 干活。
///
/// 之前这里先建 `State`、拉内核（含最多 10s 等 RPC），dispatcher 还没注册，
/// SCM 直接判定「未响应」→ 1067 终止进程，`service_main` 根本没跑到。
#[cfg(target_os = "windows")]
fn run_daemon(p: Paths) -> anyhow::Result<()> {
    win::run(p)
}

/// macOS：unix socket + peercred。
#[cfg(target_os = "macos")]
fn run_daemon(p: Paths) -> anyhow::Result<()> {
    let state = Arc::new(State::new(p.clone()));
    log_line(&p, &format!("daemon start pid={}", std::process::id()));
    boot(&state);
    serve_unix(state)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn run_daemon(_p: Paths) -> anyhow::Result<()> {
    bail!("exvlan-service 只在 macOS/Windows 上运行")
}

/// 服务启动时按持久化 mode 决定是否拉起内核（macOS / Windows 共用）。
fn boot(state: &State) {
    let mode = state.inner.lock().unwrap_or_else(|e| e.into_inner()).mode;
    match mode {
        Mode::Service => {
            if let Err(err) = state.ensure_kernel() {
                state.log(format!("boot ensure_kernel failed: {err}"));
            }
        }
        Mode::Normal | Mode::Off => state.log(format!("boot mode={mode:?}: 不自动启动内核")),
    }
}

// ---------------------------------------------------------------------------
// 平台：macOS / Linux（unix socket + peercred）
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn socket_path() -> PathBuf {
    PathBuf::from("/var/run/era.exvlan.studio/service.sock")
}

#[cfg(unix)]
fn serve_unix(state: Arc<State>) -> anyhow::Result<()> {
    use std::os::unix::net::UnixListener;

    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).context("bind unix socket 失败")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660));
        chown_root(&path);
    }
    state.log(format!("listening on {}", path.display()));

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if !peer_allowed(&stream, &state) {
            state.log("rejected: peer uid mismatch");
            continue;
        }
        let state = state.clone();
        state.session_open();
        std::thread::spawn(move || {
            handle_unix_conn(&state, stream);
            state.session_close();
        });
    }
    Ok(())
}

#[cfg(unix)]
fn peer_allowed(stream: &std::os::unix::net::UnixStream, state: &State) -> bool {
    use std::os::unix::io::AsRawFd as _;
    let expected = state
        .inner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .config
        .owner_uid;
    if expected == 0 {
        return true; // 未记录 owner（首装前）：放行，安装时一定写入
    }
    let mut uid: libc::uid_t = 0;
    #[allow(unused_mut, unused_variables)]
    let mut gid: libc::gid_t = 0;
    #[cfg(target_os = "macos")]
    let ok = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) == 0 };
    #[cfg(not(target_os = "macos"))]
    let ok = unsafe {
        let mut cred: libc::ucred = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        ) == 0
            && {
                uid = cred.uid;
                true
            }
    };
    ok && (uid == expected || uid == 0)
}

#[cfg(unix)]
fn handle_unix_conn(state: &State, stream: std::os::unix::net::UnixStream) {
    use std::io::{BufRead, BufReader, Write};
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut writer = stream;
    let mut last = Instant::now();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        last = Instant::now();
        let req: Request = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(err) => {
                let resp = Response::err("bad_request", err.to_string());
                let _ = writeln!(
                    writer,
                    "{}",
                    serde_json::to_string(&resp).unwrap_or_default()
                );
                continue;
            }
        };
        if req.proto != 0 && req.proto != PROTO {
            let resp = Response::err("proto_mismatch", format!("service proto={PROTO}"));
            let _ = writeln!(
                writer,
                "{}",
                serde_json::to_string(&resp).unwrap_or_default()
            );
            continue;
        }
        let resp = state.handle(req);
        let _ = writeln!(
            writer,
            "{}",
            serde_json::to_string(&resp).unwrap_or_default()
        );
    }
    let _ = last;
}

// ---------------------------------------------------------------------------
// 平台：Windows（命名管道 + 服务控制）
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::ffi::OsString;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_service::define_windows_service;
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_dispatcher;

    define_windows_service!(ffi_service_main, service_main);

    /// dispatcher 与 service_main 之间传路径（`Paths` 不是 'static 友好类型）。
    static PATHS: OnceLock<Paths> = OnceLock::new();
    /// 收到 Stop/Shutdown 时拉低，pipe_loop 退出并清理内核。
    pub(super) static RUNNING: AtomicBool = AtomicBool::new(true);

    pub fn run(p: Paths) -> anyhow::Result<()> {
        let _ = PATHS.set(p);
        service_dispatcher::start(SERVICE_LABEL, ffi_service_main)
            .context("启动 Windows 服务失败")?;
        Ok(())
    }

    fn service_main(_args: Vec<OsString>) {
        let p = PATHS.get().cloned().unwrap_or_else(paths);
        let state = Arc::new(State::new(p.clone()));
        state.log("windows service start");

        // 必须注册控制句柄并上报 RUNNING：不做这一步 SCM 会在超时后
        // 直接终止进程（表现为 STOPPED + WIN32_EXIT_CODE 1067）。
        let handler = {
            let state = state.clone();
            move |event| -> ServiceControlHandlerResult {
                match event {
                    ServiceControl::Stop | ServiceControl::Shutdown => {
                        state.log("SCM stop/shutdown requested");
                        RUNNING.store(false, Ordering::SeqCst);
                        // ConnectNamedPipe 在阻塞 accept 中，自连一次把它唤醒。
                        let _ = std::fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .open(r"\\.\pipe\era.exvlan.studio.service");
                        ServiceControlHandlerResult::NoError
                    }
                    ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                    _ => ServiceControlHandlerResult::NotImplemented,
                }
            }
        };
        let status_handle = match service_control_handler::register(SERVICE_LABEL, handler) {
            Ok(h) => h,
            Err(err) => {
                state.log(format!("register control handler failed: {err}"));
                return;
            }
        };
        let status = |state: ServiceState| ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        };
        if let Err(err) = status_handle.set_service_status(status(ServiceState::Running)) {
            state.log(format!("report RUNNING failed: {err}"));
        }

        // 现在才干活：按 mode 拉起内核，再进入管道 accept 循环。
        boot(&state);
        if let Err(err) = pipe_loop(state.clone()) {
            state.log(format!("pipe loop exited: {err}"));
        }

        // 退出前把内核一并收掉（否则会留下孤儿内核占着 15999 和 TUN）。
        if let Err(err) = state.stop_kernel() {
            state.log(format!("stop kernel on exit failed: {err}"));
        }
        let _ = status_handle.set_service_status(status(ServiceState::Stopped));
        state.log("windows service stopped");
    }
}

/// 命名管道的安全描述符：`D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)`。
///
/// 失败时返回 `None`，退回系统默认 ACL（并记日志），保证服务仍能用。
#[cfg(target_os = "windows")]
fn pipe_security_attributes() -> Option<windows_sys::Win32::Security::SECURITY_ATTRIBUTES> {
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    const SDDL_REVISION_1: u32 = 1;
    let sddl: Vec<u16> = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)\0"
        .encode_utf16()
        .collect();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        log_line(&paths(), "pipe SDDL 构造失败，退回默认 ACL");
        return None;
    }
    // 故意不 LocalFree：整个进程生命周期复用同一份描述符。
    Some(SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    })
}

#[cfg(target_os = "windows")]
fn pipe_loop(state: Arc<State>) -> anyhow::Result<()> {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Storage::FileSystem::*;
    use windows_sys::Win32::System::Pipes::*;

    let name: Vec<u16> = "\\\\.\\pipe\\era.exvlan.studio.service\0"
        .encode_utf16()
        .collect();
    // 显式 DACL：不依赖 LocalSystem 的默认 ACL（默认值随 Windows 版本/策略变化，
    // macOS 那次已经证明「默认权限」不可假设）。
    //   SY = LocalSystem  GA、BA = Administrators  GA、IU = 交互式登录用户  GRGW
    let sa = pipe_security_attributes();
    let sa_ptr = match &sa {
        Some(s) => s as *const windows_sys::Win32::Security::SECURITY_ATTRIBUTES,
        None => std::ptr::null(),
    };
    while win::RUNNING.load(std::sync::atomic::Ordering::SeqCst) {
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                sa_ptr,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if connected {
            state.session_open();
            let st = state.clone();
            let pipe = SendHandle(handle);
            std::thread::spawn(move || {
                // 必须走方法调用：直接读 `pipe.0` 会被闭包按字段精确捕获，
                // 从而绕过 SendHandle 的 Send 实现。
                let handle = pipe.into_inner();
                handle_pipe_conn(&st, handle);
                st.session_close();
                unsafe {
                    DisconnectNamedPipe(handle);
                    CloseHandle(handle);
                }
            });
        } else {
            unsafe {
                CloseHandle(handle);
            }
        }
    }
    Ok(())
}

/// `HANDLE` 是 `*mut c_void`，不实现 Send；句柄本身可跨线程使用。
#[cfg(target_os = "windows")]
struct SendHandle(windows_sys::Win32::Foundation::HANDLE);
#[cfg(target_os = "windows")]
unsafe impl Send for SendHandle {}
#[cfg(target_os = "windows")]
impl SendHandle {
    fn into_inner(self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0
    }
}

#[cfg(target_os = "windows")]
fn handle_pipe_conn(state: &State, handle: windows_sys::Win32::Foundation::HANDLE) {
    use windows_sys::Win32::Storage::FileSystem::*;

    let mut buf = [0u8; 4096];
    let mut pending = String::new();
    loop {
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                handle,
                buf.as_mut_ptr() as *mut _,
                buf.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            break;
        }
        pending.push_str(&String::from_utf8_lossy(&buf[..read as usize]));
        while let Some(pos) = pending.find('\n') {
            let line: String = pending.drain(..=pos).collect();
            let req: Request = match serde_json::from_str(line.trim()) {
                Ok(r) => r,
                Err(err) => {
                    let resp = Response::err("bad_request", err.to_string());
                    write_pipe(handle, &resp);
                    continue;
                }
            };
            let resp = state.handle(req);
            write_pipe(handle, &resp);
        }
        let _ = read;
    }
}

#[cfg(target_os = "windows")]
fn write_pipe(handle: windows_sys::Win32::Foundation::HANDLE, resp: &Response) {
    use windows_sys::Win32::Storage::FileSystem::WriteFile;
    let mut line = serde_json::to_string(resp).unwrap_or_default();
    line.push('\n');
    let mut written = 0u32;
    unsafe {
        WriteFile(
            handle,
            line.as_ptr() as *const _,
            line.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        );
    }
}

// ---------------------------------------------------------------------------
// 安装 / 卸载
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn install(p: &Paths, uid: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let src = std::env::current_exe()?;
    let plist = Path::new("/Library/LaunchDaemons/era.exvlan.studio.service.plist");
    std::fs::create_dir_all(p.service_bin.parent().unwrap()).ok();
    std::fs::create_dir_all(p.logs.parent().unwrap_or(&p.logs)).ok();
    // 从已安装位置原地重装时 src == dst：`fs::copy` 会报 EBUSY/EINVAL。
    if src != p.service_bin {
        std::fs::copy(&src, &p.service_bin)?;
    }
    std::fs::set_permissions(&p.service_bin, std::fs::Permissions::from_mode(0o755))?;
    chown_root(&p.service_bin);

    // 旧模型（内核直跑 launchd / 旧 helper）与新版抢 15999 与 TUN，必须先清掉。
    // 注意：列表里**不能**包含当前的 SERVICE_LABEL，否则会删掉刚写好、
    // 或者（更糟）正在用的 plist（pre.12 的 bootstrap EIO 就是这个原因）。
    cleanup_legacy_macos();

    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>--daemon</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        bin = p.service_bin.display(),
        log = p.service_log.display()
    );
    std::fs::write(plist, body)?;
    std::fs::set_permissions(plist, std::fs::Permissions::from_mode(0o644))?;
    chown_root(plist);

    let _ = write_version_file(p);

    let cfg = ServiceConfig {
        owner_uid: uid,
        mode: load_config(p).mode,
    };
    save_config(p, &cfg)?;

    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &format!("system/{SERVICE_LABEL}")])
        .output();
    let _ = Command::new("/bin/launchctl")
        .args(["enable", &format!("system/{SERVICE_LABEL}")])
        .output();
    let out = Command::new("/bin/launchctl")
        .args(["bootstrap", "system", &plist.display().to_string()])
        .output()?;
    if !out.status.success() {
        bail!(
            "bootstrap 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // 等 socket 就绪（≤10s）：必须真连上，而不是只看文件存在
    // （上一版崩溃留下的 stale socket 会让“文件存在”恒为真）。
    for _ in 0..50 {
        if std::os::unix::net::UnixStream::connect(socket_path()).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    bail!("服务已加载但 socket 未就绪")
}

/// 清理旧模型留下的守护进程（幂等）。当前 SERVICE_LABEL 绝不在列表里。
#[cfg(target_os = "macos")]
fn cleanup_legacy_macos() {
    const LEGACY_LABELS: &[&str] = &[
        "era.exvlan.studio",
        "era.exvlan.studio.helper",
        "com.exvlan.studio",
        "com.exvlan.studio.helper",
        "exvlan-studio",
    ];
    for label in LEGACY_LABELS {
        if *label == SERVICE_LABEL {
            continue;
        }
        let _ = Command::new("/bin/launchctl")
            .args(["bootout", &format!("system/{label}")])
            .output();
        for dir in ["/Library/LaunchDaemons", "/Library/LaunchAgents"] {
            let _ = std::fs::remove_file(format!("{dir}/{label}.plist"));
        }
    }
    // 旧 helper 二进制
    for stale in [
        "/Library/PrivilegedHelperTools/era.exvlan.studio.helper",
        "/Library/PrivilegedHelperTools/com.exvlan.studio.helper",
    ] {
        let _ = std::fs::remove_file(stale);
    }
}

#[cfg(target_os = "windows")]
fn install(p: &Paths, _uid: u32) -> anyhow::Result<()> {
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let src = std::env::current_exe()?;
    if let Some(dir) = p.service_bin.parent() {
        std::fs::create_dir_all(dir).ok();
    }

    // 1) 服务可能正在跑：直接覆盖 exe 会报「另一个程序正在使用此文件」(os error 32)。
    stop_service_blocking();
    cleanup_legacy_windows();

    // 2) 复制（带重试：SCM/杀软释放文件句柄需要一点时间）
    if src != p.service_bin {
        copy_with_retry(&src, &p.service_bin)?;
    }
    let _ = write_version_file(p);

    // 3) 持久化配置（此前 Windows 分支漏了 save_config → owner_uid/mode 缺失，
    //    `--diagnose` 只能显示默认值）。
    let cfg = ServiceConfig {
        owner_uid: 0,
        mode: load_config(p).mode,
    };
    save_config(p, &cfg)?;

    // 4) 注册（已存在则复用同一个服务项）
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )?;
    let info = ServiceInfo {
        // windows-service 0.8 的 name/display_name 是 OsString（0.6/0.7 才是 ServiceName）。
        name: std::ffi::OsString::from(SERVICE_LABEL),
        display_name: std::ffi::OsString::from("Exvlan Service"),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: p.service_bin.clone(),
        launch_arguments: vec!["--daemon".into()],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };
    let access = ServiceAccess::START | ServiceAccess::CHANGE_CONFIG;
    let service = manager
        .create_service(&info, access)
        .or_else(|_| manager.open_service(&info.name, access))?;

    // 5) 启动：不再 `.ok()` 吞错。起不来时必须让 GUI/安装器看到原因，
    //    否则表现为「装上但 STOPPED + 1067」，只能靠翻日志猜。
    if let Err(err) = service.start(&[] as &[&std::ffi::OsStr]) {
        // 已经 RUNNING（1056）之类的良性错误：只要管道能连就算成功。
        if !wait_pipe(Duration::from_secs(3)) {
            return Err(anyhow::anyhow!("服务已注册但启动失败: {err}"));
        }
    }

    // 6) 等命名管道可连（≤10s）。这一步同时验证了管道 ACL。
    if wait_pipe(Duration::from_secs(10)) {
        return Ok(());
    }
    bail!("服务已启动但命名管道未就绪（见 service.log）")
}

/// `sc stop` 并等到 STOPPED（或本来就没装）。
#[cfg(target_os = "windows")]
fn stop_service_blocking() {
    let _ = hidden("sc").args(["stop", SERVICE_LABEL]).output();
    for _ in 0..100 {
        let Ok(out) = hidden("sc").args(["query", SERVICE_LABEL]).output() else {
            return;
        };
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("STOPPED") || text.contains("1060") {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// 复制二进制，遇到「文件被占用」时退避重试。
#[cfg(target_os = "windows")]
fn copy_with_retry(src: &Path, dst: &Path) -> anyhow::Result<()> {
    let mut last = String::new();
    for _ in 0..25 {
        match std::fs::copy(src, dst) {
            Ok(_) => return Ok(()),
            Err(err) => {
                last = err.to_string();
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    bail!("复制服务二进制失败（{dst:?}）: {last}")
}

/// 能否连上命名管道（服务真正 accepting 的判据）。
#[cfg(target_os = "windows")]
fn wait_pipe(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(r"\\.\pipe\era.exvlan.studio.service")
            .is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// 清理旧模型留下的守护进程/计划任务（幂等）。当前服务名绝不在列表里。
#[cfg(target_os = "windows")]
fn cleanup_legacy_windows() {
    // 旧「内核直跑」服务
    for stale in ["era.exvlan.studio", "exvlan-studio"] {
        if stale == SERVICE_LABEL {
            continue;
        }
        let _ = hidden("sc").args(["stop", stale]).output();
        let _ = hidden("sc").args(["delete", stale]).output();
    }
    // 旧「普通模式高权限启动器」计划任务
    for task in ["era.exvlan.studio", "ExvlanKernelSupervisor"] {
        let _ = hidden("schtasks")
            .args(["/Delete", "/TN", task, "/F"])
            .output();
    }
}

#[cfg(target_os = "windows")]
fn replace_updater(p: &Paths, new_exe: Option<&str>) -> anyhow::Result<()> {
    let Some(new_exe) = new_exe else {
        bail!("--replace-updater 需要新二进制路径");
    };
    // 给主服务时间把 IPC 响应写完再动手。
    std::thread::sleep(Duration::from_millis(1200));

    // 1) 停服务（运行中的 exe 被文件锁占用）
    let _ = hidden("sc").args(["stop", SERVICE_LABEL]).output();
    for _ in 0..150 {
        let out = hidden("sc").args(["query", SERVICE_LABEL]).output()?;
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("STOPPED") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // 2) 备份 → 替换
    let backup = p.service_bin.with_extension("exe.prev");
    if p.service_bin.is_file() {
        let _ = std::fs::copy(&p.service_bin, &backup);
    }
    if let Err(err) = std::fs::copy(new_exe, &p.service_bin) {
        let _ = hidden("sc").args(["start", SERVICE_LABEL]).output();
        return Err(anyhow::anyhow!("替换服务二进制失败: {err}"));
    }
    let _ = write_version_file(p);

    // 3) 拉起；失败回滚
    let start = hidden("sc").args(["start", SERVICE_LABEL]).output()?;
    if !start.status.success() {
        let _ = std::fs::copy(&backup, &p.service_bin);
        let _ = hidden("sc").args(["start", SERVICE_LABEL]).output();
        bail!("替换后启动失败，已回滚");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn replace_updater(p: &Paths, new_exe: Option<&str>) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(new_exe) = new_exe else {
        bail!("--replace-updater 需要新二进制路径");
    };
    // 给主服务时间把 IPC 响应写完再动手。
    std::thread::sleep(Duration::from_millis(1200));

    let backup = p.service_bin.with_extension("prev");
    if p.service_bin.is_file() {
        let _ = std::fs::copy(&p.service_bin, &backup);
    }
    std::fs::copy(new_exe, &p.service_bin).context("替换服务二进制失败")?;
    let _ = std::fs::set_permissions(&p.service_bin, std::fs::Permissions::from_mode(0o755));
    chown_root(&p.service_bin);
    let _ = write_version_file(p);

    // kickstart -k：杀掉旧进程并按 plist 重新拉起（此时已是新二进制）。
    let out = Command::new("/bin/launchctl")
        .args(["kickstart", "-k", &format!("system/{SERVICE_LABEL}")])
        .output()?;
    if !out.status.success() {
        let _ = std::fs::copy(&backup, &p.service_bin);
        let _ = Command::new("/bin/launchctl")
            .args(["kickstart", "-k", &format!("system/{SERVICE_LABEL}")])
            .output();
        bail!(
            "替换后重启失败，已回滚: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn replace_updater(_p: &Paths, _new_exe: Option<&str>) -> anyhow::Result<()> {
    bail!("exvlan-service 只在 macOS/Windows 上运行")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn install(_p: &Paths, _uid: u32) -> anyhow::Result<()> {
    bail!("exvlan-service 只在 macOS/Windows 上安装")
}

fn uninstall(_p: &Paths) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("/bin/launchctl")
            .args(["bootout", &format!("system/{SERVICE_LABEL}")])
            .output();
        let _ = std::fs::remove_file("/Library/LaunchDaemons/era.exvlan.studio.service.plist");
        let _ = std::fs::remove_file(&_p.service_bin);
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        let _ = hidden("sc")
            .args(["stop", "era.exvlan.studio.service"])
            .output();
        let _ = hidden("sc")
            .args(["delete", "era.exvlan.studio.service"])
            .output();
        return Ok(());
    }
    #[allow(unreachable_code)]
    Ok(())
}

// ---------------------------------------------------------------------------
// 诊断
// ---------------------------------------------------------------------------

fn diagnose(p: &Paths) -> anyhow::Result<()> {
    let mut out = String::new();
    out.push_str(&format!("service: {BUILD_VERSION}\n"));
    out.push_str(&format!(
        "config: {}\n",
        serde_json::to_string(&load_config(p)).unwrap_or_default()
    ));
    out.push_str(&format!("mode(boot): {:?}\n", load_config(p).mode));
    #[allow(unused_mut)]
    let mut probes: Vec<(&str, &Path)> = vec![
        ("service.bin", &p.service_bin),
        ("kernel", &p.kernel),
        ("kernel.prev", &p.kernel_prev),
        ("service.json", &p.service_json),
    ];
    #[cfg(unix)]
    {
        let sock = socket_path();
        probes.push(("socket", Box::leak(sock.into_boxed_path())));
    }
    for (label, path) in probes {
        out.push_str(&format!(
            "{label}: {} exists={}\n",
            path.display(),
            path.exists()
        ));
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(o) = Command::new("/bin/launchctl")
            .args(["print", &format!("system/{SERVICE_LABEL}")])
            .output()
        {
            let text = String::from_utf8_lossy(&o.stdout);
            let head: String = text.lines().take(8).collect::<Vec<_>>().join("\n");
            out.push_str(&format!("launchctl:\n{head}\n"));
        }
        if let Ok(plist) =
            std::fs::read_to_string("/Library/LaunchDaemons/era.exvlan.studio.service.plist")
        {
            out.push_str(&format!("plist bytes: {}\n", plist.len()));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(o) = hidden("sc")
            .args(["query", "era.exvlan.studio.service"])
            .output()
        {
            out.push_str(&format!(
                "sc query:\n{}\n",
                String::from_utf8_lossy(&o.stdout)
            ));
        }
    }
    if let Ok(log) = std::fs::read_to_string(&p.service_log) {
        let tail: Vec<&str> = log.lines().rev().take(50).collect();
        out.push_str("--- log tail ---\n");
        for line in tail.into_iter().rev() {
            out.push_str(line);
            out.push('\n');
        }
    }
    print!("{out}");

    // 同时落一份临时文件，方便用户直接发出来。
    let tmp = std::env::temp_dir().join("era.exvlan.studio-service-diagnose.txt");
    if std::fs::write(&tmp, &out).is_ok() {
        eprintln!("\n(saved to {})", tmp.display());
    }
    Ok(())
}
