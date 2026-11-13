//! 路由与鉴权。
//!
//! 端点（`/api/v1`）：
//! - `GET  /system`              版本 / backend / supervisor / 路径 / capabilities
//! - `GET  /doctor`              体检 findings（与 `exvlan-cli doctor` 同一份）
//! - `POST /doctor/fix`          执行安全修复
//! - `GET  /diag`                诊断摘要（JSON）
//! - `GET  /service`             服务状态（IPC `status`）
//! - `POST /service/policy`      resident|session|stopped
//! - `POST /service/desired`     running|stopped
//! - `POST /service/kernel/restart`  原子重启内核
//! - `GET  /events`              WebSocket：周期性推送服务/体检摘要
//! - `/configs`                  已保存的网络配置（内核 `<config-dir>/<id>.toml`，唯一权威）
//! - `/instances`                内核里**正在跑**的实例（RPC）
//!
//! 「已保存」与「正在跑」是两件事：前者是配置目录里的文件，后者是内核里的实例。
//! 远端客户端要「保存并生效」就是先 `PUT /configs/{id}` 再 `POST /instances`。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::{
        ConnectInfo, Path as AxumPath, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

use exvlan_control::protocol::{Payload, Request};
use exvlan_core::config::api_input::NetworkConfigExt as _;

use crate::Config;

/// 展示版本：CI 注入的构建串（`<标准版本>[-pre.N][+sha]`），本地构建回落 crate 版本。
const BUILD_VERSION: &str = match option_env!("EXVLAN_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Clone)]
struct AppState {
    token: Arc<String>,
    /// 一次性配对码（**只能由对端本机生成**；兑换时由对端签发 token）。
    pair: Arc<Mutex<PairState>>,
}

/// 一个配对码：5 分钟有效、一次性、首次兑换绑定来源 IP。
#[derive(Clone)]
struct PairCode {
    role: String,
    expires_at: Instant,
    used: bool,
    bound_ip: Option<IpAddr>,
}

#[derive(Default)]
struct PairState {
    codes: HashMap<String, PairCode>,
    /// 每 IP 的兑换尝试（计数, 窗口起点）——防暴力猜码。
    attempts: HashMap<IpAddr, (u32, Instant)>,
}

/// 每个 IP 每分钟最多 5 次兑换尝试。
const PAIR_ATTEMPTS_PER_MIN: u32 = 5;
/// 配对码有效期。
const PAIR_CODE_TTL: Duration = Duration::from_secs(300);

/// 构建路由。
pub fn router(cfg: Config, token: String) -> Router {
    let _ = &cfg; // 预留：端点在后续版本会用到 bind/port（例如生成远端 URL）
    let state = AppState {
        token: Arc::new(token),
        pair: Arc::new(Mutex::new(PairState::default())),
    };
    Router::new()
        .route("/api/v1/system", get(system))
        .route("/api/v1/doctor", get(doctor))
        .route("/api/v1/doctor/fix", post(doctor_fix))
        .route("/api/v1/diag", get(diag))
        .route("/api/v1/service", get(service))
        .route("/api/v1/service/policy", post(set_policy))
        .route("/api/v1/service/desired", post(set_desired))
        .route("/api/v1/service/kernel/restart", post(kernel_restart))
        .route(
            "/api/v1/instances",
            get(list_instances).post(create_instance),
        )
        .route(
            "/api/v1/instances/{id}",
            get(get_instance)
                .patch(patch_instance)
                .delete(delete_instance),
        )
        .route("/api/v1/validate", post(validate_config))
        .route("/api/v1/configs", get(list_configs))
        .route(
            "/api/v1/configs/{id}",
            get(get_config).put(put_config).delete(delete_config),
        )
        .route("/api/v1/pair/start", post(pair_start))
        .route("/api/v1/pair/redeem", post(pair_redeem))
        .route("/api/v1/events", get(events))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// 鉴权
// ---------------------------------------------------------------------------

/// 取出 Bearer token。
fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?;
    let text = value.to_str().ok()?;
    let token = text.strip_prefix("Bearer ")?.trim();
    (!token.is_empty()).then(|| token.to_string())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 要求指定 scope（`None` = 只要求「有效 token」）。
///
/// 本地 admin token（`<user>/gateway-token`）等同全部 scope；
/// 其余 token 来自 `<user>/gateway-tokens.json`（sha256 存储、可过期/吊销）。
#[allow(clippy::result_large_err)]
fn require_scope(
    state: &AppState,
    headers: &HeaderMap,
    scope: Option<&str>,
) -> Result<exvlan_control::tokens::Claims, Response> {
    use exvlan_control::tokens::{ALL_SCOPES, Store, VerifyError};
    let token = bearer(headers).ok_or_else(unauthorized)?;
    if constant_time_eq(token.as_bytes(), state.token.as_bytes()) {
        return Ok(exvlan_control::tokens::Claims {
            id: "local-admin".into(),
            name: "local-admin".into(),
            scopes: ALL_SCOPES.iter().map(|s| s.to_string()).collect(),
        });
    }
    let mut store = Store::load(&Store::path());
    tracing::debug!(
        store = %Store::path().display(),
        tokens = store.tokens.len(),
        "verify scoped token"
    );
    let claims = store.verify(&token).map_err(|e| {
        tracing::debug!(error = ?e, "token verify failed");
        match e {
            VerifyError::Expired => err(StatusCode::UNAUTHORIZED, "token_expired", "token 已过期"),
            VerifyError::Revoked => {
                err(StatusCode::UNAUTHORIZED, "token_revoked", "token 已被吊销")
            }
            VerifyError::Unknown => unauthorized(),
        }
    })?;
    // 注意：**不在这里写回 store**。last_used 属于审计信息，但网关是并发读侧，
    // 与 `auth token *`（写侧）竞争会导致「半截文件 / 覆盖吊销」这类问题；
    // 单写者原则：store 只由 CLI 修改。
    if let Some(need) = scope
        && !claims.allows(need)
    {
        return Err(err(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!("token 缺少 scope: {need}"),
        ));
    }
    Ok(claims)
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"code": "unauthorized", "message": "缺少或错误的 Bearer token"})),
    )
        .into_response()
}

