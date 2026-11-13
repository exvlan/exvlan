//! 经 gateway 的远端管理（R2：只读）。
//!
//! 这是与「直连内核 RPC 1995」并列的第二类远端目标：控制面走对端的 HTTP gateway
//! （默认 2016），鉴权用 Bearer token。
//!
//! 方向约定（见 `docs/decisions-7.3.md` §14.10）：**谁被连，谁出码、谁签发**。
//! 对端在自己本机生成 6 位一次性码；本机拿码去 `POST /api/v1/pair/redeem`，
//! 由**对端** gateway 签发 token 并记录 sha256；本机只保存明文（要用来发请求），
//! 文件名 `remote-tokens.json`、0600，永不签发 token、不写对端的任何文件。
//!
//! 权限：本版本只读。写操作在 UI（`config_permission` 强制 READ_ONLY|NO_DELETE）
//! 与这里的命令集合双重保证 —— 这里根本不提供写命令。

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::AppHandle;

/// 单次请求上限：远端链路（mesh）偶尔抖动，但 UI 不能一直转圈。
const TIMEOUT: Duration = Duration::from_secs(10);
/// gateway 默认端口（`service.toml [gateway].port` 未改时）。
const DEFAULT_GATEWAY_PORT: u16 = 2016;

/// 一条已配对的目标：token 是本机保存的**明文**（对端只存 sha256）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteToken {
    /// 前端生成的远端目标 id（与 `remotes` localStorage 一一对应）。
    pub id: String,
    pub host: String,
    pub port: u16,
    pub token: String,
    /// 对端分配的 token id（吊销时用得上，也是审计线索）。
    pub token_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// 配对时刻（epoch 秒）。
    #[serde(default)]
    pub paired_at: u64,
    /// 过期估算时刻（epoch 秒，0 = 未知/长期）。
    #[serde(default)]
    pub expires_at: u64,
}

/// 给前端的摘要：**不含 token**（token 不出 Rust 侧）。
#[derive(Debug, Clone, Serialize)]
pub struct RemoteSummary {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub token_id: String,
    pub scopes: Vec<String>,
    pub paired_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TokenFile {
    #[serde(default)]
    remotes: Vec<RemoteToken>,
}

/// `remote-tokens.json` 的落点（用户配置目录，0600）。
fn store_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::paths::user_config_dir(app)?.join("remote-tokens.json"))
}

fn load(app: &AppHandle) -> Result<TokenFile, String> {
    let path = store_path(app)?;
    let Ok(text) = std::fs::read_to_string(&path) else {
        // 文件不存在 = 还没配过对，不是错误。
        return Ok(TokenFile::default());
    };
    serde_json::from_str(&text).map_err(|e| format!("{} 解析失败: {e}", path.display()))
}

