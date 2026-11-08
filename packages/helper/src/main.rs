//! EXVlan 特权助手（macOS）。
//!
//! 目标：**首次安装授权一次，之后升级内核/服务不再弹密码**。
//!
//! 与 ClashBar / Clash Verge 的 service 同一思路：
//! - 安装：`exvlan-helper --install <uid>`（由 GUI 通过一次 osascript 提权执行）
//!   → 复制自身到 `/Library/PrivilegedHelperTools/`、写 LaunchDaemon plist、bootstrap。
//! - 常驻：`exvlan-helper --daemon`（launchd 以 root 运行），监听
//!   `/var/run/era.exvlan.studio.helper.sock`。
//! - 客户端：`--apply <staging>` / `--status` / `--restart-service` 由 GUI 以当前用户
//!   执行，通过 socket 请求；服务端用 peercred 校验调用者 uid。
//!
//! 安全边界：
//! 1. 只安装 **minisign 签名校验通过** 的内核；
//! 2. staging 目录必须属于安装者 uid 且不可被 group/other 写；
//! 3. 只操作固定路径，不接受任意目标。

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

const HELPER_LABEL: &str = "era.exvlan.studio.helper";
const HELPER_VERSION: u32 = 1;
/// 与服务端/系统路径保持一致。
const SERVICE_LABEL: &str = "era.exvlan.studio";
/// 与 `packages/studio/src-tauri/tauri.conf.json` 里 updater 公钥同一条。
const MINISIGN_PUBKEY: &str = "RWSFKUboUUaFQYx/ap73a7ySNbuI5kjDGsiVJBtceshu+Ml99YPMFqe9";

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    pub const HELPER_BIN: &str = "/Library/PrivilegedHelperTools/era.exvlan.studio.helper";
    pub const HELPER_PLIST: &str = "/Library/LaunchDaemons/era.exvlan.studio.helper.plist";
    pub const HELPER_VERSION_FILE: &str =
        "/Library/PrivilegedHelperTools/era.exvlan.studio.helper.version";
    pub const SOCKET_PATH: &str = "/var/run/era.exvlan.studio.helper.sock";
    pub const KERNEL_DIR: &str = "/Library/Application Support/era.exvlan.studio/core";
    pub const KERNEL_BIN: &str = "exvlan";
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub const HELPER_BIN: &str = "";
    pub const HELPER_PLIST: &str = "";
    pub const HELPER_VERSION_FILE: &str = "";
    pub const SOCKET_PATH: &str = "";
    pub const KERNEL_DIR: &str = "";
    pub const KERNEL_BIN: &str = "";
}

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    op: String,
    #[serde(default)]
    staging: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Response {
    ok: bool,
    message: String,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--install") => {
            let uid = args.get(1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
            install(uid)
        }
        Some("--daemon") => run_daemon(),
        Some("--apply") => client(Request {
            op: "apply".into(),
            staging: args.get(1).cloned(),
        }),
        Some("--status") => client(Request {
            op: "status".into(),
            staging: None,
        }),
        Some("--restart-service") => client(Request {
            op: "restart_service".into(),
            staging: None,
        }),
        Some("--version") => {
            println!("{}", HELPER_VERSION);
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "usage: exvlan-helper --install <uid> | --daemon | --apply <dir> | --status | --restart-service"
            );
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// 安装（root）
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn install(owner_uid: u32) -> ExitCode {
    match install_inner(owner_uid) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("install failed: {err:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn install(_owner_uid: u32) -> ExitCode {
    eprintln!("exvlan-helper is macOS only");
    ExitCode::FAILURE
}

#[cfg(target_os = "macos")]
fn install_inner(owner_uid: u32) -> anyhow::Result<()> {
    use platform::*;
    use std::os::unix::fs::PermissionsExt as _;

    let exe = std::env::current_exe().context("定位自身可执行文件")?;
    std::fs::create_dir_all("/Library/PrivilegedHelperTools").ok();
    std::fs::create_dir_all("/Library/LaunchDaemons").ok();

    std::fs::copy(&exe, HELPER_BIN).with_context(|| format!("复制 helper 到 {HELPER_BIN}"))?;
    std::fs::set_permissions(HELPER_BIN, std::fs::Permissions::from_mode(0o755))?;
    strip_quarantine(Path::new(HELPER_BIN));

    std::fs::write(HELPER_PLIST, plist()).with_context(|| format!("写入 {HELPER_PLIST}"))?;
    std::fs::set_permissions(HELPER_PLIST, std::fs::Permissions::from_mode(0o644))?;
    strip_quarantine(Path::new(HELPER_PLIST));

    // 先注销，避免 bootstrap 撞上旧的异步注销。
    let _ = std::process::Command::new("/bin/launchctl")
        .args(["bootout", &format!("system/{HELPER_LABEL}")])
        .output();
    std::thread::sleep(Duration::from_millis(300));

    let out = std::process::Command::new("/bin/launchctl")
        .args(["bootstrap", "system", HELPER_PLIST])
        .output()
        .context("执行 launchctl bootstrap")?;
    if !out.status.success() {
        // 已经 bootstrap 时允许继续（幂等）。
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stderr.contains("already") && !stderr.contains("Input/output error") {
            bail!("launchctl bootstrap 失败: {}", stderr.trim());
        }
    }
    let _ = std::process::Command::new("/bin/launchctl")
        .args(["kickstart", "-k", &format!("system/{HELPER_LABEL}")])
        .output();

    std::fs::write(
        HELPER_VERSION_FILE,
        format!("{HELPER_VERSION} {owner_uid}\n"),
    )?;
    std::fs::set_permissions(HELPER_VERSION_FILE, std::fs::Permissions::from_mode(0o644))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn plist() -> String {
    use platform::*;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{HELPER_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{HELPER_BIN}</string>
    <string>--daemon</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardOutPath</key><string>/var/log/{HELPER_LABEL}.log</string>
  <key>StandardErrorPath</key><string>/var/log/{HELPER_LABEL}.log</string>
</dict>
</plist>
"#
    )
}

#[cfg(target_os = "macos")]
fn strip_quarantine(path: &Path) {
    let _ = std::process::Command::new("/usr/bin/xattr")
        .args(["-d", "com.apple.quarantine"])
        .arg(path)
        .output();
}

// ---------------------------------------------------------------------------
// 守护进程（root）
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn run_daemon() -> ExitCode {
    use platform::*;
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::net::UnixListener;

    let _ = std::fs::remove_file(SOCKET_PATH);
    let listener = match UnixListener::bind(SOCKET_PATH) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("bind {SOCKET_PATH} failed: {err}");
            return ExitCode::FAILURE;
        }
    };
    let _ = std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o666));

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(600)));
                let response = match handle_client(&mut stream) {
                    Ok(response) => response,
                    Err(err) => Response {
                        ok: false,
                        message: format!("{err:#}"),
                    },
                };
                let _ = serde_json::to_writer(&mut stream, &response);
                let _ = stream.write_all(b"\n");
            }
            Err(err) => eprintln!("accept failed: {err}"),
        }
    }
    ExitCode::SUCCESS
}

