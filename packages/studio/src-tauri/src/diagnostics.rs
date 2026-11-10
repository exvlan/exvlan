//! 诊断 v2 采集：环境 / 网络 / 服务定义 / DNS 解析 / 会话标记。
//!
//! 设计原则：
//! - 尽量调用系统命令并保留**原文**（策略路由、launchctl、ACL 这些没有稳定 API）；
//! - 每个采集项都有独立超时，失败写进 manifest 的 `error`，绝不拖垮整包；
//! - 只读取，不修改系统状态。

use std::net::ToSocketAddrs as _;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};

/// 单个采集项的结果。
pub struct Collected {
    pub name: &'static str,
    pub text: String,
    pub error: Option<String>,
}

fn hidden_command(program: &str) -> Command {
    crate::kernel::hidden_command(program)
}

fn run(cmd: &str, args: &[&str], timeout: Duration) -> (String, Option<String>) {
    let mut command = hidden_command(cmd);
    command.args(args);
    // macOS 没有 GNU timeout，用线程 + recv_timeout 兜底（超时线程被丢弃，不影响主流程）。
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(command.output());
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !stderr.is_empty() {
                text.push_str(&format!("\n[stderr] {stderr}"));
            }
            if out.status.success() {
                (text, None)
            } else {
                let err = format!("exit={:?}", out.status.code());
                (text, Some(err))
            }
        }
        Ok(Err(err)) => (String::new(), Some(format!("spawn failed: {err}"))),
        Err(_) => (
            String::new(),
            Some(format!("timeout after {}ms", timeout.as_millis())),
        ),
    }
}

fn file_meta(path: &Path) -> Value {
    match std::fs::metadata(path) {
        Ok(meta) => json!({
            "path": path.display().to_string(),
            "exists": true,
            "size": meta.len(),
            "readonly": meta.permissions().readonly(),
            "mtime": meta.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
            }),
        }),
        Err(err) => json!({
            "path": path.display().to_string(),
            "exists": false,
            "error": err.to_string(),
        }),
    }
}

fn is_translocated(exe: &Path) -> bool {
    exe.display().to_string().contains("/AppTranslocation/")
}

fn dir_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".diag-probe");
    match std::fs::write(&probe, b"1") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn os_version() -> (String, Option<String>) {
    #[cfg(target_os = "macos")]
    {
        // 原生读 SystemVersion.plist，避免每次诊断起一个 `sw_vers` 进程。
        let text = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist")
            .unwrap_or_default();
        let pick = |key: &str| -> String {
            let marker = format!("<key>{key}</key>");
            text.find(&marker)
                .and_then(|pos| {
                    let rest = &text[pos + marker.len()..];
                    let start = rest.find("<string>")? + "<string>".len();
                    let end = rest[start..].find("</string>")? + start;
                    Some(rest[start..end].trim().to_string())
                })
                .unwrap_or_default()
        };
        let build = {
            // 13.x 的 plist 用 ProductBuildVersion（`BuildVersion` 是 sw_vers 自己的标签）。
            let v = pick("ProductBuildVersion");
            if v.is_empty() {
                pick("BuildVersion")
            } else {
                v
            }
        };
        let (name, version) = (pick("ProductName"), pick("ProductVersion"));
        let detail = if version.is_empty() {
            "SystemVersion.plist unreadable".to_string()
        } else {
            format!("{name}\t\t{version}\nBuildVersion:\t\t{build}")
        };
        return ("macos".to_string(), Some(detail));
    }
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
        let detail = match winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", KEY_READ)
        {
            Ok(key) => {
                let product: String = key.get_value("ProductName").unwrap_or_default();
                let build: String = key.get_value("CurrentBuildNumber").unwrap_or_default();
                let display: String = key.get_value("DisplayVersion").unwrap_or_default();
                let ubr: u32 = key.get_value("UBR").unwrap_or(0);
                format!("{product}\nVersion {display} (build {build}.{ubr})")
            }
            Err(err) => format!("registry error: {err}"),
        };
        return ("windows".to_string(), Some(detail));
    }
    #[cfg(target_os = "android")]
    {
        // Android 没有 /etc/os-release，且内核路径/服务模型与桌面 Linux 完全不同。
        return ("android".to_string(), None);
    }
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
    {
        let text = std::fs::read_to_string("/etc/os-release")
            .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
            .unwrap_or_default();
        let picked: Vec<&str> = text
            .lines()
            .filter(|line| {
                line.starts_with("PRETTY_NAME=")
                    || line.starts_with("NAME=")
                    || line.starts_with("VERSION=")
                    || line.starts_with("ID=")
            })
            .collect();
        return ("linux".to_string(), Some(picked.join("\n")));
    }
    #[allow(unreachable_code)]
    {
        (std::env::consts::OS.to_string(), None)
    }
}