fn err(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({"code": code, "message": message.into()})),
    )
        .into_response()
}

// Response 较大，错误分支装箱（clippy: result_large_err）。
//
// 返回的是**协议负载**（`data`）：HTTP 侧直接把它当 `status` 对象用，
// 与 v1 的平铺状态形状最接近（只多一个 `kind` 标签）。
fn ipc(req: &Request) -> Result<Value, Box<Response>> {
    let resp = exvlan_control::ipc::call(req)
        .map_err(|e| Box::new(err(StatusCode::BAD_GATEWAY, "service_ipc", e)))?;
    match resp.data {
        Some(payload) => serde_json::to_value(&payload).map_err(|e| {
            Box::new(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "service_payload",
                format!("服务负载无法序列化: {e}"),
            ))
        }),
        None => Ok(json!({})),
    }
}

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------

async fn system(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:read")) {
        return resp;
    }
    let backend = exvlan_control::detect();
    let p = exvlan_control::paths::paths();
    let user = exvlan_control::paths::user_paths();
    let policy = if backend.has_service() {
        exvlan_control::ipc::call(&Request::Status)
            .ok()
            .and_then(|resp| match resp.data {
                Some(Payload::Status { mode, .. }) => Some(mode.as_str().to_string()),
                _ => None,
            })
    } else {
        None
    };
    Json(json!({
        "api_version": 1,
        "gateway": BUILD_VERSION,
        "backend": backend.as_str(),
        "supervisor": exvlan_control::backend::supervisor(),
        "policy": policy,
        "privileged": backend.has_service(),
        "endpoints": {
            "rpc": exvlan_control::local_rpc_addr(),
            "gateway": exvlan_control::local_gateway_addr(),
        },
        "paths": {
            "data": user.data.display().to_string(),
            "logs": user.logs.display().to_string(),
            "config": p.config.display().to_string(),
        },
        "capabilities": {
            "service": backend.has_service(),
            "instances": kernel_reachable().await,
            // 配置目录读写不依赖内核 RPC（读文件 / 服务 IPC 即可）。
            "configs": true,
            "doctor": true,
            "events": true,
        },
        "manifest": [
            "GET  /api/v1/system",
            "GET  /api/v1/doctor",
            "POST /api/v1/doctor/fix",
            "GET  /api/v1/diag",
            "GET  /api/v1/service",
            "POST /api/v1/service/policy",
            "POST /api/v1/service/desired",
            "POST /api/v1/service/kernel/restart",
            "GET  /api/v1/instances",
            "POST /api/v1/instances",
            "GET  /api/v1/instances/{id}",
            "PATCH /api/v1/instances/{id}",
            "DELETE /api/v1/instances/{id}",
            "POST /api/v1/validate",
            "GET  /api/v1/configs",
            "GET  /api/v1/configs/{id}",
            "PUT  /api/v1/configs/{id}",
            "DELETE /api/v1/configs/{id}",
            "POST /api/v1/pair/start",
            "POST /api/v1/pair/redeem",
            "GET  /api/v1/events",
        ],
    }))
    .into_response()
}

