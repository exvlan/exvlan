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

fn run(cmd: &str, args: &[&str], timeout: Duration) -> (String, Option<String>) {
    let mut command = Command::new(cmd);
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
        let (out, _) = run("sw_vers", &[], Duration::from_secs(3));
        return ("macos".to_string(), Some(out.trim().to_string()));
    }
    #[cfg(windows)]
    {
        let (out, _) = run("cmd", &["/c", "ver"], Duration::from_secs(3));
        let (build, _) = run(
            "reg",
            &[
                "query",
                r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
                "/v",
                "CurrentBuildNumber",
            ],
            Duration::from_secs(3),
        );
        return (
            "windows".to_string(),
            Some(format!("{}\n{}", out.trim(), build.trim())),
        );
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let (out, _) = run(
            "sh",
            &["-c", "cat /etc/os-release 2>/dev/null | head -8"],
            Duration::from_secs(3),
        );
        return ("linux".to_string(), Some(out.trim().to_string()));
    }
    #[allow(unreachable_code)]
    {
        (std::env::consts::OS.to_string(), None)
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
            let (out, err) = run(
                "xattr",
                &["-p", "com.apple.quarantine", &target.display().to_string()],
                Duration::from_secs(3),
            );
            quarantine.push(
                json!({"target": target.display().to_string(), "xattr": out.trim(), "error": err}),
            );
        }
    }

    let mut value = json!({
        "app_version": crate::BUILD_VERSION,
        "exe": exe.display().to_string(),
        "translocated": is_translocated(&exe),
        "os": os_version().0,
        "os_detail": os_version().1,
        "arch": std::env::consts::ARCH,
        "kernel": file_meta(&core_dir.join(crate::kernel::kernel_bin_name())),
        "kernel_version_file": std::fs::read_to_string(core_dir.join("version"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        "runtime_deps": crate::kernel::runtime_dep_status()
            .into_iter()
            .map(|(name, ok)| json!({"name": name, "present": ok}))
            .collect::<Vec<_>>(),
        "helper": {
            "installed": crate::helper::is_installed(),
            "status": crate::helper::status().unwrap_or_else(|e| e),
        },
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

    #[cfg(unix)]
    {
        let (addr, err) = run(
            "sh",
            &["-c", "ip -o addr 2>/dev/null || ifconfig -a 2>/dev/null"],
            Duration::from_secs(5),
        );
        if let Some(e) = err {
            errors.push(format!("interfaces: {e}"));
        }
        parts.push(format!("=== interfaces ===\n{addr}"));

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
                "ss -lntup 2>/dev/null | grep -E ':(11010|11011|11012|15999|22020|11211)\\b' || netstat -an | grep -E '\\.(11010|11011|11012|15999|22020|11211)\\b'",
            ],
            Duration::from_secs(5),
        );
        parts.push(format!("=== listeners (exvlan related) ===\n{listen}"));
    }

    #[cfg(windows)]
    {
        for (label, cmd, args) in [
            (
                "interfaces",
                "powershell",
                vec![
                    "-NoProfile",
                    "-Command",
                    "Get-NetIPAddress | Select-Object InterfaceAlias,IPAddress,PrefixLength,AddressFamily | Format-Table -AutoSize",
                ],
            ),
            ("routes", "route", vec!["print", "-4"]),
            (
                "rules",
                "netsh",
                vec!["interface", "ipv4", "show", "interfaces"],
            ),
            ("dns", "ipconfig", vec!["/all"]),
            ("proxy", "netsh", vec!["winhttp", "show", "proxy"]),
            ("listeners", "netstat", vec!["-ano"]),
        ] {
            let (out, err) = run(cmd, &args, Duration::from_secs(8));
            if let Some(e) = err {
                errors.push(format!("{label}: {e}"));
            }
            parts.push(format!("=== {label} ===\n{out}"));
        }
        // WinINET 代理（用户级）
        let (out, _) = run(
            "reg",
            &[
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            ],
            Duration::from_secs(5),
        );
        parts.push(format!("=== wininet proxy ===\n{out}"));
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

/// D9：服务定义与运行态。
pub fn collect_service() -> Collected {
    let label = crate::SERVICE_LABEL;
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

        let helper_label = "era.exvlan.studio.helper";
        let (out, err) = run(
            "launchctl",
            &["print", &format!("system/{helper_label}")],
            Duration::from_secs(5),
        );
        if let Some(e) = err {
            errors.push(format!("launchctl print helper: {e}"));
        }
        parts.push(format!(
            "=== launchctl print system/{helper_label} ===\n{out}"
        ));
        if let Some(plist) = crate::paths::helper_plist() {
            if let Ok(text) = std::fs::read_to_string(&plist) {
                parts.push(format!("=== helper plist {} ===\n{text}", plist.display()));
            }
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

    #[cfg(all(unix, not(target_os = "macos")))]
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
        // TXT / SRV：用系统工具（存在才用）
        for (kind, cmd, args) in [
            (
                "TXT",
                "dig",
                vec!["+short", "+time=3", "+tries=1", "TXT", host],
            ),
            (
                "SRV",
                "dig",
                vec![
                    "+short",
                    "+time=3",
                    "+tries=1",
                    "SRV",
                    &format!("_exvlan._tcp.{host}"),
                ],
            ),
        ] {
            let (text, err) = run(cmd, &args, Duration::from_secs(5));
            if err.is_none() && !text.trim().is_empty() {
                out.push_str(&format!("{kind}: {}\n", text.trim()));
            }
        }
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
pub fn build_manifest(entries: &[(String, usize, Option<String>)]) -> (String, String) {
    let items: Vec<Value> = entries
        .iter()
        .map(|(name, size, error)| json!({"name": name, "size": size, "error": error}))
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
    for (name, size, error) in entries {
        match error {
            Some(err) => txt.push_str(&format!("{name}\t{size}\tERROR: {err}\n")),
            None => txt.push_str(&format!("{name}\t{size}\tok\n")),
        }
    }
    (json_text, txt)
}

/// D11：系统统一日志片段（只取与 exvlan/helper/launchd 相关的行）。
pub fn collect_unified_log() -> Collected {
    #[cfg(target_os = "macos")]
    {
        let (out, err) = run(
            "log",
            &[
                "show",
                "--style",
                "compact",
                "--last",
                "10m",
                "--predicate",
                r#"process == "exvlan" OR process == "exvlan-helper" OR eventMessage CONTAINS "era.exvlan.studio""#,
            ],
            Duration::from_secs(12),
        );
        let mut text = out;
        if text.len() > 512 * 1024 {
            text.truncate(512 * 1024);
            text.push_str("\n[truncated at 512KB]\n");
        }
        return Collected {
            name: "unified-log.txt",
            text,
            error: err,
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

/// D12：崩溃报告（只列元信息；macOS 取最近的一份正文，Windows 只列 CrashDumps）。
pub fn collect_crash_reports() -> Collected {
    let mut out = String::new();
    #[cfg(target_os = "macos")]
    {
        let dir = std::env::var("HOME")
            .map(|h| format!("{h}/Library/Logs/DiagnosticReports"))
            .unwrap_or_default();
        let (list, err) = run(
            "sh",
            &["-c", &format!("ls -lt '{dir}' 2>/dev/null | head -8")],
            Duration::from_secs(5),
        );
        out.push_str(&format!("=== {} ===\n{list}\n", dir));
        if let Some(e) = err {
            out.push_str(&format!("[list error] {e}\n"));
        }
        // 最近一份正文（最多 64KB）
        let (newest, _) = run(
            "sh",
            &[
                "-c",
                &format!("ls -t '{dir}'/*.ips '{dir}'/*.crash 2>/dev/null | head -1"),
            ],
            Duration::from_secs(5),
        );
        let newest = newest.trim();
        if !newest.is_empty() {
            let (body, _) = run(
                "sh",
                &["-c", &format!("head -c 65536 '{newest}' 2>/dev/null")],
                Duration::from_secs(6),
            );
            out.push_str(&format!("=== {newest} ===\n{body}\n"));
        }
    }
    #[cfg(windows)]
    {
        let dir = std::env::var("LOCALAPPDATA")
            .map(|d| format!("{d}\\CrashDumps"))
            .unwrap_or_default();
        let (list, err) = run(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                &format!(
                    "Get-ChildItem -Path '{dir}' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 8 Name,Length,LastWriteTime | Format-Table -AutoSize"
                ),
            ],
            Duration::from_secs(8),
        );
        out.push_str(&format!("=== {dir} ===\n{list}\n"));
        if let Some(e) = err {
            out.push_str(&format!("[list error] {e}\n"));
        }
        if let Ok(watchdog) = std::env::var("PROGRAMDATA") {
            let wd = format!("{watchdog}\\Microsoft\\Windows\\WER\\ReportArchive");
            let (list, _) = run(
                "powershell",
                &[
                    "-NoProfile",
                    "-Command",
                    &format!(
                        "Get-ChildItem -Path '{wd}' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 8 Name,LastWriteTime | Format-Table -AutoSize"
                    ),
                ],
                Duration::from_secs(8),
            );
            out.push_str(&format!("=== {wd} ===\n{list}\n"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let (list, _) = run(
            "sh",
            &[
                "-c",
                "ls -lt ~/.local/share/systemd/coredump 2>/dev/null | head -8; ls -lt /var/crash 2>/dev/null | head -8",
            ],
            Duration::from_secs(5),
        );
        out.push_str(&list);
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
