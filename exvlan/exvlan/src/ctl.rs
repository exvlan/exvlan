//! 产品控制面命令（`exvlan-cli service|config|doctor|diag|auth|api`）。
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
    /// 网络配置（内核 `<config-dir>/<id>.toml`，唯一权威）
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
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
    /// 直接调用 gateway（新端点无需等 CLI 发版）
    Api {
        /// HTTP 方法（GET/POST/PUT/PATCH/DELETE）
        method: String,
        /// 路径，如 /api/v1/doctor
        path: String,
        /// 请求体：`@file` / `-`（stdin）/ 字面量 JSON
        #[arg(long)]
        data: Option<String>,
        /// 非安全方法必须显式确认
        #[arg(long)]
        yes: bool,
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
enum ConfigCmd {
    /// 列出已保存的网络（含启用状态）
    List {
        #[arg(long)]
        json: bool,
    },
    /// 读取一条配置（TOML）
    Get { id: String },
    /// 写入/覆盖一条配置（内容来自文件或 stdin）
    Set {
        id: String,
        /// 配置文件路径；`-` 表示 stdin
        #[arg(long)]
        file: String,
        /// 写入后不启用（内核不加载）
        #[arg(long)]
        disabled: bool,
    },
    /// 删除一条配置
    Rm { id: String },
    /// 启用（内核下次启动会加载）
    Enable { id: String },
    /// 禁用（保留配置，内核不加载）
    Disable { id: String },
}

#[derive(Subcommand, Debug)]
enum TokenCmd {
    /// 签发 scoped token（明文只显示一次）
    Create {
        #[arg(long)]
        name: String,
        /// readonly | config | admin
        #[arg(long, default_value = "readonly")]
        role: String,
        /// 追加/覆盖 scope（可重复；与 --role 叠加）
        #[arg(long = "scope")]
        scopes: Vec<String>,
        /// 有效期，如 30d / 24h / 3600（缺省不过期，上限 90d）
        #[arg(long)]
        expires: Option<String>,
    },
    /// 列出 token（不含明文）
    List,
    /// 吊销
    Revoke { id: String },
    /// 校验一个 token（排障用）
    Verify { token: String },
}

#[derive(Subcommand, Debug)]
enum KernelCmd {
    Restart,
    Stop,
    Ensure,
}

#[derive(Subcommand, Debug)]
enum AuthCmd {
    /// scoped token 管理（远程访问用）
    Token {
        #[command(subcommand)]
        cmd: TokenCmd,
    },
    /// 显示本地 token 是否存在（不回显明文）
    Status,
    /// 首次生成本地 token（0600）
    Login,
    /// 删除本地 token
    Logout,
}

/// 真实构建串（CI 注入）；本地直接 `cargo build` 时回落到 crate 版本。
fn build_version() -> &'static str {
    match option_env!("EXVLAN_BUILD_VERSION") {
        Some(v) => v,
        None => env!("CARGO_PKG_VERSION"),
    }
}

/// 入口分发：命中我们的子命令则执行并返回退出码。
pub fn maybe_run() -> Option<i32> {
    let first = std::env::args().nth(1)?;
    // `--version/-V` 由上游 clap 输出的是 crate 版本（不含 `-pre.N+sha`），
    // 排查时容易误判「装的是哪个包」。这里前置拦截，打印真实构建串。
    if matches!(first.as_str(), "--version" | "-V" | "version") {
        println!("exvlan-cli {}", build_version());
        return Some(0);
    }
    if !matches!(
        first.as_str(),
        "service" | "config" | "doctor" | "diag" | "auth" | "api"
    ) {
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
        Cmd::Config { cmd } => config_cmd(cmd),
        Cmd::Doctor { json, fix } => doctor(json, fix),
        Cmd::Diag { json } => diag(json),
        Cmd::Api {
            method,
            path,
            data,
            yes,
        } => api(&method, &path, data.as_deref(), yes),
        Cmd::Auth { cmd } => auth_cmd(cmd),
    })
}

// ---------------------------------------------------------------------------
// config（网络配置：内核配置目录是唯一权威）
// ---------------------------------------------------------------------------