/// 读 macOS 扩展属性（`com.apple.quarantine` 等），原生调用避免子进程。
#[cfg(target_os = "macos")]
pub(crate) fn read_xattr(path: &str, name: &str) -> (String, Option<String>) {
    let Ok(cpath) = std::ffi::CString::new(path) else {
        return (String::new(), Some("invalid path".to_string()));
    };
    let Ok(cname) = std::ffi::CString::new(name) else {
        return (String::new(), Some("invalid attr name".to_string()));
    };
    // 查询长度；ENOATTR（属性不存在）是正常情况，不算错误。
    let size = unsafe {
        libc::getxattr(
            cpath.as_ptr(),
            cname.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if size < 0 {
        return (String::new(), None);
    }
    let mut buf = vec![0u8; size as usize];
    let got = unsafe {
        libc::getxattr(
            cpath.as_ptr(),
            cname.as_ptr(),
            buf.as_mut_ptr().cast::<libc::c_void>(),
            buf.len(),
            0,
            0,
        )
    };
    if got < 0 {
        return (
            String::new(),
            Some(format!(
                "getxattr failed: {}",
                std::io::Error::last_os_error()
            )),
        );
    }
    buf.truncate(got as usize);
    (String::from_utf8_lossy(&buf).into_owned(), None)
}

/// 网卡与地址（native，三端一套；不解析命令输出、不弹窗）。
fn interfaces_text() -> (String, Option<String>) {
    let list = match if_addrs::get_if_addrs() {
        Ok(list) => list,
        Err(err) => return (String::new(), Some(format!("if-addrs: {err}"))),
    };
    let mut rows: Vec<String> = list
        .into_iter()
        .map(|iface| {
            let (family, addr, prefix) = match &iface.addr {
                if_addrs::IfAddr::V4(v4) => ("inet", v4.ip.to_string(), v4.prefixlen),
                if_addrs::IfAddr::V6(v6) => ("inet6", v6.ip.to_string(), v6.prefixlen),
            };
            format!(
                "{:<14} {:<6} {}/{} {}",
                iface.name,
                family,
                addr,
                prefix,
                if iface.is_p2p { "p2p" } else { "" }
            )
        })
        .collect();
    rows.sort();
    (rows.join("\n"), None)
}

/// 特权服务（`exvlan-service`）的完整状态：落点/权限/disabled/签名，
/// 用于定位「服务显示 Running 但 15999 没人监听」这类问题。
fn service_state_json() -> Value {
    let st = crate::service_client::state();
    #[allow(unused_mut)]
    let mut value = json!({
        "installed": st.installed,
        "ready": st.ready,
        "mode": st.mode,
        "kernel_pid": st.kernel_pid,
        "status": st.status,
        "installed_version": crate::service_client::installed_version(),
        "bin": crate::service_client::installed_service_bin().display().to_string(),
    });

    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let bin = crate::service_client::installed_service_bin();
        value["bin_meta"] = file_meta(&bin);
        value["bin_mode"] = json!(
            std::fs::metadata(&bin)
                .ok()
                .map(|m| format!("{:o}", m.permissions().mode() & 0o777))
                .unwrap_or_else(|| "-".to_string())
        );
        let plist = crate::paths::privileged_service_plist();
        value["plist"] = file_meta(&plist);
        value["print_disabled"] = json!(launchctl_print_disabled());
        value["codesign"] = json!(codesign_summary(&Some(bin)));
    }

    value
}

/// `launchctl print-disabled system` 里与 exvlan 相关的行。
#[cfg(target_os = "macos")]
fn launchctl_print_disabled() -> String {
    let Ok(out) = std::process::Command::new("/bin/launchctl")
        .args(["print-disabled", "system"])
        .output()
    else {
        return "unavailable".to_string();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let hits: Vec<&str> = text
        .lines()
        .filter(|line| line.to_ascii_lowercase().contains("exvlan"))
        .map(str::trim)
        .collect();
    if hits.is_empty() {
        "(no exvlan entries)".to_string()
    } else {
        hits.join(" | ")
    }
}

/// 服务二进制的签名摘要（未签名/adhoc 会影响 launchd 是否愿意加载）。
#[cfg(target_os = "macos")]
fn codesign_summary(bin: &Option<std::path::PathBuf>) -> String {
    let Some(bin) = bin else {
        return "no service binary".to_string();
    };
    let Ok(out) = std::process::Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2", &bin.to_string_lossy()])
        .output()
    else {
        return "codesign unavailable".to_string();
    };
    let mut text = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if text.is_empty() {
        text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    text.lines().take(4).collect::<Vec<_>>().join(" | ")
}

/// Windows 用户级（WinINET）代理：直接读注册表，避免 `reg query` 子进程。
#[cfg(windows)]
fn windows_proxy_text() -> String {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

    match winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Internet Settings",
        KEY_READ,
    ) {
        Ok(key) => {
            let enable: u32 = key.get_value("ProxyEnable").unwrap_or(0);
            let server: String = key.get_value("ProxyServer").unwrap_or_default();
            let bypass: String = key.get_value("ProxyOverride").unwrap_or_default();
            let pac: String = key.get_value("AutoConfigURL").unwrap_or_default();
            format!(
                "ProxyEnable={enable}\nProxyServer={server}\nProxyOverride={bypass}\nAutoConfigURL={pac}"
            )
        }
        Err(err) => format!("registry error: {err}"),
    }
}

/// D7：环境与安装完整性。
pub fn collect_system(app_config: &Path, app_log: &Path, core_dir: &Path) -> Collected {
    let exe = std::env::current_exe().unwrap_or_default();
    let mut errors: Vec<String> = Vec::new();

    #[allow(unused_mut)]
    let mut sig: Vec<Value> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        for target in [&exe, &core_dir.join("exvlan")] {
            let (out, err) = run(
                "codesign",
                &["-dv", "--verbose=4", &target.display().to_string()],
                Duration::from_secs(5),
            );
            sig.push(json!({"target": target.display().to_string(), "output": out, "error": err}));
        }
    }
    #[cfg(windows)]
    {
        let kernel = core_dir.join("exvlan.exe");
        let script = format!(
            "Get-AuthenticodeSignature '{}','{}' | Select-Object Path,Status,SignerCertificate | Format-List",
            exe.display(),
            kernel.display()
        );
        let (out, err) = run(
            "powershell",
            &["-NoProfile", "-Command", &script],
            Duration::from_secs(8),
        );
        sig.push(json!({"output": out, "error": err}));
    }

    #[allow(unused_mut)]
    let mut quarantine: Vec<Value> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        for target in [&exe, &core_dir.join("exvlan")] {
            // 原生 getxattr，避免 `xattr -p` 子进程
            let (xattr, err) = read_xattr(&target.display().to_string(), "com.apple.quarantine");
            quarantine.push(
                json!({"target": target.display().to_string(), "xattr": xattr, "error": err}),
            );
        }
    }

    let (os_name, os_detail) = os_version();
    let mut value = json!({
        "app_version": crate::BUILD_VERSION,
        "exe": exe.display().to_string(),
        "translocated": is_translocated(&exe),
        "os": os_name,
        "os_detail": os_detail,
        "arch": std::env::consts::ARCH,
        "kernel": file_meta(&core_dir.join(crate::kernel::kernel_bin_name())),
        "kernel_version_file": std::fs::read_to_string(core_dir.join("version"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        "runtime_deps": crate::kernel::runtime_dep_status()
            .into_iter()
            .map(|(name, ok)| json!({"name": name, "present": ok}))
            .collect::<Vec<_>>(),
        "service": service_state_json(),
        "signature": sig,
        "quarantine": quarantine,
        "config_dir": {"path": app_config.display().to_string(), "writable": dir_writable(app_config)},
        "log_dir": {"path": app_log.display().to_string(), "writable": dir_writable(app_log)},
        "generated_at": crate::logger::timestamp(),
    });
    if !errors.is_empty() {
        // 目前各子项错误已内联在 sig/quarantine 里；保留统一出口便于以后扩展。
        value["collect_errors"] = json!(std::mem::take(&mut errors));
    }

    Collected {
        name: "system.json",
        text: serde_json::to_string_pretty(&value)
            .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
        error: None,
    }
}

/// D8：网络状态（中间粒度：默认路由 + 策略路由 + TUN 接口相关条目）。
pub fn collect_net(tun_name: Option<&str>) -> Collected {
    let mut parts: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    #[cfg(target_os = "android")]
    {
        // Android 没有 ip/ifconfig/scutil/systemctl，只有 native 网卡信息可用。
        let (ifaces, err) = interfaces_text();
        if let Some(e) = err {
            errors.push(format!("interfaces: {e}"));
        }
        parts.push(format!("=== interfaces (native) ===\n{ifaces}"));
    }

    #[cfg(all(unix, not(target_os = "android")))]
    {
        // 网卡/地址走 native API（原来 `ip -o addr || ifconfig -a` 的输出要解析，
        // 且 Windows 上对应的是 PowerShell，启动就要 1–3s）。
        let (ifaces, err) = interfaces_text();
        if let Some(e) = err {
            errors.push(format!("interfaces: {e}"));
        }
        parts.push(format!("=== interfaces (native) ===\n{ifaces}"));

        let (mtu, err) = run(
            "sh",
            &[
                "-c",
                "ip -o link show 2>/dev/null || ifconfig -a 2>/dev/null | grep -Ei '^[a-z0-9]+:|mtu'",
            ],
            Duration::from_secs(5),
        );
        if let Some(e) = err {
            errors.push(format!("mtu: {e}"));
        }
        parts.push(format!("=== mtu ===\n{mtu}"));

        let route_script = r#"
ip -4 route show default 2>/dev/null
ip -6 route show default 2>/dev/null
ip rule show 2>/dev/null
ip -4 route show 10.254.0.0/16 2>/dev/null
ip -6 route show 10.254.0.0/16 2>/dev/null
"#;
        let (routes, err) = run("sh", &["-c", route_script], Duration::from_secs(5));
        if let Some(e) = err {
            errors.push(format!("routes: {e}"));
        }
        parts.push(format!(
            "=== routes (default / policy / 10.254) ===\n{routes}"
        ));

        if let Some(tun) = tun_name {
            let (tun_routes, _) = run(
                "sh",
                &[
                    "-c",
                    &format!(
                        "ip route show dev {tun} 2>/dev/null || netstat -rn | grep -E '{tun}' 2>/dev/null"
                    ),
                ],
                Duration::from_secs(5),
            );
            parts.push(format!("=== routes on {tun} ===\n{tun_routes}"));
            let (tun_addr, _) = run(
                "sh",
                &[
                    "-c",
                    &format!("ip addr show dev {tun} 2>/dev/null || ifconfig {tun} 2>/dev/null"),
                ],
                Duration::from_secs(5),
            );
            parts.push(format!("=== {tun} ===\n{tun_addr}"));
        }

        let dns_script = r#"
(scutil --dns 2>/dev/null | head -120) || (resolvectl status 2>/dev/null | head -80) || (cat /etc/resolv.conf 2>/dev/null)
"#;
        let (dns, err) = run("sh", &["-c", dns_script], Duration::from_secs(6));
        if let Some(e) = err {
            errors.push(format!("dns: {e}"));
        }
        parts.push(format!("=== dns ===\n{dns}"));

        let (proxy, _) = run(
            "sh",
            &[
                "-c",
                "env | grep -iE '^(http|https|all|no)_proxy=' ; scutil --proxy 2>/dev/null | head -40",
            ],
            Duration::from_secs(4),
        );
        parts.push(format!("=== proxy ===\n{proxy}"));

        let (listen, _) = run(
            "sh",
            &[
                "-c",
                "ss -lntup 2>/dev/null | grep -E ':(11010|11011|11012|15999|22020|11211)([^0-9]|$)' || netstat -an | grep -E '\\.(11010|11011|11012|15999|22020|11211)([^0-9]|$)'",
            ],
            Duration::from_secs(5),
        );
        parts.push(format!("=== listeners (exvlan related) ===\n{listen}"));
    }

    #[cfg(windows)]
    {
        // 网卡/地址走 native API（原来靠 PowerShell Get-NetIPAddress，1–3s + 一个黑窗）
        let (ifaces, err) = interfaces_text();
        if let Some(e) = err {
            errors.push(format!("interfaces: {e}"));
        }
        parts.push(format!("=== interfaces (native) ===\n{ifaces}"));

        let (mtu, err) = run(
            "netsh",
            &["interface", "ipv4", "show", "subinterfaces"],
            Duration::from_secs(5),
        );
        if let Some(e) = err {
            errors.push(format!("mtu: {e}"));
        }
        parts.push(format!("=== mtu ===\n{mtu}"));

        for (label, cmd, args) in [
            ("routes", "route", vec!["print", "-4"]),
            (
                "rules",
                "netsh",
                vec!["interface", "ipv4", "show", "interfaces"],
            ),
            ("dns", "ipconfig", vec!["/all"]),
            ("winhttp proxy", "netsh", vec!["winhttp", "show", "proxy"]),
            ("listeners", "netstat", vec!["-ano"]),
        ] {
            let (out, err) = run(cmd, &args, Duration::from_secs(8));
            if let Some(e) = err {
                errors.push(format!("{label}: {e}"));
            }
            parts.push(format!("=== {label} ===\n{out}"));
        }
        // WinINET 代理（用户级）：读注册表，不再起 `reg query`
        parts.push(format!("=== wininet proxy ===\n{}", windows_proxy_text()));
    }

    let mut value = json!({
        "tun": tun_name,
        "generated_at": crate::logger::timestamp(),
        "raw": parts.join("\n\n"),
    });
    if !errors.is_empty() {
        value["collect_errors"] = json!(errors);
    }
    Collected {
        name: "net.txt",
        text: value["raw"].as_str().unwrap_or_default().to_string(),
        error: None,
    }
}

/// D9：服务定义与运行态（自 7.3 起只有一个特权服务 `exvlan-service`）。
pub fn collect_service() -> Collected {
    let label = crate::service_client::SERVICE_LABEL;
    let mut parts: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        let plist = crate::paths::service_plist(label)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        match std::fs::read_to_string(&plist) {
            Ok(text) => parts.push(format!("=== plist {plist} ===\n{text}")),
            Err(err) => errors.push(format!("plist {plist}: {err}")),
        }
        let (out, err) = run(
            "launchctl",
            &["print", &format!("system/{label}")],
            Duration::from_secs(5),
        );
        if let Some(e) = err {
            errors.push(format!("launchctl print: {e}"));
        }
        parts.push(format!("=== launchctl print system/{label} ===\n{out}"));

        // 旧模型的 label 也打一份（升级后排查残留）
        for legacy in ["era.exvlan.studio", "era.exvlan.studio.helper"] {
            let (out, _err) = run(
                "launchctl",
                &["print", &format!("system/{legacy}")],
                Duration::from_secs(5),
            );
            let brief: String = out.lines().take(12).collect::<Vec<_>>().join("\n");
            parts.push(format!(
                "=== launchctl print system/{legacy} (legacy) ===\n{brief}"
            ));
        }
    }

    #[cfg(windows)]
    {
        for (label_name, cmd, args) in [
            ("sc qc", "sc", vec!["qc", label]),
            ("sc query", "sc", vec!["query", label]),
            ("sc sdshow", "sc", vec!["sdshow", label]),
        ] {
            let (out, err) = run(cmd, &args, Duration::from_secs(5));
            if let Some(e) = err {
                errors.push(format!("{label_name}: {e}"));
            }
            parts.push(format!("=== {label_name} ===\n{out}"));
        }
        let (out, _) = run(
            "reg",
            &["query", r"HKLM\SOFTWARE\Exvlan\Service"],
            Duration::from_secs(5),
        );
        parts.push(format!("=== registry WorkDir ===\n{out}"));
    }

    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
    {
        for (label_name, args) in [
            ("systemctl cat", vec!["cat", label]),
            (
                "systemctl status",
                vec!["status", "--no-pager", "-l", label],
            ),
            ("systemctl show", vec!["show", label]),
        ] {
            let (out, err) = run("systemctl", &args, Duration::from_secs(6));
            if let Some(e) = err {
                errors.push(format!("{label_name}: {e}"));
            }
            parts.push(format!("=== {label_name} ===\n{out}"));
        }
    }

    // 通用：服务指纹判定
    let marker = crate::user_marker_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    parts.push(format!(
        "=== fingerprint ===\nmarker={marker}\nstored={:?}",
        std::fs::read_to_string(&marker).map(|s| s.trim().to_string())
    ));

    Collected {
        name: "service.txt",
        text: if errors.is_empty() {
            parts.join("\n\n")
        } else {
            format!(
                "[collect_errors]\n{}\n\n{}",
                errors.join("\n"),
                parts.join("\n\n")
            )
        },
        error: None,
    }
}

/// TXT / SRV 查询（hickory-resolver，系统 DNS 配置；三端一致）。
fn dns_txt_srv(host: &str) -> String {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => return format!("TXT/SRV: runtime error {err}\n"),
    };
    rt.block_on(async {
        let resolver = match hickory_resolver::TokioResolver::builder_tokio() {
            Ok(builder) => builder.build(),
            Err(err) => return format!("TXT/SRV: resolver init failed: {err}\n"),
        };
        // 每个查询单独 2s 预算：某个域名解析超时不能拖住整包
        // （实测 Android/Windows 上 hickory 偶发 5s+ 超时）。
        let query_timeout = Duration::from_secs(2);
        let mut out = String::new();
        match tokio::time::timeout(query_timeout, resolver.txt_lookup(host)).await {
            Ok(Ok(lookup)) => {
                let rows: Vec<String> = lookup.iter().map(|r| r.to_string()).collect();
                if !rows.is_empty() {
                    out.push_str(&format!("TXT: {}\n", rows.join(" | ")));
                }
            }
            Ok(Err(err)) => out.push_str(&format!("TXT: {err}\n")),
            Err(_) => out.push_str("TXT: timeout after 2s\n"),
        }
        let srv_name = format!("_exvlan._tcp.{host}");
        match tokio::time::timeout(query_timeout, resolver.srv_lookup(&srv_name)).await {
            Ok(Ok(lookup)) => {
                let rows: Vec<String> = lookup
                    .iter()
                    .map(|srv| {
                        format!(
                            "{} {} {} {}",
                            srv.priority(),
                            srv.weight(),
                            srv.port(),
                            srv.target()
                        )
                    })
                    .collect();
                if !rows.is_empty() {
                    out.push_str(&format!("SRV: {}\n", rows.join(" | ")));
                }
            }
            Ok(Err(err)) => out.push_str(&format!("SRV: {err}\n")),
            Err(_) => out.push_str("SRV: timeout after 2s\n"),
        }
        out
    })
}

