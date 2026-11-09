// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod elevate;
#[cfg(target_os = "android")]
mod ffi;
mod helper;
mod kernel;
mod logger;
mod networks;
mod paths;
mod prefs;
// upgrade 全平台编译：kernel 子模块在移动端返回“不支持”，app 子模块移动端走 APK 流程。
mod upgrade;

use anyhow::Context;
use exvlan_rpc::exvlan_proto::api::config::{ConfigPatchAction, VpnPortalClientPatch};
use exvlan_rpc::exvlan_proto::api::instance::{
    GetVpnPortalInfoRequest, InstanceIdentifier, VpnPortalInfo, instance_identifier,
};
use exvlan_rpc::exvlan_proto::api::manage::{
    CollectNetworkInfoResponse, NetworkConfig, ValidateConfigResponse, VpnPortalClientConfig,
    WebClientService,
};
use exvlan_rpc::exvlan_proto::rpc_types::controller::BaseController;

// 桌面端专用：内核 crate 提供 TOML 转换、持久化 trait 与隧道拨号（P7 后移动端不再依赖）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use exvlan_rpc::exvlan_core::config::api_input::NetworkConfigExt;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use exvlan_rpc::exvlan_core::config::toml::{ConfigLoader, ConfigSource, TomlConfigLoader};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use exvlan_rpc::exvlan_core::management::config_source_to_rpc;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use exvlan_rpc::exvlan_core::management::remote_client::{
    GetNetworkMetasResponse, ListNetworkInstanceIdsJsonResp, ListNetworkProps, RemoteClientManager,
    Storage,
};
#[cfg(any(target_os = "android", target_os = "ios"))]
use manager::{GetNetworkMetasResponse, ListNetworkInstanceIdsJsonResp};

use tokio::sync::{Mutex, RwLock, RwLockReadGuard};
use uuid::Uuid;

use tauri::{AppHandle, Emitter, Manager as _};

#[cfg(not(target_os = "android"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

static CLIENT_MANAGER: once_cell::sync::Lazy<RwLock<Option<manager::GUIClientManager>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

/// 最近一次交给内核的 TUN fd（0 表示尚未设置），写进自检便于确认隧道真的挂上了。
static LAST_TUN_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// 桌面服务（launchd / systemd / Windows 服务）的稳定标识。
///
/// 20711 之前用的是 crate 名 `exvlan-studio`；20712 起统一为反向域名，
/// 与系统路径、helper 命名保持一致。
pub const SERVICE_LABEL: &str = crate::paths::APP_ID;

/// 构建版本串：`<标准版本>[-<channel>.<n>][+<sha6>]`。
///
/// 由 CI 的 `script/ci/build-version.sh` 通过 `EXVLAN_BUILD_VERSION` 注入；
/// 本地构建没有该变量时回落到 crate 版本。
pub const BUILD_VERSION: &str = match option_env!("EXVLAN_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// 旧版本残留清理开关。
///
/// 7.3 起路径 v3 硬切（见 `docs/paths.md`），旧 `exvlan` 布局不再被任何代码读取，
/// 因此恒为 `true`：新服务健康后删除旧目录/旧 plist/旧日志/UI 快照。
/// 保留这个开关是为了以后需要临时关闭时有单一入口。
pub const ENABLE_LEGACY_CLEANUP: bool = true;

macro_rules! get_client_manager {
    () => {{
        let guard = CLIENT_MANAGER
            .try_read()
            .map_err(|_| "Failed to acquire read lock for client manager")?;
        RwLockReadGuard::try_map(guard, |cm| cm.as_ref())
            .map_err(|_| "RPC connection not initialized".to_string())
    }};
}

#[tauri::command]
fn exvlan_version() -> Result<String, String> {
    // 桌面端的内核是独立进程：以已安装的内核版本为准，未安装时回退到 GUI 版本。
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    if let Some(version) = kernel::installed_version() {
        return Ok(version);
    }
    Ok(BUILD_VERSION.to_string())
}

#[tauri::command]
fn set_dock_visibility(app: tauri::AppHandle, visible: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use tauri::ActivationPolicy;
        app.set_activation_policy(if visible {
            ActivationPolicy::Regular
        } else {
            ActivationPolicy::Accessory
        })
        .map_err(|e| logger::other("cmd:set_dock_visibility", e))?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
    Ok(())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
fn parse_network_config(cfg: NetworkConfig) -> Result<String, String> {
    let toml = cfg
        .gen_config()
        .map_err(|e| logger::other("cmd:parse_network_config", e))?;
    Ok(toml.dump())
}

/// 移动端：内核的 `validate_config` 返回的就是 `gen_config().dump()`，
/// 与桌面端 `parse_network_config` 行为完全一致（导入/导出/编辑器共用）。
#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
async fn parse_network_config(cfg: NetworkConfig) -> Result<String, String> {
    let client_manager = get_client_manager!()?;
    let client = client_manager.transport.web_client();
    let resp = client
        .validate_config(
            BaseController::default(),
            exvlan_rpc::exvlan_proto::api::manage::ValidateConfigRequest { config: Some(cfg) },
        )
        .await
        .map_err(|e| logger::other("cmd:parse_network_config", e.to_string()))?;
    Ok(resp.toml_config)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
fn generate_network_config(toml_config: String) -> Result<NetworkConfig, String> {
    let config = TomlConfigLoader::new_from_str(&toml_config)
        .map_err(|e| logger::other("cmd:generate_network_config", e))?;
    let cfg = NetworkConfig::new_from_config(&config)
        .map_err(|e| logger::other("cmd:generate_network_config", e))?;
    Ok(cfg)
}

/// 移动端：TOML → NetworkConfig 走内核内部转换服务（逻辑与桌面端同源）。
#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
fn generate_network_config(toml_config: String) -> Result<NetworkConfig, String> {
    let value = exvlan_rpc::ffi_rpc::call(
        "exvlan.internal.ConfigConvert",
        "toml_to_config",
        "",
        serde_json::json!({ "tomlConfig": toml_config }),
    )
    .map_err(|e| logger::other("cmd:generate_network_config", e))?;
    serde_json::from_value(value).map_err(|e| logger::other("cmd:generate_network_config", e))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn run_network_instance(
    app: AppHandle,
    cfg: NetworkConfig,
    save: bool,
) -> Result<(), String> {
    let client_manager = get_client_manager!()?;
    let toml_config = cfg
        .gen_config()
        .map_err(|e| logger::other("cmd:run_network_instance", e))?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .pre_run_network_instance_hook(&app, &toml_config, manager::PersistedConfigSource::User)
        .await?;
    client_manager
        .handle_run_network_instance(app.clone(), cfg, save)
        .await
        .map_err(|e| logger::other("cmd:run_network_instance", e))?;
    client_manager
        .post_run_network_instance_hook(&app, &toml_config.get_id())
        .await?;
    Ok(())
}

/// 移动端：配置的 TOML 化由内核侧完成，GUI 只传 `NetworkConfig`。
#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
async fn run_network_instance(
    app: AppHandle,
    cfg: NetworkConfig,
    save: bool,
) -> Result<(), String> {
    logger::event(
        "inst",
        format!(
            "run save={save} name={:?} no_tun={:?}",
            cfg.network_name, cfg.no_tun
        ),
    );
    let instance_id: Uuid = cfg
        .instance_id
        .clone()
        .unwrap_or_default()
        .parse()
        .map_err(|e: uuid::Error| logger::other("cmd:run_network_instance", e))?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .pre_run_network_instance_hook(&app, &cfg, manager::PersistedConfigSource::User)
        .await?;
    client_manager
        .handle_run_network_instance(app.clone(), cfg, save)
        .await
        .map_err(|e| logger::other("cmd:run_network_instance", e))?;
    client_manager
        .post_run_network_instance_hook(&app, &instance_id)
        .await?;
    Ok(())
}

#[tauri::command]
async fn collect_network_info(
    app: AppHandle,
    instance_id: String,
) -> Result<CollectNetworkInfoResponse, String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| logger::other("cmd:collect_network_info", e))?;
    get_client_manager!()?
        .handle_collect_network_info(app, Some(vec![instance_id]))
        .await
        .map_err(|e| logger::other("cmd:collect_network_info", e))
}

#[tauri::command]
async fn get_vpn_portal_info(instance_id: String) -> Result<Option<VpnPortalInfo>, String> {
    let instance_id = instance_id
        .parse::<uuid::Uuid>()
        .map_err(|e| logger::other("cmd:get_vpn_portal_info", e))?;
    let client_manager = get_client_manager!()?;
    // 走 transport：桌面 = tunnel，移动端 = FFI（rpc_manager 在移动端没有连接）
    let client = client_manager.transport.vpn_portal();
    let response = client
        .get_vpn_portal_info(
            BaseController::default(),
            GetVpnPortalInfoRequest {
                instance: Some(InstanceIdentifier {
                    selector: Some(instance_identifier::Selector::Id(instance_id.into())),
                }),
            },
        )
        .await
        .map_err(|e| logger::other("cmd:get_vpn_portal_info", e))?;
    Ok(response.vpn_portal_info)
}

#[tauri::command]
async fn patch_vpn_portal_clients(
    app: AppHandle,
    instance_id: String,
    action: String,
    name: Option<String>,
    virtual_ip: Option<String>,
    groups: Option<Vec<String>>,
) -> Result<(), String> {
    let instance_id = instance_id
        .parse::<uuid::Uuid>()
        .map_err(|e| logger::other("cmd:patch_vpn_portal_clients", e))?;
    let action = match action.as_str() {
        "add" => ConfigPatchAction::Add,
        "remove" => ConfigPatchAction::Remove,
        "clear" => ConfigPatchAction::Clear,
        other => return Err(format!("invalid vpn portal client patch action: {other}")),
    };
    let client = if action == ConfigPatchAction::Clear {
        None
    } else {
        Some(VpnPortalClientConfig {
            name: name.unwrap_or_default(),
            virtual_ip: virtual_ip.unwrap_or_default(),
            groups: groups.unwrap_or_default(),
        })
    };

    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .handle_patch_vpn_portal_clients(
            app,
            instance_id,
            vec![VpnPortalClientPatch {
                action: action as i32,
                client,
            }],
        )
        .await
        .map_err(|e| logger::other("cmd:patch_vpn_portal_clients", e))
}

#[tauri::command]
async fn set_logging_level(app: AppHandle, level: String) -> Result<(), String> {
    get_client_manager!()?
        .set_logging_level(level.clone())
        .await
        .map_err(|e| logger::other("cmd:set_logging_level", e))?;
    // GUI/内核同进程：本地 logger 也要跟着热更新，否则 exvlan.log 仍是旧级别
    init_gui_logging_with(&app, Some(&level));
    Ok(())
}

#[tauri::command]
async fn set_tun_fd(fd: i32) -> Result<(), String> {
    LAST_TUN_FD.store(fd, std::sync::atomic::Ordering::Relaxed);
    logger::event("vpn", format!("set_tun_fd fd={fd}"));
    #[cfg(target_os = "android")]
    {
        // 取第一个实例，把 VpnService 的 TUN fd 交给内核（FFI）
        let names = crate::ffi::list_instances(16)?;
        if let Some(name) = names.first() {
            crate::ffi::attach_tun_fd(name, fd)?;
            return Ok(());
        }
        return Err("没有运行中的内核实例".to_string());
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = fd;
        Err("TUN 由内核守护进程创建，GUI 不再传递 fd".to_string())
    }
}

#[tauri::command]
async fn list_network_instance_ids(
    app: AppHandle,
) -> Result<ListNetworkInstanceIdsJsonResp, String> {
    get_client_manager!()?
        .handle_list_network_instance_ids(app)
        .await
        .map_err(|e| logger::other("cmd:list_network_instance_ids", e))
}

#[tauri::command]
async fn remove_network_instance(app: AppHandle, instance_id: String) -> Result<(), String> {
    logger::event("inst", format!("remove {instance_id}"));
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| logger::other("cmd:remove_network_instance", e))?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .handle_remove_network_instances(app.clone(), vec![instance_id])
        .await
        .map_err(|e| logger::other("cmd:remove_network_instance", e))?;
    client_manager
        .post_stop_network_instances_hook(&app)
        .await?;

    Ok(())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn update_network_config_state(
    app: AppHandle,
    instance_id: String,
    disabled: bool,
) -> Result<(), String> {
    logger::event(
        "inst",
        format!("update_state {instance_id} disabled={disabled}"),
    );
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| logger::other("cmd:update_network_config_state", e))?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    if !disabled {
        let (cfg, source) = client_manager
            .handle_get_network_config_with_source(app.clone(), instance_id)
            .await
            .map_err(|e| logger::other("cmd:update_network_config_state", e))?;
        let toml_config = cfg
            .gen_config()
            .map_err(|e| logger::other("cmd:update_network_config_state", e))?;
        client_manager
            .pre_run_network_instance_hook(
                &app,
                &toml_config,
                manager::PersistedConfigSource::from_runtime_source(source),
            )
            .await?;
    }
    client_manager
        .handle_update_network_state(app.clone(), instance_id, disabled)
        .await
        .map_err(|e| logger::other("cmd:update_network_config_state", e))?;

    if disabled {
        client_manager
            .post_stop_network_instances_hook(&app)
            .await?;
    } else {
        client_manager
            .post_run_network_instance_hook(&app, &instance_id)
            .await?;
    }

    Ok(())
}

/// 移动端：没有 TOML 中间态，直接用 `NetworkConfig` 走前置钩子。
#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
async fn update_network_config_state(
    app: AppHandle,
    instance_id: String,
    disabled: bool,
) -> Result<(), String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| logger::other("cmd:update_network_config_state", e))?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    if !disabled {
        let (cfg, source) = client_manager
            .handle_get_network_config_with_source(app.clone(), instance_id)
            .await
            .map_err(|e| logger::other("cmd:update_network_config_state", e))?;
        client_manager
            .pre_run_network_instance_hook(&app, &cfg, source)
            .await?;
    }
    client_manager
        .handle_update_network_state(app.clone(), instance_id, disabled)
        .await
        .map_err(|e| logger::other("cmd:update_network_config_state", e))?;

    if disabled {
        client_manager
            .post_stop_network_instances_hook(&app)
            .await?;
    } else {
        client_manager
            .post_run_network_instance_hook(&app, &instance_id)
            .await?;
    }

    Ok(())
}

#[tauri::command]
async fn save_network_config(app: AppHandle, cfg: NetworkConfig) -> Result<(), String> {
    let instance_id = cfg
        .instance_id()
        .parse()
        .map_err(|e: uuid::Error| logger::other("cmd:save_network_config", e))?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .handle_save_network_config(app, instance_id, cfg)
        .await
        .map_err(|e| logger::other("cmd:save_network_config", e))
}

#[tauri::command]
async fn validate_config(
    app: AppHandle,
    config: NetworkConfig,
) -> Result<ValidateConfigResponse, String> {
    get_client_manager!()?
        .handle_validate_config(app, config)
        .await
        .map_err(|e| logger::other("cmd:validate_config", e))
}

#[tauri::command]
async fn get_config(app: AppHandle, instance_id: String) -> Result<NetworkConfig, String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| logger::other("cmd:get_config", e))?;
    let cfg = get_client_manager!()?
        .handle_get_network_config(app, instance_id)
        .await
        .map_err(|e| logger::other("cmd:get_config", e))?;
    Ok(cfg)
}

#[tauri::command]
async fn load_configs(
    app: AppHandle,
    configs: Vec<manager::StoredGuiConfig>,
    enabled_networks: Vec<String>,
) -> Result<(), String> {
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .load_configs(app, configs, enabled_networks)
        .await
        .map_err(|e| logger::other("cmd:load_configs", e))?;
    Ok(())
}

#[tauri::command]
async fn get_network_metas(
    app: AppHandle,
    instance_ids: Vec<uuid::Uuid>,
) -> Result<GetNetworkMetasResponse, String> {
    get_client_manager!()?
        .handle_get_network_metas(app, instance_ids)
        .await
        .map_err(|e| logger::other("cmd:get_network_metas", e))
}

#[cfg(target_os = "android")]
#[tauri::command]
fn init_service() -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
fn init_service(opts: Option<service::ServiceOptions>) -> Result<(), String> {
    match opts {
        Some(args) => {
            let path = std::path::Path::new(&args.config_dir);
            if !path.exists() {
                std::fs::create_dir_all(&args.config_dir)
                    .map_err(|e| logger::other("cmd:init_service", e))?;
            } else if !path.is_dir() {
                return Err("config_dir exists but is not a directory".to_string());
            }
            let path = std::path::Path::new(&args.file_log_dir);
            if !path.exists() {
                std::fs::create_dir_all(&args.file_log_dir)
                    .map_err(|e| logger::other("cmd:init_service", e))?;
            } else if !path.is_dir() {
                return Err("file_log_dir exists but is not a directory".to_string());
            }

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                let payload = serde_json::to_string(&args)
                    .map_err(|e| logger::other("cmd:init_service", e))?;
                run_elevated(&["--install-service", &payload])?;
            }
            #[cfg(target_os = "ios")]
            service::install(args).map_err(|e| format!("{:#}", e))?;
        }
        None => {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            run_elevated(&["--uninstall-service"])?;
            #[cfg(target_os = "ios")]
            service::uninstall().map_err(|e| format!("{:#}", e))?;
        }
    }
    Ok(())
}