fn config_cmd(cmd: ConfigCmd) -> i32 {
    use exvlan_service_client::config as cfg;
    let result = match cmd {
        ConfigCmd::List { json: as_json } => cfg::list().map(|items| {
            if as_json {
                print_json(&json!(
                    items
                        .iter()
                        .map(|e| json!({
                            "id": e.id,
                            "enabled": e.enabled,
                            "bytes": e.bytes,
                            "modified": e.modified,
                        }))
                        .collect::<Vec<_>>()
                ));
            } else {
                for item in &items {
                    println!(
                        "{}  {:<9} {:>6}B  {}",
                        item.id,
                        if item.enabled { "enabled" } else { "disabled" },
                        item.bytes,
                        fmt_ts(item.modified)
                    );
                }
                if items.is_empty() {
                    println!("（没有已保存的网络配置）");
                }
            }
        }),
        ConfigCmd::Get { id } => cfg::read(&id).map(|saved| {
            if !saved.enabled {
                eprintln!("# {id} 当前为 disabled（内核不加载）");
            }
            print!("{}", saved.text);
        }),
        ConfigCmd::Set { id, file, disabled } => {
            read_input(&file).and_then(|text| cfg::write(&id, &text, !disabled))
        }
        ConfigCmd::Rm { id } => cfg::delete(&id).map(|removed| {
            if removed {
                println!("已删除 {id}");
            } else {
                println!("{id} 不存在（无需删除）");
            }
        }),
        ConfigCmd::Enable { id } => cfg::set_enabled(&id, true),
        ConfigCmd::Disable { id } => cfg::set_enabled(&id, false),
    };
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("错误: {err}");
            2
        }
    }
}

/// `--file <path>` / `--file -`（stdin）。
fn read_input(file: &str) -> Result<String, String> {
    if file == "-" {
        use std::io::Read as _;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("读取 stdin 失败: {e}"))?;
        return Ok(buf);
    }
    std::fs::read_to_string(file).map_err(|e| format!("读取 {file} 失败: {e}"))
}

fn fmt_ts(secs: u64) -> String {
    if secs == 0 {
        return "-".to_string();
    }
    // 不引入时间库：直接给 UNIX 秒，够 agent 用。
    format!("unix:{secs}")
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
// doctor / diag（实现见 exvlan_service_client::doctor，CLI 与 gateway 共用）
// ---------------------------------------------------------------------------

fn doctor(json_out: bool, fix: bool) -> i32 {
    let mut report = exvlan_service_client::doctor::collect();
    if fix {
        exvlan_service_client::doctor::fix(&mut report);
    }
    if json_out {
        print_json(&report.to_json());
    } else {
        print!("{}", report.to_text());
    }
    report.exit_code()
}

fn diag(_json_out: bool) -> i32 {
    let p = paths::paths();
    let user = exvlan_service_client::paths::user_paths();
    let report = exvlan_service_client::doctor::collect();
    print_json(&json!({
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
        "findings": report.findings.iter().map(|f| f.to_json()).collect::<Vec<_>>(),
    }));
    0
}

// ---------------------------------------------------------------------------
// api：直接把请求打到 gateway（HTTP/1.1，无第三方依赖）
// ---------------------------------------------------------------------------

fn gateway_base() -> String {
    std::env::var("EXVLAN_API")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| format!("http://{}", exvlan_service_client::local_gateway_addr()))
}

fn gateway_token() -> Option<String> {
    if let Ok(t) = std::env::var("EXVLAN_TOKEN")
        && !t.trim().is_empty()
    {
        return Some(t.trim().to_string());
    }
    let path = exvlan_service_client::paths::user_paths()
        .data
        .join("gateway-token");
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn api(method: &str, path: &str, data: Option<&str>, yes: bool) -> i32 {
    let method = method.to_ascii_uppercase();
    let safe = matches!(method.as_str(), "GET" | "HEAD");
    let body = match data {
        None => None,
        Some("-") => {
            use std::io::Read as _;
            let mut buf = String::new();
            if std::io::stdin().read_to_string(&mut buf).is_err() {
                eprintln!("读取 stdin 失败");
                return 2;
            }
            Some(buf)
        }
        Some(v) if v.starts_with('@') => match std::fs::read_to_string(&v[1..]) {
            Ok(text) => Some(text),
            Err(err) => {
                eprintln!("读取 {} 失败: {err}", &v[1..]);
                return 2;
            }
        },
        Some(v) => Some(v.to_string()),
    };
    if !safe && !yes {
        eprintln!("{method} {path} 会修改状态；确认请加 --yes（当前为预览）");
        if let Some(b) = &body {
            eprintln!("body: {b}");
        }
        return 2;
    }
    match http_request(&method, path, body.as_deref()) {
        Ok((status, text)) => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                print_json(&v);
            } else {
                println!("{text}");
            }
            if (200..300).contains(&status) { 0 } else { 2 }
        }
        Err(err) => {
            eprintln!("请求失败: {err}");
            3
        }
    }
}