/// D10：关键域名的 DNS 解析结果（A/AAAA + TXT/SRV 尽力而为）。
pub fn collect_dns_resolve() -> Collected {
    let mut out = String::new();
    for host in [
        "xvlan.era-x.cn",
        "stun.xvlan.era-x.cn",
        "console.xvlan.era-x.cn",
        "config-server.xvlan.era-x.cn",
    ] {
        out.push_str(&format!("=== {host} ===\n"));
        // A/AAAA：标准库解析（不依赖外部命令）
        match (host, 443).to_socket_addrs() {
            Ok(addrs) => {
                let list: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
                out.push_str(&format!("addr: {}\n", list.join(", ")));
            }
            Err(err) => out.push_str(&format!("addr: ERROR {err}\n")),
        }
        // TXT / SRV：走解析器（原生命令行 dig 在 Windows 上不存在）
        out.push_str(&dns_txt_srv(host));
    }
    Collected {
        name: "dns-resolve.txt",
        text: out,
        error: None,
    }
}

/// D14：会话标记（判断上次是否异常退出）。
///
/// 启动时调用一次：读取上一份标记 → 若有 `started_at` 且无 `exited_at`，
/// 说明上次是崩溃/强杀；然后把当前会话写入标记。
static PREV_SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static SESSION_ABNORMAL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