#[tauri::command]
fn set_service_status(_enable: bool) -> Result<(), String> {
    logger::record("service.ctl", format!("enable={_enable}"));
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        run_elevated(&["--service-ctl", if _enable { "start" } else { "stop" }])?;
    }
    // Android 走 VpnService（无系统服务管理器），无需启停服务。
    #[cfg(target_os = "ios")]
    {
        service::set_status(_enable).map_err(|e| format!("{:#}", e))?;
    }
    #[cfg(target_os = "android")]
    {
        let _ = _enable;
    }
    Ok(())
}

#[tauri::command]
fn get_service_status() -> Result<&'static str, String> {
    #[cfg(not(target_os = "android"))]
    {
        use exvlan_rpc::service_manager::ServiceStatus;
        let status = service::status().map_err(|e| format!("{:#}", e))?;
        match status {
            ServiceStatus::NotInstalled => Ok("NotInstalled"),
            ServiceStatus::Stopped(_) => Ok("Stopped"),
            ServiceStatus::Running => Ok("Running"),
        }
    }
    #[cfg(target_os = "android")]
    {
        Ok("NotInstalled")
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn normalize_normal_mode_rpc_portal(portal: &str) -> Result<(url::Url, url::Url), String> {
    let portal_url: url::Url = portal
        .parse()
        .map_err(|e| format!("invalid rpc portal: {:#}", e))?;
    let bind_url = portal_url.clone();
    let mut connect_url = portal_url.clone();
    // if bind addr is 0.0.0.0, should convert to 127.0.0.1
    if connect_url.host_str() == Some("0.0.0.0") {
        connect_url.set_host(Some("127.0.0.1")).unwrap();
    }
    Ok((bind_url, connect_url))
}

#[cfg(any(target_os = "android", target_os = "ios"))]
async fn resolve_rpc_bind_url(url: &url::Url) -> Result<std::net::SocketAddr, String> {
    if url.scheme() != "tcp" {
        return Err(format!("RPC portal requires tcp URL: {url}"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| format!("RPC portal has no host: {url}"))?;
    let port = url.port().unwrap_or(11010);
    tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("failed to resolve RPC portal {url}: {error}"))?
        .next()
        .ok_or_else(|| format!("RPC portal has no resolved address: {url}"))
}

/// 桌面端默认连接的本地内核守护进程（服务模式与 normal 模式共用）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const LOCAL_DAEMON_RPC_PORTAL: &str = "tcp://127.0.0.1:15999";

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn init_rpc_connection(
    _app: AppHandle,
    _is_normal_mode: bool,
    url: Option<String>,
) -> Result<(), String> {
    let mut client_manager_guard =
        tokio::time::timeout(std::time::Duration::from_secs(5), CLIENT_MANAGER.write())
            .await
            .map_err(|_| "Failed to acquire write lock for client manager")?;

    // normal 模式不再在 GUI 进程内跑实例，统一连本地内核守护进程；
    // remote 模式由前端传入显式地址。
    let client_url = url
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| LOCAL_DAEMON_RPC_PORTAL.to_string());

    let client_manager = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        manager::GUIClientManager::new(Some(client_url.clone())),
    )
    .await
    .map_err(|_| format!("connect rpc timed out: {client_url}"))?
    .with_context(|| format!("Failed to connect rpc: {client_url}"))
    .map_err(|e| format!("{:#}", e))?;
    *client_manager_guard = Some(client_manager);
    logger::record("mode", format!("desktop rpc connected: {client_url}"));
    logger::event("mode", format!("desktop rpc connected: {client_url}"));

    Ok(())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
async fn init_rpc_connection(
    _app: AppHandle,
    _is_normal_mode: bool,
    _url: Option<String>,
) -> Result<(), String> {
    // P6：移动端内核在应用进程内，不再自建 ring tunnel / 本地 RPC server，
    // GUI 通过 FfiTransport 直接经 C ABI 调内核。
    let mut guard = tokio::time::timeout(std::time::Duration::from_secs(5), CLIENT_MANAGER.write())
        .await
        .map_err(|_| "Failed to acquire write lock for client manager")?;
    *guard = Some(manager::GUIClientManager::new_ffi());
    logger::record("mode", "mobile ffi transport ready");
    logger::event("mode", "mobile ffi transport ready");
    Ok(())
}

#[tauri::command]
async fn is_client_running() -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        // 移动端内核在进程内，RPC 走 FFI：只要 GUI 管理器已初始化就算「已连接」。
        // 不能再用「是否有运行中的实例」判断 —— 那会让 UI 在冷启动时就显示
        // 「无法连接至远程客户端」，用户没有入口去启动实例。
        return Ok(CLIENT_MANAGER
            .try_read()
            .map(|guard| guard.is_some())
            .unwrap_or(false));
    }
    #[cfg(not(target_os = "android"))]
    {
        Ok(get_client_manager!()?.rpc_manager.is_running())
    }
}

#[tauri::command]
async fn init_web_client(app: AppHandle, url: Option<String>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        // P6：不再自建 in-process web client，改用内核 FFI 的 config-server 客户端
        let Some(url) = url else {
            let _ = ffi::stop_config_server();
            return Ok(());
        };
        let hostname = "exvlan-android".to_string();
        ffi::start_config_server(&url, &hostname, "", false, None, std::ptr::null_mut())?;
        let _ = app;
    }
    #[cfg(target_os = "ios")]
    {
        let _ = (app, url);
    }
    // 桌面端：config-server 同步由内核守护进程负责（`--config-server`），GUI 不再自建 web client。
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let _ = (app, url);
    Ok(())
}

#[tauri::command]
async fn is_web_client_connected() -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        return Ok(crate::ffi::config_server_connected());
    }
    #[cfg(not(target_os = "android"))]
    {
        Ok(false)
    }
}

/// GUI 侧日志初始化：与内核共用 `exvlan-rpc` 的日志栈（控制台 + 滚动文件）。
fn init_gui_logging(app: &tauri::AppHandle) {
    init_gui_logging_with(app, None);
}

/// 初始化 GUI 文件日志。
///
/// 默认级别很关键：以前 `level: None` 会被解析成 `Off`，等于**完全没有日志**，
/// 出问题时拿不到任何内核/peer 现场。现在桌面默认 info、Android 默认 debug。
fn init_gui_logging_with(app: &tauri::AppHandle, level: Option<&str>) {
    let Ok(log_dir) = get_log_dir(app) else {
        return;
    };
    let level = level.map(|s| s.to_string()).unwrap_or_else(|| {
        if cfg!(target_os = "android") {
            "debug".to_string()
        } else {
            "info".to_string()
        }
    });
    #[cfg(target_os = "android")]
    {
        // 同一目录再初始化一次 .so 内部的 logger（两份全局状态，必须各注册一份）
        if let (Ok(dir_c), Ok(level_c)) = (
            std::ffi::CString::new(log_dir.to_string_lossy().as_bytes()),
            std::ffi::CString::new(level.as_bytes()),
        ) {
            unsafe extern "C" {
                fn exvlan_ffi_init_logging(
                    dir: *const std::ffi::c_char,
                    level: *const std::ffi::c_char,
                ) -> std::ffi::c_int;
            }
            let rc = unsafe { exvlan_ffi_init_logging(dir_c.as_ptr(), level_c.as_ptr()) };
            logger::record("log.ffi", format!("exvlan_ffi_init_logging rc={rc}"));
        }
    }

    let config = exvlan_rpc::log::SimpleLoggingConfig::new(
        exvlan_rpc::log::ConsoleLoggerConfig::default(),
        exvlan_rpc::log::FileLoggerConfig {
            dir: Some(log_dir.to_string_lossy().to_string()),
            level: Some(level.clone()),
            // GUI 自己的日志必须写 studio.log：默认文件名是 exvlan.log，会和内核日志
            // 同名（不同目录），诊断包里 studio.log 永远是空文件，排查时误判成"没有日志"。
            file: Some("studio.log".to_string()),
            size_mb: Some(16),
            count: Some(3),
        },
    );
    if let Err(err) = exvlan_rpc::log::init(&config, true) {
        eprintln!("初始化 GUI 日志失败: {err:#}");
        logger::record("log.init", format!("{err:#}"));
        return;
    }
    // 自检：记录级别与文件路径；文件大小在下次启动的 [boot] 行回报
    let path = log_dir.join("studio.log");
    let size = std::fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or_default();
    logger::record(
        "log.selftest",
        format!("level={level} path={} size={size}", path.display()),
    );
}

/// Android：初始化 rustls 的平台证书校验器。
///
/// reqwest 0.13 的 rustls 后端固定使用 `rustls-platform-verifier`，而它在 Android
/// 上必须先注入 JVM + Context，否则第一次 HTTPS 就会 panic
/// （`Expect rustls-platform-verifier to be initialized`），20709 的闪退即由此而来。
#[cfg(target_os = "android")]
fn init_android_tls() {
    use jni::JavaVM;
    use jni::objects::JObject;

    let ctx = ndk_context::android_context();
    // SAFETY: ndk-context 提供进程级有效的 JVM 指针
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) };
    let raw_context = ctx.context();

    // jni 0.22 的 attach 是「回调式」：在闭包里拿到 &mut Env
    let outcome = vm.attach_current_thread(|env| {
        // SAFETY: context 是全局引用，回调期间有效
        let context = unsafe { JObject::from_raw(env, raw_context.cast()) };
        rustls_platform_verifier::android::init_with_env(env, context)
    });

    match outcome {
        Ok(()) => logger::record("tls", "rustls-platform-verifier initialized"),
        Err(err) => logger::record("tls", format!("init failed: {err}")),
    }
}

#[cfg(not(target_os = "android"))]
fn init_android_tls() {}

// 获取日志目录的辅助函数
fn get_log_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, tauri::Error> {
    // 所有平台统一用 app_log_dir：`error.log` 本来就在这，events/kernel 也放一起，
    // 避免 Android 上出现「内部目录 / 外部 cache」两套路径。
    app.path().app_log_dir()
}

#[tauri::command]
async fn get_log_dir_path(app: tauri::AppHandle) -> Result<String, String> {
    match get_log_dir(&app) {
        Ok(log_dir) => {
            std::fs::create_dir_all(&log_dir).ok();
            Ok(log_dir.to_string_lossy().to_string())
        }
        Err(e) => Err(format!("Failed to get log directory: {}", e)),
    }
}

/// `error.log` 的候选目录：用户日志目录 → 配置目录下的 logs → 临时目录。
///
/// 任何一级不可写（典型：被 root 守护进程写过，属主变成 root）都会自动降级，
/// 并把失败原因记录下来，避免再出现「日志尚未初始化」这种无从下手的状态。
fn error_log_candidates(app: &AppHandle) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(dir) = crate::paths::user_log_dir(app) {
        out.push(dir);
    }
    if let Ok(dir) = crate::paths::user_config_dir(app) {
        out.push(dir.join("logs"));
    }
    out.push(crate::paths::tmp_dir().join("logs"));
    out
}

/// 已初始化则直接返回；否则按候选顺序初始化。
fn ensure_error_log(app: &AppHandle) -> Option<std::path::PathBuf> {
    if logger::is_initialized() {
        return logger::path();
    }
    logger::init_best(&error_log_candidates(app))
}

fn dir_writable(dir: &std::path::Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".exvlan-write-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 启动自检行：路径、可写性、HOME、版本、内核/服务状态。
fn record_boot_line(app: &AppHandle) {
    use tauri::Manager as _;
    let log_dir = app.path().app_log_dir().ok();
    let config_dir = app.path().app_config_dir().ok();
    let home = std::env::var("HOME").unwrap_or_default();
    let log_writable = log_dir.as_deref().map(dir_writable);
    let config_writable = config_dir.as_deref().map(dir_writable);

    let mut line = format!(
        "version={} os={} arch={} home={home} log_dir={:?} writable={log_writable:?} config_dir={:?} writable={config_writable:?} error_log={:?}",
        BUILD_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        log_dir.as_ref().map(|p| p.display().to_string()),
        config_dir.as_ref().map(|p| p.display().to_string()),
        logger::path().map(|p| p.display().to_string()),
    );

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        line.push_str(&format!(
            " kernel={:?} kernel_version={:?} service={:?}",
            kernel::is_installed(),
            kernel::installed_version(),
            service::status().map(|s| format!("{s:?}")).ok()
        ));
        let dirs = service_default_dirs();
        line.push_str(&format!(
            " service_config_dir={} service_log_dir={}",
            dirs.config_dir, dirs.file_log_dir
        ));
        let gui_log_size = log_dir
            .as_ref()
            .and_then(|d| newest_kernel_log(d))
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len());
        line.push_str(&format!(" exvlan_log_size={gui_log_size:?}"));
    }

    logger::record("boot", line);

    // 目录不可写时单独记一条，方便直接定位
    if log_writable == Some(false) || config_writable == Some(false) {
        logger::record(
            "boot.permissions",
            format!(
                "日志或配置目录不可写（可能被 root 占用）；log_dir={:?} config_dir={:?}",
                log_dir.as_ref().map(|p| p.display().to_string()),
                config_dir.as_ref().map(|p| p.display().to_string())
            ),
        );
    }
}

/// 安装全局 panic hook：panic 信息先写 `error.log`，再交给默认 hook。
///
/// Android release 是 `panic = "abort"`，hook 仍会在 abort 前执行，
/// 这是崩溃现场唯一的落盘机会。
fn install_panic_hook() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let thread = std::thread::current()
                .name()
                .unwrap_or("<unnamed>")
                .to_string();
            let backtrace = std::backtrace::Backtrace::force_capture();
            logger::record_panic(format!(
                "thread={thread} location={location} {info} | backtrace={backtrace}"
            ));
            default_hook(info);
        }));
    });
}

fn sanitize_scope(scope: &str) -> String {
    let cleaned: String = scope
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-'))
        .take(48)
        .collect();
    if cleaned.is_empty() {
        "fe:unknown".to_string()
    } else if cleaned.starts_with("fe:") {
        cleaned
    } else {
        format!("fe:{cleaned}")
    }
}

#[derive(serde::Serialize)]
pub struct DiagnosticsInfo {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub home: String,
    pub log_dir: Option<String>,
    pub log_dir_writable: Option<bool>,
    pub config_dir: Option<String>,
    pub config_dir_writable: Option<bool>,
    pub error_log: Option<String>,
    pub error_log_size: Option<u64>,
    pub kernel_installed: Option<bool>,
    pub kernel_version: Option<String>,
    pub service_status: Option<String>,
    pub service_config_dir: Option<String>,
    pub service_log_dir: Option<String>,
}

/// 诊断信息：路径、可写性、版本、内核/服务状态（关于页展示 + 一键复制）。
#[tauri::command]
fn diagnostics_info(app: AppHandle) -> Result<DiagnosticsInfo, String> {
    use tauri::Manager as _;
    let log_dir = app.path().app_log_dir().ok();
    let config_dir = app.path().app_config_dir().ok();
    let error_log = ensure_error_log(&app);
    let size = error_log
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len());

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let (kernel_installed, kernel_version, service_status, service_config_dir, service_log_dir) = {
        let dirs = service_default_dirs();
        (
            Some(kernel::is_installed()),
            kernel::installed_version(),
            service::status().map(|s| format!("{s:?}")).ok(),
            Some(dirs.config_dir),
            Some(dirs.file_log_dir),
        )
    };
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let (kernel_installed, kernel_version, service_status, service_config_dir, service_log_dir) =
        (None, None, None, None, None);

    Ok(DiagnosticsInfo {
        version: BUILD_VERSION.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        home: std::env::var("HOME").unwrap_or_default(),
        log_dir_writable: log_dir.as_deref().map(dir_writable),
        log_dir: log_dir.map(|p| p.display().to_string()),
        config_dir_writable: config_dir.as_deref().map(dir_writable),
        config_dir: config_dir.map(|p| p.display().to_string()),
        error_log: error_log.map(|p| p.display().to_string()),
        error_log_size: size,
        kernel_installed,
        kernel_version,
        service_status,
        service_config_dir,
        service_log_dir,
    })
}

