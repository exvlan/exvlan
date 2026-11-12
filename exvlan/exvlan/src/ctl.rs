//! 产品控制面命令（`exvlan-cli service|doctor|diag|auth`）。
//!
//! 这些命令属于 EXVlan 的产品层（服务 + 后端探测 + 体检），**不是**上游内核
//! CLI 的一部分。为了不改动上游 `cli.rs` 的 clap 枚举（上游同步会冲突），这里
//! 用「入口前置分发」的方式处理：命中我们的子命令就直接执行并退出。
//!
//! 只有本地 IPC / 文件系统访问，不需要内核 RPC —— 所以即使内核没跑也能用，
//! 这正是排障时最需要的能力。

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use exvlan_service_client::{Backend, detect, paths};
use serde_json::json;

#[derive(Parser, Debug)]
#[command(name = "exvlan-cli", disable_help_subcommand = true)]
struct Ctl {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// 特权服务 / 内核生命周期（覆盖上游同名命令）
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
    /// 一键体检：backend / 服务 / 内核 / 端口 / 存储
    Doctor {
        /// JSON 输出（agent 友好）
        #[arg(long)]
        json: bool,
        /// 执行安全修复（当前仅：内核没跑时重启）
        #[arg(long)]
        fix: bool,
    },
    /// 输出诊断摘要（JSON）
    Diag {
        #[arg(long)]
        json: bool,
    },
    /// 本地 gateway token（仅本机使用）
    Auth {
        #[command(subcommand)]
        cmd: AuthCmd,
    },
}

#[derive(Subcommand, Debug)]
enum ServiceCmd {
    /// 当前状态（backend / supervisor / policy / kernel pid / socket）
    Status,
    /// 设置生命周期策略（resident|session|stopped）
    Policy { policy: String },
    /// 期望状态（running|stopped）
    Desired { state: String },
    /// 重启内核（原子：停 → 清残留 → 起）
    Reload,
    /// 内核重启 / 停止 / 确保在跑
    Kernel {
        #[command(subcommand)]
        cmd: KernelCmd,
    },
    /// 安装特权服务（service）或写 systemd unit（core）
    Install {
        /// core | service
        #[arg(long, default_value = "service")]
        backend: String,
        /// 目标用户 uid（默认当前用户）
        #[arg(long)]
        user: Option<u32>,
    },
    /// 卸载特权服务
    Uninstall,
    /// 重装（提权一次）
    Repair,
}

#[derive(Subcommand, Debug)]
enum KernelCmd {
    Restart,
    Stop,
    Ensure,
}

#[derive(Subcommand, Debug)]
enum AuthCmd {
    /// 显示本地 token 是否存在（不回显明文）
    Status,
    /// 首次生成本地 token（0600）
    Login,
    /// 删除本地 token
    Logout,
}

/// 入口分发：命中我们的子命令则执行并返回退出码。
pub fn maybe_run() -> Option<i32> {
    let first = std::env::args().nth(1)?;
    if !matches!(first.as_str(), "service" | "doctor" | "diag" | "auth") {
        return None;
    }
    // doctor/diag 的 `--json` 由我们处理；其余参数交给 clap 校验。
    let parsed = match Ctl::try_parse_from(std::env::args().filter(|a| a != "--verbose")) {
        Ok(v) => v,
        Err(err) => {
            let _ = err.print();
            return Some(2);
        }
    };
    Some(match parsed.cmd {
        Cmd::Service { cmd } => service_cmd(cmd),
        Cmd::Doctor { json, fix } => doctor(json, fix),
        Cmd::Diag { json } => diag(json),
        Cmd::Auth { cmd } => auth_cmd(cmd),
    })
}

// ---------------------------------------------------------------------------
// service
// ---------------------------------------------------------------------------

fn ipc(op: &str, extra: serde_json::Value) -> Result<serde_json::Value, String> {
    exvlan_service_client::ipc::request(op, extra)
}