pub fn start_session(log_dir: &Path) {
    let path = log_dir.join("session.json");
    let previous = std::fs::read_to_string(&path).unwrap_or_default();
    let abnormal = serde_json::from_str::<Value>(&previous)
        .ok()
        .filter(|v| v.get("exited_at").is_none())
        .map(|v| v.to_string());
    let _ = PREV_SESSION.set(previous);
    let _ = SESSION_ABNORMAL.set(abnormal);

    let _ = std::fs::create_dir_all(log_dir);
    let current = json!({
        "started_at": crate::logger::timestamp(),
        "pid": std::process::id(),
        "version": crate::BUILD_VERSION,
    });
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&current).unwrap_or_default(),
    );
}

pub fn previous_session() -> String {
    PREV_SESSION
        .get()
        .cloned()
        .unwrap_or_else(|| "{\"previous\":null}".to_string())
}

pub fn abnormal_session() -> Option<String> {
    SESSION_ABNORMAL.get().cloned().flatten()
}

/// 应用正常退出时落盘（供下次判断是否崩溃）。
pub fn mark_clean_exit(log_dir: &Path) {
    let path = log_dir.join("session.json");
    let value = json!({
        "started_at": json!(null),
        "exited_at": crate::logger::timestamp(),
        "clean": true,
    });
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    );
}