/// 生成网络自检文本（供自检命令与诊断上传共用）。
async fn build_network_report(app: &AppHandle, frontend: Option<&str>) -> Result<String, String> {
    let mut out = format!(
        "app: {}\nos: {} ({})\ntun_fd: {}\n",
        BUILD_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        LAST_TUN_FD.load(std::sync::atomic::Ordering::Relaxed),
    );

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        out.push_str("--- vpn ---\nn/a (desktop: TUN 由系统守护进程创建)\n");
    }

    // 前端才有 VPN 运行时状态（实际下发的路由、dns、所属实例），由它一并带上来。
    // 注意：前端文本自带 `--- vpn ---` / `--- config ---` 标题，直接拼进来会
    // 和后端自己的段落混淆（排查时误判成「frontend 段为空」）。这里统一缩进并加闭合标记。
    if let Some(text) = frontend.filter(|s| !s.trim().is_empty()) {
        crate::logger::record("report.frontend", format!("{}B", text.len()));
        out.push_str(&format!("--- frontend ({} bytes) ---\n", text.len()));
        for line in text.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("--- /frontend ---\n");
    }

    if let Ok(cm) = get_client_manager!() {
        // 采集类 RPC 一律加超时：坏实例会把 RPC 永久挂住，
        // 进而让「上传诊断」永远不会返回（历史 bug：点了没反应）。
        match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            cm.handle_list_network_instance_ids(app.clone()),
        )
        .await
        {
            Ok(Ok(ids)) => {
                out.push_str(&format!(
                    "instances: {}\n",
                    serde_json::to_string(&ids).unwrap_or_default()
                ));
            }
            Ok(Err(e)) => out.push_str(&format!("list_instances failed: {e}\n")),
            Err(_) => out.push_str("list_instances timeout (3s)\n"),
        }

        {
            // 采集同样加超时：坏实例会让 RPC 永久挂住，进而让上传永不返回。
            let mut collected: Vec<_> = Vec::new();
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                cm.handle_collect_network_info(app.clone(), None),
            )
            .await
            {
                Ok(Ok(resp)) => {
                    if let Some(map) = resp.info.map(|m| m.map) {
                        collected.extend(map);
                    }
                }
                Ok(Err(e)) => out.push_str(&format!("collect failed: {e}\n")),
                Err(_) => out.push_str("collect timeout (5s): 实例采集已跳过\n"),
            }

            if collected.is_empty() {
                out.push_str("collect: <empty>\n");
            }
            for (inst_id, info) in collected {
                out.push_str(&format!("--- instance {inst_id} ---\n"));
                out.push_str(&format!("running: {}\n", info.running));
                if let Some(err) = info.error_msg.as_deref().filter(|s| !s.is_empty()) {
                    out.push_str(&format!("error_msg: {err}\n"));
                }

                if let Some(node) = info.my_node_info.as_ref() {
                    out.push_str(&format!(
                        "peer_id: {}\nhostname: {}\nversion: {}\n",
                        node.peer_id, node.hostname, node.version
                    ));
                    if let Some(v4) = node.virtual_ipv4.as_ref() {
                        let addr = v4
                            .address
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default();
                        out.push_str(&format!("virtual_ipv4: {addr}/{}\n", v4.network_length));
                    }
                    if let Some(ips) = node.ips.as_ref() {
                        if let Some(p) = ips.public_ipv4.as_ref() {
                            out.push_str(&format!("public_ipv4: {p}\n"));
                        }
                        if let Some(p) = ips.public_ipv6.as_ref() {
                            out.push_str(&format!("public_ipv6: {p}\n"));
                        }
                        let v4s: Vec<String> = ips
                            .interface_ipv4s
                            .iter()
                            .map(ToString::to_string)
                            .collect();
                        let v6s: Vec<String> = ips
                            .interface_ipv6s
                            .iter()
                            .map(ToString::to_string)
                            .collect();
                        out.push_str(&format!("interface_ipv4: [{}]\n", v4s.join(", ")));
                        out.push_str(&format!("interface_ipv6: [{}]\n", v6s.join(", ")));
                    }
                    if !node.listeners.is_empty() {
                        let listeners: Vec<String> =
                            node.listeners.iter().map(ToString::to_string).collect();
                        out.push_str(&format!("listeners: [{}]\n", listeners.join(", ")));
                    }
                }

                out.push_str(&format!(
                    "routes: {} (下发给 VpnService 的以 frontend 段为准)\n",
                    info.routes.len()
                ));
                for r in &info.routes {
                    let v4 = r
                        .ipv4_addr
                        .as_ref()
                        .map(|inet| {
                            let addr = inet
                                .address
                                .as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_default();
                            format!("{addr}/{}", inet.network_length)
                        })
                        .unwrap_or_default();
                    let v6 = r
                        .ipv6_addr
                        .as_ref()
                        .map(|inet| {
                            let addr = inet
                                .address
                                .as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_default();
                            format!("{addr}/{}", inet.network_length)
                        })
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "  peer={} v4={v4} v6={v6} cost={} next_hop={} hostname={} proxy=[{}]\n",
                        r.peer_id,
                        r.cost,
                        r.next_hop_peer_id,
                        r.hostname,
                        r.proxy_cidrs.join(", "),
                    ));
                }

                out.push_str(&format!("peers: {}\n", info.peers.len()));
                for p in &info.peers {
                    // `directly_connected_conns` 只统计「对方主动连进来」的直连，
                    // **不包含打洞连接**（内核 get_directly_connections 的实现），
                    // 所以 conns - direct 约等于打洞/中转连接数。这里把 default conn
                    // 的隧道类型也带上，避免再把「打洞直连」误判成「走中转」。
                    let total = p.conns.len();
                    let direct = p.directly_connected_conns.len();
                    let hole_like = total.saturating_sub(direct);
                    let default_id = p.default_conn_id.as_ref().map(ToString::to_string);
                    let default_kind = default_id
                        .as_ref()
                        .and_then(|id| {
                            p.conns.iter().find(|c| &c.conn_id == id).and_then(|c| {
                                c.tunnel
                                    .as_ref()
                                    .map(|t| t.tunnel_type.clone())
                                    .filter(|s| !s.is_empty())
                            })
                        })
                        .unwrap_or_else(|| "-".to_string());
                    out.push_str(&format!(
                        "  peer={} conns={} direct={} hole_punched~={} default={}\n",
                        p.peer_id, total, direct, hole_like, default_kind
                    ));
                }
            }
        }
    } else {
        out.push_str("client manager not initialized\n");
    }

    if let Ok(dir) = get_log_dir(app) {
        if let Ok(text) = std::fs::read_to_string(dir.join("events.log")) {
            let tail: Vec<&str> = text.lines().rev().take(512).collect();
            out.push_str("--- events ---\n");
            out.push_str(&tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
            out.push('\n');
        }
        if let Some(text) = kernel_log_text(&dir, 8 * 1024 * 1024) {
            let tail: Vec<&str> = text.lines().rev().take(2048).collect();
            out.push_str("--- kernel ---\n");
            out.push_str(&tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
            out.push('\n');
        }
    }

    logger::event("diag", "network_diagnostics requested");
    Ok(out)
}

/// 记录最终下发的 VPN 路由（移动端排查用）。
#[tauri::command]
fn log_vpn_routes(routes: Vec<String>) {
    let mut text = format!("count={} [", routes.len());
    for (i, r) in routes.iter().enumerate() {
        if i > 0 {
            text.push_str(", ");
        }
        text.push_str(r);
        if text.len() > 2048 {
            text.push_str(", …");
            break;
        }
    }
    text.push(']');
    logger::event("vpn", format!("routes {text}"));
}

/// 网络自检：实例状态 + 采集信息 + 最近事件/内核日志（排障用）。
#[tauri::command]
async fn network_diagnostics(app: AppHandle, frontend: Option<String>) -> Result<String, String> {
    build_network_report(&app, frontend.as_deref()).await
}
/// 前端启动状态上报（写进 `error.log` 的 `[boot.ui]`，便于对照模式/选网是否恢复）。
#[tauri::command]
fn report_boot_state(
    mode: String,
    last_instance: Option<String>,
    service_current: Option<bool>,
    mode_source: Option<String>,
) {
    logger::record(
        "boot.ui",
        format!(
            "mode={mode} mode_source={} last_instance={last_instance:?} service_current={service_current:?}",
            mode_source.as_deref().unwrap_or("-")
        ),
    );
}

/// 前端异常上报（`window.onerror` / `unhandledrejection` / Vue errorHandler / toast 错误）。
#[tauri::command]
fn report_frontend_error(scope: String, message: String, stack: Option<String>) {
    let scope = sanitize_scope(&scope);
    let mut text = message;
    if let Some(stack) = stack.filter(|s| !s.trim().is_empty()) {
        text.push_str(" | stack=");
        text.push_str(&stack);
    }
    logger::record(&scope, text);
}

/// 前端诊断链路埋点：写入 `events.log`。
///
/// 「点了没反应」这类问题在 `error.log` 里看不到（没有错误发生），
/// 所以把关键节点单独打点，用于判断卡在哪一步。
#[tauri::command]
fn log_diag_step(step: String, detail: Option<String>) {
    let detail = detail.unwrap_or_default();
    let message = if detail.trim().is_empty() {
        step
    } else {
        format!("{step} {detail}")
    };
    logger::event("fe:diag", message);
}

/// `error.log` 的绝对路径（未初始化时返回错误）。
#[tauri::command]
fn error_log_path(app: AppHandle) -> Result<String, String> {
    let path = ensure_error_log(&app).ok_or_else(|| {
        let candidates = error_log_candidates(&app)
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("error.log 无法写入（已尝试: {candidates}）。请检查目录属主是否为当前用户。")
    })?;
    Ok(path.display().to_string())
}

/// 读取 `error.log` 尾部（默认 256 KB），用于复制/预览。
#[tauri::command]
fn read_error_log(app: AppHandle, tail_kb: Option<usize>) -> Result<String, String> {
    ensure_error_log(&app);
    let limit = tail_kb.unwrap_or(256).clamp(1, 2048) * 1024;
    Ok(logger::tail(limit))
}

/// 找到 `<dir>/exvlan.log*` 里最新的一个（tracing 的 daily 轮转会在文件名后加日期）。
fn newest_kernel_log(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("exvlan.log"))
                .unwrap_or(false)
        })
        .filter_map(|p| {
            let modified = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((modified, p))
        })
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// 读取内核日志尾部：跨 `.log` / `.log.1` / `.log.2`…（按 mtime 从新到旧），
/// 跳过 0 字节文件。刚发生轮转时也不会得到空内容。
fn kernel_log_text(dir: &std::path::Path, max_bytes: usize) -> Option<String> {
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("exvlan.log"))
                .unwrap_or(false)
        })
        .filter_map(|p| {
            let meta = std::fs::metadata(&p).ok()?;
            if meta.len() == 0 {
                return None;
            }
            Some((meta.modified().ok()?, p))
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    files.sort_by_key(|(time, _)| std::cmp::Reverse(*time));

    let mut out = String::new();
    for (_, path) in files {
        let Some(text) = read_capped(&path, max_bytes) else {
            continue;
        };
        if out.is_empty() {
            out = text;
        } else {
            out = format!("{text}\n{out}");
        }
        if out.len() >= max_bytes {
            break;
        }
    }
    if out.len() > max_bytes {
        out = out.split_off(out.len() - max_bytes);
    }
    Some(out)
}

/// 诊断上传服务（Cloudflare Worker，见 packages/diagnostics）。
const DIAG_UPLOAD_URL: &str = "https://exvlan-diag.era-x.cn/";
/// 短期 token 端点（Worker 用 `TOKEN_SECRET` 签发，绑定请求 IP，10 分钟有效）。
///
/// 客户端**不再内置任何长期密钥**；token 取到后在有效期内复用。
const DIAG_TOKEN_URL: &str = "https://exvlan-diag.era-x.cn/token";

/// 单份日志进 zip 的上限（压缩后很小，但避免把 10MB 的轮转文件反复读进内存）。
const DIAG_LOG_CAP: usize = 8 * 1024 * 1024;

/// 读取文件尾部（最多 `cap` 字节，保证从行首开始，避免截断出半个字符）。
fn read_capped(path: &std::path::Path, cap: usize) -> Option<String> {
    let mut bytes = std::fs::read(path).ok()?;
    let truncated = bytes.len() > cap;
    if truncated {
        bytes = bytes.split_off(bytes.len() - cap);
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if truncated && let Some(pos) = text.find('\n') {
        text = text[pos + 1..].to_owned();
    }
    Some(text)
}

/// 诊断 zip 的输出目录（唯一落点：`<用户日志目录>/diagnostics`）。
///
/// 不再有多级 fallback —— 产物落点必须可预期，否则「把诊断包发我」时
/// 用户和我都找不到文件。目录不可写时直接报错并记日志。
fn diagnostics_output_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = crate::paths::diagnostics_dir(app)?;
    match probe_writable(&dir) {
        Ok(()) => Ok(dir),
        Err(err) => {
            logger::record(
                "diag.dir",
                format!("诊断目录不可写 {}: {err}", dir.display()),
            );
            Err(format!("无法创建诊断目录（{}: {err}）", dir.display()))
        }
    }
}

/// 建目录 + 写探针文件，确认真的可写。
fn probe_writable(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(".write-probe");
    std::fs::write(&probe, b"ok")?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// 打包诊断 zip：error.log + 内核日志 + events.log + 网络自检 + ui-state + 摘要。
/// 传入 `network_report` 时一并写入（上传场景会用前端状态生成完整版）。
fn build_diagnostics_zip(
    app: &AppHandle,
    network_report: &str,
) -> Result<std::path::PathBuf, String> {
    use std::io::Write as _;
    use tauri::Manager as _;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let base = diagnostics_output_dir(app)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let path = base.join(format!("exvlan-diagnostics-{stamp}.zip"));

    let file = std::fs::File::create(&path).map_err(|e| format!("创建 zip 失败: {e}"))?;
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // 1) error.log（完整当前文件，脱敏）
    if let Some(log) = logger::path()
        && let Some(text) = read_capped(&log, DIAG_LOG_CAP)
    {
        let _ = zip.start_file("error.log", opts);
        let _ = zip.write_all(logger::redact(&text).as_bytes());
    }

    // 2) GUI 日志（界面/上传/模式/服务安装都在这里）
    if let Ok(dir) = get_log_dir(app)
        && let Some(text) = read_capped(&dir.join("studio.log"), DIAG_LOG_CAP)
    {
        let _ = zip.start_file("studio.log", opts);
        let _ = zip.write_all(logger::redact(&text).as_bytes());
    }

    // 3) 内核日志（peer/握手/TUN/路由）—— 注意在**系统**日志目录，不是用户目录
    {
        let sys_logs = crate::paths::system().logs;
        if let Some(text) = kernel_log_text(&sys_logs, DIAG_LOG_CAP) {
            let _ = zip.start_file("exvlan.log", opts);
            let _ = zip.write_all(logger::redact(&text).as_bytes());
        }
    }

    // 4) prefs.toml（脱敏整份：用于判断“模式/上次网络到底存没存”）
    if let Ok(prefs_path) = crate::prefs::prefs_path(app)
        && let Some(text) = read_capped(&prefs_path, 256 * 1024)
    {
        let _ = zip.start_file("prefs.toml", opts);
        let _ = zip.write_all(logger::redact(&text).as_bytes());
    }

    // 5) events.log（GUI 事件流：VPN 路由、诊断请求都在这里）
    if let Ok(dir) = get_log_dir(app)
        && let Some(text) = read_capped(&dir.join("events.log"), DIAG_LOG_CAP)
    {
        let _ = zip.start_file("events.log", opts);
        let _ = zip.write_all(logger::redact(&text).as_bytes());
    }

    // 6) helper.log（特权 helper：安装/apply/自更新/授权失败都记在这里）
    {
        let helper_log = crate::paths::helper_log();
        if let Some(text) = read_capped(&helper_log, DIAG_LOG_CAP) {
            let _ = zip.start_file("helper.log", opts);
            let _ = zip.write_all(logger::redact(&text).as_bytes());
        }
    }

    // 7) 网络自检（结构化：实例/节点/路由/接口/VPN 状态）
    if !network_report.trim().is_empty() {
        let _ = zip.start_file("network-report.txt", opts);
        let _ = zip.write_all(logger::redact(network_report).as_bytes());
    }

    // 8) ui-state.json（旧版遗留，若存在则一并带上）
    if let Ok(dir) = app.path().app_config_dir()
        && let Ok(text) = std::fs::read_to_string(dir.join("ui-state.json"))
    {
        let _ = zip.start_file("ui-state.json", opts);
        let _ = zip.write_all(logger::redact(&text).as_bytes());
    }

    // 9) 摘要
    let summary = format!(
        "version: {}\nos: {} ({})\nhome: {}\nlog_dir: {:?}\nconfig_dir: {:?}\nerror_log: {:?}\n",
        BUILD_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::var("HOME").unwrap_or_default(),
        app.path()
            .app_log_dir()
            .ok()
            .map(|p| p.display().to_string()),
        app.path()
            .app_config_dir()
            .ok()
            .map(|p| p.display().to_string()),
        logger::path().map(|p| p.display().to_string()),
    );
    let _ = zip.start_file("summary.txt", opts);
    let _ = zip.write_all(summary.as_bytes());

    zip.finish().map_err(|e| format!("写入 zip 失败: {e}"))?;
    Ok(path)
}

/// 上传诊断到私有仓库（Cloudflare Worker → exvlan/diagnostics issue + KV 里的完整 zip）。
///
/// 三层兜底，任一层成功即返回：
/// A 直连（`no_proxy` + IPv4 优先 + 固定 resolve）→ B 允许系统代理 → C 系统 `curl`。
///
/// 每层都写 `[upload]` 日志（含耗时），失败时错误文本带上 zip 路径，
/// 前端据此提示并复制路径 —— 不再出现「点了没反应」。
#[tauri::command]
async fn upload_diagnostics(
    app: AppHandle,
    frontend: Option<String>,
    reason: Option<String>,
) -> Result<String, String> {
    use tauri::Manager as _;

    let started = std::time::Instant::now();
    logger::record(
        "upload",
        format!(
            "start frontend={}B url={DIAG_UPLOAD_URL}zip",
            frontend.as_deref().map(str::len).unwrap_or(0)
        ),
    );

    let network_report = match build_network_report(&app, frontend.as_deref()).await {
        Ok(report) => report,
        Err(err) => {
            // 不再 `unwrap_or_default()` 静默吞掉：采集失败必须写进报告，
            // 否则诊断包只会缺一大段内容，看不出是"没采到"还是"没问题"。
            logger::record("diag.collect", format!("network_report failed: {err}"));
            format!("[collect_errors]\nnetwork_report failed: {err}\n")
        }
    };
    let zip_path = build_diagnostics_zip(&app, &network_report)?;
    let zip_bytes = std::fs::read(&zip_path).map_err(|e| format!("读取诊断包失败: {e}"))?;
    let zip_size = zip_bytes.len();
    logger::record(
        "upload",
        format!(
            "zip ok path={} size={zip_size}B in {}ms",
            zip_path.display(),
            started.elapsed().as_millis()
        ),
    );

    let summary = format!(
        "app: {}\nos: {} ({})\nmode: {}\nlog_dir: {:?}\nzip: {} bytes",
        BUILD_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        app.path()
            .app_config_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        get_log_dir(&app).ok().map(|p| p.display().to_string()),
        zip_size,
    );

    let mut upload_url = url::Url::parse(&format!("{DIAG_UPLOAD_URL}zip"))
        .map_err(|e| logger::other("cmd:upload_diagnostics", e.to_string()))?;
    upload_url
        .query_pairs_mut()
        .append_pair("version", BUILD_VERSION)
        .append_pair("os", std::env::consts::OS)
        .append_pair("arch", std::env::consts::ARCH)
        .append_pair("summary", summary.as_str());
    upload_url.query_pairs_mut().append_pair(
        "reason",
        match reason.as_deref() {
            Some("install-failed") => "install-failed",
            _ => "manual",
        },
    );

    // 外层看门狗：无论哪一层卡住，60s 内必须给出结果。
    let body = zip_bytes;
    let upload_url_for_task = upload_url.clone();
    let zip_path_for_task = zip_path.clone();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        upload_with_fallback(&upload_url_for_task, body, &zip_path_for_task),
    )
    .await;

    let body_json = match outcome {
        Ok(Ok(json)) => json,
        Ok(Err(err)) => {
            logger::record(
                "upload",
                format!("failed in {}ms: {err}", started.elapsed().as_millis()),
            );
            return Err(format!(
                "上传失败：{err}；诊断包已保存在 {}",
                zip_path.display()
            ));
        }
        Err(_) => {
            logger::record(
                "upload",
                format!("failed in {}ms: 总超时 60s", started.elapsed().as_millis()),
            );
            return Err(format!(
                "上传超时（60s）；诊断包已保存在 {}",
                zip_path.display()
            ));
        }
    };

    let mut links = Vec::new();
    if let Some(arr) = body_json.get("urls").and_then(|v| v.as_array()) {
        links.extend(arr.iter().filter_map(|v| v.as_str()).map(str::to_owned));
    }
    if let Some(zip_url) = body_json.get("zipUrl").and_then(|v| v.as_str()) {
        links.push(format!("zip: {zip_url}"));
    }
    let text = links.join("\n");
    logger::event(
        "upload",
        format!(
            "diagnostics uploaded ({zip_size} bytes, {}ms): {text}",
            started.elapsed().as_millis()
        ),
    );
    Ok(text)
}

/// 解析上传目标地址（IPv4 优先，避免 IPv6 黑洞导致连接挂住）。
async fn resolve_upload_addrs(
    host: &str,
    port: u16,
) -> (Vec<std::net::IpAddr>, Vec<std::net::IpAddr>) {
    match tokio::net::lookup_host((host, port)).await {
        Ok(iter) => {
            let (mut v4, mut v6) = (Vec::new(), Vec::new());
            for addr in iter {
                match addr.ip() {
                    std::net::IpAddr::V4(_) => v4.push(addr.ip()),
                    std::net::IpAddr::V6(_) => v6.push(addr.ip()),
                }
            }
            logger::record("upload", format!("resolve host={host} v4={v4:?} v6={v6:?}"));
            (v4, v6)
        }
        Err(err) => {
            logger::record("upload", format!("resolve host={host} failed: {err}"));
            (Vec::new(), Vec::new())
        }
    }
}

/// 构造上传用 client。`use_proxy=false` 时显式绕过系统代理。
fn build_upload_client(
    use_proxy: bool,
    ipv4: &[std::net::IpAddr],
    host: &str,
    timeout_secs: u64,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(5));
    if !use_proxy {
        builder = builder.no_proxy();
    }
    if !ipv4.is_empty() {
        let addrs: Vec<std::net::SocketAddr> = ipv4
            .iter()
            .map(|ip| std::net::SocketAddr::new(*ip, 443))
            .collect();
        builder = builder.resolve_to_addrs(host, &addrs);
    }
    builder.build().map_err(|e| e.to_string())
}