fn service_cmd(cmd: ServiceCmd) -> i32 {
    let result = match cmd {
        ServiceCmd::Status => show_status(),
        ServiceCmd::Policy { policy } => {
            ipc("mode.set", json!({ "mode": policy })).map(|v| print_json(&v))
        }
        ServiceCmd::Desired { state } => match state.as_str() {
            "running" => ipc("kernel.ensure", json!({})).map(|v| print_json(&v)),
            "stopped" => ipc("kernel.stop", json!({})).map(|v| print_json(&v)),
            other => Err(format!("未知状态: {other}（running|stopped）")),
        },
        ServiceCmd::Reload => ipc("kernel.restart", json!({})).map(|v| print_json(&v)),
        ServiceCmd::Kernel { cmd } => match cmd {
            KernelCmd::Restart => ipc("kernel.restart", json!({})).map(|v| print_json(&v)),
            KernelCmd::Stop => ipc("kernel.stop", json!({})).map(|v| print_json(&v)),
            KernelCmd::Ensure => ipc("kernel.ensure", json!({})).map(|v| print_json(&v)),
        },
        ServiceCmd::Install { backend, user } => install(&backend, user),
        ServiceCmd::Uninstall => uninstall(),
        ServiceCmd::Repair => install("service", None),
    };
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("错误: {err}");
            2
        }
    }
}

fn show_status() -> Result<(), String> {
    let p = paths::paths();
    let backend = detect();
    let mut out = json!({
        "backend": backend.as_str(),
        "supervisor": exvlan_service_client::backend::supervisor(),
        "service_bin": p.service_bin.display().to_string(),
        "service_installed": p.service_bin.is_file(),
        "kernel_bin": p.kernel_bin.display().to_string(),
        "kernel_version": read_trim(&p.kernel_version_file()),
        "service_version": read_trim(&p.service_version_file()),
        "ipc": p.service_bin.is_file(),
    });
    if backend.has_service() {
        match ipc("status", json!({})) {
            Ok(v) => {
                out["service"] = v;
            }
            Err(err) => {
                out["service_error"] = json!(err);
            }
        }
    }
    print_json(&out);
    Ok(())
}

fn read_trim(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn install(backend: &str, user: Option<u32>) -> Result<(), String> {
    let p = paths::paths();
    match backend {
        "service" => {
            if !p.service_bin.is_file() {
                return Err(format!(
                    "找不到服务二进制 {}（请确认已安装完整安装包）",
                    p.service_bin.display()
                ));
            }
            let uid = user.unwrap_or_else(current_uid);
            let bin = p.service_bin.display().to_string();
            if euid() == 0 {
                run(&bin, &["--install", &uid.to_string()])
            } else if cfg!(windows) {
                Err(format!(
                    "Windows 需要管理员权限：请在「以管理员身份运行」的终端执行\n  \"{bin}\" --install {uid}"
                ))
            } else {
                run("sudo", &[bin.as_str(), "--install", &uid.to_string()])
            }
        }
        "core" => {
            if cfg!(windows) || cfg!(target_os = "macos") {
                return Err(
                    "core backend 需要自行以 root 托管内核（utun 需 root / wintun 需管理员）；\
                     这两个平台请用 --backend service"
                        .to_string(),
                );
            }
            Err("core backend 的 systemd unit 安装将在后续版本提供；现在请用 script/service-update/"
                .to_string())
        }
        other => Err(format!("未知 backend: {other}（core|service）")),
    }
}

fn uninstall() -> Result<(), String> {
    let p = paths::paths();
    if !p.service_bin.is_file() {
        return Err("未安装特权服务".to_string());
    }
    let bin = p.service_bin.display().to_string();
    if euid() == 0 {
        run(&bin, &["--uninstall"])
    } else if cfg!(windows) {
        Err(format!(
            "Windows 需要管理员权限：请在「以管理员身份运行」的终端执行\n  \"{bin}\" --uninstall"
        ))
    } else {
        run("sudo", &[bin.as_str(), "--uninstall"])
    }
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .map_err(|e| format!("执行 {program} 失败: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} 退出码 {:?}", status.code()))
    }
}

fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|out| String::from_utf8_lossy(&out.stdout).trim().parse().ok())
            .unwrap_or(0)
    }
    #[cfg(windows)]
    0
}

/// 是否已经是 root/管理员（unix 用 `id -u`；windows 交给服务二进制的提权错误提示）。
fn euid() -> u32 {
    #[cfg(unix)]
    {
        current_uid()
    }
    #[cfg(windows)]
    0
}

// ---------------------------------------------------------------------------
// doctor / diag
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Finding {
    id: &'static str,
    level: &'static str,
    summary: String,
    evidence: serde_json::Value,
    fixable: bool,
}

fn collect_findings() -> Vec<Finding> {
    let mut out = Vec::new();
    let p = paths::paths();
    let user = exvlan_service_client::paths::user_paths();
    let backend = detect();

    out.push(Finding {
        id: "backend",
        level: "info",
        summary: format!(
            "backend={} supervisor={}",
            backend.as_str(),
            exvlan_service_client::backend::supervisor()
        ),
        evidence: json!({"backend": backend.as_str()}),
        fixable: false,
    });

    // 服务
    if backend.has_service() {
        match ipc("ping", json!({})) {
            Ok(_) => out.push(Finding {
                id: "service.ipc",
                level: "ok",
                summary: "服务 IPC 可达".into(),
                evidence: json!({"socket": p.service_bin.display().to_string()}),
                fixable: false,
            }),
            Err(err) => out.push(Finding {
                id: "service.ipc",
                level: "error",
                summary: format!("服务 IPC 不可达: {err}"),
                evidence: json!({"error": err}),
                fixable: false,
            }),
        }
        if let Ok(st) = ipc("status", json!({})) {
            let mode = st.get("mode").and_then(|v| v.as_str()).unwrap_or("-");
            let pid = st
                .get("kernel")
                .and_then(|k| k.get("pid"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            out.push(Finding {
                id: "service.policy",
                level: "info",
                summary: format!("policy={mode}"),
                evidence: st.clone(),
                fixable: false,
            });
            let want_running = mode != "off";
            if want_running && pid == 0 {
                out.push(Finding {
                    id: "kernel.running",
                    level: "error",
                    summary: format!("policy={mode} 但内核没在跑"),
                    evidence: json!({"mode": mode, "pid": pid}),
                    fixable: true,
                });
            } else {
                out.push(Finding {
                    id: "kernel.running",
                    level: "ok",
                    summary: format!("内核 pid={pid}"),
                    evidence: json!({"pid": pid}),
                    fixable: false,
                });
            }
        }
        // socket 权限（unix）
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let addr = exvlan_service_client::paths::service_ipc_addr();
            match std::fs::metadata(&addr) {
                Ok(md) => {
                    let mode = md.mode() & 0o777;
                    let ok = md.uid() == current_uid() && mode == 0o600;
                    out.push(Finding {
                        id: "service.socket_perms",
                        level: if ok { "ok" } else { "warn" },
                        summary: format!(
                            "{} owner={} mode={:o}{}",
                            addr.display(),
                            md.uid(),
                            mode,
                            if ok {
                                ""
                            } else {
                                "（应为当前用户 + 0600）"
                            }
                        ),
                        evidence: json!({"uid": md.uid(), "mode": format!("{mode:o}")}),
                        fixable: false,
                    });
                }
                Err(err) => out.push(Finding {
                    id: "service.socket_perms",
                    level: "warn",
                    summary: format!("socket 不存在: {err}"),
                    evidence: json!({"path": addr.display().to_string()}),
                    fixable: false,
                }),
            }
        }
    } else {
        out.push(Finding {
            id: "service.installed",
            level: "info",
            summary: "core backend：没有特权服务（内核由 systemd / GUI supervisor 托管）".into(),
            evidence: json!({"backend": "core"}),
            fixable: false,
        });
    }

    // 版本一致性
    let kernel_v = read_trim(&p.kernel_version_file());
    let service_v = read_trim(&p.service_version_file());
    let cli_v = env!("CARGO_PKG_VERSION").to_string();
    if let (Some(k), Some(s)) = (&kernel_v, &service_v)
        && k != s
    {
        out.push(Finding {
            id: "version.mismatch",
            level: "warn",
            summary: format!("内核 {k} ≠ 服务 {s}（重装服务可对齐）"),
            evidence: json!({"kernel": k, "service": s}),
            fixable: false,
        });
    }
    out.push(Finding {
        id: "version.info",
        level: "info",
        summary: format!(
            "cli={cli_v} kernel={} service={}",
            kernel_v.as_deref().unwrap_or("-"),
            service_v.as_deref().unwrap_or("-")
        ),
        evidence: json!({"cli": cli_v, "kernel": kernel_v, "service": service_v}),
        fixable: false,
    });

    // 端口
    let rpc = exvlan_service_client::local_rpc_addr();
    let gw = exvlan_service_client::local_gateway_addr();
    out.push(Finding {
        id: "net.ports",
        level: if tcp_open(&rpc) { "ok" } else { "warn" },
        summary: format!(
            "rpc {rpc} {} / gateway {gw} {}",
            if tcp_open(&rpc) { "listening" } else { "closed" },
            if tcp_open(&gw) { "listening" } else { "closed" }
        ),
        evidence: json!({"rpc": rpc, "rpc_open": tcp_open(&rpc), "gateway": gw, "gateway_open": tcp_open(&gw)}),
        fixable: false,
    });

    // 用户侧存储
    out.push(Finding {
        id: "app.prefs_dir",
        level: if dir_writable(&user.data) {
            "ok"
        } else {
            "error"
        },
        summary: format!(
            "prefs 目录 {} {}",
            user.data.display(),
            if dir_writable(&user.data) {
                "可写"
            } else {
                "不可写"
            }
        ),
        evidence: json!({"path": user.data.display().to_string()}),
        fixable: false,
    });

    // macOS：WebKit 目录属主
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt as _;
        let path = user
            .data
            .parent()
            .unwrap_or(std::path::Path::new("/"))
            .join("WebKit")
            .join(exvlan_service_client::APP_ID);
        let me = current_uid();
        match std::fs::metadata(&path) {
            Ok(md) if md.uid() != me => out.push(Finding {
                id: "app.webkit_owner",
                level: "error",
                summary: format!(
                    "{} 属主是 {}（应为 {me}）→ localStorage 会失效（远程目标/语言/上次网络全静默失败）",
                    path.display(),
                    md.uid()
                ),
                evidence: json!({"path": path.display().to_string(), "owner": md.uid(), "expected": me}),
                fixable: false,
            }),
            Ok(md) => out.push(Finding {
                id: "app.webkit_owner",
                level: "ok",
                summary: format!("{} 属主正确（{me}）", path.display()),
                evidence: json!({"path": path.display().to_string(), "owner": md.uid()}),
                fixable: false,
            }),
            Err(err) => out.push(Finding {
                id: "app.webkit_owner",
                level: "warn",
                summary: format!("{} 不存在: {err}", path.display()),
                evidence: json!({"path": path.display().to_string()}),
                fixable: false,
            }),
        }
    }

    // 迁移状态
    let marker = user.data.join("migrate.done");
    let legacy = [
        user.data.join("networks.toml"),
        user.data.join("service.json"),
        user.data.join("config.d"),
        user.data.join("ui-state.json"),
    ];
    let present: Vec<String> = legacy
        .iter()
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect();
    if !present.is_empty() {
        out.push(Finding {
            id: "migrate.legacy",
            level: "error",
            summary: format!(
                "检测到旧版本数据（{} 项）→ 请先运行 exvlan-migrate",
                present.len()
            ),
            evidence: json!({"paths": present, "marker": marker.display().to_string()}),
            fixable: false,
        });
    } else {
        out.push(Finding {
            id: "migrate.legacy",
            level: "ok",
            summary: "没有旧版本遗留数据".into(),
            evidence: json!({"marker": marker.display().to_string()}),
            fixable: false,
        });
    }

    out
}