/// D1：manifest（JSON + 文本双份）。
pub fn build_manifest(
    entries: &[(String, usize, Option<String>, Option<u128>)],
) -> (String, String) {
    let items: Vec<Value> = entries
        .iter()
        .map(
            |(name, size, error, ms)| json!({"name": name, "size": size, "error": error, "ms": ms}),
        )
        .collect();
    let value = json!({
        "generated_at": crate::logger::timestamp(),
        "app_version": crate::BUILD_VERSION,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "items": items,
    });
    let json_text = serde_json::to_string_pretty(&value).unwrap_or_default();
    let mut txt = String::new();
    for (name, size, error, ms) in entries {
        let timing = ms.map(|v| format!("\t{v}ms")).unwrap_or_default();
        match error {
            Some(err) => txt.push_str(&format!("{name}\t{size}{timing}\tERROR: {err}\n")),
            None => txt.push_str(&format!("{name}\t{size}{timing}\tok\n")),
        }
    }
    (json_text, txt)
}

/// D11：系统统一日志片段（只取与 exvlan/helper/launchd 相关的行）。
pub fn collect_unified_log() -> Collected {
    #[cfg(target_os = "macos")]
    {
        // `log show --last 10m` 在日志量大时能跑到 15s 以上，12s 超时会整项丢失。
        // 先给 30s；超时再降级成 2 分钟窗口重试一次，保证至少有一段统一日志。
        let predicate = r#"process == "exvlan" OR process == "exvlan-service" OR eventMessage CONTAINS "era.exvlan.studio""#;
        let mut last_error = None;
        for (window, timeout) in [("10m", 30u64), ("2m", 15u64)] {
            let (out, err) = run(
                "log",
                &[
                    "show",
                    "--style",
                    "compact",
                    "--last",
                    window,
                    "--predicate",
                    predicate,
                ],
                Duration::from_secs(timeout),
            );
            let timed_out = err.as_deref().is_some_and(|e| e.contains("timeout"));
            if timed_out {
                last_error = err;
                continue;
            }
            let mut text = out;
            if text.len() > 512 * 1024 {
                text.truncate(512 * 1024);
                text.push_str("\n[truncated at 512KB]\n");
            }
            if window != "10m" {
                text.insert_str(0, &format!("[fallback window --last {window}]\n"));
            }
            return Collected {
                name: "unified-log.txt",
                text,
                error: err,
            };
        }
        return Collected {
            name: "unified-log.txt",
            text: String::new(),
            error: last_error.or_else(|| Some("log show timed out".to_string())),
        };
    }
    #[cfg(not(target_os = "macos"))]
    {
        Collected {
            name: "unified-log.txt",
            text: "not supported on this platform\n".to_string(),
            error: None,
        }
    }
}