/// `POST /api/v1/pair/start`：**对端本机**生成一次性配对码。
///
/// 只允许来自回环（或本机 gateway 自己的调用），远端无法给自己发码。
/// body 可选 `{"role":"readonly|admin","ttl":300}`。
async fn pair_start(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: String,
) -> Response {
    // 出码必须发生在被管机器本机：
    // - 回环来源（`exvlan-cli auth pair` 直连、本机脚本）直接放行；
    // - 或带**本机管理 token**（`gateway:admin`）—— 服务托管时为 mesh 绑定地址，
    //   服务/本机 GUI 从 mesh IP 过来也在「本机」范畴；0600 token 天然隔离了远端。
    if !peer.ip().is_loopback()
        && let Err(resp) = require_scope(&state, &headers, Some("gateway:admin"))
    {
        tracing::warn!(peer = %peer.ip(), "pair/start rejected: 非回环且无 gateway:admin");
        return resp;
    }
    let req: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
    let role = req
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("readonly")
        .to_string();
    let Some(scopes) = exvlan_control::tokens::scopes_for_role(&role) else {
        return err(
            StatusCode::BAD_REQUEST,
            "bad_role",
            format!("未知角色 {role}（readonly|config|admin）"),
        );
    };
    let ttl = req
        .get("ttl")
        .and_then(Value::as_u64)
        .map(|s| Duration::from_secs(s.clamp(60, 900)))
        .unwrap_or(PAIR_CODE_TTL);

    // 6 位数字码；与现有未过期码冲突时重试。
    let mut guard = state.pair.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    guard.codes.retain(|_, c| !c.used && c.expires_at > now);
    let mut code = String::new();
    for _ in 0..32 {
        let candidate = format!("{:06}", uuid::Uuid::new_v4().as_u128() % 1_000_000);
        if !guard.codes.contains_key(&candidate) {
            code = candidate;
            break;
        }
    }
    if code.is_empty() {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "pair_code",
            "生成配对码失败",
        );
    }
    guard.codes.insert(
        code.clone(),
        PairCode {
            role: role.clone(),
            expires_at: now + ttl,
            used: false,
            bound_ip: None,
        },
    );
    Json(json!({
        "code": code,
        "role": role,
        "scopes": scopes,
        "expires_in": ttl.as_secs(),
    }))
    .into_response()
}

