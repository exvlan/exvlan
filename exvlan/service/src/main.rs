//! EXVlan 特权服务（macOS LaunchDaemon / Windows Service）。
//!
//! 职责（合并了原 helper 与内核服务）：
//! 1. 持有内核：service 模式常驻；normal 模式随 GUI session 起停
//! 2. 应用内核更新：minisign 校验 → 保留上一版 → 失败自动回滚
//! 3. 通过 IPC 暴露 status / mode / kernel / apply
//!
//! 设计要点：模式只是「生命周期策略」，**切换模式不重启内核**；
//! 配置与端口两模式一致（单一 config；RPC 端口默认 1995）。

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

/// 应用 ID（目录名 / 旧 label 前缀）。与服务侧 `paths.rs` 的 `APP_ID` 一致。
/// 仅 unix 分支用到（Windows 走命名管道与 ProgramData 字面量）。
#[cfg(unix)]
const APP_ID: &str = "era.exvlan.studio";

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
/// RPC 端口（在内核与 GUI 之间保持一致；service.toml 可覆盖）。
const RPC_PORT: u16 = 1995;
/// 默认只绑回环：远端管理走 gateway（`docs/decisions-7.3.md` §14.2）。
/// 需要 GUI 直连远端内核的老用法，在 `service.toml` 里写 `rpc_portal = "0.0.0.0:1995"`。
const RPC_PORTAL_DEFAULT: &str = "127.0.0.1:1995";
/// 回环：GUI 自己从这里连进来，**任何情况下都不能少**。
///
/// 内核的默认白名单是 `127.0.0.0/8` + `::1/128`；一旦显式传
/// `--rpc-portal-whitelist`，就是**整体替换**而不是追加。pre.13 只写了
/// `10.254.0.0/16` → GUI 从 127.0.0.1 连进来被 `on_new_client` 拒绝
/// （TCP 能建连、请求无人应答），表现为
/// `[cmd:list_network_instance_ids] Timeout: deadline has elapsed`。
const RPC_LOOPBACK: [&str; 2] = ["127.0.0.0/8", "::1/128"];

/// DHCP 实例读不到静态地址时的回退池（保守值，会写进日志提醒）。
const DHCP_FALLBACK_POOL: &str = "10.254.0.0/16";

/// 额外白名单允许的上限（防止把整张表塞进来）。
const MAX_EXTRA_WHITELIST: usize = 8;
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// 本地 RPC 地址（内核固定监听这个端口）。
const LOCAL_RPC: &str = "127.0.0.1:1995";

/// tun 的 MTU。
///
/// 内核默认 1380 → tun 实际是 1360，TCP MSS 会跟着变成 1320，即 IP 层
/// 1360 字节。而经 mesh/relay 的真实路径 MTU 常常只有 ~1330（实测：
/// 1328 字节能过、1368 不能），于握手能成、一收大包就被丢 ——
/// 表现为「TCP 连上了但请求全部超时」（issue #54/#55）。
///
/// 取 1300：减去链路开销后 tun ≈ 1280（IPv6 最小 MTU），穿任何路径都安全，
/// 代价是吞吐降约 7%。内核已有 `--mtu` 参数，无需改内核。
/// 看门狗等待上限：超过这个时间内核还没起来，就认为 boot 卡住了。
const BOOT_WATCHDOG: Duration = Duration::from_secs(20);

/// 看门狗用的**无锁**状态：故意不碰 `inner` 互斥量 ——
/// 它要能检测「boot 因为持锁而卡死」这种情况，自己去拿锁就一起卡住了。
static KERNEL_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// `boot()` 是否已经**走完**（成功或失败都算）。
///
/// 看门狗只关心「卡住」，不关心「失败」：内核缺失/端口冲突属于可解释的失败，
/// 已经写进日志，不该因为看门狗把服务反复重启（那会变成重启风暴）。
static BOOT_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    pub service_config: PathBuf,
    pub service_log: PathBuf,
    /// 随安装包分发的控制面工具（`--install --tools-dir` 拷贝到这里）。
    pub cli_bin: PathBuf,
    pub gateway_bin: PathBuf,
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

/// 安装者 uid 的旁路记录（0644）。`service.json` 是 0600 root，服务自身能读，
/// 但一旦它损坏/缺失，socket 属主就无从得知 —— 那时退化成 `0660 root:wheel`，
/// 正是 pre.13 让普通用户 EACCES 的原因。这里留一份可读的副本。
#[cfg(unix)]
fn owner_path(bin: &Path) -> PathBuf {
    let mut raw = bin.as_os_str().to_os_string();
    raw.push(".owner");
    PathBuf::from(raw)
}

#[cfg(unix)]
fn write_owner_file(p: &Paths, uid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let path = owner_path(&p.service_bin);
    std::fs::write(&path, format!("{uid}\n"))?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
    Ok(())
}

#[cfg(unix)]
fn read_owner_file(p: &Paths) -> Option<u32> {
    let text = std::fs::read_to_string(owner_path(&p.service_bin)).ok()?;
    text.trim().parse().ok().filter(|uid| *uid != 0)
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

/// `rename` + 轮询重试。
///
/// Windows 上刚停掉的内核可能还握着 exe 句柄（进程退出 → 句柄释放是异步的），
/// 直接 rename 会失败（实测 os error 32「另一个程序正在使用此文件」）。
fn rename_with_retry(from: &Path, to: &Path, attempts: u32) -> anyhow::Result<()> {
    let mut last: Option<std::io::Error> = None;
    for attempt in 0..attempts {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err) => {
                last = Some(err);
                std::thread::sleep(Duration::from_millis(100 * u64::from(attempt + 1)));
            }
        }
    }
    Err(anyhow::anyhow!(
        "rename {} → {} 失败（已重试 {attempts} 次）: {}",
        from.display(),
        to.display(),
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

fn paths() -> Paths {
    #[cfg(target_os = "macos")]
    {
        let data = PathBuf::from("/Library/Application Support/era.exvlan.studio");
        let logs = PathBuf::from("/Library/Logs/era.exvlan.studio");
        let core = data.join("core");
        let core_cli = core.join("exvlan-cli");
        let core_gw = core.join("exvlan-gateway");
        Paths {
            config: data.join("config"),
            kernel: core.join("exvlan"),
            kernel_prev: core.join("exvlan.prev"),
            core,
            service_bin: PathBuf::from("/Library/PrivilegedHelperTools/era.exvlan.studio.service"),
            service_config: data.join("service.toml"),
            service_log: logs.join("service.log"),
            logs,
            cli_bin: core_cli,
            gateway_bin: core_gw,
        }
    }
    #[cfg(target_os = "windows")]
    {
        let data = PathBuf::from(r"C:\ProgramData\era.exvlan.studio");
        let core = data.join("core");
        let core_cli = core.join("exvlan-cli.exe");
        let core_gw = core.join("exvlan-gateway.exe");
        Paths {
            config: data.join("config"),
            kernel: core.join("exvlan.exe"),
            kernel_prev: core.join("exvlan.exe.prev"),
            core,
            logs: data.join("logs"),
            service_bin: data.join("bin").join("exvlan-service.exe"),
            service_config: data.join("service.toml"),
            service_log: data.join("logs").join("service.log"),
            cli_bin: core_cli,
            gateway_bin: core_gw,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let data = PathBuf::from("/var/lib/era.exvlan.studio");
        let core = data.join("core");
        let core_cli = core.join("exvlan-cli");
        let core_gw = core.join("exvlan-gateway");
        Paths {
            config: data.join("config"),
            kernel: core.join("exvlan"),
            kernel_prev: core.join("exvlan.prev"),
            core,
            logs: PathBuf::from("/var/log/era.exvlan.studio"),
            service_bin: PathBuf::from("/usr/local/bin/exvlan-service"),
            service_config: data.join("service.toml"),
            service_log: PathBuf::from("/var/log/era.exvlan.studio/service.log"),
            cli_bin: core_cli,
            gateway_bin: core_gw,
        }
    }
}

// ---------------------------------------------------------------------------
// 配置与状态
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// 常驻（旧名 `service`）
    #[serde(rename = "resident", alias = "service", alias = "Service")]
    #[default]
    Service,
    /// 随客户端（旧名 `normal`）
    #[serde(rename = "session", alias = "normal", alias = "Normal")]
    Normal,
    /// 停止（旧名 `off`）
    #[serde(rename = "stopped", alias = "off", alias = "Off")]
    Off,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceConfig {
    #[serde(default)]
    pub owner_uid: u32,
    /// 生命周期策略。pre.17 起字段名为 `policy`；`mode` 作为旧名兼容读取。
    #[serde(default, alias = "mode")]
    pub policy: Mode,
    /// 用户额外允许访问 RPC 的网段（逃生口，例如 WSL 的 172.x）。
    /// 默认空：实际白名单由 `<config-dir>/*.toml` 里的虚拟网段自动推导。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rpc_extra_whitelist: Vec<String>,
    /// Windows：安装者（GUI 用户）的 SID，用于把命名管道 DACL 收窄到本人。
    ///
    /// 空 = 未知（旧安装 / macOS / Linux）→ 管道退回 `IU`（交互式用户），
    /// 与 pre.17 之前的行为一致，**不会**把用户锁在门外。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner_sid: String,
    /// 内核 RPC portal 的绑定地址。
    ///
    /// 默认 **`127.0.0.1:1995`**（远端统一走 gateway）。
    ///
    /// 旧行为是 `0.0.0.0:1995` + 白名单兜底；确实需要「GUI 直连远端内核」，
    /// 可显式写成 `rpc_portal = "0.0.0.0:1995"`（等于把 RPC 暴露给白名单网段）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_portal: Option<String>,
    /// 本机 gateway（控制面 HTTP/WS）。由服务托管：随服务启停、异常退避重启。
    #[serde(default)]
    pub gateway: GatewayConfig,
    /// 是否把各虚拟网段（config-dir 推导）与额外网段放进 RPC 白名单。
    ///
    /// **默认 false**：RPC 只绑回环，远端统一走 gateway（安全优先）。
    /// 旧版本（pre.19 及更早）的 GUI 直连远端内核依赖这项为 true ——
    /// 迁移工具会把旧机器显式写成 true，避免升级即断远端直连。
    #[serde(default)]
    pub rpc_mesh_whitelist: bool,
}

/// `service.toml [gateway]`：控制面网关的托管配置。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// 是否由服务拉起（默认 true）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 监听地址。空 = 自动取本机 mesh 虚拟地址；也可写 `127.0.0.1` / `0.0.0.0`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    /// 监听端口（默认 2016）。
    #[serde(default = "default_gateway_port")]
    pub port: u16,
}

fn default_true() -> bool {
    true
}

fn default_gateway_port() -> u16 {
    2016
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: None,
            port: default_gateway_port(),
        }
    }
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            owner_uid: 0,
            policy: Mode::Service,
            rpc_extra_whitelist: Vec::new(),
            owner_sid: String::new(),
            rpc_portal: None,
            rpc_mesh_whitelist: false,
            gateway: GatewayConfig::default(),
        }
    }
}