/// 列目录里最新的若干文件（原生，避免 `ls -lt | head` 子进程）。
fn list_dir_newest(dir: &Path, limit: usize) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return "(unreadable)\n".to_string();
    };
    // WER / CrashDumps 目录可能有上万条，全量 stat 会拖慢整包（实测 6s+）。
    // 只扫描前 2000 条，足够覆盖“最近几份崩溃报告”。
    let mut items: Vec<(u64, u64, String)> = entries
        .flatten()
        .take(2000)
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Some((
                mtime,
                meta.len(),
                entry.file_name().to_string_lossy().into_owned(),
            ))
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.0));
    items
        .into_iter()
        .take(limit)
        .map(|(mtime, size, name)| format!("{name}\t{size}\t{mtime}\n"))
        .collect()
}

/// 最新一个匹配后缀的文件。
#[allow(dead_code)] // 仅 macOS / Windows 的崩溃报告采集使用
fn newest_file_with_ext(dir: &Path, exts: &[&str]) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut items: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let matched = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| exts.iter().any(|want| want.eq_ignore_ascii_case(ext)));
            if !matched {
                return None;
            }
            Some((entry.metadata().ok()?.modified().ok()?, path))
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.0));
    items.into_iter().next().map(|(_, p)| p)
}

/// 读文件开头 `cap` 字节（用于崩溃报告正文）。
#[allow(dead_code)] // 仅 macOS / Windows 的崩溃报告采集使用
fn read_head(path: &Path, cap: usize) -> String {
    use std::io::Read as _;
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = file.take(cap as u64).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// D12：崩溃报告（只列元信息；macOS 取最近的一份正文，Windows 只列 CrashDumps）。
pub fn collect_crash_reports() -> Collected {
    let mut out = String::new();
    #[cfg(target_os = "macos")]
    {
        let dir = std::env::var("HOME")
            .map(|h| format!("{h}/Library/Logs/DiagnosticReports"))
            .unwrap_or_default();
        let dir_path = Path::new(&dir);
        out.push_str(&format!("=== {dir} ===\n{}", list_dir_newest(dir_path, 8)));
        if let Some(newest) = newest_file_with_ext(dir_path, &["ips", "crash"]) {
            out.push_str(&format!(
                "=== {} ===\n{}\n",
                newest.display(),
                read_head(&newest, 64 * 1024)
            ));
        }
    }
    #[cfg(windows)]
    {
        let dir = std::env::var("LOCALAPPDATA")
            .map(|d| format!("{d}\\CrashDumps"))
            .unwrap_or_default();
        out.push_str(&format!(
            "=== {dir} ===\n{}",
            list_dir_newest(Path::new(&dir), 8)
        ));
        if let Ok(watchdog) = std::env::var("PROGRAMDATA") {
            let wd = format!("{watchdog}\\Microsoft\\Windows\\WER\\ReportArchive");
            out.push_str(&format!(
                "=== {wd} ===\n{}",
                list_dir_newest(Path::new(&wd), 8)
            ));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for dir in [
            std::env::var("HOME")
                .map(|h| format!("{h}/.local/share/systemd/coredump"))
                .unwrap_or_default(),
            "/var/crash".to_string(),
        ] {
            out.push_str(&format!(
                "=== {dir} ===\n{}",
                list_dir_newest(Path::new(&dir), 8)
            ));
        }
    }
    Collected {
        name: "crashes.txt",
        text: out,
        error: None,
    }
}

/// D13：防火墙 / 安全软件状态。
pub fn collect_firewall() -> Collected {
    let mut out = String::new();
    #[cfg(target_os = "macos")]
    {
        for (label, args) in [
            ("alf global", vec!["--getglobalstate"]),
            ("alf stealth", vec!["--getstealthmode"]),
            ("alf list", vec!["--listapps"]),
        ] {
            let (text, err) = run(
                "/usr/libexec/ApplicationFirewall/socketfilterfw",
                &args,
                Duration::from_secs(6),
            );
            out.push_str(&format!("=== {label} ===\n{text}\n"));
            if let Some(e) = err {
                out.push_str(&format!("[error] {e}\n"));
            }
        }
    }
    #[cfg(windows)]
    {
        let (text, err) = run(
            "netsh",
            &["advfirewall", "show", "allprofiles"],
            Duration::from_secs(8),
        );
        out.push_str(&format!("=== netsh advfirewall ===\n{text}\n"));
        if let Some(e) = err {
            out.push_str(&format!("[error] {e}\n"));
        }
        let (text, _) = run(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "Get-CimInstance -Namespace root/SecurityCenter2 -ClassName AntiVirusProduct | Select-Object displayName,productState | Format-Table -AutoSize",
            ],
            Duration::from_secs(8),
        );
        out.push_str(&format!("=== antivirus ===\n{text}\n"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let (text, _) = run(
            "sh",
            &[
                "-c",
                "ufw status 2>/dev/null; nft list ruleset 2>/dev/null | head -40",
            ],
            Duration::from_secs(6),
        );
        out.push_str(&text);
    }
    Collected {
        name: "firewall.txt",
        text: out,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interfaces_are_collected_natively() {
        let (text, err) = interfaces_text();
        assert!(err.is_none(), "if-addrs failed: {err:?}");
        // 至少 loopback 一定存在
        assert!(text.contains("lo") || text.contains("Loopback"), "{text}");
    }

    #[test]
    fn net_collector_returns_readable_text() {
        let collected = collect_net(None);
        assert_eq!(collected.name, "net.txt");
        assert!(collected.text.contains("=== interfaces (native) ==="));
    }

    #[test]
    fn os_version_is_detected() {
        let (name, detail) = os_version();
        assert!(!name.is_empty());
        assert!(detail.is_some());
    }

    #[test]
    fn dns_txt_srv_never_panics() {
        // 离线环境下查询会失败，但必须返回可读文本而不是 panic。
        let text = dns_txt_srv("example.com");
        assert!(!text.is_empty());
    }
}