fn save(app: &AppHandle, file: &TokenFile) -> Result<(), String> {
    let path = store_path(app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(file).map_err(|e| format!("序列化失败: {e}"))?;
    // 原子写：先落临时文件再 rename，避免写一半被读到（token 丢了要重新配对）。
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写 {} 失败: {e}", tmp.display()))?;
    crate::paths::restrict_file(&tmp);
    std::fs::rename(&tmp, &path).map_err(|e| format!("替换 {} 失败: {e}", path.display()))?;
    crate::paths::restrict_file(&path);
    Ok(())
}

fn find(app: &AppHandle, id: &str) -> Result<RemoteToken, String> {
    load(app)?
        .remotes
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| format!("目标 {id} 还没有配对（先在「远程管理」里输入对端的配对码）"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `host` 已是 IPv6 字面量时补方括号（`http://[fd00::1]:2016`）。
fn host_for_url(host: &str) -> String {
    let trimmed = host.trim();
    if trimmed.contains(':') && !trimmed.starts_with('[') {
        format!("[{trimmed}]")
    } else {
        trimmed.to_string()
    }
}

fn base_url(host: &str, port: u16) -> String {
    format!("http://{}:{port}", host_for_url(host))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…")
}

/// 带 Bearer 的 GET；错误信息直接给用户看（含状态码与对端消息）。
async fn get_json(host: &str, port: u16, token: &str, path: &str) -> Result<Value, String> {
    request_json("GET", host, port, token, path, None).await
}

/// 通用请求（写操作用 POST/PATCH/DELETE）。
///
/// 403 会明确提示「token 缺少 scope」——只读配对（role=readonly）本来就该被拒绝，
/// 用户需要在对端用 `--role config`（或 `admin`）重新出码配对。
async fn request_json(
    method: &str,
    host: &str,
    port: u16,
    token: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let url = format!("{}{path}", base_url(host, port));
    let mut req = reqwest::Client::new()
        .request(
            reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| format!("非法方法: {e}"))?,
            &url,
        )
        .bearer_auth(token)
        .timeout(TIMEOUT);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("连接 {url} 失败: {e}（对端 gateway 开着吗？地址对吗？）"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(format!(
            "对端拒绝（{status}）：token 无效/已吊销，或权限不足 —— 请重新配对"
        ));
    }
    if !status.is_success() {
        return Err(format!("对端返回 {status}: {}", truncate(&text, 200)));
    }
    serde_json::from_str(&text).map_err(|e| format!("对端响应不是 JSON: {e}"))
}

/// `POST /api/v1/pair/redeem`：用对端给的码换 token（**token 由对端签发**）。
#[tauri::command]
pub async fn gateway_pair(
    app: AppHandle,
    target_id: String,
    host: String,
    port: Option<u16>,
    code: String,
    client: Option<String>,
) -> Result<RemoteSummary, String> {
    let code = code.trim().to_string();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err("配对码是 6 位数字（在对端「远程协助」里生成）".to_string());
    }
    let host = host.trim().to_string();
    if host.is_empty() {
        return Err("地址不能为空".to_string());
    }
    let port = port.unwrap_or(DEFAULT_GATEWAY_PORT);
    let client = client
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "exvlan-studio".to_string());

    let url = format!("{}/api/v1/pair/redeem", base_url(&host, port));
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&json!({ "code": code, "client": client }))
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("连接 {url} 失败: {e}（对端 gateway 开着吗？地址对吗？）"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| truncate(&text, 200));
        return Err(format!("配对失败（{status}）：{detail}"));
    }
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("对端响应不是 JSON: {e}（{})", truncate(&text, 120)))?;
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| "对端没返回 token".to_string())?
        .to_string();
    let token_id = value
        .get("token_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let scopes: Vec<String> = value
        .get("scopes")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let expires_in = value.get("expires_in").and_then(Value::as_u64).unwrap_or(0);

    let paired_at = now_secs();
    let entry = RemoteToken {
        id: target_id.clone(),
        host: host.clone(),
        port,
        token,
        token_id: token_id.clone(),
        scopes: scopes.clone(),
        paired_at,
        expires_at: if expires_in == 0 {
            0
        } else {
            paired_at + expires_in
        },
    };
    let mut file = load(&app)?;
    file.remotes.retain(|r| r.id != target_id);
    file.remotes.push(entry.clone());
    save(&app, &file)?;
    crate::logger::event(
        "gateway",
        format!("paired target={target_id} host={host}:{port} token={token_id} scopes={scopes:?}"),
    );
    Ok(summary(&entry))
}

fn summary(entry: &RemoteToken) -> RemoteSummary {
    RemoteSummary {
        id: entry.id.clone(),
        host: entry.host.clone(),
        port: entry.port,
        token_id: entry.token_id.clone(),
        scopes: entry.scopes.clone(),
        paired_at: entry.paired_at,
        expires_at: entry.expires_at,
    }
}

/// 已配对的目标（**不含 token**）。
#[tauri::command]
pub fn gateway_remotes(app: AppHandle) -> Result<Vec<RemoteSummary>, String> {
    Ok(load(&app)?.remotes.iter().map(summary).collect())
}

/// 忘掉一个目标的 token（对端那条记录仍在，可用吊销接口清掉）。
#[tauri::command]
pub fn gateway_forget(app: AppHandle, target_id: String) -> Result<(), String> {
    let mut file = load(&app)?;
    let before = file.remotes.len();
    file.remotes.retain(|r| r.id != target_id);
    if file.remotes.len() == before {
        return Err(format!("目标 {target_id} 没有保存的 token"));
    }
    save(&app, &file)?;
    crate::logger::event("gateway", format!("forgot target={target_id}"));
    Ok(())
}

/// 对端概览：`/system`（版本、policy、内核、端点）。
#[tauri::command]
pub async fn gateway_system(app: AppHandle, target_id: String) -> Result<Value, String> {
    let entry = find(&app, &target_id)?;
    get_json(&entry.host, entry.port, &entry.token, "/api/v1/system").await
}

/// 对端诊断：`/doctor`（与服务端同一份 findings）。
#[tauri::command]
pub async fn gateway_doctor(app: AppHandle, target_id: String) -> Result<Value, String> {
    let entry = find(&app, &target_id)?;
    get_json(&entry.host, entry.port, &entry.token, "/api/v1/doctor").await
}