/// `POST /api/v1/pair/redeem`：控制端用码换 token（**token 由对端签发**）。
///
/// 一次性 + 5 分钟 + 首次兑换绑定来源 IP + 每 IP 限速。
async fn pair_redeem(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: String,
) -> Response {
    let req: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return err(
                StatusCode::BAD_REQUEST,
                "bad_request",
                format!("body 不是 JSON: {e}"),
            );
        }
    };
    let code = req
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let client = req
        .get("client")
        .and_then(Value::as_str)
        .unwrap_or("remote-client")
        .to_string();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return err(StatusCode::BAD_REQUEST, "bad_code", "配对码应为 6 位数字");
    }

    // 限速：每 IP 每分钟 5 次。
    {
        let mut guard = state.pair.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = guard.attempts.entry(peer.ip()).or_insert((0, now));
        if now.duration_since(entry.1) > Duration::from_secs(60) {
            *entry = (0, now);
        }
        entry.0 += 1;
        if entry.0 > PAIR_ATTEMPTS_PER_MIN {
            return err(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "尝试过于频繁，请稍后再试",
            );
        }
    }

    // 校验并消费配对码。
    let (role, scopes) = {
        let mut guard = state.pair.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = guard.codes.get_mut(&code) else {
            return err(StatusCode::NOT_FOUND, "bad_code", "配对码不存在或已失效");
        };
        if entry.used {
            return err(StatusCode::GONE, "code_used", "配对码已被使用");
        }
        if entry.expires_at <= Instant::now() {
            return err(StatusCode::GONE, "code_expired", "配对码已过期");
        }
        // 首次兑换绑定来源 IP；之后从别的 IP 用同一个码必须失败。
        match entry.bound_ip {
            None => entry.bound_ip = Some(peer.ip()),
            Some(ip) if ip == peer.ip() => {}
            Some(_) => {
                return err(
                    StatusCode::FORBIDDEN,
                    "ip_mismatch",
                    "配对码已绑定到其它来源 IP",
                );
            }
        }
        entry.used = true;
        let role = entry.role.clone();
        let Some(scopes) = exvlan_control::tokens::scopes_for_role(&role) else {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "bad_role",
                "配对码角色异常",
            );
        };
        (role, scopes)
    };

    // **对端签发**：写对端自己的 token 文件（只存 sha256）。
    let path = exvlan_control::tokens::Store::path();
    let mut store = exvlan_control::tokens::Store::load(&path);
    let name = format!("pair:{client}@{}", peer.ip());
    let (token, record) = store.create(&name, scopes.clone(), Some(3600));
    if let Err(e) = store.save(&path) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "token_save",
            e.to_string(),
        );
    }
    tracing::info!(
        "pair redeemed: token={} role={} client={} from={}",
        record.id,
        role,
        client,
        peer.ip()
    );
    Json(json!({
        "token": token,
        "scopes": scopes,
        "token_id": record.id,
        "expires_in": 3600,
        "role": role,
    }))
    .into_response()
}

async fn doctor(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:read")) {
        return resp;
    }
    Json(exvlan_control::doctor::collect().to_json()).into_response()
}

async fn doctor_fix(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("service:control")) {
        return resp;
    }
    let mut report = exvlan_control::doctor::collect();
    exvlan_control::doctor::fix(&mut report);
    Json(report.to_json()).into_response()
}

async fn diag(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:read")) {
        return resp;
    }
    let report = exvlan_control::doctor::collect();
    let p = exvlan_control::paths::paths();
    let user = exvlan_control::paths::user_paths();
    Json(json!({
        "gateway": BUILD_VERSION,
        "backend": exvlan_control::detect().as_str(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "paths": {
            "data": user.data.display().to_string(),
            "logs": user.logs.display().to_string(),
            "config": p.config.display().to_string(),
            "core": p.core.display().to_string(),
        },
        "findings": report.findings.iter().map(|f| f.to_json()).collect::<Vec<_>>(),
    }))
    .into_response()
}

async fn service(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("service:read")) {
        return resp;
    }
    if !exvlan_control::detect().has_service() {
        return Json(json!({
            "backend": "core",
            "capabilities": {"service": false},
            "note": "core backend：内核由 systemd / GUI supervisor 托管",
        }))
        .into_response();
    }
    match ipc(&Request::Status) {
        Ok(v) => Json(v).into_response(),
        Err(resp) => *resp,
    }
}

async fn set_policy(State(state): State<AppState>, headers: HeaderMap, body: String) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("service:control")) {
        return resp;
    }
    let policy = parse_field(&body, "policy").unwrap_or_default();
    if !matches!(policy.as_str(), "resident" | "session" | "stopped") {
        return err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "policy 必须是 resident|session|stopped",
        );
    }
    match ipc(&Request::ModeSet { mode: policy }) {
        Ok(v) => Json(v).into_response(),
        Err(resp) => *resp,
    }
}

async fn set_desired(State(state): State<AppState>, headers: HeaderMap, body: String) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("service:control")) {
        return resp;
    }
    let desired = parse_field(&body, "desired").unwrap_or_default();
    let request = match desired.as_str() {
        "running" => Request::KernelEnsure,
        "stopped" => Request::KernelStop,
        _ => {
            return err(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "desired 必须是 running|stopped",
            );
        }
    };
    match ipc(&request) {
        Ok(v) => Json(v).into_response(),
        Err(resp) => *resp,
    }
}

