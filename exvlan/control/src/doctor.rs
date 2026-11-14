//! 一键体检（doctor）：backend / 服务 / 内核 / 端口 / 存储 / 迁移。
//!
//! 只依赖本 crate 的 IPC 与路径，**不依赖内核 RPC** —— 所以内核没跑、服务没装
//! 也能给出结论，这正是排障最需要的场景。
//!
//! 同一份 findings 会被 CLI（`exvlan-cli doctor`）与 gateway
//! （`GET /api/v1/doctor`）复用，避免两套判断再次漂移。

use serde_json::{Value, json};

use crate::protocol::{Payload, Request};
use crate::{backend, ipc, paths};

/// 构建版本串（CI 经 `EXVLAN_BUILD_VERSION` 注入；本地构建回落 crate 版本）。
const BUILD_VERSION: &str = match option_env!("EXVLAN_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Debug, Clone)]
pub struct Finding {
    pub id: &'static str,
    pub level: &'static str,
    pub summary: String,
    pub evidence: Value,
    pub fixable: bool,
}

impl Finding {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "level": self.level,
            "summary": self.summary,
            "evidence": self.evidence,
        })
    }
}

/// 汇总结果：`ok` 表示没有 error 级 findings。
#[derive(Debug, Clone)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn ok(&self) -> bool {
        !self.findings.iter().any(|f| f.level == "error")
    }
    pub fn has_warn(&self) -> bool {
        self.findings.iter().any(|f| f.level == "warn")
    }
    /// 退出码：0 绿 / 1 warn / 2 error。
    pub fn exit_code(&self) -> i32 {
        if !self.ok() {
            2
        } else if self.has_warn() {
            1
        } else {
            0
        }
    }
    pub fn to_json(&self) -> Value {
        json!({
            "ok": self.ok(),
            "findings": self.findings.iter().map(Finding::to_json).collect::<Vec<_>>(),
        })
    }
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for f in &self.findings {
            let mark = match f.level {
                "ok" => "✓",
                "warn" => "!",
                "error" => "✗",
                _ => "·",
            };
            out.push_str(&format!("{mark} [{}] {}\n", f.id, f.summary));
        }
        out
    }
}

/// 采集所有检查项。
pub fn collect() -> Report {
    let mut out = Vec::new();
    let p = paths::paths();
    let user = paths::user_paths();
    let backend = backend::detect();

    out.push(Finding {
        id: "backend",
        level: "info",
        summary: format!(
            "backend={} supervisor={}",
            backend.as_str(),
            backend::supervisor()
        ),
        evidence: json!({"backend": backend.as_str(), "supervisor": backend::supervisor()}),
        fixable: false,
    });

    if backend.has_service() {
        service_checks(&mut out, &p);
    } else {
        out.push(Finding {
            id: "service.installed",
            level: "info",
            summary: "core backend：没有特权服务（内核由 systemd / GUI supervisor 托管）".into(),
            evidence: json!({"backend": "core"}),
            fixable: false,
        });
    }

    version_checks(&mut out, &p);
    let svc = service_status();
    port_checks(&mut out, &svc);
    port_conflict_check(&mut out, &p, &svc);
    rpc_whitelist_check(&mut out);
    config_checks(&mut out);
    update_channel_check(&mut out, &user);
    storage_checks(&mut out, &user);
    #[cfg(target_os = "macos")]
    webkit_check(&mut out, &user);
    migrate_check(&mut out, &user);

    Report { findings: out }
}