/// 对端实例总览：内核在跑的 + 配置目录里保存的（含 disabled）。
///
/// 返回 `{ running: [id], disabled: [id], instances: { id: {config, info, meta} } }`。
/// 只读：不触发任何启动/停止。
#[tauri::command]
pub async fn gateway_overview(app: AppHandle, target_id: String) -> Result<Value, String> {
    let entry = find(&app, &target_id)?;
    let running = get_json(&entry.host, entry.port, &entry.token, "/api/v1/instances").await?;
    let saved = get_json(&entry.host, entry.port, &entry.token, "/api/v1/configs").await?;
    let running_ids: Vec<String> = running
        .get("instances")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    let disabled_ids: Vec<String> = saved
        .get("configs")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|item| {
                    item.get("enabled")
                        .and_then(Value::as_bool)
                        .map(|enabled| !enabled)
                        .unwrap_or(false)
                })
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "running": running_ids,
        "disabled": disabled_ids,
        "instances": running.get("instances").cloned().unwrap_or_else(|| json!([])),
        "saved": saved.get("configs").cloned().unwrap_or_else(|| json!([])),
    }))
}

/// 单个实例快照：`/instances/{id}`（config + info + meta + saved）。
#[tauri::command]
pub async fn gateway_instance(
    app: AppHandle,
    target_id: String,
    instance_id: String,
) -> Result<Value, String> {
    let entry = find(&app, &target_id)?;
    let path = format!("/api/v1/instances/{instance_id}");
    let mut value = get_json(&entry.host, entry.port, &entry.token, &path).await?;
    // 本机 token 有没有写权限，决定 UI 要不要显示编辑/删除入口。
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "writable".to_string(),
            json!(entry.scopes.iter().any(|s| s == "kernel:write")),
        );
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// 写操作（需要 token 带 kernel:write）
// ---------------------------------------------------------------------------

/// 下发/更新实例配置（`POST /instances`，对端内核会持久化到配置目录）。
#[tauri::command]
pub async fn gateway_run_instance(
    app: AppHandle,
    target_id: String,
    config: Value,
) -> Result<Value, String> {
    let entry = find(&app, &target_id)?;
    request_json(
        "POST",
        &entry.host,
        entry.port,
        &entry.token,
        "/api/v1/instances",
        Some(config),
    )
    .await
}

/// 启停实例（`PATCH /instances/{id} {disabled}`）。
#[tauri::command]
pub async fn gateway_update_state(
    app: AppHandle,
    target_id: String,
    instance_id: String,
    disabled: bool,
) -> Result<Value, String> {
    let entry = find(&app, &target_id)?;
    let path = format!("/api/v1/instances/{instance_id}");
    request_json(
        "PATCH",
        &entry.host,
        entry.port,
        &entry.token,
        &path,
        Some(json!({ "disabled": disabled })),
    )
    .await
}

/// 删除实例（`DELETE /instances/{id}`）。
#[tauri::command]
pub async fn gateway_delete_instance(
    app: AppHandle,
    target_id: String,
    instance_id: String,
) -> Result<Value, String> {
    let entry = find(&app, &target_id)?;
    let path = format!("/api/v1/instances/{instance_id}");
    request_json("DELETE", &entry.host, entry.port, &entry.token, &path, None).await
}

/// 校验配置（`POST /validate` → 返回 TOML 文本）。
#[tauri::command]
pub async fn gateway_validate(
    app: AppHandle,
    target_id: String,
    config: Value,
) -> Result<Value, String> {
    let entry = find(&app, &target_id)?;
    request_json(
        "POST",
        &entry.host,
        entry.port,
        &entry.token,
        "/api/v1/validate",
        Some(config),
    )
    .await
}

/// 本机生成配对码（**被管端**的动作）。
///
/// 走服务 IPC：服务托管 gateway，知道它绑在哪，也持有 admin token（0600，用户态读不到）。
#[tauri::command]
pub fn gateway_pair_start(role: Option<String>, ttl: Option<u64>) -> Result<Value, String> {
    let role = role.unwrap_or_else(|| "readonly".to_string());
    let ttl = ttl.unwrap_or(300);
    crate::service_client::pair_start(&role, ttl)
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_GATEWAY_PORT, base_url, host_for_url, truncate};

    #[test]
    fn ipv6_hosts_are_bracketed() {
        assert_eq!(
            base_url("fd00::1", DEFAULT_GATEWAY_PORT),
            "http://[fd00::1]:2016"
        );
        assert_eq!(
            base_url("10.254.1.6", DEFAULT_GATEWAY_PORT),
            "http://10.254.1.6:2016"
        );
        assert_eq!(host_for_url("127.0.0.1"), "127.0.0.1");
    }

    #[test]
    fn truncate_keeps_head_and_marks_ellipsis() {
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate("abc", 3), "abc");
    }
}