async fn kernel_restart(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("service:control")) {
        return resp;
    }
    match ipc(&Request::KernelRestart) {
        Ok(v) => Json(v).into_response(),
        Err(resp) => *resp,
    }
}

/// 极简 body 解析：`{"policy":"session"}` 或 `policy=session` 都接受。
fn parse_field(body: &str, key: &str) -> Option<String> {
    let text = body.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<Value>(text)
        && let Some(s) = v.get(key).and_then(Value::as_str)
    {
        return Some(s.to_string());
    }
    for part in text.split('&') {
        if let Some((k, v)) = part.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// instances（内核 RPC）
// ---------------------------------------------------------------------------

fn kernel_url() -> String {
    std::env::var("EXVLAN_RPC")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| format!("tcp://{}", exvlan_control::local_rpc_addr()))
}

async fn kernel_reachable() -> bool {
    tokio::time::timeout(
        std::time::Duration::from_millis(800),
        crate::kernel::Kernel::connect(&kernel_url()),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

async fn with_kernel<T, F>(f: F) -> Result<T, Box<Response>>
where
    F: FnOnce(
        crate::kernel::Kernel,
    )
        -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<T>> + Send>>,
{
    let kernel = match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        crate::kernel::Kernel::connect(&kernel_url()),
    )
    .await
    {
        Ok(Ok(k)) => k,
        Ok(Err(e)) => {
            return Err(Box::new(err(
                StatusCode::SERVICE_UNAVAILABLE,
                "kernel_unavailable",
                format!("连不上内核 RPC（{}）：{e}", kernel_url()),
            )));
        }
        Err(_) => {
            return Err(Box::new(err(
                StatusCode::SERVICE_UNAVAILABLE,
                "kernel_unavailable",
                format!("连接内核 RPC 超时（{}）", kernel_url()),
            )));
        }
    };
    f(kernel).await.map_err(|e| {
        // anyhow 的 Display 只给最外层；这里带上下文，但截断掉 RUST_BACKTRACE 尾部。
        let msg = format!("{e:#}");
        let msg = msg
            .split("\n\nStack backtrace")
            .next()
            .unwrap_or(&msg)
            .to_string();
        Box::new(err(
            StatusCode::BAD_GATEWAY,
            "kernel_rpc",
            msg.chars().take(600).collect::<String>(),
        ))
    })
}

/// 已保存的网络配置（配置目录视图）。正文用 `?full=1` 才回，列表默认只给元信息。
async fn list_configs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:read")) {
        return resp;
    }
    let full = matches!(
        params.get("full").map(String::as_str),
        Some("1") | Some("true")
    );
    match exvlan_control::config::list() {
        Ok(items) => {
            let list: Vec<Value> = items
                .iter()
                .map(|item| {
                    if full {
                        json!({
                            "id": item.id,
                            "enabled": item.enabled,
                            "bytes": item.bytes,
                            "modified": item.modified,
                            "text": item.text,
                        })
                    } else {
                        json!({
                            "id": item.id,
                            "enabled": item.enabled,
                            "bytes": item.bytes,
                            "modified": item.modified,
                        })
                    }
                })
                .collect();
            Json(json!({ "configs": list })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "config_list", e),
    }
}

async fn get_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:read")) {
        return resp;
    }
    match exvlan_control::config::read(&id) {
        Ok(saved) => Json(json!({
            "id": saved.id,
            "enabled": saved.enabled,
            "text": saved.text,
        }))
        .into_response(),
        Err(e) if e.contains("不存在") => err(StatusCode::NOT_FOUND, "not_found", e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "config_read", e),
    }
}

async fn put_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    body: String,
) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:write")) {
        return resp;
    }
    // 支持两种写法：裸 TOML 正文，或 `{"text": "...", "enabled": true}`。
    let parsed: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => Value::String(body),
    };
    let (text, enabled) = match parsed {
        Value::String(text) => (text, true),
        Value::Object(map) => {
            let text = map
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let enabled = map.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            (text, enabled)
        }
        _ => {
            return err(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "请求体应为 TOML 文本或 {text, enabled}",
            );
        }
    };
    if text.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "bad_request", "text 为空");
    }
    match exvlan_control::config::write(&id, &text, enabled) {
        Ok(()) => Json(json!({ "id": id, "enabled": enabled, "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, "config_write", e),
    }
}

async fn delete_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:write")) {
        return resp;
    }
    match exvlan_control::config::delete(&id) {
        Ok(true) => Json(json!({ "id": id, "removed": true })).into_response(),
        Ok(false) => err(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("配置不存在: {id}"),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "config_delete", e),
    }
}