#[cfg(not(target_os = "macos"))]
fn run_daemon() -> ExitCode {
    eprintln!("exvlan-helper is macOS only");
    ExitCode::FAILURE
}

#[cfg(target_os = "macos")]
fn handle_client(stream: &mut std::os::unix::net::UnixStream) -> anyhow::Result<Response> {
    let owner = recorded_owner_uid().ok_or_else(|| anyhow::anyhow!("helper 未记录安装者 uid"))?;
    let peer = peer_uid(stream).ok_or_else(|| anyhow::anyhow!("无法读取连接方 uid"))?;
    if peer != owner {
        bail!("拒绝 uid={peer}（安装者为 uid={owner}）");
    }

    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    let request: Request = serde_json::from_str(line.trim()).context("解析请求 JSON 失败")?;

    match request.op.as_str() {
        "status" => Ok(Response {
            ok: true,
            message: format!(
                "helper={HELPER_VERSION} owner={owner} kernel={}",
                installed_kernel_version()
            ),
        }),
        "restart_service" => {
            kickstart_service()?;
            Ok(Response {
                ok: true,
                message: "service restarted".into(),
            })
        }
        "apply" => {
            let staging = request
                .staging
                .ok_or_else(|| anyhow::anyhow!("apply 缺少 staging"))?;
            apply_payload(Path::new(&staging), owner)?;
            Ok(Response {
                ok: true,
                message: "kernel applied".into(),
            })
        }
        other => bail!("未知操作: {other}"),
    }
}

#[cfg(target_os = "macos")]
fn recorded_owner_uid() -> Option<u32> {
    let text = std::fs::read_to_string(platform::HELPER_VERSION_FILE).ok()?;
    let mut parts = text.split_whitespace();
    let _version = parts.next()?;
    parts.next()?.parse().ok()
}