fn http_request(method: &str, path: &str, body: Option<&str>) -> Result<(u16, String), String> {
    let base = gateway_base();
    let rest = base
        .strip_prefix("http://")
        .ok_or_else(|| format!("仅支持 http://（当前 {base}）"))?;
    let (authority, prefix) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{p}")),
        None => (rest, String::new()),
    };
    let full_path = if prefix.is_empty() {
        path.to_string()
    } else {
        format!("{}{}", prefix.trim_end_matches('/'), path)
    };
    let token = gateway_token().ok_or_else(|| {
        "没有 gateway token：先运行 `exvlan-cli auth login` 或启动 gateway 生成".to_string()
    })?;

    let mut stream = std::net::TcpStream::connect(authority)
        .map_err(|e| format!("连接 {authority} 失败: {e}"))?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(10)));

    use std::io::{Read as _, Write as _};
    let mut req = format!(
        "{method} {full_path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n"
    );
    if let Some(b) = body {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("发送请求失败: {e}"))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("读取响应失败: {e}"))?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| "无法解析响应状态".to_string())?;
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
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
        AuthCmd::Token { cmd } => token_cmd(cmd),
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

fn token_cmd(cmd: TokenCmd) -> i32 {
    use exvlan_service_client::tokens::{Store, parse_ttl, scopes_for_role};
    let path = Store::path();
    let mut store = Store::load(&path);
    match cmd {
        TokenCmd::Create {
            name,
            role,
            scopes,
            expires,
        } => {
            let mut final_scopes = match scopes_for_role(&role) {
                Some(v) => v,
                None => {
                    eprintln!("未知 role: {role}（readonly|config|admin）");
                    return 2;
                }
            };
            for s in scopes {
                if !exvlan_service_client::tokens::ALL_SCOPES.contains(&s.as_str()) {
                    eprintln!("未知 scope: {s}");
                    return 2;
                }
                if !final_scopes.contains(&s) {
                    final_scopes.push(s);
                }
            }
            let ttl = match expires.as_deref() {
                None => None,
                Some(text) => match parse_ttl(text) {
                    Some(v) if v > 0 => {
                        if v > 90 * 86_400 {
                            eprintln!("有效期上限 90d");
                            return 2;
                        }
                        Some(v)
                    }
                    _ => {
                        eprintln!("无法解析有效期: {text}（示例 30d / 24h / 3600）");
                        return 2;
                    }
                },
            };
            let (token, record) = store.create(&name, final_scopes, ttl);
            if let Err(err) = store.save(&path) {
                eprintln!("写入 {} 失败: {err}", path.display());
                return 2;
            }
            print_json(&json!({
                "id": record.id,
                "name": record.name,
                "scopes": record.scopes,
                "expires_at": record.expires_at,
                "token": token,
                "note": "明文只显示这一次，请立即保存",
            }));
            0
        }
        TokenCmd::List => {
            let items: Vec<_> = store
                .tokens
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "name": t.name,
                        "scopes": t.scopes,
                        "created_at": t.created_at,
                        "expires_at": t.expires_at,
                        "revoked": t.revoked,
                        "last_used_at": t.last_used_at,
                    })
                })
                .collect();
            print_json(&json!({ "path": path.display().to_string(), "tokens": items }));
            0
        }
        TokenCmd::Verify { token } => match store.verify(&token) {
            Ok(claims) => {
                print_json(&json!({
                    "ok": true,
                    "id": claims.id,
                    "name": claims.name,
                    "scopes": claims.scopes,
                    "store": path.display().to_string(),
                }));
                0
            }
            Err(e) => {
                print_json(&json!({
                    "ok": false,
                    "error": format!("{e:?}"),
                    "store": path.display().to_string(),
                }));
                2
            }
        },
        TokenCmd::Revoke { id } => {
            if store.revoke(&id) {
                match store.save(&path) {
                    Ok(()) => {
                        println!("已吊销 {id}");
                        0
                    }
                    Err(err) => {
                        eprintln!("写入失败: {err}");
                        2
                    }
                }
            } else {
                eprintln!("找不到 token: {id}");
                2
            }
        }
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

fn print_json(v: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
    );
}
