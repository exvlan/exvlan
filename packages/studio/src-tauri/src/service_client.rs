//! `exvlan-service` 客户端：GUI 与唯一常驻特权进程之间的 IPC。
//!
//! macOS / Linux：unix socket（`/var/run/<app>/service.sock`）；Windows：命名管道。
//! Linux 的特权服务由 systemd 托管（§14.12），与 macOS/Windows 同一套 IPC；
//! 服务**没有装/没起来**时仍回落到 supervisor（setcap + PDEATHSIG），
//! 保证「装不上服务」不会让界面连不上本机内核。
//!
//! 设计要点：
//! - 模式只是生命周期策略：`mode.set(service ↔ normal)` **不重启内核**；
//! - normal 模式靠**连接存活**：GUI 保持一条 session 连接，断开即停内核；
//! - 所有权：安装时记录 owner uid（mac peercred 校验）；
//! - 升级零弹窗：服务已装时，新二进制通过 IPC 交给服务自己替换（`service.update`）。

#![allow(dead_code)]

use std::path::PathBuf;

/// 服务 label（macOS plist / Windows 服务名）。
pub const SERVICE_LABEL: &str = crate::paths::PRIVILEGED_SERVICE_LABEL;

/// `foo.exe` → `foo.exe.sig`（`with_extension("sig")` 会得到 `foo.sig`，
/// 而 `tauri signer sign` / CI 生成的是 `<完整文件名>.sig`）。
pub fn sig_path(bin: &std::path::Path) -> PathBuf {
    let mut raw = bin.as_os_str().to_os_string();
    raw.push(".sig");
    PathBuf::from(raw)
}