/// macOS：`getsockopt(SOL_LOCAL, LOCAL_PEERCRED)`。
#[cfg(target_os = "macos")]
fn peer_uid(stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd as _;

    #[repr(C)]
    struct Xucred {
        cr_version: u32,
        cr_uid: u32,
        cr_ngroups: i16,
        cr_groups: [u32; 16],
    }

    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERCRED: libc::c_int = 0x001;

    let mut cred = Xucred {
        cr_version: 0,
        cr_uid: 0,
        cr_ngroups: 0,
        cr_groups: [0; 16],
    };
    let mut len = std::mem::size_of::<Xucred>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            SOL_LOCAL,
            LOCAL_PEERCRED,
            &mut cred as *mut Xucred as *mut libc::c_void,
            &mut len,
        )
    };
    (ret == 0).then_some(cred.cr_uid)
}

#[cfg(target_os = "macos")]
fn apply_payload(staging: &Path, owner_uid: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt as _;

    let meta = std::fs::metadata(staging).context("staging 不存在")?;
    if meta.uid() != owner_uid {
        bail!("staging 属主不是安装者");
    }
    if meta.mode() & 0o022 != 0 {
        bail!("staging 可被 group/other 写，拒绝执行");
    }

    let kernel = staging.join("kernel");
    let signature = staging.join("kernel.sig");
    if !kernel.is_file() || !signature.is_file() {
        bail!("staging 缺少 kernel / kernel.sig");
    }
    verify_minisign(&kernel, &signature).context("内核签名校验失败")?;

    let version = std::fs::read_to_string(staging.join("version"))
        .unwrap_or_default()
        .trim()
        .to_string();

    let dest_dir = Path::new(platform::KERNEL_DIR);
    std::fs::create_dir_all(dest_dir)?;
    let dest = dest_dir.join(platform::KERNEL_BIN);
    let tmp = dest_dir.join(format!("{}.new", platform::KERNEL_BIN));
    std::fs::copy(&kernel, &tmp)?;
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &dest)?;

    if !version.is_empty() {
        std::fs::write(dest_dir.join("version"), version)?;
    }

    // 可选：同一份签名包内带新 helper 时顺带自更新（rename 覆盖运行中的二进制是安全的）。
    if staging.join("helper").is_file() && staging.join("helper.sig").is_file() {
        verify_minisign(&staging.join("helper"), &staging.join("helper.sig"))
            .context("helper 签名校验失败")?;
        let tmp = Path::new(platform::HELPER_BIN).with_extension("new");
        std::fs::copy(staging.join("helper"), &tmp)?;
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        let _ = std::fs::rename(&tmp, platform::HELPER_BIN);
    }

    kickstart_service()?;
    let _ = std::fs::remove_dir_all(staging);
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify_minisign(file: &Path, signature: &Path) -> anyhow::Result<()> {
    let public_key =
        minisign_verify::PublicKey::from_base64(MINISIGN_PUBKEY).context("解析内置公钥失败")?;
    let signature_text = std::fs::read_to_string(signature)?;
    let signature =
        minisign_verify::Signature::decode(&signature_text).context("解析签名文件失败")?;
    let data = std::fs::read(file)?;
    public_key
        .verify(&data, &signature, false)
        .or_else(|_| public_key.verify(&data, &signature, true))
        .context("签名不匹配")
}

#[cfg(target_os = "macos")]
fn kickstart_service() -> anyhow::Result<()> {
    let out = std::process::Command::new("/bin/launchctl")
        .args(["kickstart", "-k", &format!("system/{SERVICE_LABEL}")])
        .output()
        .context("执行 launchctl kickstart")?;
    if !out.status.success() {
        bail!(
            "launchctl kickstart 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn installed_kernel_version() -> String {
    std::fs::read_to_string(Path::new(platform::KERNEL_DIR).join("version"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

// ---------------------------------------------------------------------------
// 客户端（当前用户）
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn client(request: Request) -> ExitCode {
    use platform::SOCKET_PATH;
    use std::os::unix::net::UnixStream;

    let mut stream = match UnixStream::connect(SOCKET_PATH) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("connect {SOCKET_PATH} failed: {err}");
            return ExitCode::FAILURE;
        }
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(600)));
    let payload = match serde_json::to_string(&request) {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!("encode request failed: {err}");
            return ExitCode::FAILURE;
        }
    };
    if stream.write_all(format!("{payload}\n").as_bytes()).is_err() {
        return ExitCode::FAILURE;
    }
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line).is_err() {
        return ExitCode::FAILURE;
    }
    match serde_json::from_str::<Response>(line.trim()) {
        Ok(response) => {
            println!("{}", response.message);
            if response.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("decode response failed: {err} ({line})");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn client(_request: Request) -> ExitCode {
    eprintln!("exvlan-helper is macOS only");
    ExitCode::FAILURE
}

// 让非 macOS 编译时 `PathBuf` 等导入不被判为未使用。
#[allow(dead_code)]
fn _unused(_: PathBuf) {}