fn tcp_open(addr: &str) -> bool {
    let Ok(sock) = addr.parse::<std::net::SocketAddr>() else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&sock, std::time::Duration::from_millis(300)).is_ok()
}

fn dir_writable(path: &std::path::Path) -> bool {
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(PathBuf::from).unwrap_or_default()
    };
    if dir.as_os_str().is_empty() {
        return false;
    }
    let probe = dir.join(".exvlan-doctor-probe");
    match std::fs::write(&probe, b"1") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn doctor(json_out: bool, fix: bool) -> i32 {
    let mut findings = collect_findings();
    if fix {
        for f in findings.iter_mut() {
            if f.id == "kernel.running" && f.level == "error" && f.fixable {
                match ipc("kernel.restart", json!({})) {
                    Ok(_) => {
                        f.level = "ok";
                        f.summary = "内核已重启".into();
                    }
                    Err(err) => {
                        f.summary = format!("重启内核失败: {err}");
                    }
                }
            }
        }
    }
    let has_error = findings.iter().any(|f| f.level == "error");
    let has_warn = findings.iter().any(|f| f.level == "warn");

    if json_out {
        print_json(&json!({
            "ok": !has_error,
            "findings": findings.iter().map(|f| json!({
                "id": f.id,
                "level": f.level,
                "summary": f.summary,
                "evidence": f.evidence,
            })).collect::<Vec<_>>(),
        }));
    } else {
        for f in &findings {
            let mark = match f.level {
                "ok" => "✓",
                "warn" => "!",
                "error" => "✗",
                _ => "·",
            };
            println!("{mark} [{}] {}", f.id, f.summary);
        }
    }
    if has_error {
        2
    } else if has_warn {
        1
    } else {
        0
    }
}