async fn list_instances(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:read")) {
        return resp;
    }
    let result = with_kernel(|kernel| {
        Box::pin(async move {
            let ids = kernel.list_ids().await?;
            let mut out = Vec::new();
            for id in ids {
                let config = kernel.get_config(&id).await?;
                out.push(json!({"id": id, "config": config}));
            }
            Ok(out)
        })
    })
    .await;
    match result {
        Ok(v) => Json(json!({ "instances": v })).into_response(),
        Err(resp) => *resp,
    }
}

/// `GET /instances/{id}`：实例快照（config + 运行信息 + 元信息 + 已保存文件状态）。
///
/// 「已保存」= 配置目录里的文件；「在跑」= 内核里的实例。两者都返回，客户端自己判断。
async fn get_instance(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:read")) {
        return resp;
    }
    let saved = exvlan_control::config::read(&id).ok();
    // config 是主信息（拿不到就是 5xx）；info/meta 是附加信息（best-effort）。
    let config = match with_kernel(|kernel| {
        let id = id.clone();
        Box::pin(async move { kernel.get_config(&id).await })
    })
    .await
    {
        Ok(v) => v,
        Err(resp) => return *resp,
    };
    let info = with_kernel(|kernel| {
        let id = id.clone();
        Box::pin(async move { kernel.collect_info(&id).await })
    })
    .await
    .ok();
    let meta = with_kernel(|kernel| {
        let id = id.clone();
        Box::pin(async move { kernel.meta(&id).await })
    })
    .await
    .ok()
    .flatten();
    let running = config.is_some();
    if !running && saved.is_none() {
        return err(StatusCode::NOT_FOUND, "not_found", "实例不存在或读不到配置");
    }
    Json(json!({
        "id": id,
        "running": running,
        "config": config,
        "info": info.unwrap_or(serde_json::Value::Null),
        "meta": meta.unwrap_or(serde_json::Value::Null),
        "saved": saved.map(|s| json!({ "enabled": s.enabled, "text": s.text })),
    }))
    .into_response()
}

/// `PATCH /instances/{id}`：改状态。
///
/// 支持两种 body（可同时给）：
/// - `{"disabled": true|false}` —— 停/起；同时把配置目录里的文件改成
///   `<id>.toml.disabled` / `<id>.toml`（配置目录是权威，重启后状态不丢）
/// - `{"portal": {"action": "add"|"remove"|"clear", "client": {...}}}`
async fn patch_instance(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    body: String,
) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:write")) {
        return resp;
    }
    let value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return err(
                StatusCode::BAD_REQUEST,
                "bad_request",
                format!("body 不是 JSON: {e}"),
            );
        }
    };

    if let Some(disabled) = value.get("disabled").and_then(serde_json::Value::as_bool) {
        if disabled {
            // 顺序很重要：**先改名**（`.toml` → `.toml.disabled`），再停实例。
            // 反了会出事：内核的 delete 会顺手删掉 `<id>.toml`，配置直接丢（实测）。
            // 临时实例（没有配置文件）没有可改名的对象，直接停即可。
            let has_file = exvlan_control::config::read(&id).is_ok();
            if has_file && let Err(e) = exvlan_control::config::set_enabled(&id, false) {
                return err(StatusCode::INTERNAL_SERVER_ERROR, "config_write", e);
            }
            // `retain`：只保留其它实例 → 停掉本实例，不碰配置目录。
            let stopped = with_kernel(|kernel| {
                let id = id.clone();
                Box::pin(async move { kernel.stop(&id).await })
            })
            .await;
            if let Err(resp) = stopped {
                return *resp;
            }
        } else {
            // 先把文件名恢复（`.toml.disabled` → `.toml`），再从配置目录读回并跑起来。
            if let Err(e) = exvlan_control::config::set_enabled(&id, true) {
                return err(StatusCode::NOT_FOUND, "not_found", e);
            }
            let saved = match exvlan_control::config::read(&id) {
                Ok(s) => s,
                Err(e) => return err(StatusCode::NOT_FOUND, "not_found", e),
            };
            let loader =
                match exvlan_core::config::toml::TomlConfigLoader::new_from_str(&saved.text) {
                    Ok(l) => l,
                    Err(e) => {
                        return err(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "config_parse",
                            e.to_string(),
                        );
                    }
                };
            let config = match exvlan_rpc::exvlan_proto::api::manage::NetworkConfig::new_from_config(
                &loader,
            ) {
                Ok(c) => c,
                Err(e) => {
                    return err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "config_parse",
                        e.to_string(),
                    );
                }
            };
            let started = with_kernel(|kernel| {
                let config = config.clone();
                Box::pin(async move { kernel.run(config, true).await })
            })
            .await;
            if let Err(resp) = started {
                return *resp;
            }
        }
    }

    if let Some(portal) = value.get("portal") {
        let action = portal
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("add");
        let client = portal.get("client").and_then(|c| {
            serde_json::from_value::<exvlan_rpc::exvlan_proto::api::manage::VpnPortalClientConfig>(
                c.clone(),
            )
            .ok()
        });
        let patched = with_kernel(|kernel| {
            let id = id.clone();
            let action = action.to_string();
            Box::pin(async move { kernel.patch_portal(&id, &action, client).await })
        })
        .await;
        if let Err(resp) = patched {
            return *resp;
        }
    }

    Json(json!({ "id": id, "ok": true })).into_response()
}