async fn try_upload(
    client: &reqwest::Client,
    url: &url::Url,
    body: &[u8],
    token: &str,
) -> Result<serde_json::Value, String> {
    let resp = client
        .post(url.clone())
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/zip")
        .body(body.to_vec())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {json}"));
    }
    Ok(json)
}

/// token 缓存：`(token, 过期时刻)`。有效期内复用，过期或 401 后重取。
static DIAG_TOKEN_CACHE: once_cell::sync::Lazy<
    std::sync::Mutex<Option<(String, std::time::Instant)>>,
> = once_cell::sync::Lazy::new(|| std::sync::Mutex::new(None));

const DIAG_TOKEN_TTL_SECS: u64 = 540; // 服务端 600s，留 60s 余量

fn cached_token() -> Option<String> {
    let guard = DIAG_TOKEN_CACHE.lock().ok()?;
    let (token, expires) = guard.as_ref()?;
    if std::time::Instant::now() < *expires {
        Some(token.clone())
    } else {
        None
    }
}

fn cache_token(token: &str) {
    if let Ok(mut guard) = DIAG_TOKEN_CACHE.lock() {
        *guard = Some((
            token.to_string(),
            std::time::Instant::now() + std::time::Duration::from_secs(DIAG_TOKEN_TTL_SECS),
        ));
    }
}

fn invalidate_token() {
    if let Ok(mut guard) = DIAG_TOKEN_CACHE.lock() {
        *guard = None;
    }
}

