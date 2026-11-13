//! 一键体检（doctor）：backend / 服务 / 内核 / 端口 / 存储 / 迁移。
//!
//! 只依赖本 crate 的 IPC 与路径，**不依赖内核 RPC** —— 所以内核没跑、服务没装
//! 也能给出结论，这正是排障最需要的场景。
//!
//! 同一份 findings 会被 CLI（`exvlan-cli doctor`）与 gateway
//! （`GET /api/v1/doctor`）复用，避免两套判断再次漂移。

use serde_json::{Value, json};

use crate::{backend, ipc, paths};

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
    port_checks(&mut out);
    port_conflict_check(&mut out);
    config_checks(&mut out);
    storage_checks(&mut out, &user);
    #[cfg(target_os = "macos")]
    webkit_check(&mut out, &user);
    migrate_check(&mut out, &user);

    Report { findings: out }
}

fn service_checks(out: &mut Vec<Finding>, p: &paths::Paths) {
    match ipc::request("ping", json!({})) {
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

    let Ok(st) = ipc::request("status", json!({})) else {
        return;
    };
    let mode = st.get("mode").and_then(Value::as_str).unwrap_or("-");
    let pid = st
        .get("kernel")
        .and_then(|k| k.get("pid"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    out.push(Finding {
        id: "service.policy",
        level: "info",
        summary: format!("policy={mode}"),
        evidence: st.clone(),
        fixable: false,
    });
    // `policy` 的规范词汇是 resident/session/stopped；旧服务会回 off/normal/service。
    let stopped = matches!(mode, "stopped" | "off");
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
        return match ipc::request("config.list", json!({})) {
            Ok(resp) => out.push(Finding {
                id: "config.dir",
                level: "ok",
                summary: format!(
                    "配置目录经服务可读写（{} 个网络）",
                    resp.get("data")
                        .and_then(|d| d.get("items"))
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0)
                ),
                evidence: json!({
                    "dir": p.config.display().to_string(),
                    "networks": resp.get("data").and_then(|d| d.get("items")).cloned().unwrap_or_else(|| json!([])),
                }),
                fixable: false,
            }),
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

/// RPC 端口是否被**外来**进程占着（pre.19 起服务会返回结构化 `port_in_use`，
/// 这里做「事前体检」：内核没跑时也能看出来端口被谁占了）。
fn port_conflict_check(out: &mut Vec<Finding>) {
    if !backend::detect().has_service() {
        return;
    }
    let Ok(resp) = ipc::request("port.owner", json!({})) else {
        return;
    };
    let Some(data) = resp.get("data") else {
        return;
    };
    if data.get("free").and_then(Value::as_bool).unwrap_or(false) {
        return;
    }
    let ours = data
        .get("is_our_kernel")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let port = data.get("port").and_then(Value::as_u64).unwrap_or(0);
    if ours {
        out.push(Finding {
            id: "net.port_conflict",
            level: "ok",
            summary: format!("RPC 端口 {port} 由本机内核占用（正常）"),
            evidence: data.clone(),
            fixable: false,
        });
        return;
    }
    let name = data
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let pid = data.get("pid").and_then(Value::as_u64).unwrap_or(0);
    out.push(Finding {
        id: "net.port_conflict",
        level: "error",
        summary: format!(
            "RPC 端口 {port} 被 {name}（pid={pid}）占用，内核起不来；\
             若这是编辑器/SSH 的端口转发，请在它的 Ports 面板停掉该端口，\
             或把 service.toml 的 rpc_portal 改成 \"127.0.0.1:{}\"",
            crate::DEFAULT_RPC_PORT.saturating_add(1)
        ),
        evidence: data.clone(),
        fixable: false,
    });
}

fn version_checks(out: &mut Vec<Finding>, p: &paths::Paths) {
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
}

fn port_checks(out: &mut Vec<Finding>) {
    let rpc = crate::local_rpc_addr();
    let gw = crate::local_gateway_addr();
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
    if present.is_empty() {
        out.push(Finding {
            id: "migrate.legacy",
            level: "ok",
            summary: "没有旧版本遗留数据".into(),
            evidence: json!({"marker": marker.display().to_string()}),
            fixable: false,
        });
    } else {
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
    }
}

/// 执行安全修复（目前只有「内核没跑 → 原子重启」）。
pub fn fix(report: &mut Report) {
    for f in report.findings.iter_mut() {
        if f.id == "kernel.running" && f.level == "error" && f.fixable {
            match ipc::request("kernel.restart", json!({})) {
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