/// `POST /validate`：校验配置并返回内核 TOML（不启动实例）。
async fn validate_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:read")) {
        return resp;
    }
    let config: exvlan_rpc::exvlan_proto::api::manage::NetworkConfig =
        match serde_json::from_str(&body) {
            Ok(c) => c,
            Err(e) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    format!("配置解析失败: {e}"),
                );
            }
        };
    match with_kernel(|kernel| {
        let config = config.clone();
        Box::pin(async move { kernel.validate(config).await })
    })
    .await
    {
        Ok(toml_config) => Json(json!({ "toml_config": toml_config })).into_response(),
        Err(resp) => *resp,
    }
}

async fn create_instance(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:write")) {
        return resp;
    }
    let config: exvlan_rpc::exvlan_proto::api::manage::NetworkConfig =
        match serde_json::from_str(&body) {
            Ok(c) => c,
            Err(e) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    format!("配置解析失败: {e}"),
                );
            }
        };
    match with_kernel(|kernel| {
        Box::pin(async move { kernel.run(config, true).await.map(|id| json!({"id": id})) })
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(resp) => *resp,
    }
}

async fn delete_instance(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:write")) {
        return resp;
    }
    let target = id.clone();
    match with_kernel(|kernel| {
        let id = id.clone();
        Box::pin(async move { kernel.delete(&id).await.map(|_| json!({"deleted": id})) })
    })
    .await
    {
        Ok(v) => {
            // 配置目录里的文件也一并删掉：内核只在「实例正在跑」时才顺手删文件，
            // 停用中的实例删了之后重启又会出现（语义不一致）。
            match exvlan_control::config::delete(&target) {
                Ok(_) => {}
                Err(e) => tracing::warn!(%target, error = %e, "删除配置文件失败"),
            }
            Json(v).into_response()
        }
        Err(resp) => *resp,
    }
}

// ---------------------------------------------------------------------------
// WebSocket：周期性推送服务/体检摘要（替代客户端轮询）
// ---------------------------------------------------------------------------

async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if let Err(resp) = require_scope(&state, &headers, Some("kernel:read")) {
        return resp;
    }
    ws.on_upgrade(push_loop)
}

async fn push_loop(mut socket: WebSocket) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        ticker.tick().await;
        let report = exvlan_control::doctor::collect();
        let payload = json!({
            "type": "status",
            "ok": report.ok(),
            "findings": report.findings.iter().filter(|f| f.level != "info")
                .map(|f| f.to_json()).collect::<Vec<_>>(),
        });
        if socket
            .send(Message::Text(payload.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
    }
}