fn parse_token(json: &serde_json::Value) -> Result<String, String> {
    json.get("token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("token 响应缺少 token 字段: {json}"))
}

async fn fetch_token_with(client: &reqwest::Client) -> Result<String, String> {
    let resp = client
        .get(DIAG_TOKEN_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {json}"));
    }
    parse_token(&json)
}

/// 取 token：命中缓存直接返回；否则直连 → 系统代理 → curl。
async fn diag_token() -> Result<String, String> {
    if let Some(token) = cached_token() {
        return Ok(token);
    }
    let host = url::Url::parse(DIAG_TOKEN_URL)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    let (ipv4, _) = resolve_upload_addrs(&host, 443).await;

    for (label, use_proxy) in [("A(direct)", false), ("B(proxy)", true)] {
        let started = std::time::Instant::now();
        let client = match build_upload_client(use_proxy, &ipv4, &host, 8) {
            Ok(c) => c,
            Err(err) => {
                logger::record(
                    "upload",
                    format!("token {label} client build failed: {err}"),
                );
                continue;
            }
        };
        match fetch_token_with(&client).await {
            Ok(token) => {
                logger::record(
                    "upload",
                    format!("token {label} ok in {}ms", started.elapsed().as_millis()),
                );
                cache_token(&token);
                return Ok(token);
            }
            Err(err) => logger::record(
                "upload",
                format!(
                    "token {label} failed in {}ms: {err}",
                    started.elapsed().as_millis()
                ),
            ),
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let started = std::time::Instant::now();
        match curl_token().await {
            Ok(token) => {
                logger::record(
                    "upload",
                    format!("token C(curl) ok in {}ms", started.elapsed().as_millis()),
                );
                cache_token(&token);
                return Ok(token);
            }
            Err(err) => logger::record(
                "upload",
                format!(
                    "token C(curl) failed in {}ms: {err}",
                    started.elapsed().as_millis()
                ),
            ),
        }
    }

    Err("无法获取上传 token（直连/代理/curl 均失败）".to_string())
}

/// 上传一次（A 直连 → B 系统代理 → C 系统 curl）。
async fn upload_with_fallback(
    url: &url::Url,
    body: Vec<u8>,
    zip_path: &std::path::Path,
) -> Result<serde_json::Value, String> {
    let mut refreshed = false;
    loop {
        match upload_once(url, &body, zip_path).await {
            Ok(json) => return Ok(json),
            Err(err) => {
                // token 绑定 IP 且 10 分钟过期：切网/超时后服务端会返回 401，
                // 这里作废缓存并重试一次（只重试一次，避免打转）。
                let auth_failed =
                    err.contains("401") || err.contains("expired") || err.contains("ip-mismatch");
                if !refreshed && auth_failed {
                    invalidate_token();
                    refreshed = true;
                    logger::record("upload", format!("token 失效，重取后重试: {err}"));
                    continue;
                }
                return Err(err);
            }
        }
    }
}

/// 单轮上传：A 直连 → B 系统代理 → C 系统 curl。
async fn upload_once(
    url: &url::Url,
    body: &[u8],
    zip_path: &std::path::Path,
) -> Result<serde_json::Value, String> {
    let host = url.host_str().unwrap_or_default().to_string();
    let (ipv4, _ipv6) = resolve_upload_addrs(&host, 443).await;

    let token = diag_token().await?;

    let started = std::time::Instant::now();
    match build_upload_client(false, &ipv4, &host, 8) {
        Ok(client) => match try_upload(&client, url, body, &token).await {
            Ok(json) => {
                logger::record(
                    "upload",
                    format!("try A(direct) ok in {}ms", started.elapsed().as_millis()),
                );
                return Ok(json);
            }
            Err(err) => logger::record(
                "upload",
                format!(
                    "try A(direct) failed in {}ms: {err}",
                    started.elapsed().as_millis()
                ),
            ),
        },
        Err(err) => logger::record("upload", format!("try A client build failed: {err}")),
    }

    let started = std::time::Instant::now();
    match build_upload_client(true, &ipv4, &host, 25) {
        Ok(client) => match try_upload(&client, url, body, &token).await {
            Ok(json) => {
                logger::record(
                    "upload",
                    format!("try B(proxy) ok in {}ms", started.elapsed().as_millis()),
                );
                return Ok(json);
            }
            Err(err) => logger::record(
                "upload",
                format!(
                    "try B(proxy) failed in {}ms: {err}",
                    started.elapsed().as_millis()
                ),
            ),
        },
        Err(err) => logger::record("upload", format!("try B client build failed: {err}")),
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let started = std::time::Instant::now();
        match curl_upload(url.as_str(), zip_path, &token).await {
            Ok(json) => {
                logger::record(
                    "upload",
                    format!("try C(curl) ok in {}ms", started.elapsed().as_millis()),
                );
                return Ok(json);
            }
            Err(err) => logger::record(
                "upload",
                format!(
                    "try C(curl) failed in {}ms: {err}",
                    started.elapsed().as_millis()
                ),
            ),
        }
    }

    Err("直连 / 系统代理 / curl 均失败".to_string())
}

/// 兜底通道：调用系统 curl 上传同一个 zip（桌面端实测可用）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn curl_upload(
    url: &str,
    zip_path: &std::path::Path,
    token: &str,
) -> Result<serde_json::Value, String> {
    let out = tokio::process::Command::new("curl")
        .args(["-sS", "--max-time", "60", "-X", "POST", url])
        .arg("-H")
        .arg(format!("authorization: Bearer {token}"))
        .arg("-H")
        .arg("content-type: application/zip")
        .arg("--data-binary")
        .arg(format!("@{}", zip_path.display()))
        .output()
        .await
        .map_err(|e| format!("spawn curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl exit {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("parse curl body: {e}"))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn curl_token() -> Result<String, String> {
    let out = tokio::process::Command::new("curl")
        .args(["-sS", "--max-time", "15", DIAG_TOKEN_URL])
        .output()
        .await
        .map_err(|e| format!("spawn curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl exit {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("parse curl body: {e}"))?;
    parse_token(&json)
}

/// 打开诊断包目录（上传失败时前端兜底：路径已复制，用户可一键打开）。
#[tauri::command]
fn open_diagnostics_dir(app: AppHandle) -> Result<String, String> {
    let dir = diagnostics_output_dir(&app)?;
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return Err("移动端请使用系统的「分享」入口".to_string());

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        cmd.arg(&dir);
        cmd.spawn().map_err(|e| format!("打开目录失败: {e}"))?;
        Ok(dir.display().to_string())
    }
}

/// 分享诊断包：Android 弹系统分享面板（zip）；桌面返回路径（前端打开目录）。
#[tauri::command]
async fn share_error_log(app: AppHandle) -> Result<String, String> {
    ensure_error_log(&app);
    let report = build_network_report(&app, None).await.unwrap_or_default();

    // zip 建不出来（目录不可写/磁盘满）时降级为分享原始 error.log，
    // 而不是让「分享」整体失败。
    let (path, mime, title) = match build_diagnostics_zip(&app, &report) {
        Ok(path) => (path, "application/zip", "Exvlan 诊断日志"),
        Err(err) => {
            logger::record("share", format!("zip failed: {err}; fallback to error.log"));
            match logger::path() {
                Some(path) => (path, "text/plain", "Exvlan error.log"),
                None => return Err(err),
            }
        }
    };

    #[cfg(target_os = "android")]
    {
        use tauri_plugin_vpnservice::VpnserviceExt;
        app.vpnservice()
            .share_file(tauri_plugin_vpnservice::models::ShareFileRequest {
                path: path.display().to_string(),
                mime: Some(mime.to_string()),
                title: Some(title.to_string()),
            })
            .map_err(|e| logger::other("share_error_log", e.to_string()))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (&app, mime, title);
    }
    Ok(path.display().to_string())
}

#[cfg(not(target_os = "android"))]
fn toggle_window_visibility(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or_default();
        let minimized = window.is_minimized().unwrap_or_default();
        let focused = window.is_focused().unwrap_or_default();

        let should_show = !visible || minimized || !focused;
        if should_show {
            if !visible {
                let _ = window.show();
            }
            if minimized {
                let _ = window.unminimize();
            }
            if !focused {
                let _ = window.set_focus();
            }
            let _ = set_dock_visibility(app.clone(), true);
        } else {
            let _ = window.hide();
            let _ = set_dock_visibility(app.clone(), false);
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod manager {
    use super::*;
    use async_trait::async_trait;
    use dashmap::{DashMap, DashSet};
    #[cfg(any(target_os = "android", target_os = "ios"))]
    use exvlan_rpc::exvlan_core::config::api_input::{NetworkConfig, NetworkConfigExt};
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    use exvlan_rpc::exvlan_core::connectivity::protocol::raw::TunnelDialer as _;
    use exvlan_rpc::exvlan_core::management::remote_client::PersistentConfig;
    use exvlan_rpc::exvlan_core::rpc::bidirect::BidirectRpcManager;
    use exvlan_rpc::exvlan_proto::api::config::ConfigRpc;
    use exvlan_rpc::exvlan_proto::api::logger::{LoggerRpc, SetLoggerConfigRequest};
    use exvlan_rpc::exvlan_proto::api::manage::RunNetworkInstanceRequest;
    use exvlan_rpc::exvlan_proto::rpc_types::controller::BaseController;
    use std::sync::Arc;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    #[derive(Default)]
    pub(super) enum PersistedConfigSource {
        User,
        #[serde(alias = "webhook")]
        Web,
        #[serde(other)]
        #[default]
        Legacy,
    }

    impl PersistedConfigSource {
        pub(super) fn from_runtime_source(source: ConfigSource) -> Self {
            match source {
                ConfigSource::User => Self::User,
                ConfigSource::Web => Self::Web,
            }
        }

        fn merge_persisted(self, incoming: Self) -> Self {
            match (self, incoming) {
                // Older runtimes report missing source as `user`. Keep the stronger persisted
                // ownership until web sync or an explicit user save repairs it.
                (Self::Web, Self::User) | (Self::Legacy, Self::User) => self,
                (_, next) => next,
            }
        }

        fn to_runtime_source(self) -> ConfigSource {
            match self {
                Self::User | Self::Legacy => ConfigSource::User,
                Self::Web => ConfigSource::Web,
            }
        }

        #[cfg(any(test, target_os = "android"))]
        fn is_web_like(self) -> bool {
            matches!(self, Self::Web)
        }
    }

    #[derive(Clone)]
    pub(super) struct GUIConfig {
        inst_id: String,
        pub(crate) config: NetworkConfig,
        source: PersistedConfigSource,
    }

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    pub(super) struct StoredGuiConfig {
        config: NetworkConfig,
        #[serde(default)]
        source: PersistedConfigSource,
    }

    impl GUIConfig {
        fn new(inst_id: String, config: NetworkConfig, source: PersistedConfigSource) -> Self {
            Self {
                inst_id,
                config,
                source,
            }
        }

        fn into_stored(self) -> StoredGuiConfig {
            StoredGuiConfig {
                config: self.config,
                source: self.source,
            }
        }
    }

    impl PersistentConfig<anyhow::Error> for GUIConfig {
        fn get_network_inst_id(&self) -> &str {
            &self.inst_id
        }
        fn get_network_config(&self) -> Result<NetworkConfig, anyhow::Error> {
            Ok(self.config.clone())
        }
        fn get_network_config_source(&self) -> ConfigSource {
            self.source.to_runtime_source()
        }
    }

    pub(super) struct GUIStorage {
        network_configs: DashMap<Uuid, GUIConfig>,
        enabled_networks: DashSet<Uuid>,
    }
    impl GUIStorage {
        fn new() -> Self {
            Self {
                network_configs: DashMap::new(),
                enabled_networks: DashSet::new(),
            }
        }

        fn save_configs(&self, app: &AppHandle) -> anyhow::Result<()> {
            let configs = self
                .network_configs
                .iter()
                .map(|entry| entry.value().clone().into_stored())
                .collect::<Vec<_>>();
            app.emit("save_configs", configs)?;
            Ok(())
        }

        fn save_enabled_networks(&self, app: &AppHandle) -> anyhow::Result<()> {
            let payload: Vec<String> = self
                .enabled_networks
                .iter()
                .map(|entry| entry.key().to_string())
                .collect();
            app.emit("save_enabled_networks", payload)?;
            Ok(())
        }

        fn save_config(
            &self,
            app: &AppHandle,
            inst_id: Uuid,
            cfg: NetworkConfig,
            source: PersistedConfigSource,
        ) -> anyhow::Result<()> {
            let source = self
                .network_configs
                .get(&inst_id)
                .map(|existing| existing.source.merge_persisted(source))
                .unwrap_or(source);
            let config = GUIConfig::new(inst_id.to_string(), cfg, source);
            self.network_configs.insert(inst_id, config);
            self.save_configs(app)
        }
    }
    #[async_trait]
    impl Storage<AppHandle, GUIConfig, anyhow::Error> for GUIStorage {
        async fn insert_or_update_user_network_config(
            &self,
            app: AppHandle,
            network_inst_id: Uuid,
            network_config: NetworkConfig,
            source: ConfigSource,
        ) -> Result<(), anyhow::Error> {
            self.save_config(
                &app,
                network_inst_id,
                network_config,
                PersistedConfigSource::from_runtime_source(source),
            )?;
            self.enabled_networks.insert(network_inst_id);
            self.save_enabled_networks(&app)?;
            Ok(())
        }

        async fn delete_network_configs(
            &self,
            app: AppHandle,
            network_inst_ids: &[Uuid],
        ) -> Result<(), anyhow::Error> {
            for network_inst_id in network_inst_ids {
                self.network_configs.remove(network_inst_id);
                self.enabled_networks.remove(network_inst_id);
            }
            self.save_configs(&app)?;
            self.save_enabled_networks(&app)?;
            Ok(())
        }

        async fn update_network_config_state(
            &self,
            app: AppHandle,
            network_inst_id: Uuid,
            disabled: bool,
        ) -> Result<(), anyhow::Error> {
            if disabled {
                self.enabled_networks.remove(&network_inst_id);
            } else {
                self.enabled_networks.insert(network_inst_id);
            }
            self.save_enabled_networks(&app)?;
            Ok(())
        }

        async fn list_network_configs(
            &self,
            _: AppHandle,
            props: ListNetworkProps,
        ) -> Result<Vec<GUIConfig>, anyhow::Error> {
            let mut ret = Vec::new();
            for entry in self.network_configs.iter() {
                let id: Uuid = entry.key().to_owned();
                match props {
                    ListNetworkProps::All => {
                        ret.push(entry.value().clone());
                    }
                    ListNetworkProps::EnabledOnly => {
                        if self.enabled_networks.contains(&id) {
                            ret.push(entry.value().clone());
                        }
                    }
                    ListNetworkProps::DisabledOnly => {
                        if !self.enabled_networks.contains(&id) {
                            ret.push(entry.value().clone());
                        }
                    }
                }
            }
            Ok(ret)
        }

        async fn get_network_config(
            &self,
            _: AppHandle,
            network_inst_id: &str,
        ) -> Result<Option<GUIConfig>, anyhow::Error> {
            let uuid = Uuid::parse_str(network_inst_id)?;
            Ok(self
                .network_configs
                .get(&uuid)
                .map(|entry| entry.value().clone()))
        }
    }

    pub(super) struct GUIClientManager {
        pub(super) storage: GUIStorage,
        pub(super) rpc_manager: Arc<BidirectRpcManager>,
        /// RPC 客户端来源：桌面走 tunnel，移动端走 FFI（call_json_rpc）。
        pub(super) transport: Box<dyn exvlan_rpc::transport::RpcTransport>,
        pub(super) config_mutation: Mutex<()>,
    }
    impl GUIClientManager {
        /// 桌面：连本地内核守护进程（tcp RPC），transport = TunnelTransport。
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        pub async fn new(rpc_url: Option<String>) -> Result<Self, anyhow::Error> {
            let url = rpc_url.context("desktop 端必须提供 rpc url（本地内核守护进程）")?;
            let tunnel = exvlan_rpc::rpc::runtime_rpc_dialer(url.parse()?)
                .connect()
                .await?;

            let rpc_manager = Arc::new(BidirectRpcManager::new());
            rpc_manager.run_with_tunnel(tunnel);
            let transport: Box<dyn exvlan_rpc::transport::RpcTransport> =
                Box::new(exvlan_rpc::transport::TunnelTransport {
                    rpc_manager: rpc_manager.clone(),
                });

            Ok(Self {
                storage: GUIStorage::new(),
                rpc_manager,
                transport,
                config_mutation: Mutex::new(()),
            })
        }

        /// 移动端：内核在应用进程内，transport = FfiTransport（call_json_rpc）。
        #[cfg(any(target_os = "android", target_os = "ios"))]
        pub fn new_ffi() -> Self {
            Self {
                storage: GUIStorage::new(),
                rpc_manager: Arc::new(BidirectRpcManager::new()),
                transport: Box::new(exvlan_rpc::transport::FfiTransport),
                config_mutation: Mutex::new(()),
            }
        }

        pub fn get_enabled_instances_with_tun_ids(&self) -> impl Iterator<Item = uuid::Uuid> + '_ {
            self.storage
                .network_configs
                .iter()
                .filter(|v| self.storage.enabled_networks.contains(v.key()))
                .filter(|v| !v.config.no_tun())
                .filter_map(|c| c.config.instance_id().parse::<uuid::Uuid>().ok())
        }

        #[cfg(target_os = "android")]
        pub fn get_enabled_instances_with_web_like_tun_ids(
            &self,
        ) -> impl Iterator<Item = uuid::Uuid> + '_ {
            self.storage
                .network_configs
                .iter()
                .filter(|v| self.storage.enabled_networks.contains(v.key()))
                .filter(|v| !v.config.no_tun())
                .filter(|v| v.source.is_web_like())
                .filter_map(|c| c.config.instance_id().parse::<uuid::Uuid>().ok())
        }

        #[cfg(target_os = "android")]
        pub(super) async fn disable_instances_with_tun(
            &self,
            app: &AppHandle,
            web_only: bool,
        ) -> Result<
            (),
            exvlan_rpc::exvlan_core::management::remote_client::RemoteClientError<anyhow::Error>,
        > {
            let inst_ids: Vec<uuid::Uuid> = if web_only {
                self.get_enabled_instances_with_web_like_tun_ids().collect()
            } else {
                self.get_enabled_instances_with_tun_ids().collect()
            };
            for inst_id in inst_ids {
                self.handle_update_network_state(app.clone(), inst_id, true)
                    .await?;
            }
            Ok(())
        }

        pub(super) fn notify_vpn_stop_if_no_tun(&self, app: &AppHandle) -> Result<(), String> {
            let has_tun = self.get_enabled_instances_with_tun_ids().any(|_| true);
            if !has_tun {
                app.emit("vpn_service_stop", "").map_err(|e| {
                    logger::other("cmd:get_enabled_instances_with_web_like_tun_ids", e)
                })?;
            }
            Ok(())
        }

        pub(super) async fn pre_run_network_instance_hook(
            &self,
            app: &AppHandle,
            cfg: &exvlan_rpc::exvlan_core::config::toml::TomlConfigLoader,
            source: PersistedConfigSource,
        ) -> Result<(), String> {
            let instance_id = cfg.get_id();
            app.emit("pre_run_network_instance", instance_id.to_string())
                .map_err(|e| logger::other("cmd:get_enabled_instances_with_web_like_tun_ids", e))?;

            #[cfg(target_os = "android")]
            if !cfg.get_flags().no_tun {
                match source {
                    PersistedConfigSource::User | PersistedConfigSource::Legacy => {
                        self.disable_instances_with_tun(app, false)
                            .await
                            .map_err(|e| {
                                logger::other("cmd:get_enabled_instances_with_web_like_tun_ids", e)
                            })?;
                    }
                    PersistedConfigSource::Web => {
                        self.disable_instances_with_tun(app, true)
                            .await
                            .map_err(|e| {
                                logger::other("cmd:get_enabled_instances_with_web_like_tun_ids", e)
                            })?;
                        if self.get_enabled_instances_with_tun_ids().next().is_some() {
                            return Err(
                                "Android only supports one active TUN network; user-managed VPN remains active"
                                    .to_string(),
                            );
                        }
                    }
                }
            }

            self.storage
                .save_config(
                    app,
                    instance_id,
                    NetworkConfig::new_from_config(cfg).map_err(|e| {
                        logger::other("cmd:get_enabled_instances_with_web_like_tun_ids", e)
                    })?,
                    source,
                )
                .map_err(|e| logger::other("cmd:get_enabled_instances_with_web_like_tun_ids", e))?;

            Ok(())
        }

        pub(super) async fn post_run_network_instance_hook(
            &self,
            app: &AppHandle,
            instance_id: &uuid::Uuid,
        ) -> Result<(), String> {
            self.storage.enabled_networks.insert(*instance_id);

            app.emit("post_run_network_instance", instance_id.to_string())
                .map_err(|e| logger::other("cmd:get_enabled_instances_with_web_like_tun_ids", e))?;

            Ok(())
        }

        #[cfg(any(target_os = "android", target_os = "ios"))]
        pub(super) async fn post_remote_remove_network_instances_hook(
            &self,
            app: &AppHandle,
            ids: &[uuid::Uuid],
        ) -> Result<(), String> {
            self.storage
                .delete_network_configs(app.clone(), ids)
                .await
                .map_err(|e| logger::other("cmd:get_enabled_instances_with_web_like_tun_ids", e))?;
            self.notify_vpn_stop_if_no_tun(app)?;
            Ok(())
        }

        pub(super) async fn post_stop_network_instances_hook(
            &self,
            app: &AppHandle,
        ) -> Result<(), String> {
            self.notify_vpn_stop_if_no_tun(app)?;
            Ok(())
        }

        fn get_logger_rpc_client(
            &self,
        ) -> Option<Box<dyn LoggerRpc<Controller = BaseController> + Send>> {
            Some(self.transport.logger())
        }

        pub(super) async fn set_logging_level(&self, level: String) -> Result<(), anyhow::Error> {
            let logger_rpc = self
                .get_logger_rpc_client()
                .ok_or_else(|| anyhow::anyhow!("Logger RPC client not available"))?;
            logger_rpc
                .set_logger_config(
                    BaseController::default(),
                    SetLoggerConfigRequest {
                        level: exvlan_rpc::exvlan_core::management::parse_log_level(&level).into(),
                    },
                )
                .await?;
            Ok(())
        }

        pub(super) async fn load_configs(
            &self,
            app: AppHandle,
            configs: Vec<StoredGuiConfig>,
            enabled_networks: Vec<String>,
        ) -> anyhow::Result<()> {
            self.storage.network_configs.clear();
            for stored in configs {
                let instance_id = stored.config.instance_id();
                self.storage.network_configs.insert(
                    instance_id.parse()?,
                    GUIConfig::new(instance_id.to_string(), stored.config, stored.source),
                );
            }

            self.storage.enabled_networks.clear();
            let client = self
                .get_rpc_client(app.clone())
                .ok_or_else(|| anyhow::anyhow!("RPC client not found"))?;
            for id in enabled_networks {
                if let Ok(uuid) = id.parse()
                    && !self.storage.enabled_networks.contains(&uuid)
                {
                    let config = self
                        .storage
                        .network_configs
                        .get(&uuid)
                        .map(|i| (i.value().config.clone(), i.value().source));
                    let Some((config, source)) = config else {
                        continue;
                    };
                    let toml_config = config.gen_config()?;
                    self.pre_run_network_instance_hook(&app, &toml_config, source)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?;
                    client
                        .run_network_instance(
                            BaseController::default(),
                            RunNetworkInstanceRequest {
                                inst_id: None,
                                config: Some(config),
                                overwrite: false,
                                source: config_source_to_rpc(source.to_runtime_source()),
                            },
                        )
                        .await?;
                    self.post_run_network_instance_hook(&app, &uuid)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?;
                }
            }
            Ok(())
        }
    }
    use exvlan_rpc::exvlan_proto::api::manage::WebClientServiceClientFactory;

    impl RemoteClientManager<AppHandle, GUIConfig, anyhow::Error> for GUIClientManager {
        fn get_config_rpc_client(
            &self,
            _: AppHandle,
        ) -> Option<Box<dyn ConfigRpc<Controller = BaseController> + Send>> {
            Some(self.transport.config())
        }

        fn get_rpc_client(
            &self,
            _: AppHandle,
        ) -> Option<Box<dyn WebClientService<Controller = BaseController> + Send>> {
            Some(
                self.rpc_manager
                    .rpc_client()
                    .scoped_client::<WebClientServiceClientFactory<BaseController>>(
                        1,
                        1,
                        "".to_string(),
                    ),
            )
        }

        fn get_storage(&self) -> &impl Storage<AppHandle, GUIConfig, anyhow::Error> {
            &self.storage
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{PersistedConfigSource, StoredGuiConfig};
        use exvlan_rpc::exvlan_proto::api::manage::NetworkConfig;

        #[test]
        fn stored_gui_config_defaults_missing_source_to_legacy() {
            let stored: StoredGuiConfig = serde_json::from_value(serde_json::json!({
                "config": NetworkConfig::default(),
            }))
            .unwrap();
            assert_eq!(stored.source, PersistedConfigSource::Legacy);
        }

        #[test]
        fn stored_gui_config_deserializes_webhook_source_as_web() {
            let stored: StoredGuiConfig = serde_json::from_value(serde_json::json!({
                "config": NetworkConfig::default(),
                "source": "webhook",
            }))
            .unwrap();
            assert_eq!(stored.source, PersistedConfigSource::Web);
        }

        #[test]
        fn stored_gui_config_defaults_unknown_source_to_legacy() {
            let stored: StoredGuiConfig = serde_json::from_value(serde_json::json!({
                "config": NetworkConfig::default(),
                "source": "unknown",
            }))
            .unwrap();
            assert_eq!(stored.source, PersistedConfigSource::Legacy);
        }

        #[test]
        fn persisted_source_merge_keeps_legacy_and_web_over_ambiguous_user() {
            assert_eq!(
                PersistedConfigSource::Legacy.merge_persisted(PersistedConfigSource::User),
                PersistedConfigSource::Legacy
            );
            assert_eq!(
                PersistedConfigSource::Web.merge_persisted(PersistedConfigSource::User),
                PersistedConfigSource::Web
            );
            assert_eq!(
                PersistedConfigSource::Legacy.merge_persisted(PersistedConfigSource::Web),
                PersistedConfigSource::Web
            );
        }

        #[test]
        fn only_web_configs_are_web_like() {
            assert!(!PersistedConfigSource::Legacy.is_web_like());
            assert!(!PersistedConfigSource::User.is_web_like());
            assert!(PersistedConfigSource::Web.is_web_like());
        }
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile;

#[cfg(any(target_os = "android", target_os = "ios"))]
use crate::mobile as manager;

#[cfg(not(target_os = "android"))]
mod service {
    use crate::SERVICE_LABEL;
    use anyhow::Context;

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    pub struct ServiceOptions {
        pub(super) config_dir: String,
        pub(super) rpc_portal: String,
        pub(super) file_log_level: String,
        pub(super) file_log_dir: String,
        pub(super) config_server: Option<String>,
    }
    impl ServiceOptions {
        fn to_args_vec(&self) -> Vec<std::ffi::OsString> {
            let mut args = vec![
                "--config-dir".into(),
                self.config_dir.clone().into(),
                "--rpc-portal".into(),
                self.rpc_portal.clone().into(),
                // 固定 info：用户选的级别由 GUI 连接后通过 `set_logging_level` 热更新，
                // 不进入服务启动参数（否则改日志级别会改变指纹 → 又弹一次授权）。
                "--file-log-level".into(),
                "info".into(),
                "--file-log-dir".into(),
                self.file_log_dir.clone().into(),
                // 统一 16MB × 3（GUI 侧 error.log 同参数，便于对照时序）
                "--file-log-size".into(),
                "16".into(),
                "--file-log-count".into(),
                "3".into(),
                "--daemon".into(),
            ];

            if let Some(config_server) = &self.config_server {
                args.push("--config-server".into());
                args.push(config_server.clone().into());
            }

            args
        }
    }

    #[cfg(target_os = "macos")]
    fn service_environment() -> Option<Vec<(String, String)>> {
        // System LaunchDaemons run as root but launchd does not provide HOME.
        Some(vec![("HOME".to_string(), "/var/root".to_string())])
    }

    #[cfg(not(target_os = "macos"))]
    fn service_environment() -> Option<Vec<(String, String)>> {
        None
    }

    /// 服务定义指纹（程序 + 参数）。用于判断「是否需要重装」，避免每次启动都提权。
    pub fn fingerprint(opts: &ServiceOptions) -> String {
        let mut parts = vec![crate::kernel::kernel_path().display().to_string()];
        parts.extend(
            opts.to_args_vec()
                .iter()
                .map(|a| a.to_string_lossy().to_string()),
        );
        parts.join("\u{0}")
    }

    /// 指纹存**用户侧**：由 GUI 自己写、自己读。
    ///
    /// 20711 之前存在 root 侧（内核目录），GUI 读不到/读不全时会永远判定
    /// 「服务需要重装」→ 每次启动都提权弹密码。
    fn marker_path() -> std::path::PathBuf {
        crate::user_marker_path()
            .unwrap_or_else(|| crate::kernel::kernel_dir().join("service.fingerprint"))
    }

    /// 当前已安装的服务是否与本配置一致。
    ///
    /// 双保险：用户侧指纹 + macOS plist 关键参数交叉校验；
    /// 不一致时把差异写进 error.log，方便定位「为什么又重装了」。
    pub fn is_current(opts: &ServiceOptions) -> bool {
        let expected = fingerprint(opts);
        let stored = std::fs::read_to_string(marker_path()).unwrap_or_default();

        if stored == expected {
            if plist_matches(opts) == Some(false) {
                crate::logger::record(
                    "service.fingerprint",
                    "marker 匹配但 plist 参数不一致，判定需要重装",
                );
                return false;
            }
            return true;
        }

        crate::logger::record(
            "service.fingerprint",
            format!(
                "mismatch expected={:?} stored={:?}",
                truncate_for_log(&expected, 160),
                truncate_for_log(&stored, 160)
            ),
        );

        // 自愈：标记丢失（首次安装后被杀软/清理工具删掉、或旧版本写在 root 侧）
        // 但 plist 参数完全一致时，直接补写标记，不再走提权重装。
        if stored.trim().is_empty() && plist_matches(opts) == Some(true) {
            if let Err(err) = write_marker(opts) {
                crate::logger::record("service.marker", format!("self-heal failed: {err:#}"));
            } else {
                crate::logger::record("service.fingerprint", "marker 丢失但 plist 一致，已自愈");
                return true;
            }
        }

        false
    }

    fn truncate_for_log(s: &str, max: usize) -> String {
        if s.chars().count() <= max {
            s.to_string()
        } else {
            s.chars().take(max).collect::<String>() + "…"
        }
    }

    /// 校验 launchd plist 里的关键参数是否与当前配置一致（非 macOS 返回 None）。
    fn plist_matches(opts: &ServiceOptions) -> Option<bool> {
        #[cfg(target_os = "macos")]
        {
            let plist = crate::paths::service_plist(SERVICE_LABEL)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let Ok(text) = std::fs::read_to_string(&plist) else {
                crate::logger::record("service.plist", format!("读取 {plist} 失败"));
                return Some(false);
            };
            let expected = [
                crate::kernel::kernel_path().display().to_string(),
                opts.config_dir.clone(),
                opts.rpc_portal.clone(),
                opts.file_log_dir.clone(),
            ];
            for item in expected {
                if !text.contains(&item) {
                    crate::logger::record("service.plist", format!("plist 缺少参数 {item}"));
                    return Some(false);
                }
            }
            Some(true)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = opts;
            None
        }
    }

    fn write_marker(opts: &ServiceOptions) -> anyhow::Result<()> {
        let path = marker_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, fingerprint(opts))
            .map_err(|e| anyhow::anyhow!("写入服务指纹失败（{}）: {e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        }
        Ok(())
    }

    pub fn install(opts: ServiceOptions) -> anyhow::Result<()> {
        let service = exvlan_rpc::service_manager::Service::new(SERVICE_LABEL.to_string())?;
        // 服务执行的是内核仓库中的二进制（root-owned），而不是 app 内的 GUI 二进制。
        let program = crate::kernel::kernel_path();
        if !program.is_file() {
            anyhow::bail!("内核未安装（{}），请先安装内核", program.display());
        }
        let options = exvlan_rpc::service_manager::ServiceInstallOptions {
            program,
            args: opts.to_args_vec(),
            work_directory: std::env::current_dir()?,
            environment: service_environment(),
            disable_autostart: false,
            description: Some("Exvlan Gui Service".to_string()),
            display_name: Some("Exvlan Gui Service".to_string()),
            disable_restart_on_failure: false,
        };

        // 旧服务与新版抢同一个 RPC/TUN 端口，必须先停；失败时恢复（见下）。
        let legacy_backup = stop_legacy_service();

        // 守护进程启动第一步是初始化文件日志：目录不存在会直接退出（20710 实测
        // 表现为「launchctl 显示 Running 但 15999 无人监听」）。这里以 root 预建。
        prepare_runtime_dirs(&opts);

        // 19707/20708 实测：服务已注册时 `service_manager.update()` 会先 bootout 再立刻
        // `launchctl bootstrap`，而 launchd 的注销是异步的，抢跑就返回
        // `Bootstrap failed: 5: Input/output error`，导致「每次启动都弹授权、每次都失败」。
        // 这里改为：先彻底注销并**等到 launchctl 真的看不到该 job**，再安装；失败重试。
        let _ = service.uninstall();
        wait_until_uninstalled(&service);

        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=3u32 {
            match service.install(&options) {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(err) => {
                    crate::logger::record(
                        "service.install",
                        format!("attempt={attempt}/3 {err:#}"),
                    );
                    last_error = Some(err);
                    // 清场后重试（含 launchd 的异步注销窗口）
                    let _ = service.uninstall();
                    wait_until_uninstalled(&service);
                    std::thread::sleep(std::time::Duration::from_millis(300 * u64::from(attempt)));
                }
            }
        }

        if let Some(err) = last_error {
            crate::logger::record("service.install.diag", service_diagnostics(&service, &opts));
            restore_legacy_service(&legacy_backup);
            return Err(err).with_context(|| "Failed to install service");
        }

        // 指纹由 GUI 进程写入（提权子进程没有 Tauri 上下文，写不到用户目录）。

        // 装上了不代表能连：以「RPC 端口真的能连上」为准（launchctl 的 Running 会骗人，
        // 20710 就是守护进程起来即退出、launchd 反复重启，状态一直显示 Running）。
        if wait_until_listening(&opts.rpc_portal, std::time::Duration::from_secs(6)) {
            cleanup_stale_paths();
            return Ok(());
        }
        // 再 kickstart 一次（首次 bootstrap 的 RunAtLoad 偶尔没触发）
        let _ = service.start();
        if wait_until_listening(&opts.rpc_portal, std::time::Duration::from_secs(4)) {
            return Ok(());
        }
        let diag = service_diagnostics(&service, &opts);
        crate::logger::record("service.install.not_listening", &diag);
        restore_legacy_service(&legacy_backup);
        Err(anyhow::anyhow!(
            "服务已安装但 RPC 端口 {} 无法连接（守护进程可能启动即退出）。诊断已写入 error.log。",
            opts.rpc_portal
        ))
    }

    /// 卸载旧 label（`exvlan-studio`），避免新旧两个守护进程并存。
    /// preview 期间**不删除**旧服务定义：只停掉（释放端口），plist 改名保留。
    ///
    /// 旧服务和新服务抢同一个 RPC/TUN 端口，必须先停；若新服务安装失败，
    /// 用 [`restore_legacy_service`] 把旧服务拉回来。
    #[derive(Default)]
    struct LegacyBackup {
        #[cfg(target_os = "macos")]
        plist: Option<(std::path::PathBuf, std::path::PathBuf)>,
    }

    fn stop_legacy_service() -> LegacyBackup {
        const LEGACY: &str = "exvlan-studio";
        #[allow(unused_mut)]
        let mut backup = LegacyBackup::default();
        if SERVICE_LABEL == LEGACY {
            return backup;
        }

        #[cfg(target_os = "macos")]
        {
            let label = format!("system/{LEGACY}");
            let _ = std::process::Command::new("/bin/launchctl")
                .args(["bootout", &label])
                .output();
            let plist = crate::paths::service_plist(LEGACY).unwrap_or_default();
            let bak = std::path::PathBuf::from(format!("{}.pre-7.2", plist.display()));
            if plist.is_file() && !bak.exists() {
                match std::fs::rename(&plist, &bak) {
                    Ok(()) => {
                        crate::logger::record(
                            "service.migrate",
                            format!("bootout {LEGACY}; plist → {}", bak.display()),
                        );
                        backup.plist = Some((plist, bak));
                    }
                    Err(err) => crate::logger::record(
                        "service.migrate",
                        format!("rename {} failed: {err}", plist.display()),
                    ),
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("sc")
                .args(["stop", LEGACY])
                .output();
            let _ = std::process::Command::new("sc")
                .args(["config", LEGACY, "start=", "disabled"])
                .output();
            crate::logger::record("service.migrate", format!("stopped legacy {LEGACY}"));
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = std::process::Command::new("systemctl")
                .args(["disable", "--now", LEGACY])
                .output();
            crate::logger::record("service.migrate", format!("disabled legacy {LEGACY}"));
        }

        backup
    }

    /// 新服务安装失败时，把旧服务恢复回去（回退保障）。
    #[allow(unused_variables)]
    fn restore_legacy_service(backup: &LegacyBackup) {
        const LEGACY: &str = "exvlan-studio";

        #[cfg(target_os = "macos")]
        if let Some((plist, bak)) = &backup.plist {
            let _ = std::fs::rename(bak, plist);
            let _ = std::process::Command::new("/bin/launchctl")
                .args(["bootstrap", "system", &plist.display().to_string()])
                .output();
            crate::logger::record("service.migrate", "restored legacy service");
            return;
        }

        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("sc")
                .args(["config", LEGACY, "start=", "auto"])
                .output();
            let _ = std::process::Command::new("sc")
                .args(["start", LEGACY])
                .output();
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = std::process::Command::new("systemctl")
                .args(["enable", "--now", LEGACY])
                .output();
        }

        let _ = LEGACY;
    }

    /// 删除旧版本残留（幂等：只删存在的路径，不读取、不拷贝、不恢复）。
    ///
    /// 清单见 `paths::stale_paths()`（唯一允许出现旧绝对路径的地方）。
    fn cleanup_stale_paths() {
        let mut removed: Vec<String> = Vec::new();
        for path in crate::paths::stale_paths() {
            if !path.exists() {
                continue;
            }
            let ok = if path.is_dir() {
                std::fs::remove_dir_all(&path).is_ok()
            } else {
                std::fs::remove_file(&path).is_ok()
            };
            if ok {
                removed.push(path.display().to_string());
            }
        }
        if !removed.is_empty() {
            crate::logger::record(
                "service.cleanup",
                format!(
                    "removed {} legacy path(s): {}",
                    removed.len(),
                    removed.join(", ")
                ),
            );
        }
    }

    /// GUI 侧写入服务指纹（提权成功后调用）。
    pub fn mark_current(opts: &ServiceOptions) {
        if let Err(err) = write_marker(opts) {
            crate::logger::record("service.marker", format!("{err:#}"));
        }
    }

    /// 以 root 预建守护进程要用的目录（配置 + 日志），并收紧属主/权限。
    fn prepare_runtime_dirs(opts: &ServiceOptions) {
        for dir in [&opts.config_dir, &opts.file_log_dir] {
            let path = std::path::Path::new(dir);
            match std::fs::create_dir_all(path) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::ffi::OsStrExt as _;
                        if let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
                            // root:wheel(0:0)
                            unsafe {
                                libc::chown(c.as_ptr(), 0, 0);
                            }
                        }
                        // 配置目录只允许 root（含网络密钥）；日志目录保持可读，便于分享与诊断。
                        let mode = if dir == &opts.config_dir {
                            0o700
                        } else {
                            0o755
                        };
                        crate::paths::ensure_dir_mode(path, mode);
                    }
                    crate::logger::record("service.dirs", format!("ready: {dir}"));
                }
                Err(err) => crate::logger::record(
                    "service.dirs",
                    format!("create_dir_all({dir}) failed: {err}"),
                ),
            }
        }
    }

    /// 轮询直到 RPC 端口真的能建立 TCP 连接。
    fn wait_until_listening(portal: &str, timeout: std::time::Duration) -> bool {
        let addr = portal
            .trim()
            .trim_start_matches("tcp://")
            .replace("0.0.0.0", "127.0.0.1");
        let Ok(sock) = addr.parse::<std::net::SocketAddr>() else {
            crate::logger::record("service.listen", format!("invalid rpc portal: {portal}"));
            return false;
        };
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if std::net::TcpStream::connect_timeout(&sock, std::time::Duration::from_millis(300))
                .is_ok()
            {
                crate::logger::record("service.listen", format!("ok: {sock}"));
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    /// 轮询直到服务「不再注册」（launchd 的 bootout 是异步的）。
    fn wait_until_uninstalled(service: &exvlan_rpc::service_manager::Service) {
        use exvlan_rpc::service_manager::ServiceStatus;
        for _ in 0..20 {
            match service.status() {
                Ok(ServiceStatus::NotInstalled) => return,
                Ok(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
                Err(_) => return,
            }
        }
    }

    /// 服务安装/启动失败时的现场：launchctl 状态 + plist 内容。
    fn service_diagnostics(
        _service: &exvlan_rpc::service_manager::Service,
        opts: &ServiceOptions,
    ) -> String {
        let label = SERVICE_LABEL;
        let mut parts: Vec<String> = Vec::new();

        // 守护进程自己的日志（最能说明「为什么没起来」）
        let daemon_log = std::path::Path::new(&opts.file_log_dir).join("exvlan.log");
        match std::fs::read_to_string(&daemon_log) {
            Ok(text) => {
                let tail: Vec<&str> = text.lines().rev().take(40).collect();
                let tail: Vec<&str> = tail.into_iter().rev().collect();
                parts.push(format!(
                    "daemon_log({})={}",
                    daemon_log.display(),
                    tail.join(" | ")
                ));
            }
            Err(err) => parts.push(format!(
                "daemon_log({}) 读取失败: {err}",
                daemon_log.display()
            )),
        }

        #[cfg(target_os = "macos")]
        {
            let plist = crate::paths::service_plist(label)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            match std::fs::read_to_string(&plist) {
                Ok(text) => parts.push(format!("plist({plist})={}", text.replace('\n', " | "))),
                Err(err) => parts.push(format!("plist({plist}) 读取失败: {err}")),
            }
            match std::process::Command::new("/bin/launchctl")
                .args(["print", &format!("system/{label}")])
                .output()
            {
                Ok(out) => parts.push(format!(
                    "launchctl print status={} stdout={} stderr={}",
                    out.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&out.stdout).replace('\n', " | "),
                    String::from_utf8_lossy(&out.stderr).replace('\n', " | ")
                )),
                Err(err) => parts.push(format!("launchctl print 执行失败: {err}")),
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            parts.push(format!("label={label}"));
        }

        parts.join(" ; ")
    }

    pub fn uninstall() -> anyhow::Result<()> {
        use exvlan_rpc::service_manager::ServiceStatus;
        let service = exvlan_rpc::service_manager::Service::new(SERVICE_LABEL.to_string())?;
        // 幂等：本来就没装 → 视为成功（切模式/重复点击不该报错）
        if service.status()? == ServiceStatus::NotInstalled {
            return Ok(());
        }
        service.uninstall()?;
        Ok(())
    }

    pub fn set_status(enable: bool) -> anyhow::Result<()> {
        use exvlan_rpc::service_manager::*;
        let service = Service::new(SERVICE_LABEL.to_string())?;
        let status = service.status()?;
        // 幂等：目标状态已达成时直接成功
        if (enable && status == ServiceStatus::Running)
            || (!enable && status != ServiceStatus::Running)
        {
            return Ok(());
        }
        if status == ServiceStatus::NotInstalled {
            return Err(anyhow::anyhow!("Service not installed"));
        }
        if enable {
            service.start().with_context(|| "Failed to start service")?;
        } else {
            service.stop().with_context(|| "Failed to stop service")?;
        }
        Ok(())
    }

    /// 重启服务（停止 → 启动）。
    pub fn restart() -> anyhow::Result<()> {
        use exvlan_rpc::service_manager::ServiceStatus;
        let service = exvlan_rpc::service_manager::Service::new(SERVICE_LABEL.to_string())?;
        if service.status()? == ServiceStatus::NotInstalled {
            anyhow::bail!("Service not installed");
        }

        // macOS：`launchctl kickstart -k` 是原子的「杀掉并重启」，不会出现
        // stop 尚未生效就 start → "Service is already running" 的假失败。
        #[cfg(target_os = "macos")]
        {
            let out = std::process::Command::new("/bin/launchctl")
                .args(["kickstart", "-k", &format!("system/{SERVICE_LABEL}")])
                .output()
                .with_context(|| "启动 launchctl 失败")?;
            if !out.status.success() {
                anyhow::bail!(
                    "launchctl kickstart 失败: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            return Ok(());
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = service.stop();
            service.start().with_context(|| "Failed to start service")?;
        }
        Ok(())
    }

    /// 强制重装服务（清指纹 → 重新 install，含 bootout/健康检查）。
    pub fn reinstall(opts: ServiceOptions) -> anyhow::Result<()> {
        let _ = std::fs::remove_file(marker_path());
        install(opts)
    }

    pub fn status() -> anyhow::Result<exvlan_rpc::service_manager::ServiceStatus> {
        let service = exvlan_rpc::service_manager::Service::new(SERVICE_LABEL.to_string())?;
        service.status()
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn service_environment_matches_platform() {
            #[cfg(target_os = "macos")]
            assert_eq!(
                super::service_environment(),
                Some(vec![("HOME".to_string(), "/var/root".to_string())])
            );

            #[cfg(not(target_os = "macos"))]
            assert_eq!(super::service_environment(), None);
        }
    }
}

/// 服务模式（root 守护进程）的默认配置/日志目录。
///
/// **必须是系统路径**：守护进程以 root 运行，如果指向用户目录会把目录属主改成
/// root，导致 GUI 之后既写不了 `error.log` 也写不了 `ui-state.json`（20709 实测）。
#[derive(Clone, serde::Serialize)]
pub struct ServiceDefaultDirs {
    pub config_dir: String,
    pub file_log_dir: String,
}

fn service_default_dirs() -> ServiceDefaultDirs {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        desktop_service_default_dirs()
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        // 移动端没有「服务模式」，命令只为让 generate_handler 全平台统一而存在。
        ServiceDefaultDirs {
            config_dir: String::new(),
            file_log_dir: String::new(),
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn desktop_service_default_dirs() -> ServiceDefaultDirs {
    let sys = crate::paths::system();
    ServiceDefaultDirs {
        config_dir: sys.config.display().to_string(),
        file_log_dir: sys.logs.display().to_string(),
    }
}

#[tauri::command]
fn service_default_dirs_cmd() -> ServiceDefaultDirs {
    service_default_dirs()
}

/// 前端据此删除 localStorage 里的旧快照（`networkList`）。
#[tauri::command]
fn legacy_cleanup_enabled() -> bool {
    ENABLE_LEGACY_CLEANUP
}

/// 提权子命令入口（由 `main.rs` 在解析到内部参数时调用）。
///
/// 这些命令只会在通过 `elevate::Command` 以 root 拉起时执行；普通用户直接运行
/// 同名参数没有任何跳过校验的捷径（写内核目录本身就要 root）。
///
/// **仅桌面端**：Android 没有 service 模块。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn run_elevated_cli(args: &[String]) -> Option<std::process::ExitCode> {
    use std::process::ExitCode;

    /// 提权子进程的错误：同时写 stdout（父进程可捕获）与 /tmp 日志（stderr 在
    /// AuthorizationExecuteWithPrivileges 下会进系统日志，拿不到）。
    fn report(tag: &str, err: &str) {
        let line = format!("[{tag}] {err}");
        println!("{line}");
        eprintln!("{line}");
        use std::io::Write as _;
        let log_path = crate::paths::elevate_log();
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        rotate_elevate_log_if_needed(&log_path);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = writeln!(f, "{line}");
        }
    }

    let code = match args.first().map(|s| s.as_str()) {
        Some("--elevate-selftest") => {
            report(
                "selftest",
                &format!(
                    "ok pid={} uid={} exe={}",
                    std::process::id(),
                    helper::current_uid(),
                    std::env::current_exe()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                ),
            );
            ExitCode::SUCCESS
        }
        Some("--install-kernel") => {
            let src = args.get(1).cloned().unwrap_or_default();
            let version = args.get(2).cloned().unwrap_or_default();
            match kernel::install_kernel(std::path::Path::new(&src), &version) {
                Ok(()) => {
                    eprintln!(
                        "[kernel] 已安装 {version} → {}",
                        kernel::kernel_path().display()
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    report("kernel", &format!("安装失败: {err:#}"));
                    ExitCode::FAILURE
                }
            }
        }
        Some("--install-service") => {
            let payload = args.get(1).cloned().unwrap_or_default();
            match serde_json::from_str::<service::ServiceOptions>(&payload) {
                Ok(opts) => match service::install(opts) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(err) => {
                        report("service", &format!("安装失败: {err:#}"));
                        ExitCode::FAILURE
                    }
                },
                Err(err) => {
                    report("service", &format!("参数解析失败: {err:#}"));
                    ExitCode::FAILURE
                }
            }
        }
        Some("--uninstall-service") => match service::uninstall() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                report("service", &format!("卸载失败: {err:#}"));
                ExitCode::FAILURE
            }
        },
        Some("--provision") => {
            let payload = args.get(1).cloned().unwrap_or_default();
            match serde_json::from_str::<ProvisionPayload>(&payload) {
                Ok(req) => match provision(&req) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(err) => {
                        report("provision", &format!("初始化失败: {err:#}"));
                        ExitCode::FAILURE
                    }
                },
                Err(err) => {
                    report("provision", &format!("参数解析失败: {err:#}"));
                    ExitCode::FAILURE
                }
            }
        }
        Some("--repair-permissions") => {
            let payload = args.get(1).cloned().unwrap_or_default();
            match serde_json::from_str::<RepairPermissionsPayload>(&payload) {
                Ok(req) => match repair_permissions(&req) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(err) => {
                        report("repair", &format!("修复权限失败: {err:#}"));
                        ExitCode::FAILURE
                    }
                },
                Err(err) => {
                    report("repair", &format!("参数解析失败: {err:#}"));
                    ExitCode::FAILURE
                }
            }
        }
        Some("--service-reinstall") => {
            let payload = args.get(1).cloned().unwrap_or_default();
            match serde_json::from_str::<service::ServiceOptions>(&payload) {
                Ok(opts) => match service::reinstall(opts) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(err) => {
                        report("service", &format!("重装失败: {err:#}"));
                        ExitCode::FAILURE
                    }
                },
                Err(err) => {
                    report("service", &format!("参数解析失败: {err:#}"));
                    ExitCode::FAILURE
                }
            }
        }
        Some("--service-ctl") => {
            let action = args.get(1).map(|s| s.as_str()).unwrap_or("start");
            if action == "restart" {
                let outcome = service::restart();
                return Some(match outcome {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(err) => {
                        report("service", &format!("重启失败: {err:#}"));
                        ExitCode::FAILURE
                    }
                });
            }
            let enable = action == "start";
            match service::set_status(enable) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    report("service", &format!("启停失败: {err:#}"));
                    ExitCode::FAILURE
                }
            }
        }
        _ => return None,
    };
    Some(code)
}

/// 一次提权完成「装内核 + 装服务」，避免首次安装连弹两次密码。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct ProvisionPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_src: Option<String>,
    pub kernel_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<service::ServiceOptions>,
    /// 随包 helper 路径（macOS；为空表示不需要安装）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_src: Option<String>,
    /// 安装者 uid（helper 只接受该用户的请求）。
    #[serde(default)]
    pub owner_uid: u32,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn stop_service_if_running() -> bool {
    use exvlan_rpc::service_manager::ServiceStatus;
    if !matches!(service::status(), Ok(ServiceStatus::Running)) {
        return false;
    }
    match service::set_status(false) {
        Ok(()) => {
            crate::logger::record("provision.service", "stopped before kernel replace");
            true
        }
        Err(err) => {
            crate::logger::record("provision.service", format!("stop failed: {err:#}"));
            false
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn provision(req: &ProvisionPayload) -> anyhow::Result<()> {
    // 替换内核二进制前必须先停服务：Windows 上运行中的 exe/dll 被文件锁占用，
    // rename 会直接失败；Unix 上虽然允许覆盖，但运行中的进程仍在跑旧镜像。
    let stopped_for_kernel = req.kernel_src.is_some() && stop_service_if_running();

    let result = (|| -> anyhow::Result<()> {
        if let Some(src) = req.kernel_src.as_deref() {
            kernel::install_kernel(std::path::Path::new(src), &req.kernel_version)?;
            crate::logger::record(
                "provision.kernel",
                format!("installed {}", req.kernel_version),
            );
        }
        if let Some(opts) = req.service.clone() {
            service::install(opts)?;
            crate::logger::record("provision.service", "installed");
        }
        if let Some(src) = req.helper_src.as_deref() {
            crate::helper::install_from(std::path::Path::new(src), req.owner_uid)?;
            crate::logger::record("provision.helper", "installed");
            // 启动竞态：bootstrap 后 socket 还没 bind，这里轮询 3s 再判定。
            crate::helper::verify_healthy(std::time::Duration::from_secs(3));
        }
        Ok(())
    })();

    // 只换了内核、没有重装服务 → 把服务拉回来（失败时也拉回，避免用户彻底断网）。
    if stopped_for_kernel && req.service.is_none() {
        if let Err(err) = service::set_status(true) {
            crate::logger::record(
                "provision.service",
                format!("restart after kernel install failed: {err:#}"),
            );
        }
    }

    result
}

/// 修复用户目录属主（目录被 root 守护进程写过时会变成 root，用户态无法再写）。
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct RepairPermissionsPayload {
    pub paths: Vec<String>,
    pub uid: u32,
    pub gid: u32,
}

#[cfg(unix)]
fn repair_permissions(req: &RepairPermissionsPayload) -> anyhow::Result<()> {
    fn chown_tree(path: &std::path::Path, uid: u32, gid: u32) -> anyhow::Result<()> {
        use std::os::unix::ffi::OsStrExt as _;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| anyhow::anyhow!("路径包含 NUL: {}", path.display()))?;
        // 先改自己，再递归子项（root 下不受目录权限限制）
        unsafe {
            libc::chown(c.as_ptr(), uid, gid);
        }
        if path.is_dir()
            && let Ok(entries) = std::fs::read_dir(path)
        {
            for entry in entries.flatten() {
                chown_tree(&entry.path(), uid, gid)?;
            }
        }
        Ok(())
    }

    for path in &req.paths {
        chown_tree(std::path::Path::new(path), req.uid, req.gid)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn repair_permissions(_req: &RepairPermissionsPayload) -> anyhow::Result<()> {
    Ok(())
}

/// 用户日志/配置目录不可写时，提权修复一次属主。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn maybe_repair_user_dirs(app: &AppHandle) {
    use tauri::Manager as _;
    let log_dir = app.path().app_log_dir().ok();
    let config_dir = app.path().app_config_dir().ok();

    let log_bad = log_dir.as_deref().map(|d| !dir_writable(d));
    let config_bad = config_dir.as_deref().map(|d| !dir_writable(d));
    if log_bad != Some(true) && config_bad != Some(true) {
        return;
    }

    let mut paths: Vec<String> = Vec::new();
    for dir in [log_dir.clone(), config_dir.clone()].into_iter().flatten() {
        paths.push(dir.display().to_string());
        // 旧版内核/服务可能还往 config.d 里写过文件
        paths.push(dir.join("config.d").display().to_string());
    }
    if paths.is_empty() {
        return;
    }

    #[cfg(unix)]
    let payload = RepairPermissionsPayload {
        paths: paths.clone(),
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
    };
    #[cfg(not(unix))]
    let payload = RepairPermissionsPayload {
        paths: paths.clone(),
        uid: 0,
        gid: 0,
    };

    logger::record(
        "repair.start",
        format!("检测到用户目录不可写，尝试提权修复: {paths:?}"),
    );
    let json = match serde_json::to_string(&payload) {
        Ok(j) => j,
        Err(err) => {
            logger::record("repair.failed", format!("序列化失败: {err}"));
            return;
        }
    };
    match run_elevated(&["--repair-permissions", &json]) {
        Ok(()) => {
            let fixed = log_dir.as_deref().map(dir_writable) == Some(true)
                && config_dir.as_deref().map(dir_writable) == Some(true);
            logger::record("repair.done", format!("fixed={fixed}"));
            if fixed {
                // 修好后重新初始化日志（之前可能落到了 fallback 目录）
                ensure_error_log(app);
            }
        }
        Err(err) => logger::record("repair.failed", err),
    }
}

/// 用户侧服务指纹文件的目录（启动时由 setup 写入）。
static USER_STATE_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn user_marker_path() -> Option<std::path::PathBuf> {
    USER_STATE_DIR.get().map(|d| d.join("service.fingerprint"))
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn user_marker_path() -> Option<std::path::PathBuf> {
    None
}

/// 守护进程以 root 运行：目录必须是系统路径，且**不沿用**任何旧配置。
///
/// 测试阶段不做向后兼容：旧值一律忽略，避免历史脏数据反复污染。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn normalize_service_options(opts: &mut service::ServiceOptions) {
    let defaults = service_default_dirs();
    if opts.config_dir != defaults.config_dir {
        logger::record(
            "service.config_dir",
            format!(
                "覆盖为系统路径: {} → {}",
                opts.config_dir, defaults.config_dir
            ),
        );
        opts.config_dir = defaults.config_dir.clone();
    }
    if opts.file_log_dir != defaults.file_log_dir {
        logger::record(
            "service.file_log_dir",
            format!(
                "覆盖为系统路径: {} → {}",
                opts.file_log_dir, defaults.file_log_dir
            ),
        );
        opts.file_log_dir = defaults.file_log_dir.clone();
    }

    // 日志级别必须归一化：不同页面曾分别传 `info` / `off`，导致服务指纹来回变化、
    // 每次启动都被判「需要重装」→ 反复弹授权。
    let level = opts.file_log_level.trim().to_ascii_lowercase();
    opts.file_log_level = match level.as_str() {
        "off" | "error" | "warn" | "info" | "debug" | "trace" => level,
        _ => "info".to_string(),
    };
}

/// 确保「内核已安装 + 服务定义与当前配置一致」。
///
/// 返回值：`"installed"`（本次新装/重装）或 `"current"`（无需动作）。
/// 需要提权时只会弹一次系统授权框；指纹一致时**不会**提权。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn ensure_kernel_service(
    app: AppHandle,
    opts: service::ServiceOptions,
) -> Result<String, String> {
    let mut opts = opts;
    normalize_service_options(&mut opts);

    // 一次提权完成「内核（版本不一致时覆盖）+ 服务（指纹不一致时重装）」，
    // 避免首次安装连弹两次密码。
    // 必须用带构建标识的串：同一基线下的预览版之间（26.410.20712 → -pre.9+sha）
    // 若只比 CARGO_PKG_VERSION 会永远判为“已是最新”，内核与 helper 都不更新。
    let want_version = crate::BUILD_VERSION;
    let need_kernel =
        !kernel::is_installed() || kernel::installed_version().as_deref() != Some(want_version);
    let need_service = !service::is_current(&opts);
    // helper 仅 macOS 存在；其它平台恒为 false，否则每次启动都会判定「需要安装」
    // → 反复提权。
    #[cfg(target_os = "macos")]
    let need_helper = !helper::is_ready();
    #[cfg(not(target_os = "macos"))]
    let need_helper = false;

    if !need_kernel && !need_service && !need_helper {
        return Ok("current".to_string());
    }

    // 只需换内核、且 helper 已就绪 → 交给 helper，**不弹授权**。
    #[cfg(target_os = "macos")]
    if need_kernel && !need_service && !need_helper {
        match helper::apply_bundled(&app) {
            Ok(()) => {
                logger::record("helper.apply", format!("kernel → {want_version}"));
                return Ok("installed".to_string());
            }
            Err(err) => logger::record("helper.apply", format!("{err:#}（回退提权）")),
        }
    }

    let kernel_src = if need_kernel {
        Some(
            kernel::bundled_kernel(&app)
                .ok_or_else(|| {
                    format!(
                        "未找到随包内核（{}）。请确认安装包完整。",
                        kernel::kernel_bin_name()
                    )
                })?
                .display()
                .to_string(),
        )
    } else {
        None
    };

    let helper_src = if need_helper {
        helper::bundled_helper(&app).map(|p| p.display().to_string())
    } else {
        None
    };
    let payload = ProvisionPayload {
        kernel_src,
        kernel_version: want_version.to_string(),
        service: need_service.then(|| opts.clone()),
        helper_src,
        owner_uid: helper::current_uid(),
    };
    let json = serde_json::to_string(&payload)
        .map_err(|e| logger::other("cmd:ensure_kernel_service", e))?;
    logger::record(
        "provision",
        format!("kernel={need_kernel} service={need_service}"),
    );
    run_elevated(&["--provision", &json])?;
    // 指纹必须由 GUI（当前用户）写入：提权子进程没有 Tauri 上下文，
    // 之前写到 root 侧目录，导致 GUI 永远读不到 → 每次启动都重装服务。
    service::mark_current(&opts);
    Ok("installed".to_string())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
async fn ensure_kernel_service(
    _app: AppHandle,
    _opts: serde_json::Value,
) -> Result<String, String> {
    Ok("current".to_string())
}

/// 服务自检（只读）：内核/服务状态、RPC 端口连通性、daemon 日志尾部。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
fn service_diagnostics(rpc_portal: Option<String>) -> Result<String, String> {
    let dirs = service_default_dirs();
    let portal = rpc_portal.unwrap_or_else(|| "127.0.0.1:15999".to_string());
    let status = service::status()
        .map(|s| format!("{s:?}"))
        .unwrap_or_else(|e| format!("unknown({e})"));

    let reachable = {
        let addr = portal
            .trim()
            .trim_start_matches("tcp://")
            .replace("0.0.0.0", "127.0.0.1");
        addr.parse::<std::net::SocketAddr>()
            .map(|sock| {
                std::net::TcpStream::connect_timeout(&sock, std::time::Duration::from_millis(500))
                    .is_ok()
            })
            .unwrap_or(false)
    };

    let daemon_log = std::path::Path::new(&dirs.file_log_dir).join("exvlan.log");
    let daemon_tail = std::fs::read_to_string(&daemon_log)
        .map(|text| {
            text.lines()
                .rev()
                .take(30)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|e| format!("(无法读取 {}: {e})", daemon_log.display()));

    let report = format!(
        "app_version: {}\nos: {} ({})\nkernel_installed: {}\nkernel_version: {}\nservice_status: {}\nrpc_portal: {} (reachable={})\nconfig_dir: {}\nlog_dir: {}\ndaemon_log: {}\n--- daemon log tail ---\n{}",
        BUILD_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        kernel::is_installed(),
        kernel::installed_version().unwrap_or_else(|| "-".to_string()),
        status,
        portal,
        reachable,
        dirs.config_dir,
        dirs.file_log_dir,
        daemon_log.display(),
        daemon_tail,
    );
    let mut report = report;
    report.push_str(&format!(
        "[helper] installed={} status={}\n",
        helper::is_installed(),
        helper::status().unwrap_or_else(|e| e)
    ));
    // 内核运行时依赖（Windows 上 Packet.dll / wintun.dll / WinDivert*.sys 缺一不可）
    let deps = kernel::runtime_dep_status();
    if !deps.is_empty() {
        let list = deps
            .iter()
            .map(|(name, ok)| format!("{name}={}", if *ok { "ok" } else { "MISSING" }))
            .collect::<Vec<_>>()
            .join(" ");
        report.push_str(&format!("[core] {list}\n"));
    }
    report.push_str(&format!("[core.dir] {}\n", kernel::kernel_dir().display()));
    if let Ok(text) = std::fs::read_to_string(kernel::kernel_dir().join("version")) {
        report.push_str(&format!("[core.version] {}\n", text.trim()));
    }
    logger::record("service.diagnostics", &report);
    Ok(report)
}

/// 重启服务（提权）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
fn service_restart() -> Result<(), String> {
    run_elevated(&["--service-ctl", "restart"])?;
    logger::record("service.restart", "done");
    Ok(())
}

/// 强制重装服务（提权，含 bootout + 目录预建 + 真连自检）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn service_reinstall(
    app: tauri::AppHandle,
    opts: service::ServiceOptions,
) -> Result<(), String> {
    let mut opts = opts;
    normalize_service_options(&mut opts);

    // 内核没装时不能只重装服务（服务指向的二进制不存在）——走完整 provision。
    if !kernel::is_installed() {
        let kernel_src = kernel::bundled_kernel(&app)
            .ok_or_else(|| {
                format!(
                    "未找到随包内核（{}）。请确认安装包完整。",
                    kernel::kernel_bin_name()
                )
            })?
            .display()
            .to_string();
        let payload = ProvisionPayload {
            kernel_src: Some(kernel_src),
            kernel_version: crate::BUILD_VERSION.to_string(),
            service: Some(opts.clone()),
            helper_src: {
                #[cfg(target_os = "macos")]
                {
                    helper::bundled_helper(&app).map(|p| p.display().to_string())
                }
                #[cfg(not(target_os = "macos"))]
                {
                    None
                }
            },
            owner_uid: helper::current_uid(),
        };
        let json = serde_json::to_string(&payload)
            .map_err(|e| logger::other("cmd:service_reinstall", e))?;
        logger::record("provision", "kernel=false(missing) service=true");
        run_elevated(&["--provision", &json])?;
        service::mark_current(&opts);
        logger::record("service.reinstall", "done (with kernel)");
        return Ok(());
    }

    let payload =
        serde_json::to_string(&opts).map_err(|e| logger::other("cmd:service_reinstall", e))?;
    run_elevated(&["--service-reinstall", &payload])?;
    service::mark_current(&opts);
    logger::record("service.reinstall", "done");
    Ok(())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
fn service_diagnostics(_rpc_portal: Option<String>) -> Result<String, String> {
    Err("该功能仅桌面端支持".to_string())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
fn service_restart() -> Result<(), String> {
    Err("该功能仅桌面端支持".to_string())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
fn service_reinstall(_opts: serde_json::Value) -> Result<(), String> {
    Err("该功能仅桌面端支持".to_string())
}

/// 内核更新信息（当前版本 / 最新版本 / 可用版本列表 / 分发源）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn kernel_update_info(source: Option<String>) -> Result<upgrade::kernel::UpdateInfo, String> {
    let base = source.unwrap_or_else(|| upgrade::KERNEL_BASE_URL.to_string());
    upgrade::kernel::check_update(&base, env!("CARGO_PKG_VERSION"))
        .await
        .map_err(|e| format!("{e:#}"))
}

/// 安装指定内核版本（下载 → 校验 → 提权写入系统目录 → 重启服务）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn kernel_install_version(version: String, source: Option<String>) -> Result<(), String> {
    logger::record("kernel.install", format!("version={version}"));
    let base = source.unwrap_or_else(|| upgrade::KERNEL_BASE_URL.to_string());
    upgrade::kernel::install_version(&base, &version, env!("CARGO_PKG_VERSION"))
        .await
        .map_err(|e| format!("{e:#}"))
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
async fn kernel_update_info(_source: Option<String>) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({"available": false, "versions": []}))
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
async fn kernel_install_version(_version: String, _source: Option<String>) -> Result<(), String> {
    Err("移动端内核随应用发布，不支持在线升级".to_string())
}

/// 应用更新信息（当前版本 / 最新版本 / 更新说明 / 可用版本）。
///
/// 桌面优先走官方 updater（minisign 校验），失败或指定了 `source` 时回退自建 manifest。
#[tauri::command]
async fn app_update_info(
    app: AppHandle,
    source: Option<String>,
) -> Result<upgrade::app::AppUpdateInfo, String> {
    if source.is_none() {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(info) = upgrade::app::check_update_official(&app).await {
            return Ok(info);
        }
        let _ = &app;
    }
    let base = source.unwrap_or_else(|| upgrade::APP_BASE_URL.to_string());
    upgrade::app::check_update(&base)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// 安装应用更新：桌面优先官方 updater（成功后重启），失败回退「下载并打开安装包」。
#[tauri::command]
async fn app_update_apply(
    app: AppHandle,
    version: String,
    source: Option<String>,
) -> Result<(), String> {
    logger::record("update.apply", format!("version={version}"));
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    if source.is_none() {
        match upgrade::app::install_official(&app).await {
            Ok(()) => {
                app.restart();
            }
            Err(err) => eprintln!("[upgrade] 官方 updater 安装失败，回退自建流程: {err:#}"),
        }
    }
    let base = source.unwrap_or_else(|| upgrade::APP_BASE_URL.to_string());
    let dest = update_download_dir(&app)?;
    let path = upgrade::app::download_installer(&base, &version, dest.as_deref())
        .await
        .map_err(|e| format!("{e:#}"))?;
    app_install_update(app, path.display().to_string())
}

/// 下载应用安装包（已校验 sha256），返回本地路径。
#[tauri::command]
async fn app_download_update(
    app: AppHandle,
    version: String,
    source: Option<String>,
) -> Result<String, String> {
    let base = source.unwrap_or_else(|| upgrade::APP_BASE_URL.to_string());
    let dest = update_download_dir(&app)?;
    upgrade::app::download_installer(&base, &version, dest.as_deref())
        .await
        .map(|p| p.display().to_string())
        .map_err(|e| format!("{e:#}"))
}

/// 移动端更新包要落在应用 cache 目录（FileProvider 才能共享给系统安装器）。
fn update_download_dir(app: &AppHandle) -> Result<Option<std::path::PathBuf>, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let dir = app
            .path()
            .app_cache_dir()
            .map_err(|e| format!("无法定位应用缓存目录: {e}"))?
            .join("upgrade");
        Ok(Some(dir))
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        Ok(None)
    }
}

/// 打开已下载的安装包（桌面：DMG/MSI；Android 走 FileProvider + 系统安装器）。
#[tauri::command]
fn app_install_update(app: AppHandle, path: String) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        use tauri_plugin_vpnservice::VpnserviceExt;
        app.vpnservice()
            .install_apk(tauri_plugin_vpnservice::models::InstallApkRequest { path })
            .map_err(|e| format!("打开系统安装器失败: {e}"))?;
        return Ok(());
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = &app;
        upgrade::app::install_installer(std::path::Path::new(&path)).map_err(|e| format!("{e:#}"))
    }
}

/// Android：把 FFI 事件队列里的事件转发到前端（Rust 侧消费，前端无需轮询）。
#[cfg(target_os = "android")]
fn spawn_kernel_event_forwarder(app: AppHandle) {
    std::thread::spawn(move || {
        loop {
            match ffi::poll_event() {
                Some(json) => {
                    let (event, instance) = match serde_json::from_str::<serde_json::Value>(&json) {
                        Ok(v) => (
                            v.get("type")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            v.get("instance")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                        ),
                        Err(_) => continue,
                    };
                    let name = match event.as_str() {
                        "dhcp_ip_changed" => "dhcp_ip_changed",
                        "proxy_cidrs_updated" => "proxy_cidrs_updated",
                        _ => continue,
                    };
                    let _ = app.emit(name, instance);
                }
                None => std::thread::sleep(std::time::Duration::from_millis(500)),
            }
        }
    });
}

/// 从内核返回的实例信息 JSON 里取出「会影响 VPN 配置」的部分（虚拟 IP / 路由 / 运行状态）。
///
/// 不能直接比较整段 JSON：`peers`、`events` 等字段的顺序与内容会频繁抖动。
#[cfg(target_os = "android")]
fn network_info_signature(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let pick = |obj: &serde_json::Value, snake: &str, camel: &str| {
        obj.get(snake).or_else(|| obj.get(camel)).cloned()
    };
    let node_info = pick(&value, "my_node_info", "myNodeInfo").unwrap_or_default();
    let virtual_ipv4 = pick(&node_info, "virtual_ipv4", "virtualIpv4").unwrap_or_default();
    let mut routes: Vec<String> = pick(&value, "routes", "routes")
        .and_then(|r| r.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|r| {
            // 路由按目标/掩码/代理标记排序，忽略字段顺序抖动
            let target = r
                .get("dest")
                .or_else(|| r.get("destination"))
                .map(|x| x.to_string())
                .unwrap_or_default();
            format!(
                "{target}|{}",
                r.get("proxy").map(|x| x.to_string()).unwrap_or_default()
            )
        })
        .collect();
    routes.sort();
    let running = pick(&value, "running", "running").unwrap_or_default();
    Some(format!("{virtual_ipv4}|{running}|{}", routes.join(",")))
}

/// Android：事件兜底——每 30s 比对实例网络信息（DHCP IP / 路由），有变化就 emit。
///
/// FFI 事件队列容量有限，极端情况下会丢事件导致路由不同步；这里用定时比对兜底。
#[cfg(target_os = "android")]
fn spawn_network_info_watchdog(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last: std::collections::HashMap<String, String> = Default::default();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let infos = match ffi::collect_running_network_infos(16) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut current: std::collections::HashMap<String, String> =
                std::collections::HashMap::with_capacity(infos.len());
            for (name, info) in infos {
                let signature = match network_info_signature(&info) {
                    Some(sig) => sig,
                    None => continue,
                };
                let changed = last.get(&name) != Some(&signature);
                if changed {
                    let _ = app.emit("dhcp_ip_changed", name.clone());
                }
                current.insert(name, signature);
            }
            // 实例消失时清掉快照，避免下次启动重复判断
            last = current;
        }
    });
}

/// 以 root 重新拉起自身执行内部子命令（桌面端）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[cfg(target_os = "macos")]
fn run_elevated(args: &[&str]) -> Result<(), String> {
    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
    fn apple_escape(value: &str) -> String {
        value.replace('\\', "\\\\").replace('"', "\\\"")
    }

    let exe = std::env::current_exe().map_err(|e| format!("无法定位自身可执行文件: {e}"))?;
    let mut command = shell_quote(&exe.display().to_string());
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    let script = format!(
        "do shell script \"{}\" with administrator privileges with prompt \"Exvlan 需要系统权限（仅首次安装/升级）\"",
        apple_escape(&command)
    );
    let out = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("无法启动 osascript: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if stderr.contains("-128") || stderr.to_lowercase().contains("cancel") {
        return Err("已取消管理员授权".to_string());
    }
    logger::record("elevate", format!("args={args:?} osascript: {stderr}"));
    Err(format!("提权命令失败（osascript）: {stderr}"))
}

/// 提权日志轮转：8MB × 2（历史文件 `.1`）。
fn rotate_elevate_log_if_needed(path: &std::path::Path) {
    const MAX: u64 = 8 * 1024 * 1024;
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() < MAX {
        return;
    }
    let backup = path.with_extension("log.1");
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::rename(path, &backup);
}

/// 提权日志里写一条分段标记，便于失败时只展示本次调用的输出。
fn mark_elevate_begin(args: &[&str]) {
    let path = crate::paths::elevate_log();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write as _;
    rotate_elevate_log_if_needed(&path);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "--- begin {} args={args:?} ---", logger::timestamp());
    }
}