fn load_config(p: &Paths) -> ServiceConfig {
    // pre.17 起统一用 TOML（与 prefs.toml / config 目录一致）；
    // 旧 `service.json` 由 `exvlan-migrate` 一次性转换，运行时不做兼容读取。
    std::fs::read_to_string(&p.service_config)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_config(p: &Paths, cfg: &ServiceConfig) -> anyhow::Result<()> {
    if let Some(dir) = p.service_config.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let text = toml::to_string_pretty(cfg)?;
    std::fs::write(&p.service_config, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&p.service_config, std::fs::Permissions::from_mode(0o600));
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
    /// `whitelist.set` 的额外网段
    #[serde(default)]
    extra: Option<Vec<String>>,
    /// `config.*` 的网络实例 id（也是配置文件名 `<id>.toml`）
    #[serde(default)]
    id: Option<String>,
    /// `config.write` 的配置正文（内核 TOML）
    #[serde(default)]
    text: Option<String>,
    /// `config.write` 是否启用（false → `<id>.toml.disabled`，内核不加载）
    #[serde(default)]
    enabled: Option<bool>,
    /// `gateway.set` 的增量修改（未给的字段保持不变）
    #[serde(default)]
    gateway: Option<GatewayPatch>,
    /// `gateway.pair.start` 的角色（readonly|config|admin）
    #[serde(default)]
    role: Option<String>,
    /// `gateway.pair.start` 的配对码有效期（秒）
    #[serde(default)]
    ttl: Option<u64>,
}

/// `gateway.set`：只写给出的字段，其余沿用当前配置。
#[derive(Debug, Clone, Default, Deserialize)]
struct GatewayPatch {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    bind: Option<String>,
    #[serde(default)]
    port: Option<u16>,
}

#[derive(Debug, Serialize)]
struct KernelInfo {
    pid: u32,
    running: bool,
    rpc: String,
}

#[derive(Debug, Serialize)]
struct GatewayInfo {
    pid: u32,
    running: bool,
    addr: String,
    enabled: bool,
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
    gateway: Option<GatewayInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    /// 结构化数据（whitelist / port 这类查询）
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl Response {
    fn ok() -> Self {
        Self {
            ok: true,
            proto: None,
            service: None,
            mode: None,
            kernel: None,
            gateway: None,
            code: None,
            message: None,
            data: None,
        }
    }
    /// 把 `port_in_use|{json}` 形式的错误拆成结构化响应（GUI 据此渲染卡片）。
    fn conflict(payload: serde_json::Value) -> Self {
        let port = payload.get("port").and_then(|v| v.as_u64()).unwrap_or(0);
        let name = payload
            .get("occupant")
            .and_then(|o| o.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let pid = payload
            .get("occupant")
            .and_then(|o| o.get("pid"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let kind = payload
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("foreign");
        let message = format!("RPC 端口 {port} 被 {name}（pid={pid}, kind={kind}）占用");
        Self::err_with_data("port_in_use", message, payload)
    }

    fn err_with_data(code: &str, message: impl Into<String>, data: serde_json::Value) -> Self {
        let mut r = Self::err(code, message);
        r.data = Some(data);
        r
    }

    fn err(code: &str, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            proto: None,
            service: None,
            mode: None,
            kernel: None,
            gateway: None,
            code: Some(code.to_string()),
            message: Some(message.into()),
            data: None,
        }
    }
}

// ---------------------------------------------------------------------------
// 内核生命周期
// ---------------------------------------------------------------------------

struct Inner {
    /// 当前内核实际启动时用的白名单（用于判断「配置改了但还没重启」）。
    applied_whitelist: Option<String>,
    /// 已有线程正在启动内核（避免并发重复 spawn）。
    starting: bool,
    child: Option<Child>,
    mode: Mode,
    config: ServiceConfig,
    sessions: usize,
    /// 正在执行「原子重启内核」：期间不要因会话归零而停内核。
    restarting: bool,
    /// 托管的 gateway 子进程（stdin 管道保持打开：服务没了 → 对端读到 EOF 自行退出）。
    gateway_child: Option<Child>,
    /// gateway 连续重启次数（用于指数退避；稳定运行一轮后清零）。
    gateway_restarts: u32,
}

pub struct State {
    paths: Paths,
    inner: Mutex<Inner>,
}

// ---------------------------------------------------------------------------
// RPC 白名单：由已配置的虚拟网段推导
// ---------------------------------------------------------------------------

/// `10.254.1.22/27` → `10.254.1.0/27`（把主机位清零）。
fn cidr_network(value: &str) -> Option<String> {
    let (addr, len) = value.trim().split_once('/')?;
    let len: u8 = len.trim().parse().ok()?;
    if let Ok(v4) = addr.trim().parse::<std::net::Ipv4Addr>() {
        if len > 32 {
            return None;
        }
        let mask = if len == 0 {
            0
        } else {
            u32::MAX << (32 - u32::from(len))
        };
        let net = std::net::Ipv4Addr::from(u32::from(v4) & mask);
        return Some(format!("{net}/{len}"));
    }
    if let Ok(v6) = addr.trim().parse::<std::net::Ipv6Addr>() {
        if len > 128 {
            return None;
        }
        let mask = if len == 0 {
            0
        } else {
            u128::MAX << (128 - u32::from(len))
        };
        let net = std::net::Ipv6Addr::from(u128::from(v6) & mask);
        return Some(format!("{net}/{len}"));
    }
    None
}

/// 只允许私网 / 链路本地 / 回环。拒绝 `0.0.0.0/0` 与公网段 —— 白名单是唯一门禁，
/// 放公网等于把无认证的内核 RPC 暴露到互联网。
fn validate_cidr(cidr: &str) -> Result<String, String> {
    let normalized = cidr_network(cidr).ok_or_else(|| format!("不是合法 CIDR: {cidr}"))?;
    let (addr, len) = normalized.split_once('/').expect("cidr_network 保证有 /");
    let len: u8 = len.parse().expect("cidr_network 保证 len 是数字");

    let v4_ranges: [(std::net::Ipv4Addr, u8); 5] = [
        ("10.0.0.0".parse().unwrap(), 8),
        ("172.16.0.0".parse().unwrap(), 12),
        ("192.168.0.0".parse().unwrap(), 16),
        ("127.0.0.0".parse().unwrap(), 8),
        ("100.64.0.0".parse().unwrap(), 10),
    ];
    if let Ok(v4) = addr.parse::<std::net::Ipv4Addr>() {
        for (base, base_len) in v4_ranges {
            if len >= base_len {
                let mask = u32::MAX << (32 - u32::from(base_len));
                if u32::from(v4) & mask == u32::from(base) & mask {
                    return Ok(normalized);
                }
            }
        }
        // 169.254.0.0/16（链路本地）
        if len >= 16
            && u32::from(v4) & 0xffff_0000
                == u32::from("169.254.0.0".parse::<std::net::Ipv4Addr>().unwrap())
        {
            return Ok(normalized);
        }
        return Err(format!("只允许私网/链路本地/回环网段（拒绝 {cidr}）"));
    }
    if let Ok(v6) = addr.parse::<std::net::Ipv6Addr>() {
        let bits = u128::from(v6);
        // ::1/128
        if bits == 1 {
            return Ok(normalized);
        }
        // fc00::/7、fe80::/10
        for (base, base_len) in [("fc00::", 7u8), ("fe80::", 10)] {
            if len >= base_len {
                let mask = u128::MAX << (128 - u32::from(base_len));
                let base_bits = u128::from(base.parse::<std::net::Ipv6Addr>().unwrap());
                if bits & mask == base_bits & mask {
                    return Ok(normalized);
                }
            }
        }
        return Err(format!("只允许私网/链路本地/回环网段（拒绝 {cidr}）"));
    }
    Err(format!("不是合法 CIDR: {cidr}"))
}

// ---------------------------------------------------------------------------
// 网络配置读写（`config.*`）
// ---------------------------------------------------------------------------
//
// 权威数据：内核从 `<config-dir>/*.toml` 加载网络实例。pre.17 起 GUI **不再**
// 自己维护 `networks.toml`，而是经这里写文件（服务是唯一写者，目录属主是 root）。
//
// 禁用状态用后缀表达：`<id>.toml.disabled`（内核只扫 `*.toml`，自然不加载），
// 这样「禁用」不会丢配置，也不需要第二份状态文件。

/// 启用中的配置：`<id>.toml`。
const CONFIG_ENABLED_SUFFIX: &str = ".toml";
/// 已禁用的配置：`<id>.toml.disabled`。
const CONFIG_DISABLED_SUFFIX: &str = ".toml.disabled";

/// 实例 id → 文件名。**必须**挡住 `../`、绝对路径等输入：IPC 对面是 root。
fn validate_network_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty() || id.len() > 64 {
        bail!("实例 id 长度必须在 1..=64 之间");
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        bail!("实例 id 只允许 [A-Za-z0-9._-]");
    }
    if id.starts_with('.') || id.contains("..") {
        bail!("实例 id 不能以 . 开头，也不能包含 ..");
    }
    Ok(())
}

fn config_file_path(config_dir: &Path, id: &str, enabled: bool) -> PathBuf {
    let suffix = if enabled {
        CONFIG_ENABLED_SUFFIX
    } else {
        CONFIG_DISABLED_SUFFIX
    };
    config_dir.join(format!("{id}{suffix}"))
}

#[derive(Debug, Serialize)]
struct ConfigEntry {
    id: String,
    enabled: bool,
    bytes: u64,
    /// 最后修改时间（UNIX 秒）；拿不到时为 0。
    modified: u64,
    /// 配置正文。列表一次带回来，客户端不必再逐条 `config.read`
    /// （UI 每次轮询都要列表，N+1 次 IPC 会明显变慢）。
    text: String,
}

/// 列出配置目录里的网络（`*.toml` + `*.toml.disabled`）。
fn list_network_configs(config_dir: &Path) -> Vec<ConfigEntry> {
    let mut out: Vec<ConfigEntry> = Vec::new();
    let Ok(entries) = std::fs::read_dir(config_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // 隐藏文件（含我们自己的 `.xxx.toml.tmp`）一律不看。
        if name.starts_with('.') {
            continue;
        }
        let (id, enabled) = if let Some(id) = name.strip_suffix(CONFIG_DISABLED_SUFFIX) {
            (id.to_string(), false)
        } else if let Some(id) = name.strip_suffix(CONFIG_ENABLED_SUFFIX) {
            (id.to_string(), true)
        } else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let meta = entry.metadata().ok();
        let bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // 读不到正文（权限/竞态）就不列这一条：客户端拿到的条目一定能用。
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        out.push(ConfigEntry {
            id,
            enabled,
            bytes,
            modified,
            text,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// 最小校验：必须是 TOML 表，且带 `[network_identity] network_name`。
///
/// 不做完整字段校验 —— 那是内核 `TomlConfig` 的事；这里只拦住「写进去也起不来」
/// 的明显错误，避免把坏文件留在目录里让内核下次启动直接报错。
fn validate_network_toml(text: &str) -> anyhow::Result<()> {
    let doc: toml::Value = toml::from_str(text).context("配置不是合法 TOML")?;
    if !doc.is_table() {
        bail!("配置必须是 TOML 表（应包含 [network_identity] 等小节）");
    }
    let name = doc
        .get("network_identity")
        .and_then(|v| v.get("network_name"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if name.trim().is_empty() {
        bail!("缺少 [network_identity] network_name（内核无法加载该实例）");
    }
    Ok(())
}

/// 原子写：临时文件 → 校验 → rename，再删掉另一种（启用/禁用）形态。
fn write_network_config(
    config_dir: &Path,
    id: &str,
    text: &str,
    enabled: bool,
) -> anyhow::Result<u64> {
    validate_network_id(id)?;
    validate_network_toml(text)?;
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("创建配置目录失败: {}", config_dir.display()))?;

    // 临时文件用 `.` 前缀：内核与我们自己的 `list` 都不会把它当成实例。
    let tmp = config_dir.join(format!(".{id}.toml.tmp"));
    std::fs::write(&tmp, text).with_context(|| format!("写入 {} 失败", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).ok();
    }
    let dst = config_file_path(config_dir, id, enabled);
    if let Err(err) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("替换 {} 失败", dst.display()));
    }
    // 同一个 id 只能有一种形态，否则会出现「启用 + 禁用」两份。
    let _ = std::fs::remove_file(config_file_path(config_dir, id, !enabled));
    Ok(text.len() as u64)
}

/// 读配置：返回 (正文, 是否启用)。
fn read_network_config(config_dir: &Path, id: &str) -> anyhow::Result<(String, bool)> {
    validate_network_id(id)?;
    let enabled_path = config_file_path(config_dir, id, true);
    if enabled_path.is_file() {
        let text = std::fs::read_to_string(&enabled_path)
            .with_context(|| format!("读取 {} 失败", enabled_path.display()))?;
        return Ok((text, true));
    }
    let disabled_path = config_file_path(config_dir, id, false);
    if disabled_path.is_file() {
        let text = std::fs::read_to_string(&disabled_path)
            .with_context(|| format!("读取 {} 失败", disabled_path.display()))?;
        return Ok((text, false));
    }
    bail!("配置不存在: {id}");
}

/// 删除配置（两种形态都删），返回是否真的删掉了东西。
fn delete_network_config(config_dir: &Path, id: &str) -> anyhow::Result<bool> {
    validate_network_id(id)?;
    let mut removed = false;
    for enabled in [true, false] {
        let path = config_file_path(config_dir, id, enabled);
        if path.is_file() {
            std::fs::remove_file(&path).with_context(|| format!("删除 {} 失败", path.display()))?;
            removed = true;
        }
    }
    Ok(removed)
}

/// 扫 `<config-dir>/*.toml`，收集每个网络实例的虚拟网段。
///
/// 内核就是从该目录加载 `*.toml`（`exvlan/src/core.rs`），所以这里是权威来源。
/// 返回 (网段列表, 提示信息)。
fn configured_networks(config_dir: &Path) -> (Vec<String>, Vec<String>) {
    let mut cidrs: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(config_dir) else {
        return (cidrs, notes);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let Ok(text) = std::fs::read_to_string(&path) else {
            notes.push(format!("{name}: 无法读取，已跳过"));
            continue;
        };
        // 注意：`str::parse::<toml::Value>()` 解析的是「值」而不是文档，
        // 必须用 `toml::from_str`（否则每个网络配置都会被当成解析失败，
        // 白名单退化成只有回环 → 远程管理静默失效）。
        let Ok(value) = toml::from_str::<toml::Value>(&text) else {
            notes.push(format!("{name}: TOML 解析失败，已跳过"));
            continue;
        };
        let mut found = false;
        for key in ["ipv4", "ipv6"] {
            let Some(raw) = value.get(key).and_then(|v| v.as_str()) else {
                continue;
            };
            match cidr_network(raw) {
                Some(cidr) => {
                    if !cidrs.contains(&cidr) {
                        cidrs.push(cidr);
                    }
                    found = true;
                }
                None => notes.push(format!("{name}: {key}={raw} 无法解析")),
            }
        }
        if !found {
            // DHCP 或未配置静态地址：无法知道实际网段，回退到默认池。
            if !cidrs.contains(&DHCP_FALLBACK_POOL.to_string()) {
                cidrs.push(DHCP_FALLBACK_POOL.to_string());
            }
            notes.push(format!(
                "{name}: 无静态地址（DHCP），使用默认池 {DHCP_FALLBACK_POOL}"
            ));
        }
    }
    (cidrs, notes)
}

/// 取本机在某个虚拟网络里的地址（`ipv4 = "10.254.1.6/27"` → `10.254.1.6`）。
///
/// gateway 默认绑这里：绑回环只有本机能用，绑 mesh 地址才能被远端接入。
fn configured_self_ip(config_dir: &Path) -> Option<String> {
    let mut fallback: Option<String> = None;
    let entries = std::fs::read_dir(config_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        for key in ["ipv4", "ipv6"] {
            let Some(raw) = value.get(key).and_then(|v| v.as_str()) else {
                continue;
            };
            let host = raw.split('/').next().unwrap_or(raw).trim();
            if host.is_empty() || host.parse::<std::net::IpAddr>().is_err() {
                continue;
            }
            // 优先私有网段（EasyTier 默认池就是 10.126/10.254 这类）。
            if host.starts_with("10.")
                || host.starts_with("192.168.")
                || host.starts_with("172.")
                || host.starts_with("fd")
            {
                return Some(host.to_string());
            }
            if fallback.is_none() {
                fallback = Some(host.to_string());
            }
        }
    }
    fallback
}

/// 与本机 gateway 做一次 JSON 请求（HTTP/1.1，`Connection: close`）。
///
/// 服务自身只需要这一个调用（生成配对码），不值得为此引入 HTTP 客户端依赖。
fn http_json(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    use std::io::{Read as _, Write as _};
    let mut stream = std::net::TcpStream::connect(addr)
        .with_context(|| format!("连接 {addr} 失败（gateway 在跑吗？）"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(token) = token {
        req.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(body) = body {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    if let Some(body) = body {
        req.push_str(body);
    }
    stream.write_all(req.as_bytes()).context("发送请求失败")?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).context("读取响应失败")?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .context("无法解析响应状态")?;
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    let value: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!({ "raw": body }));
    if !(200..300).contains(&status) {
        bail!("gateway 返回 {status}: {value}");
    }
    Ok(value)
}

/// gateway 的 spawn 串行化（watchdog + IPC 可能同时触发）。
static GATEWAY_SPAWN_LOCK: Mutex<()> = Mutex::new(());

/// 统一把 `port_in_use|{json}` 的错误拆成结构化响应；其它错误保持原 code。
fn conflict_or(err: anyhow::Error, fallback_code: &str) -> Response {
    let msg = err.to_string();
    if let Some(rest) = msg.strip_prefix("port_in_use|")
        && let Ok(payload) = serde_json::from_str::<serde_json::Value>(rest)
    {
        return Response::conflict(payload);
    }
    Response::err(fallback_code, msg)
}

// ---------------------------------------------------------------------------
// 端口占用诊断（RPC 端口被残留内核占住时，现场要能看出是谁）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, serde::Serialize)]
struct PortOwner {
    pid: u32,
    name: String,
    /// 可执行文件完整路径（可能为空：拿不到权限时）。
    path: String,
}

/// 端口占用者的性质（决定 GUI 给什么指引）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortKind {
    /// 我们自己的内核（残留进程 / 另一份安装 / WSL 里的实例）
    Exvlan,
    /// 端口转发器（VS Code Remote / WSL relay / SSH 隧道）
    Forwarded,
    /// 其它程序
    Foreign,
}

impl PortKind {
    fn as_str(self) -> &'static str {
        match self {
            PortKind::Exvlan => "exvlan",
            PortKind::Forwarded => "forwarded",
            PortKind::Foreign => "foreign",
        }
    }
    fn hint(self) -> &'static str {
        match self {
            PortKind::Exvlan => {
                "占用者是另一个 Exvlan 内核（残留进程 / 另一份安装 / WSL 里的实例）。                 结束它，或在 service.toml 里改 rpc_portal"
            }
            PortKind::Forwarded => {
                "疑似编辑器/SSH 的端口转发（如 VS Code Remote 的 remote.autoForwardPorts）。                 在 VS Code 的 Ports 面板停止转发 1995，或加 \"remote.portsAttributes\" 忽略该端口"
            }
            PortKind::Foreign => {
                "该端口被其它程序占用。可结束该程序，或在 service.toml 里把 rpc_portal 改成 127.0.0.1:1996"
            }
        }
    }
}

/// 判定端口占用者性质：先看是不是我们自己的内核产物，再看是不是已知的转发器。
fn classify_port_owner(owner: &PortOwner, kernel: &Path) -> PortKind {
    if owner.is_stale_kernel_of(kernel) {
        return PortKind::Exvlan;
    }
    let name = owner.name.to_ascii_lowercase();
    let path = owner.path.to_ascii_lowercase();
    let forwarded = [
        "code.exe",
        "code",
        "wslrelay.exe",
        "ssh.exe",
        "ssh",
        "vscode-server",
    ]
    .iter()
    .any(|n| name == *n);
    let forwarded = forwarded
        || ["\\.vscode\\", "microsoft vs code", "wslrelay", "openssh"]
            .iter()
            .any(|frag| path.contains(frag));
    if forwarded {
        PortKind::Forwarded
    } else {
        PortKind::Foreign
    }
}

impl PortOwner {
    /// 内核目录下、名字属于 `{exvlan[.exe], exvlan[.exe].prev}` 的进程。
    ///
    /// 换内核时旧二进制会被改名成 `.prev`，但**进程仍在跑**并占着 RPC 端口；
    /// 只按「路径完全等于当前内核」判断会把它当外人，永远清不掉。
    fn is_stale_kernel_of(&self, kernel: &Path) -> bool {
        if self.path.is_empty() {
            return false;
        }
        let p = Path::new(&self.path);
        let (Some(dir), Some(base)) =
            (kernel.parent(), kernel.file_name().and_then(|s| s.to_str()))
        else {
            return false;
        };
        let same_dir = if cfg!(windows) {
            p.parent()
                .map(|d| {
                    d.to_string_lossy()
                        .eq_ignore_ascii_case(&dir.to_string_lossy())
                })
                .unwrap_or(false)
        } else {
            p.parent() == Some(dir)
        };
        if !same_dir {
            return false;
        }
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let (name, base) = if cfg!(windows) {
            (name.to_ascii_lowercase(), base.to_ascii_lowercase())
        } else {
            (name.to_string(), base.to_string())
        };
        name == base || name == format!("{base}.prev")
    }

    /// 只清「我们自己的内核」：路径必须与 `<core>/exvlan(.exe)` 完全一致。
    /// 只按进程名判断不安全（可能误杀同名程序）。
    fn is_our_kernel(&self, kernel: &Path) -> bool {
        if self.path.is_empty() {
            return false;
        }
        let a = Path::new(&self.path);
        let b = kernel;
        // Windows 路径大小写不敏感
        if cfg!(windows) {
            a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy())
        } else {
            a == b
        }
    }
}

#[cfg(unix)]
fn port_owner(port: u16) -> Option<PortOwner> {
    let out = Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpc"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pid: Option<u32> = None;
    let mut name = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            pid = rest.trim().parse::<u32>().ok();
        } else if let Some(rest) = line.strip_prefix('c') {
            name = rest.trim().to_string();
        }
        if pid.is_some() && !name.is_empty() {
            break;
        }
    }
    let pid = pid?;
    let path = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .and_then(|line| line.split_whitespace().next().map(str::to_owned))
        .unwrap_or_default();
    Some(PortOwner { pid, name, path })
}

/// Windows：`OpenProcess` + `QueryFullProcessImageNameW` 取真实路径。
#[cfg(target_os = "windows")]
fn process_image_path(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

#[cfg(target_os = "windows")]
fn port_owner(port: u16) -> Option<PortOwner> {
    let out = hidden("netstat").args(["-ano"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let needle = format!(":{port}");
    let mut pid: Option<u32> = None;
    for line in text.lines() {
        if !line.contains("LISTENING") {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if !fields.iter().any(|f| f.ends_with(&needle)) {
            continue;
        }
        if let Some(last) = fields.last().and_then(|s| s.parse::<u32>().ok()) {
            pid = Some(last);
            break;
        }
    }
    let pid = pid?;
    let path = process_image_path(pid).unwrap_or_default();
    let name = Path::new(&path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    Some(PortOwner { pid, name, path })
}

#[cfg(not(any(unix, target_os = "windows")))]
fn port_owner(_port: u16) -> Option<PortOwner> {
    None
}

#[cfg(unix)]
fn terminate_pid(pid: u32) -> anyhow::Result<()> {
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    if rc != 0 {
        bail!("kill({pid}) 失败: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn terminate_pid(pid: u32) -> anyhow::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        bail!(
            "OpenProcess({pid}) 失败: {}",
            std::io::Error::last_os_error()
        );
    }
    let ok = unsafe { TerminateProcess(handle, 1) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        bail!(
            "TerminateProcess({pid}) 失败: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(not(any(unix, target_os = "windows")))]
fn terminate_pid(_pid: u32) -> anyhow::Result<()> {
    bail!("不支持终止进程")
}

/// 生效白名单 = 回环 + 各虚拟网段 + 用户额外追加。
///
/// **无锁纯函数**：调用方可以在任意状态（包括持锁）安全调用。
fn compute_whitelist(
    config_dir: &Path,
    extra: &[String],
    mesh_whitelist: bool,
) -> (String, Vec<String>) {
    let mut list: Vec<String> = RPC_LOOPBACK.iter().map(|s| s.to_string()).collect();
    let mut notes = Vec::new();
    if !mesh_whitelist {
        // 默认：RPC 只服务回环（远端走 gateway）。显式 open 时才把网段放进来。
        return (list.join(","), notes);
    }
    let (networks, more) = configured_networks(config_dir);
    notes.extend(more);
    for cidr in networks {
        if !list.contains(&cidr) {
            list.push(cidr);
        }
    }
    for raw in extra {
        match validate_cidr(raw) {
            Ok(cidr) => {
                if !list.contains(&cidr) {
                    list.push(cidr);
                }
            }
            Err(err) => notes.push(format!("额外白名单 {raw} 已忽略: {err}")),
        }
    }
    (list.join(","), notes)
}

impl State {
    fn new(paths: Paths) -> Self {
        let config = load_config(&paths);
        let mode = config.policy;
        Self {
            paths,
            inner: Mutex::new(Inner {
                applied_whitelist: None,
                starting: false,
                child: None,
                mode,
                config,
                sessions: 0,
                restarting: false,
                gateway_child: None,
                gateway_restarts: 0,
            }),
        }
    }

    /// 统一取锁（毒化也继续用）。
    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn log(&self, msg: impl AsRef<str>) {
        log_line(&self.paths, msg.as_ref());
    }

    fn kernel_pid(&self) -> u32 {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        take_pid(&mut guard)
    }

    /// 结构化白名单信息（GUI 的「设置 → 服务」展示用）。
    fn whitelist_info(&self) -> serde_json::Value {
        let (networks, mut notes) = configured_networks(&self.paths.config);
        let extra = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .config
            .rpc_extra_whitelist
            .clone();
        let mesh_whitelist = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .config
            .rpc_mesh_whitelist;
        let (effective, more_notes) = compute_whitelist(&self.paths.config, &extra, mesh_whitelist);
        notes.extend(more_notes);
        let applied = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .applied_whitelist
            .clone();
        // 配置改了但内核还没重启时，新网段暂时不生效（远程管理会连不上）。
        let needs_restart = applied.as_deref().is_some_and(|a| a != effective);
        serde_json::json!({
            "effective": effective,
            "applied": applied,
            "needs_restart": needs_restart,
            "loopback": RPC_LOOPBACK,
            "networks": networks,
            "extra": extra,
            "notes": notes,
            "dhcp_fallback": DHCP_FALLBACK_POOL,
            "max_extra": MAX_EXTRA_WHITELIST,
        })
    }

    /// 计算生效的 RPC 白名单：回环 + 各虚拟网段 + 用户额外追加。
    ///
    /// 返回 `(逗号分隔参数, 提示/告警)`，提示会写进 service.log 与诊断包。
    fn kernel_args(&self, whitelist: &str) -> Vec<String> {
        let portal = self.rpc_portal();
        vec![
            // 由服务托管的"守护进程"：实例数归零（配置重建 / 删除最后一个网络）
            // 时**必须继续存活**，否则内核会静默退出、RPC 端口消失。
            "--daemon".into(),
            "--config-dir".into(),
            self.paths.config.display().to_string(),
            "--rpc-portal".into(),
            portal,
            "--rpc-portal-whitelist".into(),
            whitelist.to_string(),
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

    /// 确保内核在跑，返回它的 pid。
    ///
    /// **锁纪律**：`inner` 只在「读/写 child 字段」时短暂持有；
    /// `spawn` 与最长 10s 的 `wait_rpc_ready` 一律在锁外完成。
    /// （pre.14 事故：早期实现在持锁状态下调用 `rpc_whitelist()`，
    /// 同一线程二次 lock → 永久阻塞，SCM 只看到 RUNNING 却永远没有内核。）
    fn ensure_kernel(&self) -> anyhow::Result<KernelInfo> {
        // 1) 快路径：已有活着的子进程
        // 2) 已有别的线程在启动：等它的结果，不要重复 spawn
        {
            let mut guard = self.lock_inner();
            let pid = take_pid(&mut guard);
            if pid != 0 {
                return Ok(KernelInfo {
                    pid,
                    running: true,
                    rpc: LOCAL_RPC.into(),
                });
            }
            if guard.starting {
                drop(guard);
                return self.await_kernel_start();
            }
            guard.starting = true;
        }

        let outcome = self.start_kernel();

        let mut guard = self.lock_inner();
        guard.starting = false;
        match outcome {
            Ok((child, whitelist)) => {
                let pid = child.id();
                guard.child = Some(child);
                guard.applied_whitelist = Some(whitelist);
                drop(guard);
                KERNEL_UP.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(KernelInfo {
                    pid,
                    running: true,
                    rpc: LOCAL_RPC.into(),
                })
            }
            Err(err) => Err(err),
        }
    }

    /// 另一个线程正在启动内核时的等待路径（全程不持锁）。
    fn await_kernel_start(&self) -> anyhow::Result<KernelInfo> {
        let deadline = Instant::now() + READY_TIMEOUT + Duration::from_secs(2);
        loop {
            {
                let mut guard = self.lock_inner();
                let pid = take_pid(&mut guard);
                if pid != 0 {
                    return Ok(KernelInfo {
                        pid,
                        running: true,
                        rpc: LOCAL_RPC.into(),
                    });
                }
                if !guard.starting {
                    // 对端已经结束但没有留下存活的内核 → 它失败了
                    bail!("内核启动失败（见 service.log）");
                }
            }
            if Instant::now() >= deadline {
                bail!("等待内核启动超时（{}s）", READY_TIMEOUT.as_secs());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// 真正的启动动作。**不持有 `inner` 锁**，只读一次配置快照。
    fn start_kernel(&self) -> anyhow::Result<(Child, String)> {
        let extra = self.lock_inner().config.rpc_extra_whitelist.clone();
        let mesh_whitelist = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .config
            .rpc_mesh_whitelist;
        let (whitelist, notes) = compute_whitelist(&self.paths.config, &extra, mesh_whitelist);
        self.log(format!("rpc whitelist = {whitelist}"));
        for note in notes {
            self.log(format!("rpc whitelist: {note}"));
        }

        if !self.paths.kernel.is_file() {
            // 没有内核也要先把白名单写进日志：CI 冒烟以此判断 boot 是否走完。
            bail!("内核不存在: {}", self.paths.kernel.display());
        }
        std::fs::create_dir_all(&self.paths.config).ok();
        std::fs::create_dir_all(&self.paths.logs).ok();

        let mut cmd = Command::new(&self.paths.kernel);
        cmd.args(self.kernel_args(&whitelist))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("启动内核失败: {}", self.paths.kernel.display()))?;
        let pid = child.id();
        self.log(format!("kernel spawned pid={pid}"));

        if let Err(err) = wait_rpc_ready(READY_TIMEOUT) {
            self.log(format!("kernel not ready: {err}"));
            let _ = child.kill();
            let _ = child.wait();
            bail!("kernel start failed: {err}");
        }
        // `wait_rpc_ready` 只看端口通不通：若 RPC 端口已被**残留**的旧内核占着，
        // 我们刚拉起的这个会因 bind 失败立刻退出，而探活仍会“成功”。
        if let Ok(Some(status)) = child.try_wait() {
            self.log(format!("kernel exited immediately: {status}"));
            bail!(
                "内核启动后立即退出（{status}）：很可能是 RPC 端口被残留进程占用，\
                 请结束遗留的 exvlan 进程后重试"
            );
        }
        // 端口通 ≠ 是我们的内核在应答：残留的旧内核同样能让探活“成功”。
        match port_owner(self.rpc_port()) {
            Some(owner) if owner.pid == pid => {}
            Some(owner) => {
                self.log(format!(
                    "rpc responder mismatch: want pid={pid}, got pid={} path={}",
                    owner.pid, owner.path
                ));
                let _ = child.kill();
                let _ = child.wait();
                return Err(self.port_conflict_error(owner));
            }
            None => {
                self.log("rpc responder missing after ready");
                let _ = child.kill();
                let _ = child.wait();
                bail!("探活成功但 RPC 端口无人监听");
            }
        }
        Ok((child, whitelist))
    }

    /// 若 RPC 端口被**我们自己的**内核产物（含 `.prev` 残留）占着，杀掉它；
    /// 被外来进程占着则明确报错（不误杀）。
    fn cleanup_stale_kernel(&self) -> anyhow::Result<()> {
        let Some(owner) = port_owner(self.rpc_port()) else {
            return Ok(());
        };
        if !owner.is_stale_kernel_of(&self.paths.kernel) {
            self.log(format!(
                "rpc port {} occupied by foreign process: {} pid={} path={}",
                self.rpc_port(),
                owner.name,
                owner.pid,
                owner.path
            ));
            return Err(self.port_conflict_error(owner));
        }
        self.log(format!(
            "cleanup stale kernel pid={} path={}",
            owner.pid, owner.path
        ));
        terminate_pid(owner.pid)?;
        for _ in 0..50 {
            if port_owner(self.rpc_port()).is_none() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!("清理残留内核后 RPC 端口仍被占用")
    }

    /// 原子重启内核：停 → 清残留 → 起。期间挂起「normal 模式会话归零停机」。
    fn restart_kernel(&self) -> anyhow::Result<KernelInfo> {
        self.lock_inner().restarting = true;
        let outcome = (|| {
            self.stop_kernel()?;
            self.cleanup_stale_kernel()?;
            self.ensure_kernel()
        })();
        self.lock_inner().restarting = false;
        self.log(match &outcome {
            Ok(info) => format!("kernel.restart ok pid={}", info.pid),
            Err(err) => format!("kernel.restart failed: {err}"),
        });
        outcome
    }

    fn stop_kernel(&self) -> anyhow::Result<()> {
        // 先把 child 摘出来，等待进程退出时**不持锁**（否则 status 会被卡住）。
        let Some(mut child) = self.lock_inner().child.take() else {
            return Ok(());
        };
        KERNEL_UP.store(false, std::sync::atomic::Ordering::SeqCst);
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
            guard.config.policy = mode;
            let cfg = guard.config.clone();
            // 持久化在锁外做，避免阻塞其它请求。
            drop(guard);
            save_config(&self.paths, &cfg)?;
        }
        self.log(format!("mode set to {mode:?}"));
        match mode {
            Mode::Off => {
                self.stop_kernel()?;
                // 服务完全不干活时把 gateway 也收掉（否则远端还能看到它）。
                self.stop_gateway();
            }
            // service ↔ normal：**不重启内核**，只改变“断开是否停”的策略。
            Mode::Service | Mode::Normal => {
                if self.kernel_pid() == 0 {
                    self.ensure_kernel()?;
                }
                self.ensure_gateway()?;
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

        // 先停掉正在跑的内核（不能只依赖 Child 句柄：服务自更新后句柄会丢，
        // 旧进程会被改名成 `.prev` 却继续占着 RPC 端口）。
        let was_running = self.kernel_pid() != 0;
        if was_running {
            self.stop_kernel()?;
        }
        self.cleanup_stale_kernel()?;

        // 保留上一版，失败可回滚。
        //
        // Windows 上刚停掉的内核可能还握着 exe 句柄（杀进程 → 句柄释放是异步的），
        // 直接 rename 会失败（实测 error 32）。这里轮询重试；**失败就不动原内核**，
        // 把原因结构化返回（GUI 提示「关掉所有 Exvlan 后重试」），而不是把机器留在半截。
        if self.paths.kernel.is_file() {
            let _ = std::fs::remove_file(&self.paths.kernel_prev);
            rename_with_retry(&self.paths.kernel, &self.paths.kernel_prev, 10)
                .context("kernel_busy: 备份当前内核失败（内核可能仍在退出，请稍后重试）")?;
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

        if was_running && let Err(err) = self.ensure_kernel() {
            self.log(format!("new kernel failed ({err}), rolling back"));
            let _ = std::fs::remove_file(&self.paths.kernel);
            if self.paths.kernel_prev.is_file() {
                std::fs::rename(&self.paths.kernel_prev, &self.paths.kernel).ok();
            }
            self.ensure_kernel().ok();
            return Err(anyhow::anyhow!("kernel_rollback: {err}"));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // gateway 托管（服务模式）：随服务启停 + 异常退避重启
    // -----------------------------------------------------------------------

    fn gateway_config(&self) -> GatewayConfig {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .config
            .gateway
            .clone()
    }

    /// 监听地址：`[gateway].bind` 优先；为空时自动取本机 mesh 虚拟地址
    /// （拿不到就退回回环 —— 宁可本机可用，也不要绑到 0.0.0.0）。
    fn gateway_bind(&self) -> String {
        let cfg = self.gateway_config();
        match cfg.bind.as_deref().map(str::trim) {
            Some(bind) if !bind.is_empty() => bind.to_string(),
            _ => configured_self_ip(&self.paths.config).unwrap_or_else(|| "127.0.0.1".to_string()),
        }
    }

    fn gateway_addr(&self) -> String {
        format!("{}:{}", self.gateway_bind(), self.gateway_config().port)
    }

    /// gateway 的静态 token 文件（服务创建、0600；远端 CLI 读同一份）。
    fn gateway_token_file(&self) -> PathBuf {
        self.paths
            .service_config
            .parent()
            .map(|dir| dir.join("gateway-token"))
            .unwrap_or_else(|| self.paths.service_config.with_file_name("gateway-token"))
    }

    /// 拉起 gateway（幂等）。二进制缺失时只记日志：内核仍可用，重装会带回。
    fn ensure_gateway(&self) -> anyhow::Result<u32> {
        // watchdog 与 IPC 可能同时触发，串行化避免双开（端口冲突）。
        let _serial = GATEWAY_SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pid) = self.gateway_pid() {
            return Ok(pid);
        }
        let cfg = self.gateway_config();
        if !cfg.enabled {
            return Ok(0);
        }
        let bin = &self.paths.gateway_bin;
        if !bin.is_file() {
            self.log(format!("gateway 未安装（{}），跳过", bin.display()));
            return Ok(0);
        }
        let addr = self.gateway_addr();
        let token_file = self.gateway_token_file();

        #[cfg(target_os = "windows")]
        let mut cmd = hidden(bin);
        #[cfg(not(target_os = "windows"))]
        let mut cmd = Command::new(bin);

        cmd.env("EXVLAN_GATEWAY_BIND", self.gateway_bind())
            .env("EXVLAN_GATEWAY_PORT", cfg.port.to_string())
            .env("EXVLAN_GATEWAY_TOKEN_FILE", &token_file)
            // 父进程存亡靠 stdin EOF 传播：服务没了 → gateway 自行退出，
            // 不会留下孤儿进程占着端口（比信号处理跨平台可靠得多）。
            .env("EXVLAN_GATEWAY_PARENT_STDIN", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        // gateway 自己的日志单独一份，排查「远端连不上」时不用翻 service.log。
        let log_path = self.paths.logs.join("gateway.log");
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            && let Ok(clone) = file.try_clone()
        {
            cmd.stdout(Stdio::from(clone));
            cmd.stderr(Stdio::from(file));
        }

        let child = cmd.spawn().context("拉起 gateway 失败")?;
        let pid = child.id();
        self.lock_inner().gateway_child = Some(child);
        self.log(format!("gateway started pid={pid} addr={addr}"));
        Ok(pid)
    }

    fn gateway_pid(&self) -> Option<u32> {
        let mut guard = self.lock_inner();
        let child = guard.gateway_child.as_mut()?;
        match child.try_wait() {
            Ok(None) => Some(child.id()),
            _ => {
                // 已退出：顺手回收，避免僵尸。
                guard.gateway_child = None;
                None
            }
        }
    }

    fn stop_gateway(&self) {
        let mut guard = self.lock_inner();
        if let Some(mut child) = guard.gateway_child.take() {
            let pid = child.id();
            let _ = child.kill();
            let _ = child.wait();
            drop(guard);
            self.log(format!("gateway stopped pid={pid}"));
        }
    }

    fn restart_gateway(&self) -> anyhow::Result<u32> {
        self.stop_gateway();
        let pid = self.ensure_gateway()?;
        self.lock_inner().gateway_restarts = 0;
        Ok(pid)
    }

    /// 本机生成一次性配对码（`POST /api/v1/pair/start`）。
    ///
    /// 走本机 gateway：回环直连（无 token）；若 gateway 绑的是 mesh 地址，就带本机
    /// admin token —— token 是服务自己创建的 0600 文件，用户态读不到，天然是「本机」。
    fn pair_start(&self, role: &str, ttl: u64) -> anyhow::Result<serde_json::Value> {
        let cfg = self.gateway_config();
        if !cfg.enabled {
            bail!("gateway 已在 service.toml 里关闭，无法生成配对码");
        }
        if self.gateway_pid().is_none() {
            self.ensure_gateway()?;
        }
        let addr = match self.gateway_bind().as_str() {
            // 通配地址：连回环（本机一定有）。
            "0.0.0.0" => format!("127.0.0.1:{}", cfg.port),
            "::" => format!("[::1]:{}", cfg.port),
            other if other.contains(':') => format!("[{other}]:{}", cfg.port),
            other => format!("{other}:{}", cfg.port),
        };
        let token = std::fs::read_to_string(self.gateway_token_file())
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        let body = serde_json::json!({ "role": role, "ttl": ttl }).to_string();
        let value = http_json(
            &addr,
            "POST",
            "/api/v1/pair/start",
            Some(&body),
            token.as_deref(),
        )?;
        self.log(format!("pair code issued role={role} ttl={ttl}"));
        Ok(value)
    }

    fn gateway_info(&self) -> GatewayInfo {
        let enabled = self.gateway_config().enabled;
        let pid = self.gateway_pid().unwrap_or(0);
        GatewayInfo {
            pid,
            running: pid != 0,
            addr: self.gateway_addr(),
            enabled,
        }
    }

    /// gateway 监督：每 5s 探一次，异常退出按 1→2→5→10s 指数退避重启。
    /// 稳定运行一轮后把退避清零（否则偶发崩溃会一路涨到 10s 上限）。
    fn spawn_gateway_watchdog(self: &Arc<Self>) {
        // 模式切换会重复调用；只允许起一个监督线程。
        static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let mut backoff = Duration::from_secs(1);
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let cfg = me.gateway_config();
                if !cfg.enabled || me.lock_inner().mode == Mode::Off {
                    continue;
                }
                if me.gateway_pid().is_some() {
                    if me.lock_inner().gateway_restarts != 0 {
                        me.lock_inner().gateway_restarts = 0;
                        backoff = Duration::from_secs(1);
                    }
                    continue;
                }
                std::thread::sleep(backoff);
                match me.ensure_gateway() {
                    Ok(pid) if pid != 0 => {
                        let n = {
                            let mut guard = me.lock_inner();
                            guard.gateway_restarts = guard.gateway_restarts.saturating_add(1);
                            guard.gateway_restarts
                        };
                        me.log(format!("gateway restarted pid={pid} (n={n})"));
                        backoff = match n {
                            1 => Duration::from_secs(1),
                            2 => Duration::from_secs(2),
                            3 => Duration::from_secs(5),
                            _ => Duration::from_secs(10),
                        };
                    }
                    Ok(_) => {}
                    Err(err) => me.log(format!("gateway restart failed: {err}")),
                }
            }
        });
    }

    /// 端口冲突 → 结构化错误（`port_in_use|{json}`），由 `handle()` 拆成 Response。
    fn port_conflict_error(&self, owner: PortOwner) -> anyhow::Error {
        let kind = classify_port_owner(&owner, &self.paths.kernel);
        let payload = serde_json::json!({
            "port": self.rpc_port(),
            "occupant": {
                "pid": owner.pid,
                "name": owner.name,
                "path": owner.path,
            },
            "kind": kind.as_str(),
            "hint": kind.hint(),
        });
        anyhow::anyhow!("port_in_use|{payload}")
    }

    /// 生效的 RPC portal 绑定地址（`service.toml` 的 `rpc_portal` 优先）。
    fn rpc_portal(&self) -> String {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .config
            .rpc_portal
            .clone()
            .unwrap_or_else(|| RPC_PORTAL_DEFAULT.to_string())
    }

    /// 生效的 RPC 端口（用于端口占用/清理；`rpc_portal` 可改端口）。
    fn rpc_port(&self) -> u16 {
        self.rpc_portal()
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok())
            .unwrap_or(RPC_PORT)
    }

    fn status(&self) -> Response {
        let pid = self.kernel_pid();
        let mode = self.inner.lock().unwrap_or_else(|e| e.into_inner()).mode;
        let mut resp = Response::ok();
        resp.mode = Some(mode);
        resp.kernel = Some(KernelInfo {
            pid,
            running: pid != 0,
            rpc: LOCAL_RPC.into(),
        });
        resp.gateway = Some(self.gateway_info());
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
                Err(err) => conflict_or(err, "kernel_start_failed"),
            },
            "kernel.stop" => match self.stop_kernel() {
                Ok(()) => self.status(),
                Err(err) => Response::err("internal", err.to_string()),
            },
            "kernel.restart" => match self.restart_kernel() {
                Ok(info) => {
                    let mut r = self.status();
                    r.kernel = Some(info);
                    r
                }
                Err(err) => conflict_or(err, "kernel_restart_failed"),
            },
            "apply" => match req.staging.as_deref() {
                Some(dir) => match self.apply(Path::new(dir)) {
                    Ok(()) => self.status(),
                    Err(err) => {
                        let msg = err.to_string();
                        if msg.starts_with("port_in_use|") {
                            conflict_or(err, "apply_failed")
                        } else if msg.contains("kernel_rollback") {
                            Response::err("kernel_rollback", msg)
                        } else {
                            Response::err("apply_failed", msg)
                        }
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
            "whitelist.get" => {
                let mut r = Response::ok();
                r.data = Some(self.whitelist_info());
                r
            }
            "whitelist.set" => {
                let raw = req.extra.unwrap_or_default();
                if raw.len() > MAX_EXTRA_WHITELIST {
                    return Response::err(
                        "too_many",
                        format!("额外网段最多 {MAX_EXTRA_WHITELIST} 条"),
                    );
                }
                let mut cleaned = Vec::new();
                for item in &raw {
                    match validate_cidr(item) {
                        Ok(cidr) => {
                            if !cleaned.contains(&cidr) {
                                cleaned.push(cidr);
                            }
                        }
                        Err(err) => return Response::err("bad_cidr", err),
                    }
                }
                {
                    let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    guard.config.rpc_extra_whitelist = cleaned;
                    let cfg = guard.config.clone();
                    drop(guard);
                    if let Err(err) = save_config(&self.paths, &cfg) {
                        return Response::err("save_failed", err.to_string());
                    }
                }
                self.log("rpc_extra_whitelist updated (restart required)");
                let mut r = Response::ok();
                r.data = Some(self.whitelist_info());
                r
            }
            // -- gateway 托管 --------------------------------------------------
            "gateway.status" => {
                let mut r = Response::ok();
                r.gateway = Some(self.gateway_info());
                r
            }
            "gateway.set" => {
                let Some(patch) = req.gateway else {
                    return Response::err("bad_request", "gateway 缺失");
                };
                if let Some(port) = patch.port
                    && port == 0
                {
                    return Response::err("bad_port", "端口不能为 0");
                }
                if let Some(bind) = patch.bind.as_deref() {
                    let trimmed = bind.trim();
                    if !trimmed.is_empty()
                        && trimmed != "localhost"
                        && trimmed.parse::<std::net::IpAddr>().is_err()
                    {
                        // 只接受 IP 字面量：主机名解析结果不可预期，监听地址必须确定。
                        return Response::err("bad_bind", format!("bind 需为 IP 地址: {trimmed}"));
                    }
                }
                {
                    let mut guard = self.lock_inner();
                    if let Some(enabled) = patch.enabled {
                        guard.config.gateway.enabled = enabled;
                    }
                    if let Some(bind) = patch.bind {
                        let trimmed = bind.trim();
                        guard.config.gateway.bind = if trimmed.is_empty() || trimmed == "auto" {
                            None
                        } else {
                            Some(trimmed.to_string())
                        };
                    }
                    if let Some(port) = patch.port {
                        guard.config.gateway.port = port;
                    }
                    let cfg = guard.config.clone();
                    drop(guard);
                    if let Err(err) = save_config(&self.paths, &cfg) {
                        return Response::err("save_failed", err.to_string());
                    }
                }
                match self.restart_gateway() {
                    Ok(pid) => {
                        self.log(format!("gateway 配置已更新 pid={pid}"));
                        let mut r = Response::ok();
                        r.gateway = Some(self.gateway_info());
                        r
                    }
                    Err(err) => Response::err("gateway_failed", err.to_string()),
                }
            }
            "gateway.pair.start" => {
                let role = req.role.clone().unwrap_or_else(|| "readonly".to_string());
                let ttl = req.ttl.unwrap_or(300).clamp(60, 900);
                match self.pair_start(&role, ttl) {
                    Ok(value) => {
                        let mut r = Response::ok();
                        r.data = Some(value);
                        r
                    }
                    Err(err) => Response::err("pair_failed", err.to_string()),
                }
            }
            "gateway.restart" => match self.restart_gateway() {
                Ok(pid) => {
                    let mut r = Response::ok();
                    r.data = Some(serde_json::json!({ "pid": pid, "addr": self.gateway_addr() }));
                    r.gateway = Some(self.gateway_info());
                    r
                }
                Err(err) => Response::err("gateway_failed", err.to_string()),
            },
            "config.list" => {
                let items = list_network_configs(&self.paths.config);
                let mut r = Response::ok();
                r.data = Some(serde_json::json!({
                    "dir": self.paths.config.display().to_string(),
                    "items": items,
                }));
                r
            }
            "config.read" => match req.id.as_deref() {
                None => Response::err("bad_request", "id 缺失"),
                Some(id) => match read_network_config(&self.paths.config, id) {
                    Ok((text, enabled)) => {
                        let mut r = Response::ok();
                        r.data = Some(serde_json::json!({
                            "id": id,
                            "enabled": enabled,
                            "text": text,
                        }));
                        r
                    }
                    Err(err) => Response::err("not_found", err.to_string()),
                },
            },
            "config.write" => match (req.id.as_deref(), req.text.as_deref()) {
                (Some(id), Some(text)) => {
                    let enabled = req.enabled.unwrap_or(true);
                    match write_network_config(&self.paths.config, id, text, enabled) {
                        Ok(bytes) => {
                            self.log(format!(
                                "config.write id={id} enabled={enabled} bytes={bytes}"
                            ));
                            let mut r = Response::ok();
                            r.data = Some(serde_json::json!({
                                "id": id,
                                "enabled": enabled,
                                "bytes": bytes,
                            }));
                            r
                        }
                        Err(err) => Response::err("write_failed", err.to_string()),
                    }
                }
                (None, _) => Response::err("bad_request", "id 缺失"),
                (_, None) => Response::err("bad_request", "text 缺失"),
            },
            "config.delete" => match req.id.as_deref() {
                None => Response::err("bad_request", "id 缺失"),
                Some(id) => match delete_network_config(&self.paths.config, id) {
                    Ok(true) => {
                        self.log(format!("config.delete id={id}"));
                        Response::ok()
                    }
                    Ok(false) => Response::err("not_found", format!("配置不存在: {id}")),
                    Err(err) => Response::err("delete_failed", err.to_string()),
                },
            },
            "port.owner" => {
                let mut r = Response::ok();
                r.data = Some(match port_owner(self.rpc_port()) {
                    Some(owner) => serde_json::json!({
                        "port": self.rpc_port(),
                        "pid": owner.pid,
                        "name": owner.name,
                        "path": owner.path,
                        "is_our_kernel": owner.is_our_kernel(&self.paths.kernel),
                    }),
                    None => serde_json::json!({ "port": self.rpc_port(), "free": true }),
                });
                r
            }
            "port.cleanup" => match port_owner(self.rpc_port()) {
                None => Response::err("nothing", "RPC 端口当前没有被占用"),
                Some(owner) if !owner.is_stale_kernel_of(&self.paths.kernel) => Response::err(
                    "not_ours",
                    format!(
                        "占用者是 {}（pid={}, path={}），不是本产品的内核，拒绝终止",
                        owner.name, owner.pid, owner.path
                    ),
                ),
                Some(owner) => match terminate_pid(owner.pid) {
                    Ok(()) => {
                        self.log(format!(
                            "port cleanup: killed stale kernel pid={}",
                            owner.pid
                        ));
                        Response::ok()
                    }
                    Err(err) => Response::err("kill_failed", err.to_string()),
                },
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

        // 关键：GUI 在收到 IPC 响应后会立刻删除 staging 目录，而 updater 是
        // 异步的（等主进程退出）。所以必须**同步**把新二进制搬到服务自己的
        // 目录，updater 只用这个稳定路径。（pre.14 实测：staging 被删后
        // updater 报 os error 3，然后每次启动都重试，形成死循环。）
        let staged = {
            let mut raw = self.paths.service_bin.as_os_str().to_os_string();
            raw.push(".new");
            PathBuf::from(raw)
        };
        std::fs::copy(&new_bin, &staged).context("暂存新服务二进制失败")?;

        // 随包工具（cli/gateway）一并更新：否则升级完 `core/` 里还是旧版 gateway，
        // 「配对 / 写操作 / 服务托管」这些新能力等于装了但没生效。
        let tools_src = staging.join("tools");
        let tools_staged = self.paths.core.join("tools.new");
        let mut tools_arg: Option<PathBuf> = None;
        if tools_src.is_dir() {
            let _ = std::fs::remove_dir_all(&tools_staged);
            std::fs::create_dir_all(&tools_staged).context("暂存工具目录失败")?;
            for entry in std::fs::read_dir(&tools_src)
                .context("读取随包工具目录失败")?
                .flatten()
            {
                let src = entry.path();
                if !src.is_file() {
                    continue;
                }
                let dst = tools_staged.join(entry.file_name());
                std::fs::copy(&src, &dst)
                    .with_context(|| format!("暂存 {} 失败", src.display()))?;
            }
            tools_arg = Some(tools_staged);
        }

        let exe = std::env::current_exe().context("定位自身可执行文件失败")?;
        let mut cmd = Command::new(&exe);
        cmd.arg("--replace-updater").arg(&staged);
        if let Some(ref tools) = tools_arg {
            cmd.arg(tools);
        }
        cmd.stdin(Stdio::null())
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
    //
    // 「关」的一半原本就有（最后一个会话断开 → 停内核），「开」的一半缺失：
    // 客户端连上来时没人拉内核，于是关掉界面再打开就永远连不上（issue #58/#60）。
    fn session_open(self: &Arc<Self>) {
        let should_start = {
            let mut guard = self.lock_inner();
            guard.sessions += 1;
            guard.sessions == 1
                && guard.mode != Mode::Off
                && !guard.restarting
                && guard.child.is_none()
        };
        if !should_start {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            // 锁外执行（ensure_kernel 会等 RPC 就绪），且**不阻塞**会话握手。
            if let Err(err) = me.ensure_kernel() {
                me.log(format!("session_open: ensure_kernel failed: {err}"));
            }
        });
    }

    fn session_close(self: &Arc<Self>) {
        let stop = {
            let mut guard = self.lock_inner();
            guard.sessions = guard.sessions.saturating_sub(1);
            guard.sessions == 0
                && guard.mode == Mode::Normal
                && !guard.restarting
                && guard.child.is_some()
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
            &LOCAL_RPC.parse().unwrap(),
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
fn chown_to(path: &Path, uid: u32) {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    // gid 保持 0（wheel）：属主已是目标用户，组权限无实际作用。
    unsafe {
        libc::chown(c.as_ptr(), uid, 0);
    }
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
    let op = args.first().cloned().unwrap_or_default();
    let result = run_cli(&p, &args);
    // Windows 上是 GUI 子系统：stderr 没有去处，PowerShell 也不会等我们退出。
    // 所以 `--install` 之类失败原本是**完全静默**的，现场只能靠猜。
    // 这里把子命令结果写进 service.log（运维/诊断都看得到）。
    if !op.is_empty() && op != "--daemon" && op != "--version" {
        match &result {
            Ok(()) => log_line(&p, &format!("cli {op} ok")),
            Err(err) => log_line(&p, &format!("cli {op} failed: {err:#}")),
        }
    }
    result
}

fn run_cli(p: &Paths, args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("exvlan-service {BUILD_VERSION}");
            Ok(())
        }
        Some("--daemon") => run_daemon(p.clone()),
        Some("--install") => {
            let uid = args.get(1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
            // 第 3 个参数是 Windows 安装者 SID（macOS/Linux 忽略）。
            let sid = args.get(2).map(String::as_str).unwrap_or("");
            install(p, uid, sid)?;
            // 控制面工具（cli/gateway）随安装包分发，但只有服务有权限把它们
            // 放到系统路径（GUI 跑在用户态）。缺工具不阻断安装：日志里说明。
            if let Some(dir) = flag_value(args, "--tools-dir") {
                match install_tools(p, Path::new(&dir)) {
                    Ok(()) => log_line(p, &format!("tools installed from {dir}")),
                    Err(err) => log_line(p, &format!("tools install skipped: {err:#}")),
                }
            }
            Ok(())
        }
        Some("--uninstall") => uninstall(p),
        Some("--status") => {
            println!("{}", serde_json::to_string_pretty(&load_config(p))?);
            Ok(())
        }
        Some("--diagnose") => diagnose(p),
        // 提权入口：重启服务本体（不改配置、不换二进制）。
        Some("--restart") => restart_service(),
        // 自更新落点：由运行中的服务 fork 出来，等它退出后替换二进制并拉起。
        Some("--replace-updater") => replace_updater(
            p,
            args.get(1).map(String::as_str),
            args.get(2).map(String::as_str),
        ),
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
    spawn_boot_watchdog(p.clone());
    boot(&state);
    serve_unix(state)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn run_daemon(_p: Paths) -> anyhow::Result<()> {
    bail!("exvlan-service 只在 macOS/Windows 上运行")
}

/// 启动看门狗：`boot()` 卡住时主动退出，让 launchd/SCM 重启服务。
///
/// 背景：pre.14 的第一次实现里 `ensure_kernel` 持锁调用了会再次取锁的函数，
/// 结果服务进程活着、SCM 显示 RUNNING、但内核永远起不来，日志里也看不出
/// 任何异常 —— 这种状态用户无法自救。看门狗把「静默卡死」变成「重启」。
fn spawn_boot_watchdog(paths: Paths) {
    std::thread::spawn(move || {
        let deadline = Instant::now() + BOOT_WATCHDOG;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
            if BOOT_DONE.load(std::sync::atomic::Ordering::SeqCst) {
                return; // boot 走完了（成功或失败都算，失败已在日志里）
            }
        }
        log_line(
            &paths,
            &format!(
                "boot watchdog: boot 在 {}s 内未完成（疑似卡死；内核已起={}），主动退出以便重启",
                BOOT_WATCHDOG.as_secs(),
                KERNEL_UP.load(std::sync::atomic::Ordering::SeqCst)
            ),
        );
        std::process::exit(70);
    });
}

/// 服务启动时按持久化 mode 决定是否拉起内核（macOS / Windows 共用）。
fn boot(state: &Arc<State>) {
    // 每次启动都写一遍版本文件：替换二进制成功后（新进程启动时）版本立刻反映，
    // 不必等下一次 install/update；也顺带修复「版本文件丢失」的情况。
    let _ = write_version_file(&state.paths);

    #[cfg(target_os = "macos")]
    {
        // 历史脏数据：早期版本以 root 跑过 WebView，`~/Library/WebKit/era.exvlan.studio`
        // 变成 root 属主 → WKWebView 建不出 LocalStorage 的 sqlite，
        // 前端 localStorage 变成黑洞（远程目标列表/语言/上次网络全部失效）。
        let owner = state.lock_inner().config.owner_uid;
        fix_webkit_owner(state, owner);
    }

    let mode = state.lock_inner().mode;
    match mode {
        Mode::Service => {
            if let Err(err) = state.ensure_kernel() {
                state.log(format!("boot ensure_kernel failed: {err}"));
            }
        }
        Mode::Normal | Mode::Off => state.log(format!("boot mode={mode:?}: 不自动启动内核")),
    }
    // gateway 由服务托管：service/normal 都要在（normal 模式下 GUI 关了也照样能远端管理）；
    // Off 表示「服务不干活」，不拉。
    if mode != Mode::Off {
        match state.ensure_gateway() {
            Ok(0) => {}
            Ok(pid) => state.log(format!("boot: gateway pid={pid}")),
            Err(err) => state.log(format!("boot ensure_gateway failed: {err}")),
        }
        state.spawn_gateway_watchdog();
    }
    // 无论成功失败，boot 到这里就算走完：看门狗据此区分「卡住」与「失败」。
    BOOT_DONE.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// macOS：把 `~/Library/WebKit[/era.exvlan.studio]` 的属主修回用户。
///
/// 只在「当前属主是 root」时动手（幂等）；失败只记日志，不影响启动。
#[cfg(target_os = "macos")]
fn fix_webkit_owner(state: &State, owner_uid: u32) {
    use std::os::unix::fs::MetadataExt as _;

    if owner_uid == 0 {
        return;
    }
    let Some(home) = home_dir_for_uid(owner_uid) else {
        state.log(format!(
            "webkit chown skipped: 无法解析 uid={owner_uid} 的家目录"
        ));
        return;
    };
    let base = home.join("Library").join("WebKit");
    let app = base.join(APP_ID);
    let mut fixed = 0usize;
    for path in [&base, &app] {
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        if meta.uid() != 0 {
            continue;
        }
        match chown_recursive(path, owner_uid) {
            Ok(n) => {
                fixed += n;
                state.log(format!(
                    "webkit chown: {} → uid={owner_uid} ({n} entries)",
                    path.display()
                ));
            }
            Err(err) => state.log(format!("webkit chown failed: {}: {err}", path.display())),
        }
    }
    if fixed > 0 {
        state.log("webkit chown done（localStorage 应恢复可用）");
    }
}

/// `getpwuid_r` → 家目录。
#[cfg(target_os = "macos")]
fn home_dir_for_uid(uid: u32) -> Option<std::path::PathBuf> {
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0i8; 4096];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwuid_r(
            uid as libc::uid_t,
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    let dir = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) };
    Some(std::path::PathBuf::from(dir.to_string_lossy().into_owned()))
}

/// 递归 chown（不跟随符号链接），返回处理的条目数。
#[cfg(target_os = "macos")]
fn chown_recursive(root: &std::path::Path, uid: u32) -> std::io::Result<usize> {
    use std::os::unix::ffi::OsStrExt as _;

    fn chown(path: &std::path::Path, uid: u32) -> std::io::Result<()> {
        use std::os::unix::ffi::OsStrExt as _;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::other("path contains NUL"))?;
        // gid 用 -1 表示不修改
        let rc = unsafe { libc::lchown(c.as_ptr(), uid as libc::uid_t, -1i32 as libc::gid_t) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    let _ = root.as_os_str().as_bytes();
    let mut count = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        chown(&path, uid)?;
        count += 1;
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(entry.path());
            } else {
                chown(&entry.path(), uid)?;
                count += 1;
            }
        }
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// 平台：macOS / Linux（unix socket + peercred）
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn socket_path() -> PathBuf {
    Path::new("/var/run").join(APP_ID).join("service.sock")
}

#[cfg(unix)]
fn serve_unix(state: Arc<State>) -> anyhow::Result<()> {
    use std::os::unix::net::UnixListener;

    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
        // launchd 下 umask 常为 077 → `create_dir_all` 会建成 0700 root，
        // 普通用户连目录都穿越不进去（同样是 connect EACCES）。
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
        }
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).context("bind unix socket 失败")?;

    // socket 属主必须是**调用方**（GUI 用户）：pre.13 用的是 0660 + root:wheel，
    // 普通用户不在 wheel 组里 → connect 直接 EACCES（服务 running、socket 在，
    // 但 GUI 永远「未就绪」）。属主来源三级：service.json → .owner 文件 → 收紧。
    let owner = {
        let from_json = state
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .config
            .owner_uid;
        if from_json != 0 {
            from_json
        } else {
            read_owner_file(&state.paths).unwrap_or(0)
        }
    };
    {
        use std::os::unix::fs::PermissionsExt as _;
        if owner != 0 {
            chown_to(&path, owner);
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        } else {
            // 拿不到属主时**不放宽**：0o666 会让任意本地用户都能连（peercred 在
            // expected==0 时也会放行）→ 本地可反复停内核。宁可让 GUI 显示横幅。
            chown_root(&path);
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            log_line(
                &state.paths,
                "socket owner 未知（service.json 与 .owner 都不可用），已收紧为 0600 root",
            );
        }
    }
    state.log(format!(
        "listening on {} (owner_uid={owner})",
        path.display()
    ));

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if !peer_allowed(&stream, &state) {
            state.log("rejected: peer uid mismatch");
            continue;
        }
        let state = state.clone();
        std::thread::spawn(move || handle_unix_conn(&state, stream));
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
fn handle_unix_conn(state: &Arc<State>, stream: std::os::unix::net::UnixStream) {
    use std::io::{BufRead, BufReader, Write};
    // 只有**发过 hello** 的连接才算「一个会话」。
    //
    // 之前是「连上就算」：`exvlan-cli doctor` / 一次 `config list` 这种短连接在
    // session 策略下会把会话数从 1 抖到 0，顺手把内核停掉（CLI 一查就断网）。
    let mut is_session = false;
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
        if req.op == "hello" && !is_session {
            is_session = true;
            state.session_open();
        }
        let resp = state.handle(req);
        let _ = writeln!(
            writer,
            "{}",
            serde_json::to_string(&resp).unwrap_or_default()
        );
    }
    let _ = last;
    if is_session {
        state.session_close();
    }
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
        spawn_boot_watchdog(p.clone());
        boot(&state);
        if let Err(err) = pipe_loop(state.clone()) {
            state.log(format!("pipe loop exited: {err}"));
        }

        // 退出前把内核一并收掉（否则会留下孤儿内核占着 RPC 端口和 TUN）。
        if let Err(err) = state.stop_kernel() {
            state.log(format!("stop kernel on exit failed: {err}"));
        }
        state.stop_gateway();
        let _ = status_handle.set_service_status(status(ServiceState::Stopped));
        state.log("windows service stopped");
    }
}

/// 命名管道的安全描述符：`D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)`。
///
/// 失败时返回 `None`，退回系统默认 ACL（并记日志），保证服务仍能用。
/// 组装命名管道的 SDDL。
///
/// - `SY` = LocalSystem 全权、`BA` = Administrators 全权；
/// - 最后一项是「客户端」：有安装者 SID 就用它（**只有本人**），否则退回
///   `IU`（交互式登录用户）—— 与 pre.17 之前一致，宁可宽一点也不锁门。
///
/// 只接受形如 `S-1-5-21-...` 的 SID：这段字符串会被拼进 SDDL，随手塞
/// `WD)(A;;GA;;;WD` 这类内容就能把权限放开，所以必须先过白名单。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn pipe_sddl(owner_sid: &str) -> String {
    let sane = owner_sid.starts_with("S-1-")
        && owner_sid.len() <= 184
        && owner_sid
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S');
    if sane {
        format!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;{owner_sid})")
    } else {
        "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)".to_string()
    }
}

#[cfg(target_os = "windows")]
fn pipe_security_attributes() -> Option<windows_sys::Win32::Security::SECURITY_ATTRIBUTES> {
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    const SDDL_REVISION_1: u32 = 1;

    let owner_sid = load_config(&paths()).owner_sid;
    let sddl_text = pipe_sddl(&owner_sid);
    let make = |text: &str| -> PSECURITY_DESCRIPTOR {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { std::ptr::null_mut() } else { sd }
    };

    let mut sd = make(&sddl_text);
    if sd.is_null() {
        // SID 写法有问题（或解析失败）：退回 IU，绝不留一个默认 ACL 的管道。
        log_line(
            &paths(),
            &format!("pipe SDDL 构造失败（owner_sid={owner_sid:?}），退回 IU"),
        );
        sd = make("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)");
    }
    if sd.is_null() {
        log_line(&paths(), "pipe SDDL 构造失败，退回默认 ACL");
        return None;
    }
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

    /// 预建的管道实例数。
    ///
    /// **必须 > 1**：GUI 启动后会常驻一条会话连接（`ensure_session`），
    /// 之前每次只建 1 个实例、接完再建下一个 → 那条会话把唯一实例占死，
    /// 之后所有客户端（`exvlan-cli`、gateway、doctor、GUI 的 apply/mode.set、
    /// config.write）全部 `ERROR_PIPE_BUSY (os error 231)`。
    /// `PIPE_UNLIMITED_INSTANCES` 只是上限，**不会**自动扩容。
    const PIPE_INSTANCES: usize = 16;

    let name: Vec<u16> = "\\\\.\\pipe\\era.exvlan.studio.service\0"
        .encode_utf16()
        .collect();
    // 显式 DACL：不依赖 LocalSystem 的默认 ACL（默认值随 Windows 版本/策略变化）。
    //   SY = LocalSystem GA、BA = Administrators GA、装者 SID 或 IU = 客户端 GRGW
    let sa = pipe_security_attributes();
    let sa_ptr = match &sa {
        Some(s) => s as *const windows_sys::Win32::Security::SECURITY_ATTRIBUTES,
        None => std::ptr::null(),
    };

    let mut workers = 0usize;
    for _ in 0..PIPE_INSTANCES {
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                // `PIPE_REJECT_REMOTE_CLIENTS`：只服务本机客户端，拒绝 SMB 远端。
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                sa_ptr,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            log_line(
                &state.paths,
                &format!(
                    "CreateNamedPipe 第 {} 个实例失败（继续用已建好的 {} 个）：os error {}",
                    workers + 1,
                    workers,
                    unsafe { GetLastError() }
                ),
            );
            break;
        }
        let st = state.clone();
        let pipe = SendHandle(handle);
        std::thread::spawn(move || {
            // 必须走方法调用：直接读 `pipe.0` 会被闭包按字段精确捕获，
            // 从而绕过 SendHandle 的 Send 实现。
            pipe_instance_loop(&st, pipe.into_inner());
        });
        workers += 1;
    }
    if workers == 0 {
        anyhow::bail!("没有任何管道实例创建成功");
    }
    state.log(format!("pipe listening: {workers} instances"));

    // 阻塞到服务停止（SCM Stop/Shutdown 会把 RUNNING 拉低并自连唤醒）。
    while win::RUNNING.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
    }
    // 每个实例都阻塞在 ConnectNamedPipe，自连 N 次把它们逐个唤醒。
    for _ in 0..workers {
        let _ = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(r"\\.\pipe\era.exvlan.studio.service");
    }
    std::thread::sleep(Duration::from_millis(300));
    Ok(())
}

/// 单个管道实例的循环：等连接 → 处理 → 断开 → 再等。
///
/// 句柄在整个服务生命周期内复用（不反复 Create/Close），实例数恒定 = 预建数。
#[cfg(target_os = "windows")]
fn pipe_instance_loop(state: &Arc<State>, handle: windows_sys::Win32::Foundation::HANDLE) {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::System::Pipes::*;

    loop {
        let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if connected {
            handle_pipe_conn(state, handle);
            unsafe {
                DisconnectNamedPipe(handle);
            }
        }
        if !win::RUNNING.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        if !connected {
            // 管道被关闭/出错：该实例退休（其余实例继续服务）。
            break;
        }
    }
    unsafe {
        CloseHandle(handle);
    }
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
fn handle_pipe_conn(state: &Arc<State>, handle: windows_sys::Win32::Foundation::HANDLE) {
    use windows_sys::Win32::Storage::FileSystem::*;

    let mut buf = [0u8; 4096];
    let mut pending = String::new();
    // 与 unix 一致：只有发过 hello 的连接才算会话（短命令不会停内核）。
    let mut is_session = false;
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
            if req.op == "hello" && !is_session {
                is_session = true;
                state.session_open();
            }
            let resp = state.handle(req);
            write_pipe(handle, &resp);
        }
        let _ = read;
    }
    if is_session {
        state.session_close();
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

/// 取 `--flag <value>` 的值（也支持 `--flag=value`）。
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        return args.get(pos + 1).cloned();
    }
    args.iter()
        .find_map(|a| a.strip_prefix(&format!("{flag}=")).map(str::to_string))
}

/// 把随包的 `exvlan-cli` / `exvlan-gateway` 安装到系统路径。
///
/// 目的：agent / 用户不依赖 app 包路径就能用控制面（`exvlan-cli doctor` 等）。
/// 由服务（root）执行，因为目标目录在系统路径下。
#[allow(dead_code)]
fn install_tools(p: &Paths, dir: &Path) -> anyhow::Result<()> {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    let mut copied = Vec::new();
    for (name, dst) in [
        ("exvlan-cli", &p.cli_bin),
        ("exvlan-gateway", &p.gateway_bin),
    ] {
        let src = dir.join(format!("{name}{exe}"));
        if !src.is_file() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        // 先写临时文件再 rename：gateway 可能正在运行（macOS 上覆写运行中的可执行
        // 文件会 ETXTBSY），rename 是原子替换，运行中的旧进程继续用旧 inode。
        let tmp = dst.with_extension("new");
        std::fs::copy(&src, &tmp)
            .with_context(|| format!("拷贝 {} → {} 失败", src.display(), tmp.display()))?;
        std::fs::rename(&tmp, dst).with_context(|| format!("替换 {} 失败", dst.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o755));
            // macOS 的 `fs::copy` 会保留源文件属主（app 包里的工具属于安装者），
            // 但系统路径里的可执行文件必须是 root —— 否则普通用户可替换它。
            chown_root(dst);
        }
        copied.push(dst.display().to_string());
    }
    if copied.is_empty() {
        bail!("{} 里没有 exvlan-cli / exvlan-gateway", dir.display());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn install(p: &Paths, uid: u32, sid: &str) -> anyhow::Result<()> {
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

    // 旧模型（内核直跑 launchd / 旧 helper）与新版抢同一端口与 TUN，必须先清掉。
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
  <key>StandardErrorPath</key><string>{errlog}</string>
</dict>
</plist>
"#,
        bin = p.service_bin.display(),
        errlog = p.service_log.with_file_name("service.stderr.log").display()
    );
    std::fs::write(plist, body)?;
    std::fs::set_permissions(plist, std::fs::Permissions::from_mode(0o644))?;
    chown_root(plist);

    let _ = write_version_file(p);
    let _ = write_owner_file(p, uid);

    let cfg = ServiceConfig {
        owner_uid: uid,
        policy: load_config(p).policy,
        // 重装时保留用户手工追加的网段，不要静默清空。
        rpc_extra_whitelist: load_config(p).rpc_extra_whitelist,
        // macOS 不用 SID（socket 0600 + peercred），传了也保留原值。
        owner_sid: if sid.is_empty() {
            load_config(p).owner_sid
        } else {
            sid.to_string()
        },
        // 重装保留用户显式配置的绑定地址与白名单开关（默认回环/关闭）。
        rpc_portal: load_config(p).rpc_portal,
        rpc_mesh_whitelist: load_config(p).rpc_mesh_whitelist,
        gateway: load_config(p).gateway,
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
    // 旧 helper 的运行时 socket（pre.12 遗留，权限还是 0666）
    let _ = std::fs::remove_file(Path::new("/var/run").join(APP_ID).join("helper.sock"));
    // 旧 helper 二进制
    for stale in [
        "/Library/PrivilegedHelperTools/era.exvlan.studio.helper",
        "/Library/PrivilegedHelperTools/com.exvlan.studio.helper",
    ] {
        let _ = std::fs::remove_file(stale);
    }
}

#[cfg(target_os = "windows")]
fn install(p: &Paths, _uid: u32, sid: &str) -> anyhow::Result<()> {
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
    // 管道 DACL 要收窄到「安装者本人」，所以必须在安装时把 SID 记下来：
    // 服务以 LocalSystem 运行，事后无法推断是哪个用户点的安装。
    let cfg = ServiceConfig {
        owner_uid: 0,
        policy: load_config(p).policy,
        rpc_extra_whitelist: load_config(p).rpc_extra_whitelist,
        owner_sid: if sid.is_empty() {
            load_config(p).owner_sid
        } else {
            sid.to_string()
        },
        // 重装保留用户显式配置的绑定地址与白名单开关。
        rpc_portal: load_config(p).rpc_portal,
        rpc_mesh_whitelist: load_config(p).rpc_mesh_whitelist,
        gateway: load_config(p).gateway,
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
fn replace_updater(p: &Paths, new_exe: Option<&str>, tools: Option<&str>) -> anyhow::Result<()> {
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
    // 不在这里写 `service.version`：本进程是**旧**二进制，写进去的是旧版本；
    // 新服务启动时 `boot()` 会写自己的版本。

    // 2.5) 随包工具（服务已停，gateway 子进程也随之退出 → 文件不再被占用）
    if let Some(dir) = tools {
        match install_tools(p, Path::new(dir)) {
            Ok(()) => {}
            Err(err) => log_line(p, &format!("tools update failed: {err:#}")),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

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
fn replace_updater(p: &Paths, new_exe: Option<&str>, tools: Option<&str>) -> anyhow::Result<()> {
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
    // 同上：版本文件由新服务启动时写入，避免这里写成旧版本。

    // 随包工具（cli/gateway）：旧进程还在跑也没关系（rename 原子替换 + 新进程用新 inode）。
    if let Some(dir) = tools {
        match install_tools(p, Path::new(dir)) {
            Ok(()) => {}
            Err(err) => log_line(p, &format!("tools update failed: {err:#}")),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

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
fn replace_updater(_p: &Paths, _new_exe: Option<&str>, _tools: Option<&str>) -> anyhow::Result<()> {
    bail!("exvlan-service 只在 macOS/Windows 上运行")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn install(_p: &Paths, _uid: u32, _sid: &str) -> anyhow::Result<()> {
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
    // `service.json` 是 0600 root：普通用户跑 `--diagnose` 读不到它。
    // 明确报出来，而不是用 `unwrap_or_default()` 打印一份假的 {owner_uid:0}。
    match std::fs::read_to_string(&p.service_config) {
        Ok(text) => out.push_str(&format!("config: {}\n", text.trim())),
        Err(err) => out.push_str(&format!(
            "config: <unreadable: {err}> path={}\n",
            p.service_config.display()
        )),
    }
    out.push_str(&format!("policy(boot): {:?}\n", load_config(p).policy));
    let boot_sid = load_config(p).owner_sid;
    out.push_str(&format!(
        "owner_sid(boot): {}\n",
        if boot_sid.is_empty() {
            "(unknown → pipe ACL 退回 IU)"
        } else {
            boot_sid.as_str()
        }
    ));
    #[allow(unused_mut)]
    let mut probes: Vec<(&str, &Path)> = vec![
        ("service.bin", &p.service_bin),
        ("kernel", &p.kernel),
        ("kernel.prev", &p.kernel_prev),
        ("service.toml", &p.service_config),
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

#[cfg(test)]
mod whitelist_tests {
    use super::{cidr_network, compute_whitelist, configured_self_ip, validate_cidr};
    use std::path::Path;

    /// 端到端：从 `<config-dir>/*.toml` 推导出网段。
    /// 用真实临时目录，覆盖「用户改了网段」这个 pre.13 会静默失效的场景。
    #[test]
    fn derives_networks_from_config_dir() {
        let dir = std::env::temp_dir().join(format!("exvlan-wl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("net1.toml"), "ipv4 = \"10.254.1.22/27\"\n").unwrap();
        std::fs::write(dir.join("net2.toml"), "ipv4 = \"192.168.50.7/24\"\n").unwrap();
        // DHCP 实例：没有静态地址 → 回退默认池
        std::fs::write(dir.join("net3.toml"), "dhcp = true\n").unwrap();
        // 坏文件不应导致 panic
        std::fs::write(dir.join("broken.toml"), "this is not = toml [[[").unwrap();
        std::fs::write(dir.join("ignored.txt"), "ipv4 = \"10.9.9.9/24\"\n").unwrap();

        let (effective, notes) = compute_whitelist(&dir, &[], true);
        let list: Vec<&str> = effective.split(',').collect();
        assert!(list.contains(&"127.0.0.0/8"), "{effective}");
        assert!(list.contains(&"::1/128"), "{effective}");
        assert!(list.contains(&"10.254.1.0/27"), "{effective}");
        assert!(list.contains(&"192.168.50.0/24"), "{effective}");
        assert!(
            list.contains(&"10.254.0.0/16"),
            "DHCP 回退池缺失: {effective}"
        );
        assert!(notes.iter().any(|n| n.contains("DHCP")), "{notes:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_whitelist_is_validated_not_trusted() {
        let dir = Path::new("/nonexistent-exvlan-config");
        let extra = vec![
            "172.20.0.0/16".to_string(),
            "0.0.0.0/0".to_string(),  // 必须被拒绝
            "8.8.8.0/24".to_string(), // 公网，必须被拒绝
        ];
        let (effective, notes) = compute_whitelist(dir, &extra, true);
        assert!(effective.contains("172.20.0.0/16"), "{effective}");
        assert!(!effective.contains("0.0.0.0/0"), "{effective}");
        assert!(!effective.contains("8.8.8.0/24"), "{effective}");
        assert_eq!(notes.iter().filter(|n| n.contains("已忽略")).count(), 2);
    }

    #[test]
    fn gateway_defaults_are_enabled_on_2016() {
        let cfg: ServiceConfig = toml::from_str("policy = \"service\"\n").expect("parse");
        assert!(cfg.gateway.enabled, "默认应托管 gateway");
        assert_eq!(cfg.gateway.port, 2016);
        assert!(cfg.gateway.bind.is_none(), "默认自动取 mesh 地址");
    }

    #[test]
    fn gateway_config_reads_section() {
        let text = "policy = \"service\"\n\n[gateway]\nenabled = false\nbind = \"0.0.0.0\"\nport = 12016\n";
        let cfg: ServiceConfig = toml::from_str(text).expect("parse");
        assert!(!cfg.gateway.enabled);
        assert_eq!(cfg.gateway.bind.as_deref(), Some("0.0.0.0"));
        assert_eq!(cfg.gateway.port, 12016);
    }

    #[test]
    fn configured_self_ip_reads_mesh_address() {
        let dir = temp_config_dir("gw-ip");
        std::fs::write(dir.join("a.toml"), "ipv4 = \"10.254.1.6/27\"\n").unwrap();
        assert_eq!(configured_self_ip(&dir).as_deref(), Some("10.254.1.6"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn configured_self_ip_ignores_dhcp_and_broken() {
        let dir = temp_config_dir("gw-ip-bad");
        std::fs::write(dir.join("a.toml"), "dhcp = true\n").unwrap();
        std::fs::write(dir.join("b.toml"), "not = toml [[[").unwrap();
        assert_eq!(configured_self_ip(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rpc_whitelist_defaults_to_loopback_only() {
        let dir = temp_config_dir("wl-off");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("net.toml"), "ipv4 = \"10.254.1.22/27\"\n").unwrap();

        let (effective, notes) = compute_whitelist(&dir, &["172.20.0.0/16".to_string()], false);
        assert!(effective.contains("127.0.0.0/8"), "{effective}");
        assert!(
            !effective.contains("10.254.1.0/27"),
            "默认不应包含 mesh 网段: {effective}"
        );
        assert!(
            !effective.contains("172.20.0.0/16"),
            "默认不应包含额外网段: {effective}"
        );
        assert!(notes.is_empty(), "{notes:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn masks_host_bits() {
        assert_eq!(
            cidr_network("10.254.1.22/27").as_deref(),
            Some("10.254.1.0/27")
        );
        assert_eq!(
            cidr_network("192.168.5.7/24").as_deref(),
            Some("192.168.5.0/24")
        );
        assert_eq!(cidr_network("10.0.0.1/8").as_deref(), Some("10.0.0.0/8"));
        assert_eq!(cidr_network("fd00::5/64").as_deref(), Some("fd00::/64"));
        assert_eq!(cidr_network("bogus/24"), None);
        assert_eq!(cidr_network("10.1.1.1/33"), None);
    }

    #[test]
    fn accepts_private_and_loopback() {
        for cidr in [
            "127.0.0.0/8",
            "::1/128",
            "10.254.1.0/27",
            "172.20.0.0/16",
            "192.168.1.0/24",
            "100.64.0.0/16",
            "169.254.0.0/16",
            "fd00::/64",
            "fe80::/10",
        ] {
            assert!(validate_cidr(cidr).is_ok(), "{cidr} 应被接受");
        }
    }

    #[test]
    fn rejects_public_and_default_routes() {
        for cidr in [
            "0.0.0.0/0",
            "::/0",
            "8.8.8.0/24",
            "2001:4860::/32",
            "1.1.1.1/32",
        ] {
            assert!(validate_cidr(cidr).is_err(), "{cidr} 必须被拒绝");
        }
    }

    use super::{Mode, ServiceConfig};

    #[test]
    fn pipe_sddl_narrows_to_owner_sid_only_when_valid() {
        let sid = "S-1-5-21-1111111111-2222222222-3333333333-1001";
        let sddl = crate::pipe_sddl(sid);
        assert!(sddl.contains(&format!(";;;{sid})")), "{sddl}");
        assert!(sddl.contains(";;;SY)") && sddl.contains(";;;BA)"), "{sddl}");
        // 空/未知 → 退回 IU（不能因此把用户锁在门外）
        assert!(crate::pipe_sddl("").contains(";;;IU)"));
        // 想借 SID 字段塞 ACE 的值一律不认
        for evil in [
            "S-1-5-21-1-2-3-1001)(A;;GA;;;WD",
            "WD)(A;;GA;;;WD",
            "S-1-5-18",
        ] {
            let out = crate::pipe_sddl(evil);
            if evil == "S-1-5-18" {
                // 合法 SID（SYSTEM）：允许，但不会引入新 ACE
                assert!(out.contains(";;;S-1-5-18)"), "{out}");
            } else {
                assert!(out.contains(";;;IU)"), "{evil} 必须被拒绝: {out}");
                assert!(!out.contains(";;;WD"), "{out}");
            }
        }
    }

    #[test]
    fn service_config_toml_accepts_new_and_legacy_names() {
        let cfg: ServiceConfig = toml::from_str(
            "owner_uid = 501\npolicy = \"session\"\nrpc_extra_whitelist = [\"10.254.66.0/24\"]\n",
        )
        .expect("parse new");
        assert_eq!(cfg.policy, Mode::Normal);
        assert_eq!(cfg.owner_uid, 501);

        // 旧 service.json 的取值（Service/Normal/Off）与旧字段名 `mode` 仍可读
        let legacy: ServiceConfig =
            toml::from_str("owner_uid = 7\nmode = \"Normal\"\n").expect("parse legacy");
        assert_eq!(legacy.policy, Mode::Normal);

        // 写回用规范词汇（resident/session/stopped）
        let text = toml::to_string_pretty(&cfg).expect("dump");
        assert!(text.contains("policy = \"session\""), "{text}");
        assert!(!text.contains("mode ="), "{text}");
    }

    // ---- config.* 读写 ----

    fn temp_config_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-service-test-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    const VALID_CFG: &str = "hostname = \"n1\"\n[network_identity]\nnetwork_name = \"exvlan\"\nnetwork_secret = \"s\"\n";

    #[test]
    fn rejects_path_traversal_ids() {
        for bad in [
            "",
            "..",
            "../etc/passwd",
            "a/b",
            "/abs",
            ".hidden",
            "x..y",
            "a b",
            "网",
            &"a".repeat(65),
        ] {
            assert!(
                crate::validate_network_id(bad).is_err(),
                "{bad:?} 必须被拒绝"
            );
        }
        for good in [
            "1467ec85-9cf8-47bc-8c06-8dc320e9b9c5",
            "net_1",
            "a.b",
            "A-9",
        ] {
            assert!(crate::validate_network_id(good).is_ok(), "{good} 应被接受");
        }
    }

    #[test]
    fn config_write_read_delete_round_trip() {
        let dir = temp_config_dir("roundtrip");
        let id = "1467ec85-9cf8-47bc-8c06-8dc320e9b9c5";

        let bytes = crate::write_network_config(&dir, id, VALID_CFG, true).expect("write");
        assert_eq!(bytes, VALID_CFG.len() as u64);
        let (text, enabled) = crate::read_network_config(&dir, id).expect("read");
        assert_eq!(text, VALID_CFG);
        assert!(enabled);

        // 禁用：改成 `.toml.disabled`，且启用形态消失（不会有第二份状态）
        crate::write_network_config(&dir, id, VALID_CFG, false).expect("disable");
        assert!(!crate::config_file_path(&dir, id, true).exists());
        assert!(crate::config_file_path(&dir, id, false).exists());
        let (_, enabled) = crate::read_network_config(&dir, id).expect("read disabled");
        assert!(!enabled);

        let list = crate::list_network_configs(&dir);
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!(list[0].id, id);

        assert!(crate::delete_network_config(&dir, id).expect("delete"));
        assert!(!crate::delete_network_config(&dir, id).expect("delete again"));
        assert!(crate::list_network_configs(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_write_rejects_bad_toml_and_leaves_nothing_behind() {
        let dir = temp_config_dir("badtoml");
        let id = "net1";

        assert!(crate::write_network_config(&dir, id, "not = [valid", true).is_err());
        assert!(
            crate::write_network_config(&dir, id, "hostname = \"n\"\n", true).is_err(),
            "缺 network_identity 必须被拒绝"
        );
        assert!(crate::list_network_configs(&dir).is_empty());
        assert!(!dir.join(format!("{id}.toml")).exists());
        // 不留临时文件
        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(leftovers.is_empty(), "临时文件没清干净: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_ignores_tmp_and_foreign_files() {
        let dir = temp_config_dir("list");
        std::fs::write(dir.join("a.toml"), VALID_CFG).expect("write a");
        std::fs::write(dir.join("b.toml.disabled"), VALID_CFG).expect("write b");
        std::fs::write(dir.join(".c.toml.tmp"), VALID_CFG).expect("write tmp");
        std::fs::write(dir.join("networks.json"), "{}").expect("write json");
        let list = crate::list_network_configs(&dir);
        assert_eq!(list.len(), 2, "{list:?}");
        assert_eq!(list[0].id, "a");
        assert!(list[0].enabled);
        assert_eq!(list[1].id, "b");
        assert!(!list[1].enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn classifies_port_owner_kinds() {
        let kernel = Path::new("/Library/Application Support/era.exvlan.studio/core/exvlan");
        let owner = |name: &str, path: &str| crate::PortOwner {
            pid: 42,
            name: name.into(),
            path: path.into(),
        };

        // 我们自己的内核（含 .prev 残留）
        assert_eq!(
            crate::classify_port_owner(&owner("exvlan", kernel.to_str().unwrap()), kernel),
            crate::PortKind::Exvlan
        );
        // VS Code 的端口转发（本次 pre.18 真实事故）
        assert_eq!(
            crate::classify_port_owner(
                &owner("Code.exe", r"C:\Program Files\Microsoft VS Code\Code.exe"),
                kernel
            ),
            crate::PortKind::Forwarded
        );
        // 其它程序
        assert_eq!(
            crate::classify_port_owner(&owner("nginx", "/usr/sbin/nginx"), kernel),
            crate::PortKind::Foreign
        );
        // kind 的展示串稳定（GUI/日志都依赖）
        assert_eq!(crate::PortKind::Forwarded.as_str(), "forwarded");
        assert!(!crate::PortKind::Forwarded.hint().is_empty());
    }
}