pub fn current_uid() -> u32 {
    #[cfg(unix)]
    unsafe {
        libc::getuid() as u32
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// 随包的服务二进制（`resources/service/exvlan-service`）。
pub fn bundled_service(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager as _;
    let dir = app.path().resource_dir().ok()?;
    let bin = if cfg!(windows) {
        "exvlan-service.exe"
    } else {
        "exvlan-service"
    };
    [
        dir.join("service").join(bin),
        dir.join("resources").join("service").join(bin),
        dir.join(bin),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// 随包的 askpass 脚本（`resources/tools/exvlan-askpass.sh`，Linux `sudo -A` 用）。
pub const ASKPASS_SCRIPT: &str = "exvlan-askpass.sh";

/// 随包的控制面工具目录（`resources/tools/`：cli / gateway / migrate）。
pub fn bundled_tools_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager as _;
    let dir = app.path().resource_dir().ok()?;
    [
        dir.join("tools"),
        dir.join("resources").join("tools"),
        dir.clone(),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
}

/// 已安装的服务二进制落点（必须与 `exvlan-service` 内部的 `paths()` 一致）。
pub fn installed_service_bin() -> PathBuf {
    crate::paths::privileged_service_bin()
}

/// 二进制旁边的 `service.version`（服务在 install / 自更新时写入，0644）。
pub fn version_file_path(bin: &std::path::Path) -> PathBuf {
    let mut raw = bin.as_os_str().to_os_string();
    raw.push(".version");
    PathBuf::from(raw)
}

/// 服务的三态。
///
/// 只靠 `installed` / `ready` 两个布尔判断是不够的 —— dev.1 现场就是这么误判的：
/// 服务在跑、只是管道一时连不上，前端把它当成「未安装」去重装，
/// 于是撞上「覆盖运行中的 exe」(os error 32) 并弹出 UAC。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceHealth {
    /// 没装好：二进制不在，**或**系统里没有注册项（卸载后重装、手工 `sc delete` 过）
    /// → **只有这一态**才应该走「安装」（`install` 会把注册补回来）。
    NotInstalled,
    /// 装了但 IPC 不通（服务停了 / 崩过 / 升级窗口内）→ 应当**重启**，不要重装。
    InstalledNotReady,
    /// IPC 可用 → 健康。
    InstalledRunning,
}

/// 服务当前状态（供界面判断是否要显示横幅 / 诊断包）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceState {
    pub installed: bool,
    pub ready: bool,
    pub health: ServiceHealth,
    pub mode: String,
    pub kernel_pid: u32,
    pub status: String,
}

/// 服务是否已在系统里注册。
///
/// Windows 上二进制与注册项是**两份状态**：MSI 卸载会 `sc delete`（本次改动）而
/// ProgramData 里的二进制保留 → 「二进制在、注册项没了」。这种机器必须走
/// `install` 重新注册，不能走 `repair`（`--restart` 对不存在的服务只会报错）。
#[cfg(target_os = "windows")]
fn service_registered() -> bool {
    !matches!(
        crate::service::status(),
        Ok(exvlan_rpc::service_manager::ServiceStatus::NotInstalled)
    )
}

/// 其它平台由 `is_installed()` 的判据覆盖（macOS 看 plist，Linux 看二进制）。
#[cfg(not(target_os = "windows"))]
fn service_registered() -> bool {
    true
}

fn health_of(installed: bool, registered: bool, ready: bool) -> ServiceHealth {
    if !installed || !registered {
        ServiceHealth::NotInstalled
    } else if ready {
        ServiceHealth::InstalledRunning
    } else {
        ServiceHealth::InstalledNotReady
    }
}

/// 「装了但 IPC 不通」时给用户的可操作提示（不要只说「未就绪」）。
fn not_ready_hint() -> String {
    // 用 cfg! 而非 #[cfg] 属性：这个函数在移动端也会被编译，
    // 缺一个绑定就会在 android/ios 上编译失败。
    let inspect = if cfg!(target_os = "windows") {
        format!("sc query {SERVICE_LABEL}")
    } else if cfg!(target_os = "macos") {
        format!("launchctl print system/{SERVICE_LABEL}")
    } else {
        format!("systemctl status {SERVICE_LABEL}")
    };
    format!(
        "服务已注册，但 IPC 仍连不上（内核可能还在冷启动）。\n\
         · 查看状态：{inspect}\n\
         · 服务日志：{}\n\
         · 也可以先退出界面等 30 秒再打开；仍不行时用「设置 → 特权服务 → 重试」或「强制重装」。",
        crate::paths::service_log().display()
    )
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub use imp::{
    apply_bundled, ensure_kernel, ensure_normal, ensure_service, install, installed_version,
    is_installed, is_ready, kernel_status, pair_start, port_cleanup, port_owner, repair,
    restart_kernel, set_extra_whitelist, set_mode, state, stop, stop_kernel, update,
    whitelist_info,
};

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub use stub::{
    apply_bundled, ensure_kernel, ensure_normal, ensure_service, install, installed_version,
    is_installed, is_ready, kernel_status, pair_start, port_cleanup, port_owner, repair,
    restart_kernel, set_extra_whitelist, set_mode, state, stop, stop_kernel, update,
    whitelist_info,
};

// ---------------------------------------------------------------------------
// macOS / Linux / Windows：IPC 实现
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
mod imp {
    use super::*;
    use std::io::{BufRead as _, BufReader, Write};
    use std::sync::Mutex;

    use exvlan_control::protocol::{Payload, Request, Response, WireRequest};

    /// 保持打开的一条 session（normal 模式的存活锚点）。
    struct Session {
        writer: Box<dyn Write + Send>,
        #[allow(dead_code)]
        reader: BufReader<Box<dyn std::io::Read + Send>>,
    }

    static SESSION: Mutex<Option<Session>> = Mutex::new(None);

    #[cfg(unix)]
    fn connect() -> Result<Session, String> {
        let path = crate::paths::privileged_service_socket();
        let stream = std::os::unix::net::UnixStream::connect(&path)
            .map_err(|e| format!("连接 {} 失败: {e}", path.display()))?;
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(30)));
        let writer = Box::new(stream.try_clone().map_err(|e| e.to_string())?);
        let reader = BufReader::new(Box::new(stream) as Box<dyn std::io::Read + Send>);
        Ok(Session { writer, reader })
    }

    #[cfg(target_os = "windows")]
    fn connect() -> Result<Session, String> {
        use std::fs::OpenOptions;
        // 命名管道在 Rust 标准库里按文件打开即可（读写）。
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(r"\\.\pipe\era.exvlan.studio.service")
            .map_err(|e| format!("连接服务管道失败: {e}"))?;
        let writer = Box::new(file.try_clone().map_err(|e| e.to_string())?);
        let reader = BufReader::new(Box::new(file) as Box<dyn std::io::Read + Send>);
        Ok(Session { writer, reader })
    }

    /// 一次性类型化调用（连上 → 发 → 断开）。
    fn call(req: &Request) -> Result<Response, String> {
        let mut session = connect()?;
        send(&mut session, req)?;
        read_reply(&mut session)
    }

    fn send(session: &mut Session, req: &Request) -> Result<(), String> {
        let wire = serde_json::to_string(&WireRequest::new(req.clone()))
            .map_err(|e| format!("序列化请求失败: {e}"))?;
        writeln!(session.writer, "{wire}").map_err(|e| format!("发送请求失败: {e}"))?;
        session.writer.flush().ok();
        Ok(())
    }

    fn read_reply(session: &mut Session) -> Result<Response, String> {
        let mut line = String::new();
        session
            .reader
            .read_line(&mut line)
            .map_err(|e| format!("读取响应失败: {e}"))?;
        if line.trim().is_empty() {
            return Err("服务返回空响应".to_string());
        }
        let resp: Response = serde_json::from_str(line.trim()).map_err(|e| {
            format!("响应不是协议 v2（服务端可能还是旧版，请把客户端与服务一起升级）: {e}")
        })?;
        // 与 `exvlan-control` 的客户端同一套判据（含 proto 校验与错误文案）。
        exvlan_control::ipc::into_result(resp)
    }

    /// 把响应负载转成 JSON（前端 / 诊断包按 JSON 消费）。
    fn data_json(resp: Response) -> serde_json::Value {
        resp.data
            .map(|payload| serde_json::to_value(&payload).unwrap_or_default())
            .unwrap_or_default()
    }

    /// 建立（或复用）session；normal 模式的生命周期由它决定。
    fn ensure_session() -> Result<(), String> {
        let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return Ok(());
        }
        let mut session = connect()?;
        send(&mut session, &Request::Hello).map_err(|e| format!("服务未响应 hello: {e}"))?;
        read_reply(&mut session).map_err(|e| format!("服务未响应 hello: {e}"))?;
        *guard = Some(session);
        Ok(())
    }

    pub fn is_installed() -> bool {
        #[cfg(target_os = "macos")]
        {
            installed_service_bin().is_file() && crate::paths::privileged_service_plist().is_file()
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            // Linux：systemd 由服务自己 `--install` 写 unit，GUI 只看二进制是否落盘
            // （与 Windows 一致；版本串在 `<bin>.version` 里）。
            installed_service_bin().is_file()
        }
    }

    /// 已安装服务的版本：优先读版本文件（零子进程、零窗口），
    /// 文件缺失时才回退到 `--version`（用 `hidden_command` 避免闪 cmd 窗口）。
    pub fn installed_version() -> Option<String> {
        let bin = installed_service_bin();
        if !bin.is_file() {
            return None;
        }
        if let Ok(text) = std::fs::read_to_string(version_file_path(&bin)) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        let out = crate::kernel::hidden_command(&bin)
            .arg("--version")
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        // 形如 "exvlan-service 26.410.20715-pre.13+abcd"
        text.split_whitespace().nth(1).map(str::to_owned)
    }

    pub fn is_ready() -> bool {
        call(&Request::Ping).is_ok()
    }

    pub fn status() -> Result<String, String> {
        let st = call(&Request::Status)?;
        let Some(Payload::Status { mode, kernel, .. }) = st.data else {
            return Err("服务没有返回 status 负载".to_string());
        };
        let pid = kernel.map(|k| k.pid).unwrap_or(0);
        // 版本串只有 `hello` 上报（status 只管运行态）。
        let service = match call(&Request::Hello).ok().and_then(|h| h.data) {
            Some(Payload::Hello { service, .. }) => service,
            _ => "-".to_string(),
        };
        Ok(format!(
            "service={SERVICE_LABEL} v={service} mode={} kernel_pid={pid}",
            mode.as_str()
        ))
    }

    pub fn state() -> ServiceState {
        let installed = is_installed();
        // 「注册项是否在」只在 Windows 有意义（MSI 卸载会 sc delete 而保留二进制）。
        let registered = installed && service_registered();
        match call(&Request::Status) {
            Ok(resp) => match resp.data {
                Some(Payload::Status { mode, kernel, .. }) => ServiceState {
                    installed,
                    ready: true,
                    health: health_of(installed, registered, true),
                    mode: mode.as_str().to_string(),
                    kernel_pid: kernel.map(|k| k.pid).unwrap_or(0),
                    status: "ok".to_string(),
                },
                _ => ServiceState {
                    installed,
                    ready: true,
                    health: health_of(installed, registered, true),
                    mode: "unknown".to_string(),
                    kernel_pid: 0,
                    status: "服务没有返回 status 负载".to_string(),
                },
            },
            Err(err) => ServiceState {
                installed,
                ready: false,
                health: health_of(installed, registered, false),
                mode: "unknown".to_string(),
                kernel_pid: 0,
                status: err,
            },
        }
    }

    /// 「装了但没就绪」的修复：**只重启服务**，绝不覆盖二进制。
    ///
    /// 与 `install` 的分工：安装只在 `NotInstalled` 时用。升级窗口/服务崩过之后
    /// 服务本体还在，重装既没必要，还会在服务运行时覆盖 `bin\exvlan-service.exe`
    /// → 「另一个程序正在使用此文件 (os error 32)」→ 界面误报安装失败（dev.1 现场）。
    pub fn repair() -> Result<(), String> {
        if !is_installed() || !service_registered() {
            return Err(format!(
                "服务未安装或注册项已丢失（{}），请重新安装（会重新注册）而不是重启",
                installed_service_bin().display()
            ));
        }
        crate::logger::record("service.repair", "restart 已注册服务（不覆盖二进制）");
        crate::run_elevated(&["--exvlan-service-ctl", "restart"])?;
        // 服务重启会重新拉起内核/gateway；内核冷启动最长等 10s RPC，给 30s 余量。
        for _ in 0..150 {
            if is_ready() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err(not_ready_hint())
    }

    /// 首次安装（提权一次）：跑随包 `exvlan-service --install <uid>`。
    #[cfg(not(target_os = "linux"))]
    pub fn install(app: &tauri::AppHandle) -> Result<(), String> {
        // 按平台拼文件名：Windows 是 .exe，写死会让错误信息本身就无法执行。
        let bin = if cfg!(windows) {
            "exvlan-service.exe"
        } else {
            "exvlan-service"
        };
        let src = bundled_service(app)
            .ok_or_else(|| format!("安装包内未找到服务二进制（resources/service/{bin}）"))?;
        let uid = current_uid().to_string();
        // Windows：把**当前用户**的 SID 交给安装器，服务据此把命名管道 DACL
        // 收窄到本人（服务以 LocalSystem 跑，事后反推不出是谁装的）。
        // 取不到就传空串 → 服务退回 `IU`（不会锁门）。
        let sid = exvlan_control::current_user_sid().unwrap_or_default();
        let tools = bundled_tools_dir(app)
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        crate::run_elevated(&[
            "--install-exvlan-service",
            &src.display().to_string(),
            &uid,
            &sid,
            &tools,
        ])?;
        // 安装脚本内部已等过就绪，这里再兜一次；冷启动 + 内核拉起给 30s。
        for _ in 0..150 {
            if is_ready() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err(not_ready_hint())
    }

    /// Linux：提权跑随包 `exvlan-service --install <uid> [--tools-dir <dir>]`，
    /// 由服务自己写 systemd unit、`enable --now` 并等 `sd_notify READY=1`。
    #[cfg(target_os = "linux")]
    pub fn install(app: &tauri::AppHandle) -> Result<(), String> {
        let src = bundled_service(app).ok_or_else(|| {
            "安装包内未找到服务二进制（resources/service/exvlan-service）".to_string()
        })?;
        let uid = current_uid();
        let tools = bundled_tools_dir(app);

        let mut args: Vec<String> = vec!["--install".to_string(), uid.to_string()];
        if let Some(dir) = tools.as_ref() {
            args.push("--tools-dir".to_string());
            args.push(dir.display().to_string());
        }

        let mut command = std::process::Command::new(&src);
        command.args(&args);
        // 提权链路：pkexec → sudo -A（见 `elevate::linux`）。
        match crate::elevate::Command::new(command).output() {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                let detail = {
                    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    if stdout.is_empty() { stderr } else { stdout }
                };
                return Err(linux_install_help(
                    &src,
                    uid,
                    tools.as_deref(),
                    format!(
                        "安装特权服务失败（exit={}）：{}",
                        out.status,
                        if detail.is_empty() {
                            "（无输出）"
                        } else {
                            &detail
                        }
                    ),
                ));
            }
            Err(err) => {
                return Err(linux_install_help(
                    &src,
                    uid,
                    tools.as_deref(),
                    format!("无法提权执行安装: {err:#}"),
                ));
            }
        }

        // 服务侧已经等过 READY，这里再给 systemd 一点余量（冷启动时 unit 依赖较多）。
        for _ in 0..50 {
            if is_ready() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Err(format!(
            "服务已安装但 IPC 未就绪（{}）。排查：journalctl -u {SERVICE_LABEL} -n 20",
            crate::paths::privileged_service_socket().display()
        ))
    }

    /// 提权不可用/失败时补一段**可直接复制**的手动命令（界面直接展示这段文本）。
    #[cfg(target_os = "linux")]
    fn linux_install_help(
        src: &std::path::Path,
        uid: u32,
        tools: Option<&std::path::Path>,
        reason: String,
    ) -> String {
        let installed = installed_service_bin();
        let tools_arg = tools
            .map(|dir| format!(" --tools-dir '{}'", dir.display()))
            .unwrap_or_default();
        format!(
            "{reason}\n可在终端手动执行（复制即可）：\n  sudo cp '{src}' '{installed}'\n  sudo {installed} --install {uid}{tools_arg}",
            src = src.display(),
            installed = installed.display(),
        )
    }

    /// Linux：没有「服务自替换」（`--replace-updater` 仅 mac/win 实现），
    /// 版本不一致时直接提权重装一次，systemd 会把新二进制拉起来。
    #[cfg(target_os = "linux")]
    pub fn update(app: &tauri::AppHandle) -> Result<bool, String> {
        if installed_version().as_deref() == Some(crate::BUILD_VERSION) {
            return Ok(false);
        }
        install(app)?;
        Ok(true)
    }

    /// 免密升级服务本体：把新二进制交给运行中的服务，由它自己替换并重启。
    /// 返回 `true` = 确实做了替换。
    #[cfg(not(target_os = "linux"))]
    pub fn update(app: &tauri::AppHandle) -> Result<bool, String> {
        let want = crate::BUILD_VERSION;
        if installed_version().as_deref() == Some(want) {
            return Ok(false);
        }
        let src = bundled_service(app).ok_or_else(|| "安装包内未找到服务二进制".to_string())?;
        let sig = sig_path(&src);
        if !sig.is_file() {
            return Err(format!("缺少服务签名 {}", sig.display()));
        }
        let staging = std::env::temp_dir().join(format!(
            "era-exvlan-service-{}-{}",
            std::process::id(),
            crate::logger::timestamp().replace(':', "-")
        ));
        std::fs::create_dir_all(&staging).map_err(|e| format!("创建 staging 失败: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700));
        }
        let staged = staging.join(if cfg!(windows) {
            "exvlan-service.exe"
        } else {
            "exvlan-service"
        });
        std::fs::copy(&src, &staged).map_err(|e| e.to_string())?;
        std::fs::copy(&sig, staging.join("service.sig")).map_err(|e| e.to_string())?;
        // 随包工具一并交给服务替换：否则升级后 `core/` 里还是旧版 gateway/cli，
        // 「配对 / 写操作 / 服务托管」这些新能力等于装了但没生效。
        if let Some(tools) = bundled_tools_dir(app) {
            let dst = staging.join("tools");
            std::fs::create_dir_all(&dst).map_err(|e| format!("创建工具 staging 失败: {e}"))?;
            let exe = if cfg!(windows) { ".exe" } else { "" };
            for name in ["exvlan-cli", "exvlan-gateway"] {
                let file = format!("{name}{exe}");
                let from = tools.join(&file);
                if from.is_file() {
                    std::fs::copy(&from, dst.join(&file))
                        .map_err(|e| format!("拷贝 {file} 失败: {e}"))?;
                }
            }
        }

        let result = call(&Request::ServiceUpdate {
            staging: staging.display().to_string(),
            version: Some(want.to_string()),
        });
        let _ = std::fs::remove_dir_all(&staging);
        result?;

        // 服务侧只负责「拉住 updater 并立即返回」，真正的替换是异步的。
        // 这里必须确认版本真的变了，否则会在「装不上 → 下次启动又重装」
        // 之间死循环（pre.14 实测每 3 秒一次 sc stop/start）。
        for _ in 0..30 {
            if installed_version().as_deref() == Some(want) {
                return Ok(true);
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err(format!(
            "服务自更新后版本仍为 {:?}（期望 {want}，见 service.log），改走提权安装",
            installed_version()
        ))
    }

    pub fn set_mode(mode: &str) -> Result<(), String> {
        ensure_session()?;
        call(&Request::ModeSet {
            mode: mode.to_string(),
        })
        .map(|_| ())
    }

    pub fn ensure_kernel() -> Result<u32, String> {
        ensure_session()?;
        let resp = call(&Request::KernelEnsure)?;
        match resp.data {
            Some(Payload::KernelEnsure { kernel }) => Ok(kernel.map(|k| k.pid).unwrap_or(0)),
            _ => Err("服务没有返回 kernel.ensure 负载".to_string()),
        }
    }

    pub fn stop_kernel() -> Result<(), String> {
        call(&Request::KernelStop).map(|_| ())
    }

    /// 原子重启内核（停 → 清残留 → 起），期间服务端会挂起 session 的停机逻辑。
    pub fn restart_kernel() -> Result<serde_json::Value, String> {
        Ok(data_json(call(&Request::KernelRestart)?))
    }

    pub fn kernel_status() -> Result<(u32, bool), String> {
        let pid = match call(&Request::Status)?.data {
            Some(Payload::Status { kernel, .. }) => kernel.map(|k| k.pid).unwrap_or(0),
            _ => 0,
        };
        Ok((pid, pid != 0))
    }

    /// 普通模式：建 session → mode=normal → 确保内核在跑。
    ///
    /// Linux：服务没装/没起来时回落到 supervisor（setcap + PDEATHSIG 子进程）——
    /// 「装不上服务」不应该连本机内核都起不来。
    pub fn ensure_normal(_app: &tauri::AppHandle) -> Result<String, String> {
        #[cfg(target_os = "linux")]
        if !is_ready() {
            return super::stub::ensure_normal(_app);
        }
        set_mode("normal")?;
        ensure_kernel()?;
        Ok("127.0.0.1:1995".to_string())
    }

    /// 服务模式：建 session → mode=service → 确保内核在跑。
    ///
    /// Linux 无服务时内核已由提权 provision 起好，直接返回地址（与旧行为一致）。
    pub fn ensure_service(_app: &tauri::AppHandle) -> Result<String, String> {
        #[cfg(target_os = "linux")]
        if !is_ready() {
            return super::stub::ensure_service(_app);
        }
        set_mode("service")?;
        ensure_kernel()?;
        Ok("127.0.0.1:1995".to_string())
    }

    pub fn stop() {
        let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None; // 断开连接：normal 模式下服务会停内核
        #[cfg(target_os = "linux")]
        super::stub::stop(); // supervisor 回退路径（未装服务时）也要停
    }

    /// 生效的 RPC 白名单（回环 + 各虚拟网段 + 额外追加）。
    pub fn whitelist_info() -> Result<serde_json::Value, String> {
        Ok(data_json(call(&Request::WhitelistGet)?))
    }

    /// 设置额外网段；服务端会做 CIDR 校验并拒绝公网段。
    pub fn set_extra_whitelist(extra: &[String]) -> Result<serde_json::Value, String> {
        Ok(data_json(call(&Request::WhitelistSet {
            extra: extra.to_vec(),
        })?))
    }

    /// 生成本机一次性配对码（对端出码；服务托管时由服务调本机 gateway）。
    pub fn pair_start(role: &str, ttl: u64) -> Result<serde_json::Value, String> {
        Ok(data_json(call(&Request::GatewayPairStart {
            role: role.to_string(),
            ttl,
        })?))
    }

    /// 谁占着 RPC 端口（诊断用）。
    pub fn port_owner() -> Result<serde_json::Value, String> {
        Ok(data_json(call(&Request::PortOwner)?))
    }

    /// 结束占用 RPC 端口的**自家内核**（服务端会校验路径，非自家拒绝）。
    pub fn port_cleanup() -> Result<(), String> {
        call(&Request::PortCleanup).map(|_| ())
    }

    /// 免密换内核：把随包内核 + 签名 + 版本放进 staging，再让服务 apply。
    pub fn apply_bundled(app: &tauri::AppHandle) -> Result<(), String> {
        let kernel = crate::kernel::bundled_kernel(app).ok_or("未找到随包内核")?;
        let sig = sig_path(&kernel);
        if !sig.is_file() {
            return Err(format!("缺少内核签名 {}", sig.display()));
        }
        let staging = std::env::temp_dir().join(format!(
            "era-exvlan-apply-{}-{}",
            std::process::id(),
            crate::logger::timestamp().replace(':', "-")
        ));
        std::fs::create_dir_all(&staging).map_err(|e| format!("创建 staging 失败: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700));
        }
        std::fs::copy(&kernel, staging.join("kernel")).map_err(|e| e.to_string())?;
        std::fs::copy(&sig, staging.join("kernel.sig")).map_err(|e| e.to_string())?;
        std::fs::write(staging.join("version"), crate::BUILD_VERSION).map_err(|e| e.to_string())?;

        let req = Request::Apply {
            staging: staging.display().to_string(),
        };
        // 服务正在重启（例如刚做完自更新）时会读到空响应；等它起来再试一次。
        let result = match call(&req) {
            Err(err)
                if err.contains("空响应") || err.contains("os error 2") || err.contains("拒绝") =>
            {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                call(&req)
            }
            other => other,
        };
        let _ = std::fs::remove_dir_all(&staging);
        result.map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// 移动端：没有特权服务；Linux 未装服务时也用它回落（supervisor 起内核）
// ---------------------------------------------------------------------------

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod stub {
    use super::*;

    pub fn is_installed() -> bool {
        false
    }
    pub fn installed_version() -> Option<String> {
        None
    }
    pub fn is_ready() -> bool {
        true
    }
    pub fn status() -> Result<String, String> {
        Ok("unsupported (linux: systemd / setcap)".to_string())
    }
    pub fn state() -> ServiceState {
        ServiceState {
            installed: false,
            ready: true,
            health: ServiceHealth::NotInstalled,
            mode: "service".to_string(),
            kernel_pid: 0,
            status: "unsupported".to_string(),
        }
    }
    pub fn repair() -> Result<(), String> {
        Err("当前平台没有特权服务".to_string())
    }
    pub fn install(_app: &tauri::AppHandle) -> Result<(), String> {
        Ok(())
    }
    pub fn update(_app: &tauri::AppHandle) -> Result<bool, String> {
        Ok(false)
    }
    pub fn set_mode(_mode: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn ensure_kernel() -> Result<u32, String> {
        Ok(0)
    }
    pub fn stop_kernel() -> Result<(), String> {
        Ok(())
    }
    pub fn restart_kernel() -> Result<serde_json::Value, String> {
        Err("当前平台没有特权服务（core 模式由 systemd/GUI supervisor 管理内核）".to_string())
    }
    pub fn kernel_status() -> Result<(u32, bool), String> {
        Ok((0, false))
    }
    /// 移动端内核在进程内（FFI），没有可拉起的子进程。
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub fn ensure_normal(_app: &tauri::AppHandle) -> Result<String, String> {
        Ok("ffi".to_string())
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn ensure_normal(app: &tauri::AppHandle) -> Result<String, String> {
        crate::supervisor::ensure(app).map_err(|e| format!("{e:#}"))
    }
    pub fn ensure_service(_app: &tauri::AppHandle) -> Result<String, String> {
        Ok("127.0.0.1:1995".to_string())
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub fn stop() {}
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn stop() {
        crate::supervisor::stop();
    }
    pub fn apply_bundled(_app: &tauri::AppHandle) -> Result<(), String> {
        Err("Linux 没有特权服务（换内核走提权）".to_string())
    }
    pub fn whitelist_info() -> Result<serde_json::Value, String> {
        Err("当前平台没有特权服务".to_string())
    }
    pub fn set_extra_whitelist(_extra: &[String]) -> Result<serde_json::Value, String> {
        Err("当前平台没有特权服务".to_string())
    }
    /// 裸核平台没有特权服务托管 gateway：配对码用 `exvlan-cli auth pair` 直连 gateway。
    pub fn pair_start(_role: &str, _ttl: u64) -> Result<serde_json::Value, String> {
        Err("当前平台没有特权服务：请用 `exvlan-cli auth pair` 生成配对码".to_string())
    }
    pub fn port_owner() -> Result<serde_json::Value, String> {
        Err("当前平台没有特权服务".to_string())
    }
    pub fn port_cleanup() -> Result<(), String> {
        Err("当前平台没有特权服务".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三态判据（B4 的根因就是判据错误，这里把它钉住）。
    #[test]
    fn health_covers_three_states() {
        // 二进制缺失 → 必须走「安装」
        assert_eq!(
            health_of(false, false, false),
            ServiceHealth::NotInstalled,
            "二进制不在时必须判为未安装"
        );
        // 二进制在、注册项丢失（MSI 卸载 sc delete 后重装 / 手工 sc delete）→ 也要走「安装」
        assert_eq!(
            health_of(true, false, false),
            ServiceHealth::NotInstalled,
            "注册项丢失不能当成就绪或可重启"
        );
        // 装了但管道不通（服务停了 / 崩过 / 升级窗口内）→ 只重启
        assert_eq!(
            health_of(true, true, false),
            ServiceHealth::InstalledNotReady,
            "管道不通不能判为未安装（否则会重复覆盖 exe）"
        );
        // 健康
        assert_eq!(
            health_of(true, true, true),
            ServiceHealth::InstalledRunning,
            "IPC 可用即健康"
        );
    }

    /// 前端 `ServiceState.health` 的联合类型依赖这层序列化形状。
    #[test]
    fn health_serializes_kebab_case() {
        let cases = [
            (ServiceHealth::NotInstalled, "\"not-installed\""),
            (ServiceHealth::InstalledNotReady, "\"installed-not-ready\""),
            (ServiceHealth::InstalledRunning, "\"installed-running\""),
        ];
        for (health, want) in cases {
            assert_eq!(serde_json::to_string(&health).unwrap(), want);
        }
    }
}