/// 只取最后一次 `--- begin ` 之后的日志（避免把上一次调用的旧错误混进本次失败提示）。
fn elevate_log_tail_since_begin(max_lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(crate::paths::elevate_log()) else {
        return String::new();
    };
    let mut lines: Vec<&str> = text.lines().collect();
    if let Some(pos) = lines.iter().rposition(|l| l.starts_with("--- begin ")) {
        lines = lines[pos..].to_vec();
    }
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run_elevated(args: &[&str]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位自身可执行文件: {e}"))?;
    let mut cmd = std::process::Command::new(exe);
    #[cfg(not(target_os = "macos"))]
    cmd.args(args);
    mark_elevate_begin(args);
    let output = match elevate::Command::new(cmd).output() {
        Ok(out) => out,
        Err(err) => {
            let msg = err.to_string();
            if msg.contains("-60006") || msg.to_lowercase().contains("cancel") {
                return Err("已取消管理员授权".to_string());
            }
            return Err(format!("无法启动提权进程: {msg}"));
        }
    };
    if output.status.success() {
        return Ok(());
    }

    let mut detail = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if detail.is_empty() {
        detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    }
    if detail.is_empty() {
        detail = elevate_log_tail_since_begin(20);
    }
    let message = format!(
        "提权命令失败（exit={}）：{}",
        output.status,
        if detail.is_empty() {
            format!("（无输出，详见 {}）", crate::paths::elevate_log().display())
        } else {
            detail
        }
    );
    logger::record("elevate", format!("args={args:?} {message}"));
    Err(message)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn run_elevated(_args: &[&str]) -> Result<(), String> {
    Err("提权仅桌面端支持".to_string())
}
/// 提权链路自检：跑一次 `--elevate-selftest`，返回真实退出码/错误。
///
/// 用于 Windows 上确认"UAC → 子进程启动 → 参数传递 → 等待退出"整条链路可用。
#[tauri::command]
async fn elevate_selftest() -> Result<String, String> {
    run_elevated(&["--elevate-selftest"])?;
    Ok("提权链路正常".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[cfg(windows)]
mod win_error_mode {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetErrorMode(mode: u32) -> u32;
    }

    /// 关掉「缺 DLL / 未处理异常」的系统硬错误对话框。
    /// 子进程会继承该设置，所以拉起内核时也不会再弹窗打断用户。
    pub fn suppress_hard_error_dialogs() {
        const SEM_FAILCRITICALERRORS: u32 = 0x0001;
        const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
        unsafe {
            SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
        }
    }
}

pub fn run_gui() -> std::process::ExitCode {
    // Windows：抑制加载器/GPF 的硬错误对话框。
    // 内核是控制台程序，GUI 拉起它时若缺少 Packet.dll，系统会弹
    // “The code execution cannot proceed…” 并阻塞；SetErrorMode 会被子进程继承。
    #[cfg(windows)]
    win_error_mode::suppress_hard_error_dialogs();

    // 桌面端不再自动提权：内核以独立 root 守护进程运行，GUI 始终是普通用户。
    let mut builder = tauri::Builder::default();

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            app.webview_windows()
                .values()
                .next()
                .expect("Sorry, no window found")
                .set_focus()
                .expect("Can't Bring Window to Focus");
        }));
    }

    builder = builder
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_shell::init());

    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        builder = builder.plugin(tauri_plugin_vpnservice::init());
    }

    // 桌面：官方 updater（minisign 校验 + 自动替换安装）
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    let app = builder
        .setup(|app| {
            let handle = app.app_handle().clone();

            // 统一错误出口：必须在任何命令/FFI 调用之前就绪。
            // 20709 在 macOS 上踩到日志目录被 root 占用，这里多级兜底 + 自检/修复。
            ensure_error_log(&handle);
            if let Ok(dir) = get_log_dir(&handle) {
                logger::init_events(&dir);
            }
            init_android_tls();
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            if let Ok(dir) = {
                use tauri::Manager as _;
                handle.path().app_config_dir()
            } {
                let _ = USER_STATE_DIR.set(dir);
            }

            install_panic_hook();
            record_boot_line(&handle);

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            maybe_repair_user_dirs(&handle);

            init_gui_logging(app.app_handle());

            #[cfg(target_os = "android")]
            spawn_kernel_event_forwarder(app.app_handle().clone());

            #[cfg(target_os = "android")]
            spawn_network_info_watchdog(app.app_handle().clone());

            // for tray icon, menu need to be built in js
            #[cfg(not(target_os = "android"))]
            let _tray_menu = TrayIconBuilder::with_id("main")
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        toggle_window_visibility(app);
                    }
                })
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/icon.png"
                ))?)
                .icon_as_template(true)
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            parse_network_config,
            generate_network_config,
            run_network_instance,
            collect_network_info,
            get_vpn_portal_info,
            patch_vpn_portal_clients,
            set_logging_level,
            set_tun_fd,
            exvlan_version,
            set_dock_visibility,
            list_network_instance_ids,
            remove_network_instance,
            update_network_config_state,
            save_network_config,
            validate_config,
            get_config,
            load_configs,
            get_network_metas,
            init_service,
            ensure_kernel_service,
            kernel_update_info,
            app_update_info,
            app_download_update,
            app_install_update,
            app_update_apply,
            kernel_install_version,
            set_service_status,
            get_service_status,
            service_diagnostics,
            service_restart,
            service_reinstall,
            init_rpc_connection,
            is_client_running,
            init_web_client,
            is_web_client_connected,
            get_log_dir_path,
            report_frontend_error,
            log_diag_step,
            open_diagnostics_dir,
            elevate_selftest,
            report_boot_state,
            diagnostics_info,
            service_default_dirs_cmd,
            legacy_cleanup_enabled,
            error_log_path,
            read_error_log,
            share_error_log,
            upload_diagnostics,
            network_diagnostics,
            log_vpn_routes,
            prefs::get_ui_state,
            prefs::set_ui_state,
            prefs::prefs_path_cmd,
            networks::get_networks,
            networks::set_networks,
            networks::migrate_networks,
            networks::networks_path_cmd,
        ])
        .on_window_event(|_win, event| match event {
            #[cfg(not(target_os = "android"))]
            tauri::WindowEvent::CloseRequested { api, .. } => {
                let _ = _win.hide();
                let _ = set_dock_visibility(_win.app_handle().clone(), false);
                api.prevent_close();
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .unwrap();

    app.run(|_app, _event| {});

    std::process::ExitCode::SUCCESS
}