fn diag(_json_out: bool) -> i32 {
    let p = paths::paths();
    let user = exvlan_service_client::paths::user_paths();
    let payload = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "backend": detect().as_str(),
        "supervisor": exvlan_service_client::backend::supervisor(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "paths": {
            "data": user.data.display().to_string(),
            "logs": user.logs.display().to_string(),
            "config": p.config.display().to_string(),
            "core": p.core.display().to_string(),
        },
        "versions": {
            "kernel": read_trim(&p.kernel_version_file()),
            "service": read_trim(&p.service_version_file()),
        },
        "findings": collect_findings().iter().map(|f| json!({
            "id": f.id, "level": f.level, "summary": f.summary, "evidence": f.evidence,
        })).collect::<Vec<_>>(),
    });
    // diag 目前只有 JSON 形态（供 agent / 诊断包直接消费）。
    print_json(&payload);
    0
}

fn print_json(v: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
    );
}

// ---------------------------------------------------------------------------
// auth（本地 token）
// ---------------------------------------------------------------------------

fn token_path() -> PathBuf {
    exvlan_service_client::paths::user_paths()
        .data
        .join("gateway-token")
}

fn auth_cmd(cmd: AuthCmd) -> i32 {
    let path = token_path();
    match cmd {
        AuthCmd::Status => {
            let exists = path.is_file();
            print_json(&json!({
                "token_path": path.display().to_string(),
                "exists": exists,
            }));
            0
        }
        AuthCmd::Login => {
            if path.is_file() {
                println!("token 已存在: {}", path.display());
                return 0;
            }
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let token = random_token();
            match write_private(&path, &token) {
                Ok(()) => {
                    println!("已生成本地 token: {}", path.display());
                    0
                }
                Err(err) => {
                    eprintln!("写入 token 失败: {err}");
                    2
                }
            }
        }
        AuthCmd::Logout => match std::fs::remove_file(&path) {
            Ok(()) => {
                println!("已删除 {}", path.display());
                0
            }
            Err(err) => {
                eprintln!("删除失败: {err}");
                2
            }
        },
    }
}

fn random_token() -> String {
    // 不引入 rand：用时间 + pid + 地址熵，够本地 token 用（gateway 侧只存 hash）。
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("exv_{nanos:x}{pid:x}")
}

fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[allow(dead_code)]
fn backend_str(b: &Backend) -> &'static str {
    b.as_str()
}
