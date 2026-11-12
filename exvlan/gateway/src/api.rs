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
//!
//! 尚未提供 `/instances`（需要内核 RPC 客户端）；`/system.capabilities.instances`
//! 会如实返回 `false`，客户端据此隐藏相关操作。

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{
        Path as AxumPath, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

use crate::Config;

#[derive(Clone)]
struct AppState {
    token: Arc<String>,
}

/// 构建路由。
pub fn router(cfg: Config, token: String) -> Router {
    let _ = &cfg; // 预留：端点在后续版本会用到 bind/port（例如生成远端 URL）
    let state = AppState {
        token: Arc::new(token),
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
        .route("/api/v1/instances", get(list_instances).post(create_instance))
        .route(
            "/api/v1/instances/{id}/config",
            get(get_instance_config).delete(delete_instance),
        )
        .route("/api/v1/events", get(events))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// 鉴权
// ---------------------------------------------------------------------------

fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
        return false;
    };
    let Ok(text) = value.to_str() else {
        return false;
    };
    let Some(token) = text.strip_prefix("Bearer ") else {
        return false;
    };
    // 常量时间比较（长度不同直接 false）。
    let a = token.trim().as_bytes();
    let b = state.token.as_bytes();
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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
fn ipc(op: &str, extra: Value) -> Result<Value, Box<Response>> {
    exvlan_service_client::ipc::request(op, extra)
        .map_err(|e| Box::new(err(StatusCode::BAD_GATEWAY, "service_ipc", e)))
}

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------

async fn system(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let backend = exvlan_service_client::detect();
    let p = exvlan_service_client::paths::paths();
    let user = exvlan_service_client::paths::user_paths();
    let service = if backend.has_service() {
        exvlan_service_client::ipc::request("status", json!({})).ok()
    } else {
        None
    };
    Json(json!({
        "api_version": 1,
        "gateway": env!("CARGO_PKG_VERSION"),
        "backend": backend.as_str(),
        "supervisor": exvlan_service_client::backend::supervisor(),
        "policy": service.as_ref().and_then(|s| s.get("mode").cloned()),
        "privileged": backend.has_service(),
        "endpoints": {
            "rpc": exvlan_service_client::local_rpc_addr(),
            "gateway": exvlan_service_client::local_gateway_addr(),
        },
        "paths": {
            "data": user.data.display().to_string(),
            "logs": user.logs.display().to_string(),
            "config": p.config.display().to_string(),
        },
        "capabilities": {
            "service": backend.has_service(),
            "instances": kernel_reachable().await,
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
            "GET  /api/v1/instances/{id}/config",
            "DELETE /api/v1/instances/{id}/config",
            "GET  /api/v1/events",
        ],
    }))
    .into_response()
}

async fn doctor(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    Json(exvlan_service_client::doctor::collect().to_json()).into_response()
}

async fn doctor_fix(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let mut report = exvlan_service_client::doctor::collect();
    exvlan_service_client::doctor::fix(&mut report);
    Json(report.to_json()).into_response()
}

async fn diag(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let report = exvlan_service_client::doctor::collect();
    let p = exvlan_service_client::paths::paths();
    let user = exvlan_service_client::paths::user_paths();
    Json(json!({
        "gateway": env!("CARGO_PKG_VERSION"),
        "backend": exvlan_service_client::detect().as_str(),
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
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    if !exvlan_service_client::detect().has_service() {
        return Json(json!({
            "backend": "core",
            "capabilities": {"service": false},
            "note": "core backend：内核由 systemd / GUI supervisor 托管",
        }))
        .into_response();
    }
    match ipc("status", json!({})) {
        Ok(v) => Json(v).into_response(),
        Err(resp) => *resp,
    }
}

async fn set_policy(State(state): State<AppState>, headers: HeaderMap, body: String) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let policy = parse_field(&body, "policy").unwrap_or_default();
    if !matches!(policy.as_str(), "resident" | "session" | "stopped") {
        return err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "policy 必须是 resident|session|stopped",
        );
    }
    match ipc("mode.set", json!({ "mode": policy })) {
        Ok(v) => Json(v).into_response(),
        Err(resp) => *resp,
    }
}

async fn set_desired(State(state): State<AppState>, headers: HeaderMap, body: String) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let desired = parse_field(&body, "desired").unwrap_or_default();
    let op = match desired.as_str() {
        "running" => "kernel.ensure",
        "stopped" => "kernel.stop",
        _ => {
            return err(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "desired 必须是 running|stopped",
            );
        }
    };
    match ipc(op, json!({})) {
        Ok(v) => Json(v).into_response(),
        Err(resp) => *resp,
    }
}

async fn kernel_restart(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    match ipc("kernel.restart", json!({})) {
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
        .unwrap_or_else(|| format!("tcp://{}", exvlan_service_client::local_rpc_addr()))
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
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<T>> + Send>,
    >,
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

async fn list_instances(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
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

async fn get_instance_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    match with_kernel(|kernel| {
        let id = id.clone();
        Box::pin(async move { kernel.get_config(&id).await })
    })
    .await
    {
        Ok(Some(cfg)) => Json(json!({"id": id, "config": cfg})).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "not_found", "实例不存在或读不到配置"),
        Err(resp) => *resp,
    }
}

async fn create_instance(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let config: exvlan_rpc::exvlan_proto::api::manage::NetworkConfig =
        match serde_json::from_str(&body) {
            Ok(c) => c,
            Err(e) => return err(StatusCode::BAD_REQUEST, "bad_request", format!("配置解析失败: {e}")),
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
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    match with_kernel(|kernel| {
        let id = id.clone();
        Box::pin(async move { kernel.delete(&id).await.map(|_| json!({"deleted": id})) })
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
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
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    ws.on_upgrade(push_loop)
}

async fn push_loop(mut socket: WebSocket) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        ticker.tick().await;
        let report = exvlan_service_client::doctor::collect();
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