fn service_checks(out: &mut Vec<Finding>, p: &paths::Paths) {
    match ipc::call(&Request::Ping) {
        Ok(_) => out.push(Finding {
            id: "service.ipc",
            level: "ok",
            summary: "服务 IPC 可达".into(),
            evidence: json!({"service_bin": p.service_bin.display().to_string()}),
            fixable: false,
        }),
        Err(err) if is_pipe_busy(&err) => out.push(Finding {
            id: "service.pipe_busy",
            level: "error",
            summary: "服务管道实例耗尽（ERROR_PIPE_BUSY）：GUI 会话占满了唯一实例，                      其它客户端（CLI/gateway/自更新/保存配置）全部连不上。                      pre.19 已改为预建多实例；旧版本请先关掉 GUI 再跑 CLI"
                .into(),
            evidence: json!({"error": err, "os_error": 231}),
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

    let Ok(status) = ipc::call(&Request::Status) else {
        return;
    };
    let Some(Payload::Status { mode, kernel, .. }) = &status.data else {
        return;
    };
    let mode = mode.as_str();
    let pid = kernel.as_ref().map(|k| k.pid).unwrap_or(0);
    out.push(Finding {
        id: "service.policy",
        level: "info",
        summary: format!("policy={mode}"),
        evidence: ipc::data_json(&status),
        fixable: false,
    });
    // `policy` 的规范词汇是 resident/session/stopped；旧服务会回 off/normal/service。
    let stopped = mode == "stopped";
    if !stopped && pid == 0 {
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

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let addr = paths::service_ipc_addr();
        match std::fs::metadata(&addr) {
            Ok(md) => {
                let mode = md.mode() & 0o777;
                let me = current_uid();
                let ok = md.uid() == me && mode == 0o600;
                out.push(Finding {
                    id: "service.socket_perms",
                    level: if ok { "ok" } else { "warn" },
                    summary: format!(
                        "{} owner={} mode={:o}{}",
                        addr.display(),
                        md.uid(),
                        mode,
                        if ok { "" } else { "（应为当前用户 + 0600）" }
                    ),
                    evidence: json!({"uid": md.uid(), "mode": format!("{mode:o}"), "current_uid": me}),
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
}

/// 配置目录是 pre.17 的唯一权威：不可读写 = 「保存了但没生效」。
fn config_checks(out: &mut Vec<Finding>) {
    let p = paths::paths();
    if backend::detect().has_service() {
        // 有服务：目录归 root，客户端只关心「经 IPC 能不能读写」。
        // 注意这里直接打 IPC，不走 `config::list()`（那条路在服务不可达时会
        // 悄悄退回本地文件，正好把「服务坏了」这个事实藏起来）。
        return match ipc::call(&Request::ConfigList) {
            Ok(resp) => {
                let count = match &resp.data {
                    Some(Payload::ConfigList { items, .. }) => items.len(),
                    _ => 0,
                };
                let data = ipc::data_json(&resp);
                out.push(Finding {
                    id: "config.dir",
                    level: "ok",
                    summary: format!("配置目录经服务可读写（{count} 个网络）"),
                    evidence: json!({
                        "dir": p.config.display().to_string(),
                        "networks": data.get("items").cloned().unwrap_or_else(|| json!([])),
                    }),
                    fixable: false,
                })
            }
            Err(err) => out.push(Finding {
                id: "config.dir",
                level: "error",
                summary: format!("服务读不到配置目录: {err}"),
                evidence: json!({"error": err}),
                fixable: false,
            }),
        };
    }

    // core（无特权服务）：GUI / CLI 直接写这个目录。
    let writable = dir_writable(&p.config);
    out.push(Finding {
        id: "config.dir",
        level: if writable { "ok" } else { "error" },
        summary: if writable {
            format!("配置目录可写: {}", p.config.display())
        } else {
            format!(
                "配置目录不可写（保存网络会失败）: {} —— 需要 root 把属主改成当前用户（chown -R $(id -u) '{}'）",
                p.config.display(),
                p.config.display()
            )
        },
        evidence: json!({
            "dir": p.config.display().to_string(),
            "writable": writable,
        }),
        fixable: false,
    });
}

/// 是否是「管道实例全被占用」（Windows 的 `ERROR_PIPE_BUSY`）。
fn is_pipe_busy(err: &str) -> bool {
    // 系统消息会随语言本地化，认 os error 码最稳。
    err.contains("os error 231") || err.to_ascii_lowercase().contains("pipe instance")
}

/// 服务 `status` 里 doctor 需要的几项（多处复用，只打一次 IPC）。
#[derive(Debug, Default, Clone)]
struct ServiceStatus {
    /// 内核 pid（`None` = 没在跑）。
    kernel_pid: Option<u32>,
    /// gateway 实际监听地址（`None` = 服务没报，调用方回落回环）。
    gateway_addr: Option<String>,
}

fn service_status() -> ServiceStatus {
    if !backend::detect().has_service() {
        return ServiceStatus::default();
    }
    let Ok(resp) = ipc::call(&Request::Status) else {
        return ServiceStatus::default();
    };
    let Some(Payload::Status {
        kernel, gateway, ..
    }) = &resp.data
    else {
        return ServiceStatus::default();
    };
    ServiceStatus {
        kernel_pid: kernel.as_ref().map(|k| k.pid).filter(|pid| *pid != 0),
        gateway_addr: gateway
            .as_ref()
            .map(|g| g.addr.trim().to_string())
            .filter(|addr| !addr.is_empty()),
    }
}

/// RPC 端口是否被**外来**进程占着（pre.19 起服务会返回结构化 `port_in_use`，
/// 这里做「事前体检」：内核没跑时也能看出来端口被谁占了）。
fn port_conflict_check(out: &mut Vec<Finding>, p: &paths::Paths, svc: &ServiceStatus) {
    if !backend::detect().has_service() {
        return;
    }
    let Ok(resp) = ipc::call(&Request::PortOwner) else {
        return;
    };
    let evidence = ipc::data_json(&resp);
    let Some(Payload::PortOwner {
        port,
        pid,
        name,
        path,
        is_our_kernel,
        free,
        ..
    }) = &resp.data
    else {
        return;
    };
    if *free {
        return;
    }
    let occupant_pid = pid.unwrap_or(0);
    // 占用者是不是**我们自己的内核**：服务自己的判断之外，再用「内核 pid」与
    // 「可执行文件路径」各兜一次 —— 服务刚重启/刚拉起内核时 `is_our_kernel`
    // 可能还没置位，否则 doctor 会把自家内核报成冲突（dev.1 的假阳性）。
    if own_kernel_occupant(
        *is_our_kernel,
        occupant_pid,
        path.as_deref(),
        svc,
        &p.kernel_bin,
    ) {
        out.push(Finding {
            id: "net.port_conflict",
            level: "ok",
            summary: format!("RPC 端口 {port} 由本机内核占用（pid={occupant_pid}，正常）"),
            evidence,
            fixable: false,
        });
        return;
    }
    let name = name.clone().unwrap_or_else(|| "unknown".to_string());
    out.push(Finding {
        id: "net.port_conflict",
        level: "error",
        summary: format!(
            "RPC 端口 {port} 被 {name}（pid={occupant_pid}）占用，内核起不来；\
             若这是编辑器/SSH 的端口转发，请在它的 Ports 面板停掉该端口，\
             或把 service.toml 的 rpc_portal 改成 \"127.0.0.1:{}\"",
            crate::DEFAULT_RPC_PORT.saturating_add(1)
        ),
        evidence,
        fixable: false,
    });
}

/// 两个路径（canonicalize 后）是否指向同一个可执行文件。
fn same_exe(raw: &str, expected: &std::path::Path) -> bool {
    let raw = raw.trim();
    if raw.is_empty() {
        return false;
    }
    let canon =
        |path: &std::path::Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canon(std::path::Path::new(raw)) == canon(expected)
}

/// 端口占用者是否就是本机内核（三条判据任一成立）。
fn own_kernel_occupant(
    is_our_kernel: Option<bool>,
    occupant_pid: u32,
    occupant_path: Option<&str>,
    svc: &ServiceStatus,
    kernel_bin: &std::path::Path,
) -> bool {
    is_our_kernel.unwrap_or(false)
        // pid=0 是「拿不到」的哨兵值，不能与「内核 pid 未知」互认。
        || (occupant_pid != 0 && svc.kernel_pid.is_some_and(|k| k == occupant_pid))
        || occupant_path.is_some_and(|path| same_exe(path, kernel_bin))
}

/// RPC 白名单模式：默认只回环（远端走 gateway）；旧机器可能显式打开 mesh 网段。
fn rpc_whitelist_check(out: &mut Vec<Finding>) {
    if !backend::detect().has_service() {
        return;
    }
    let Ok(resp) = ipc::call(&Request::WhitelistGet) else {
        return;
    };
    let Some(Payload::WhitelistGet { info }) = &resp.data else {
        return;
    };
    let effective = info.effective.clone();
    let mesh = effective
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|s| !(s.starts_with("127.") || s.starts_with("::1")));
    out.push(Finding {
        id: "net.rpc_whitelist",
        level: "info",
        summary: if mesh {
            format!("RPC 白名单含 mesh 网段（远端可直连内核）：{effective}")
        } else {
            "RPC 白名单只含回环（远端统一走 gateway）".to_string()
        },
        evidence: ipc::data_json(&resp),
        fixable: false,
    });
}

fn version_checks(out: &mut Vec<Finding>, p: &paths::Paths) {
    let kernel_v = read_trim(&p.kernel_version_file());
    let service_v = read_trim(&p.service_version_file());
    let cli_v = env!("CARGO_PKG_VERSION").to_string();
    if let (Some(k), Some(s)) = (&kernel_v, &service_v)
        && !same_version(k, s)
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
}

/// 版本比较**忽略构建元数据**（`+<sha6>`）：同一标准版本的不同构建不算不一致。
///
/// `26.410.20715-dev.1+bd16ae` 与 `26.410.20715-dev.1` 是同一版；
/// 不这样处理时，每次安装都会报一条假的 `version.mismatch`。
pub fn same_version(a: &str, b: &str) -> bool {
    version_core(a) == version_core(b)
}

/// 版本主体（去掉 `+<build>` 与首尾空白）。
fn version_core(version: &str) -> &str {
    version.split('+').next().unwrap_or(version).trim()
}

/// 从版本串推断更新通道：`<标准版本>[-<channel>.<n>][+<sha6>]`。
///
/// 无通道后缀 = `stable`；仅用于「prefs 里没写通道」时给出信息项，**不联网**。
pub fn channel_from_version(version: &str) -> &'static str {
    let Some((_, tail)) = version_core(version).split_once('-') else {
        return "stable";
    };
    let name = tail.split(['.', '+']).next().unwrap_or("").trim();
    match name {
        "nightly" => "nightly",
        "dev" => "dev",
        "beta" => "beta",
        "rc" => "rc",
        _ => "stable",
    }
}

/// 更新通道（**只读** `prefs.toml`，不联网）。
///
/// 排障时「为什么没收到 nightly」的第一个问题就是「当前通道是哪个」，
/// 以前只能翻 GUI 设置页。
fn update_channel_check(out: &mut Vec<Finding>, user: &paths::UserPaths) {
    let path = user.data.join("prefs.toml");
    let read = std::fs::read_to_string(&path);
    let prefs_channel = read
        .as_deref()
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(text).ok())
        .and_then(|v| {
            v.get("update")
                .and_then(|u| u.get("channel"))
                .and_then(toml::Value::as_str)
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty());
    let from_prefs = prefs_channel.is_some();
    let inferred = channel_from_version(BUILD_VERSION);
    let summary = if let Some(channel) = &prefs_channel {
        format!("更新通道 {channel}（prefs.toml）")
    } else {
        let why = match &read {
            Ok(_) => "无 [update] channel".to_string(),
            Err(err) => format!("读不到: {err}"),
        };
        format!("更新通道未设置（prefs.toml {why}）→ 按版本串推断 {inferred}")
    };
    out.push(Finding {
        id: "doctor.update.channel",
        level: "info",
        summary,
        evidence: json!({
            "channel": prefs_channel.unwrap_or_else(|| inferred.to_string()),
            "source": if from_prefs { "prefs" } else { "version" },
            "inferred": inferred,
            "path": path.display().to_string(),
        }),
        fixable: false,
    });
}

fn port_checks(out: &mut Vec<Finding>, svc: &ServiceStatus) {
    let rpc = crate::local_rpc_addr();
    // gateway 可能绑在 mesh 地址（`[gateway] bind` 或自动取虚拟地址），
    // 只探回环会误报 closed —— 用服务上报的 addr，拿不到才回落回环。
    let gw = svc
        .gateway_addr
        .as_deref()
        .map(normalize_probe_addr)
        .unwrap_or_else(crate::local_gateway_addr);
    let rpc_open = tcp_open(&rpc);
    let gw_open = tcp_open(&gw);
    out.push(Finding {
        id: "net.ports",
        level: if rpc_open { "ok" } else { "warn" },
        summary: format!(
            "rpc {rpc} {} / gateway {gw} {}",
            if rpc_open { "listening" } else { "closed" },
            if gw_open { "listening" } else { "closed" }
        ),
        evidence: json!({"rpc": rpc, "rpc_open": rpc_open, "gateway": gw, "gateway_open": gw_open}),
        fixable: false,
    });
}

/// 服务上报的 `addr` 形如 `10.66.0.2:2016`；裸 IPv6（服务端用 `{bind}:{port}`
/// 拼接）要补方括号，否则 `SocketAddr` 解析必然失败、探测结果永远 closed。
fn normalize_probe_addr(addr: &str) -> String {
    let addr = addr.trim();
    if addr.starts_with('[') {
        return addr.to_string();
    }
    match addr.rsplit_once(':') {
        Some((host, port)) if host.contains(':') => format!("[{host}]:{port}"),
        _ => addr.to_string(),
    }
}

fn storage_checks(out: &mut Vec<Finding>, user: &paths::UserPaths) {
    let ok = dir_writable(&user.data);
    out.push(Finding {
        id: "app.prefs_dir",
        level: if ok { "ok" } else { "error" },
        summary: format!(
            "prefs 目录 {} {}",
            user.data.display(),
            if ok { "可写" } else { "不可写" }
        ),
        evidence: json!({"path": user.data.display().to_string()}),
        fixable: false,
    });
}

#[cfg(target_os = "macos")]
fn webkit_check(out: &mut Vec<Finding>, user: &paths::UserPaths) {
    use std::os::unix::fs::MetadataExt as _;

    let path = user
        .data
        .parent()
        .unwrap_or(std::path::Path::new("/"))
        .join("WebKit")
        .join(crate::APP_ID);
    let me = current_uid();
    match std::fs::metadata(&path) {
        Ok(md) if md.uid() != me => out.push(Finding {
            id: "app.webkit_owner",
            level: "error",
            summary: format!(
                "{} 属主是 {}（应为 {me}）→ localStorage 会失效（远程目标/语言/上次网络全静默失败）；\
                 服务下次启动会自动 chown",
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
        // 目录还没建：全新安装的正常状态（WKWebView 首次写入时才创建），不该报警。
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => out.push(Finding {
            id: "app.webkit_owner",
            level: "info",
            summary: "WebKit 目录尚未创建（首次写入 localStorage 后出现，属正常）".into(),
            evidence: json!({"path": path.display().to_string()}),
            fixable: false,
        }),
        Err(err) => out.push(Finding {
            id: "app.webkit_owner",
            level: "warn",
            summary: format!("{} 读不到: {err}", path.display()),
            evidence: json!({"path": path.display().to_string()}),
            fixable: false,
        }),
    }
}

fn migrate_check(out: &mut Vec<Finding>, user: &paths::UserPaths) {
    let marker = user.data.join("migrate.done");
    // 与 GUI 迁移闸门、`exvlan-migrate` 共用同一份检测（各写一遍必然漂）。
    let legacy = crate::legacy::detect(user, &paths::paths());
    if legacy.pending() {
        out.push(Finding {
            id: "migrate.legacy",
            level: "error",
            summary: format!(
                "检测到旧版本数据（{} 项）→ 请先运行 exvlan-migrate",
                legacy.items.len()
            ),
            evidence: json!({
                "items": legacy.item_labels(),
                "marker": marker.display().to_string(),
            }),
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
}

/// 执行安全修复（目前只有「内核没跑 → 原子重启」）。
pub fn fix(report: &mut Report) {
    for f in report.findings.iter_mut() {
        if f.id == "kernel.running" && f.level == "error" && f.fixable {
            match ipc::call(&Request::KernelRestart) {
                Ok(_) => {
                    f.level = "ok";
                    f.summary = "内核已重启".into();
                }
                Err(err) => f.summary = format!("重启内核失败: {err}"),
            }
        }
    }
}

fn read_trim(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
        path.parent()
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
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

pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(0)
    }
    #[cfg(windows)]
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn status(pid: Option<u32>, gateway: Option<&str>) -> ServiceStatus {
        ServiceStatus {
            kernel_pid: pid,
            gateway_addr: gateway.map(str::to_string),
        }
    }

    #[test]
    fn version_compare_ignores_build_metadata() {
        // dev.1 真机验证：`+bd16ae` 被当成不一致 → 假的 version.mismatch。
        assert!(same_version(
            "26.410.20715-dev.1+bd16ae",
            "26.410.20715-dev.1"
        ));
        assert!(same_version(
            "26.410.20715-dev.1",
            "26.410.20715-dev.1+bd16ae"
        ));
        assert!(same_version("26.410.20715+aaa", "26.410.20715+bbb"));
        assert!(!same_version("26.410.20715-dev.1", "26.410.20715-dev.2"));
        assert!(!same_version("26.410.20715", "26.411.20713"));
    }

    #[test]
    fn channel_inferred_from_version_string() {
        assert_eq!(channel_from_version("26.410.20715"), "stable");
        assert_eq!(channel_from_version("26.410.20715+bd16ae"), "stable");
        assert_eq!(channel_from_version("26.410.20715-dev.1"), "dev");
        assert_eq!(channel_from_version("26.410.20715-dev.1+bd16ae"), "dev");
        assert_eq!(channel_from_version("26.410.20715-beta.2"), "beta");
        assert_eq!(channel_from_version("26.410.20715-rc.3"), "rc");
        assert_eq!(channel_from_version("26.410.20715-nightly.0425"), "nightly");
    }

    #[test]
    fn own_kernel_is_not_reported_as_conflict() {
        let dir = temp_dir("exe");
        let kernel = dir.join("exvlan");
        std::fs::write(&kernel, b"#!").expect("write fake kernel");
        let foreign = dir.join("code");
        std::fs::write(&foreign, b"#!").expect("write fake foreign binary");

        // ① 服务自己已经判定是我们的内核
        assert!(own_kernel_occupant(
            Some(true),
            1234,
            Some(&foreign.display().to_string()),
            &status(None, None),
            &kernel
        ));
        // ② pid 与 status.kernel.pid 相同（is_our_kernel 尚未置位）
        assert!(own_kernel_occupant(
            None,
            4242,
            None,
            &status(Some(4242), None),
            &kernel
        ));
        // ③ 路径 canonicalize 后与内核二进制一致（`..` 会被折叠）
        std::fs::create_dir_all(dir.join("core")).expect("temp core dir");
        let alias = dir.join("core").join("..").join("exvlan");
        assert!(own_kernel_occupant(
            Some(false),
            9,
            Some(&alias.display().to_string()),
            &status(Some(4242), None),
            &kernel
        ));
        // 外来进程：pid 与路径都不匹配（同目录的另一个可执行文件）
        assert!(!own_kernel_occupant(
            Some(false),
            9,
            Some(&foreign.display().to_string()),
            &status(Some(4242), None),
            &kernel
        ));
        // pid=0 是「拿不到」的哨兵值，不能与「内核 pid 未知」互认
        assert!(!own_kernel_occupant(
            None,
            0,
            None,
            &status(None, None),
            &kernel
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gateway_probe_uses_reported_mesh_addr() {
        // gateway 绑 mesh 地址时，只探回环会误报 closed。
        let port = crate::DEFAULT_GATEWAY_PORT;
        let mesh = format!("10.66.0.2:{port}");
        let svc = status(None, Some(&format!(" {mesh} ")));
        let addr = svc
            .gateway_addr
            .as_deref()
            .map(normalize_probe_addr)
            .unwrap_or_else(crate::local_gateway_addr);
        assert_eq!(addr, mesh);

        // 拿不到 → 回落回环
        let fallback = status(None, None)
            .gateway_addr
            .as_deref()
            .map(normalize_probe_addr)
            .unwrap_or_else(crate::local_gateway_addr);
        assert_eq!(fallback, crate::local_gateway_addr());

        // 裸 IPv6（服务端用 `{bind}:{port}` 拼接）必须补方括号
        let bare = format!("fd00::5:{port}");
        let bracketed = format!("[fd00::5]:{port}");
        assert_eq!(normalize_probe_addr(&bare), bracketed);
        assert_eq!(normalize_probe_addr(&bracketed), bracketed);
        assert_eq!(
            normalize_probe_addr(&format!("0.0.0.0:{port}")),
            format!("0.0.0.0:{port}")
        );
    }

    /// 测试用临时目录（doctor 的单测不碰真实系统路径）。
    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-doctor-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn update_channel_finding_reads_prefs_only() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "exvlan-doctor-prefs-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let user = crate::paths::UserPaths {
            data: dir.clone(),
            logs: dir.clone(),
        };

        // 没有 prefs.toml：info 级、按版本串推断，不报错
        let mut out = Vec::new();
        update_channel_check(&mut out, &user);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "doctor.update.channel");
        assert_eq!(out[0].level, "info");
        assert_eq!(out[0].evidence["source"], "version");

        // 写了通道：原样输出
        std::fs::write(
            dir.join("prefs.toml"),
            "[update]\nchannel = \"dev\"\nauto_check = true\n",
        )
        .expect("write prefs");
        let mut out = Vec::new();
        update_channel_check(&mut out, &user);
        assert_eq!(out[0].level, "info");
        assert_eq!(out[0].evidence["channel"], "dev");
        assert_eq!(out[0].evidence["source"], "prefs");
        assert!(out[0].summary.contains("dev"), "{}", out[0].summary);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
