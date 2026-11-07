// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod elevate;
mod error_log;
#[cfg(target_os = "android")]
mod ffi_kernel;
mod kernel;
mod ui_state;
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
    Ok(env!("CARGO_PKG_VERSION").to_string())
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
        .map_err(|e| error_log::other("cmd:set_dock_visibility", e))?;
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
        .map_err(|e| error_log::other("cmd:parse_network_config", e))?;
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
        .map_err(|e| error_log::other("cmd:parse_network_config", e.to_string()))?;
    Ok(resp.toml_config)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
fn generate_network_config(toml_config: String) -> Result<NetworkConfig, String> {
    let config = TomlConfigLoader::new_from_str(&toml_config)
        .map_err(|e| error_log::other("cmd:generate_network_config", e))?;
    let cfg = NetworkConfig::new_from_config(&config)
        .map_err(|e| error_log::other("cmd:generate_network_config", e))?;
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
    .map_err(|e| error_log::other("cmd:generate_network_config", e))?;
    serde_json::from_value(value).map_err(|e| error_log::other("cmd:generate_network_config", e))
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
        .map_err(|e| error_log::other("cmd:run_network_instance", e))?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .pre_run_network_instance_hook(&app, &toml_config, manager::PersistedConfigSource::User)
        .await?;
    client_manager
        .handle_run_network_instance(app.clone(), cfg, save)
        .await
        .map_err(|e| error_log::other("cmd:run_network_instance", e))?;
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
    error_log::event(
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
        .map_err(|e: uuid::Error| error_log::other("cmd:run_network_instance", e))?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .pre_run_network_instance_hook(&app, &cfg, manager::PersistedConfigSource::User)
        .await?;
    client_manager
        .handle_run_network_instance(app.clone(), cfg, save)
        .await
        .map_err(|e| error_log::other("cmd:run_network_instance", e))?;
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
        .map_err(|e: uuid::Error| error_log::other("cmd:collect_network_info", e))?;
    get_client_manager!()?
        .handle_collect_network_info(app, Some(vec![instance_id]))
        .await
        .map_err(|e| error_log::other("cmd:collect_network_info", e))
}

#[tauri::command]
async fn get_vpn_portal_info(instance_id: String) -> Result<Option<VpnPortalInfo>, String> {
    let instance_id = instance_id
        .parse::<uuid::Uuid>()
        .map_err(|e| error_log::other("cmd:get_vpn_portal_info", e))?;
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
        .map_err(|e| error_log::other("cmd:get_vpn_portal_info", e))?;
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
        .map_err(|e| error_log::other("cmd:patch_vpn_portal_clients", e))?;
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
        .map_err(|e| error_log::other("cmd:patch_vpn_portal_clients", e))
}

#[tauri::command]
async fn set_logging_level(app: AppHandle, level: String) -> Result<(), String> {
    get_client_manager!()?
        .set_logging_level(level.clone())
        .await
        .map_err(|e| error_log::other("cmd:set_logging_level", e))?;
    // GUI/内核同进程：本地 logger 也要跟着热更新，否则 exvlan.log 仍是旧级别
    init_gui_logging_with(&app, Some(&level));
    Ok(())
}

#[tauri::command]
async fn set_tun_fd(fd: i32) -> Result<(), String> {
    error_log::event("vpn", format!("set_tun_fd fd={fd}"));
    #[cfg(target_os = "android")]
    {
        // 取第一个实例，把 VpnService 的 TUN fd 交给内核（FFI）
        let names = crate::ffi_kernel::list_instances(16)?;
        if let Some(name) = names.first() {
            crate::ffi_kernel::attach_tun_fd(name, fd)?;
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
        .map_err(|e| error_log::other("cmd:list_network_instance_ids", e))
}

#[tauri::command]
async fn remove_network_instance(app: AppHandle, instance_id: String) -> Result<(), String> {
    error_log::event("inst", format!("remove {instance_id}"));
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| error_log::other("cmd:remove_network_instance", e))?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .handle_remove_network_instances(app.clone(), vec![instance_id])
        .await
        .map_err(|e| error_log::other("cmd:remove_network_instance", e))?;
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
    error_log::event(
        "inst",
        format!("update_state {instance_id} disabled={disabled}"),
    );
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| error_log::other("cmd:update_network_config_state", e))?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    if !disabled {
        let (cfg, source) = client_manager
            .handle_get_network_config_with_source(app.clone(), instance_id)
            .await
            .map_err(|e| error_log::other("cmd:update_network_config_state", e))?;
        let toml_config = cfg
            .gen_config()
            .map_err(|e| error_log::other("cmd:update_network_config_state", e))?;
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
        .map_err(|e| error_log::other("cmd:update_network_config_state", e))?;

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
        .map_err(|e: uuid::Error| error_log::other("cmd:update_network_config_state", e))?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    if !disabled {
        let (cfg, source) = client_manager
            .handle_get_network_config_with_source(app.clone(), instance_id)
            .await
            .map_err(|e| error_log::other("cmd:update_network_config_state", e))?;
        client_manager
            .pre_run_network_instance_hook(&app, &cfg, source)
            .await?;
    }
    client_manager
        .handle_update_network_state(app.clone(), instance_id, disabled)
        .await
        .map_err(|e| error_log::other("cmd:update_network_config_state", e))?;

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
        .map_err(|e: uuid::Error| error_log::other("cmd:save_network_config", e))?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .handle_save_network_config(app, instance_id, cfg)
        .await
        .map_err(|e| error_log::other("cmd:save_network_config", e))
}

#[tauri::command]
async fn validate_config(
    app: AppHandle,
    config: NetworkConfig,
) -> Result<ValidateConfigResponse, String> {
    get_client_manager!()?
        .handle_validate_config(app, config)
        .await
        .map_err(|e| error_log::other("cmd:validate_config", e))
}

#[tauri::command]
async fn get_config(app: AppHandle, instance_id: String) -> Result<NetworkConfig, String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| error_log::other("cmd:get_config", e))?;
    let cfg = get_client_manager!()?
        .handle_get_network_config(app, instance_id)
        .await
        .map_err(|e| error_log::other("cmd:get_config", e))?;
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
        .map_err(|e| error_log::other("cmd:load_configs", e))?;
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
        .map_err(|e| error_log::other("cmd:get_network_metas", e))
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
                    .map_err(|e| error_log::other("cmd:init_service", e))?;
            } else if !path.is_dir() {
                return Err("config_dir exists but is not a directory".to_string());
            }
            let path = std::path::Path::new(&args.file_log_dir);
            if !path.exists() {
                std::fs::create_dir_all(&args.file_log_dir)
                    .map_err(|e| error_log::other("cmd:init_service", e))?;
            } else if !path.is_dir() {
                return Err("file_log_dir exists but is not a directory".to_string());
            }

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                let payload = serde_json::to_string(&args)
                    .map_err(|e| error_log::other("cmd:init_service", e))?;
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
    error_log::record("service.ctl", format!("enable={_enable}"));
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
    error_log::record("mode", format!("desktop rpc connected: {client_url}"));
    error_log::event("mode", format!("desktop rpc connected: {client_url}"));

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
    error_log::record("mode", "mobile ffi transport ready");
    error_log::event("mode", "mobile ffi transport ready");
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
            let _ = ffi_kernel::stop_config_server();
            return Ok(());
        };
        let hostname = "exvlan-android".to_string();
        ffi_kernel::start_config_server(&url, &hostname, "", false, None, std::ptr::null_mut())?;
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
        return Ok(crate::ffi_kernel::config_server_connected());
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
    let config = exvlan_rpc::log::SimpleLoggingConfig::new(
        exvlan_rpc::log::ConsoleLoggerConfig::default(),
        exvlan_rpc::log::FileLoggerConfig {
            dir: Some(log_dir.to_string_lossy().to_string()),
            level: Some(level.clone()),
            file: None,
            size_mb: Some(100),
            count: Some(10),
        },
    );
    if let Err(err) = exvlan_rpc::log::init(&config, true) {
        eprintln!("初始化 GUI 日志失败: {err:#}");
        error_log::record("log.init", format!("{err:#}"));
        return;
    }
    // 自检：记录级别与文件路径；文件大小在下次启动的 [boot] 行回报
    let path = log_dir.join("exvlan.log");
    let size = std::fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or_default();
    error_log::record(
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
        Ok(()) => error_log::record("tls", "rustls-platform-verifier initialized"),
        Err(err) => error_log::record("tls", format!("init failed: {err}")),
    }
}

#[cfg(not(target_os = "android"))]
fn init_android_tls() {}

// 获取日志目录的辅助函数
fn get_log_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, tauri::Error> {
    if cfg!(target_os = "android") {
        // Android: cache_dir + logs 子目录
        app.path().cache_dir().map(|p| p.join("logs"))
    } else {
        // 其他平台: 默认日志目录
        app.path().app_log_dir()
    }
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
    use tauri::Manager as _;
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(dir) = app.path().app_log_dir() {
        out.push(dir);
    }
    if let Ok(dir) = app.path().app_config_dir() {
        out.push(dir.join("logs"));
    }
    out.push(std::env::temp_dir().join("exvlan-logs"));
    out
}

/// 已初始化则直接返回；否则按候选顺序初始化。
fn ensure_error_log(app: &AppHandle) -> Option<std::path::PathBuf> {
    if error_log::is_initialized() {
        return error_log::path();
    }
    error_log::init_best(&error_log_candidates(app))
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
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        log_dir.as_ref().map(|p| p.display().to_string()),
        config_dir.as_ref().map(|p| p.display().to_string()),
        error_log::path().map(|p| p.display().to_string()),
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

    error_log::record("boot", line);

    // 目录不可写时单独记一条，方便直接定位
    if log_writable == Some(false) || config_writable == Some(false) {
        error_log::record(
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
            error_log::record_panic(format!(
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
        version: env!("CARGO_PKG_VERSION").to_string(),
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

/// 网络自检：实例状态 + 采集信息 + 最近事件/内核日志（排障用）。
#[tauri::command]
async fn network_diagnostics(app: AppHandle) -> Result<String, String> {
    let mut out = format!(
        "app: {}\nos: {} ({})\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    if let Ok(cm) = get_client_manager!() {
        match cm.handle_list_network_instance_ids(app.clone()).await {
            Ok(ids) => {
                out.push_str(&format!(
                    "instances: {}\n",
                    serde_json::to_string(&ids).unwrap_or_default()
                ));
            }
            Err(e) => out.push_str(&format!("list_instances failed: {e}\n")),
        }
        match cm.handle_collect_network_info(app.clone(), None).await {
            Ok(info) => {
                let text = serde_json::to_string(&info).unwrap_or_default();
                out.push_str(&format!(
                    "collect: {}\n",
                    error_log::redact(&text.chars().take(4000).collect::<String>())
                ));
            }
            Err(e) => out.push_str(&format!("collect failed: {e}\n")),
        }
    } else {
        out.push_str("client manager not initialized\n");
    }

    if let Ok(dir) = get_log_dir(&app) {
        if let Ok(text) = std::fs::read_to_string(dir.join("events.log")) {
            let tail: Vec<&str> = text.lines().rev().take(40).collect();
            out.push_str("--- events ---\n");
            out.push_str(&tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
            out.push('\n');
        }
        if let Some(p) = newest_kernel_log(&dir)
            && let Ok(text) = std::fs::read_to_string(p)
        {
            let tail: Vec<&str> = text.lines().rev().take(40).collect();
            out.push_str("--- kernel ---\n");
            out.push_str(&tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
            out.push('\n');
        }
    }

    error_log::event("diag", "network_diagnostics requested");
    Ok(out)
}
/// 前端启动状态上报（写进 `error.log` 的 `[boot.ui]`，便于对照模式/选网是否恢复）。
#[tauri::command]
fn report_boot_state(mode: String, last_instance: Option<String>, service_current: Option<bool>) {
    error_log::record(
        "boot.ui",
        format!("mode={mode} last_instance={last_instance:?} service_current={service_current:?}"),
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
    error_log::record(&scope, text);
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
    Ok(error_log::tail(limit))
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

/// 诊断上传服务（Cloudflare Worker，见 packages/diagnostics）。
const DIAG_UPLOAD_URL: &str = "https://exvlan-diag.era-x.cn/";
/// 与 Worker 的 UPLOAD_KEY 一致，仅用于防滥用（拿到也只能往私有仓库灌 issue）。
const DIAG_UPLOAD_KEY: &str = "5f06999dbefc2508435ed33ea0ccb10d";

/// 打包诊断 zip：error.log + exvlan.log 尾部 + ui-state.json（脱敏）+ 摘要。
fn build_diagnostics_zip(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    use std::io::Write as _;
    use tauri::Manager as _;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let base = app
        .path()
        .app_cache_dir()
        .or_else(|_| app.path().app_log_dir())
        .map_err(|e| format!("无法定位缓存目录: {e}"))?
        .join("diagnostics");
    std::fs::create_dir_all(&base).map_err(|e| format!("创建诊断目录失败: {e}"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let path = base.join(format!("exvlan-diagnostics-{stamp}.zip"));

    let file = std::fs::File::create(&path).map_err(|e| format!("创建 zip 失败: {e}"))?;
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // 1) error.log
    if let Some(log) = error_log::path()
        && let Ok(text) = std::fs::read_to_string(&log)
    {
        let _ = zip.start_file("error.log", opts);
        let _ = zip.write_all(error_log::redact(&text).as_bytes());
    }

    // 2) exvlan.log 尾部（内核 peer/握手/TUN 都在这里）
    if let Ok(dir) = get_log_dir(app)
        && let Some(kernel_log) = newest_kernel_log(&dir)
        && let Ok(text) = std::fs::read_to_string(&kernel_log)
    {
        let lines: Vec<&str> = text.lines().rev().take(300).collect();
        let tail: Vec<&str> = lines.into_iter().rev().collect();
        let _ = zip.start_file("exvlan.log.tail", opts);
        let _ = zip.write_all(error_log::redact(&tail.join("\n")).as_bytes());
    }

    // 3) ui-state.json（脱敏）
    if let Ok(dir) = app.path().app_config_dir()
        && let Ok(text) = std::fs::read_to_string(dir.join("ui-state.json"))
    {
        let _ = zip.start_file("ui-state.json", opts);
        let _ = zip.write_all(error_log::redact(&text).as_bytes());
    }

    // 4) events.log（GUI 事件流）
    if let Ok(dir) = get_log_dir(app)
        && let Ok(text) = std::fs::read_to_string(dir.join("events.log"))
    {
        let lines: Vec<&str> = text.lines().rev().take(500).collect();
        let tail: Vec<&str> = lines.into_iter().rev().collect();
        let _ = zip.start_file("events.log.tail", opts);
        let _ = zip.write_all(error_log::redact(&tail.join("\n")).as_bytes());
    }

    // 5) 摘要
    let summary = format!(
        "version: {}\nos: {} ({})\nhome: {}\nlog_dir: {:?}\nconfig_dir: {:?}\nerror_log: {:?}\n",
        env!("CARGO_PKG_VERSION"),
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
        error_log::path().map(|p| p.display().to_string()),
    );
    let _ = zip.start_file("summary.txt", opts);
    let _ = zip.write_all(summary.as_bytes());

    zip.finish().map_err(|e| format!("写入 zip 失败: {e}"))?;
    Ok(path)
}

/// 上传诊断到私有仓库（Cloudflare Worker → exvlan/diagnostics issue）。
#[tauri::command]
async fn upload_diagnostics(app: AppHandle) -> Result<String, String> {
    use tauri::Manager as _;

    let error_log_text = error_log::tail(24_000);
    let events_text = get_log_dir(&app)
        .ok()
        .and_then(|d| std::fs::read_to_string(d.join("events.log")).ok())
        .unwrap_or_default();
    let events_tail = events_text
        .lines()
        .rev()
        .take(400)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    let kernel_tail = get_log_dir(&app)
        .ok()
        .and_then(|d| newest_kernel_log(&d))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| {
            text.lines()
                .rev()
                .take(300)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    let summary = format!(
        "app: {}\nos: {} ({})\nmode: {}\nlog_dir: {:?}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        app.path()
            .app_config_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        get_log_dir(&app).ok().map(|p| p.display().to_string()),
    );

    let payload = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "summary": summary,
        "errorLog": error_log::redact(&error_log_text),
        "eventsTail": error_log::redact(&events_tail),
        "kernelTail": error_log::redact(&kernel_tail),
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(40))
        .build()
        .map_err(|e| error_log::other("cmd:upload_diagnostics", e))?;
    let resp = client
        .post(DIAG_UPLOAD_URL)
        .header("x-diag-key", DIAG_UPLOAD_KEY)
        .json(&payload)
        .send()
        .await
        .map_err(|e| error_log::other("cmd:upload_diagnostics", e.to_string()))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| error_log::other("cmd:upload_diagnostics", e.to_string()))?;
    if !status.is_success() {
        return Err(format!("上传失败（HTTP {status}）: {body}"));
    }
    let urls = body
        .get("urls")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    error_log::event("upload", format!("diagnostics uploaded: {urls}"));
    Ok(urls)
}

/// 分享诊断包：Android 弹系统分享面板（zip）；桌面返回路径（前端打开目录）。
#[tauri::command]
fn share_error_log(app: AppHandle) -> Result<String, String> {
    ensure_error_log(&app);
    let path = build_diagnostics_zip(&app)?;
    #[cfg(target_os = "android")]
    {
        use tauri_plugin_vpnservice::VpnserviceExt;
        app.vpnservice()
            .share_file(tauri_plugin_vpnservice::models::ShareFileRequest {
                path: path.display().to_string(),
                mime: Some("application/zip".to_string()),
                title: Some("Exvlan 诊断日志".to_string()),
            })
            .map_err(|e| error_log::other("share_error_log", e.to_string()))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = &app;
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
                    error_log::other("cmd:get_enabled_instances_with_web_like_tun_ids", e)
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
                .map_err(|e| {
                    error_log::other("cmd:get_enabled_instances_with_web_like_tun_ids", e)
                })?;

            #[cfg(target_os = "android")]
            if !cfg.get_flags().no_tun {
                match source {
                    PersistedConfigSource::User | PersistedConfigSource::Legacy => {
                        self.disable_instances_with_tun(app, false)
                            .await
                            .map_err(|e| {
                                error_log::other(
                                    "cmd:get_enabled_instances_with_web_like_tun_ids",
                                    e,
                                )
                            })?;
                    }
                    PersistedConfigSource::Web => {
                        self.disable_instances_with_tun(app, true)
                            .await
                            .map_err(|e| {
                                error_log::other(
                                    "cmd:get_enabled_instances_with_web_like_tun_ids",
                                    e,
                                )
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
                        error_log::other("cmd:get_enabled_instances_with_web_like_tun_ids", e)
                    })?,
                    source,
                )
                .map_err(|e| {
                    error_log::other("cmd:get_enabled_instances_with_web_like_tun_ids", e)
                })?;

            Ok(())
        }

        pub(super) async fn post_run_network_instance_hook(
            &self,
            app: &AppHandle,
            instance_id: &uuid::Uuid,
        ) -> Result<(), String> {
            self.storage.enabled_networks.insert(*instance_id);

            app.emit("post_run_network_instance", instance_id.to_string())
                .map_err(|e| {
                    error_log::other("cmd:get_enabled_instances_with_web_like_tun_ids", e)
                })?;

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
                .map_err(|e| {
                    error_log::other("cmd:get_enabled_instances_with_web_like_tun_ids", e)
                })?;
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
#[path = "mobile_manager.rs"]
mod manager;

#[cfg(not(target_os = "android"))]
mod service {
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
                "--file-log-level".into(),
                self.file_log_level.clone().into(),
                "--file-log-dir".into(),
                self.file_log_dir.clone().into(),
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
                crate::error_log::record(
                    "service.fingerprint",
                    "marker 匹配但 plist 参数不一致，判定需要重装",
                );
                return false;
            }
            return true;
        }

        crate::error_log::record(
            "service.fingerprint",
            format!(
                "mismatch expected={:?} stored={:?}",
                truncate_for_log(&expected, 160),
                truncate_for_log(&stored, 160)
            ),
        );
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
            let plist = format!("/Library/LaunchDaemons/{}.plist", env!("CARGO_PKG_NAME"));
            let Ok(text) = std::fs::read_to_string(&plist) else {
                crate::error_log::record("service.plist", format!("读取 {plist} 失败"));
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
                    crate::error_log::record("service.plist", format!("plist 缺少参数 {item}"));
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
        let service =
            exvlan_rpc::service_manager::Service::new(env!("CARGO_PKG_NAME").to_string())?;
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
                    crate::error_log::record(
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
            crate::error_log::record("service.install.diag", service_diagnostics(&service, &opts));
            return Err(err).with_context(|| "Failed to install service");
        }

        write_marker(&opts)?;

        // 装上了不代表能连：以「RPC 端口真的能连上」为准（launchctl 的 Running 会骗人，
        // 20710 就是守护进程起来即退出、launchd 反复重启，状态一直显示 Running）。
        if wait_until_listening(&opts.rpc_portal, std::time::Duration::from_secs(6)) {
            return Ok(());
        }
        // 再 kickstart 一次（首次 bootstrap 的 RunAtLoad 偶尔没触发）
        let _ = service.start();
        if wait_until_listening(&opts.rpc_portal, std::time::Duration::from_secs(4)) {
            return Ok(());
        }
        let diag = service_diagnostics(&service, &opts);
        crate::error_log::record("service.install.not_listening", &diag);
        Err(anyhow::anyhow!(
            "服务已安装但 RPC 端口 {} 无法连接（守护进程可能启动即退出）。诊断已写入 error.log。",
            opts.rpc_portal
        ))
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
                        use std::os::unix::fs::PermissionsExt as _;
                        let _ =
                            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
                    }
                    crate::error_log::record("service.dirs", format!("ready: {dir}"));
                }
                Err(err) => crate::error_log::record(
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
            crate::error_log::record("service.listen", format!("invalid rpc portal: {portal}"));
            return false;
        };
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if std::net::TcpStream::connect_timeout(&sock, std::time::Duration::from_millis(300))
                .is_ok()
            {
                crate::error_log::record("service.listen", format!("ok: {sock}"));
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
        let label = env!("CARGO_PKG_NAME");
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
            let plist = format!("/Library/LaunchDaemons/{label}.plist");
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
        let service =
            exvlan_rpc::service_manager::Service::new(env!("CARGO_PKG_NAME").to_string())?;
        // 幂等：本来就没装 → 视为成功（切模式/重复点击不该报错）
        if service.status()? == ServiceStatus::NotInstalled {
            return Ok(());
        }
        service.uninstall()?;
        Ok(())
    }

    pub fn set_status(enable: bool) -> anyhow::Result<()> {
        use exvlan_rpc::service_manager::*;
        let service = Service::new(env!("CARGO_PKG_NAME").to_string())?;
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
        let service =
            exvlan_rpc::service_manager::Service::new(env!("CARGO_PKG_NAME").to_string())?;
        if service.status()? == ServiceStatus::NotInstalled {
            anyhow::bail!("Service not installed");
        }
        let _ = service.stop();
        service.start().with_context(|| "Failed to start service")?;
        Ok(())
    }

    /// 强制重装服务（清指纹 → 重新 install，含 bootout/健康检查）。
    pub fn reinstall(opts: ServiceOptions) -> anyhow::Result<()> {
        let _ = std::fs::remove_file(marker_path());
        install(opts)
    }

    pub fn status() -> anyhow::Result<exvlan_rpc::service_manager::ServiceStatus> {
        let service =
            exvlan_rpc::service_manager::Service::new(env!("CARGO_PKG_NAME").to_string())?;
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
    #[cfg(target_os = "macos")]
    {
        ServiceDefaultDirs {
            config_dir: "/Library/Application Support/exvlan/config.d".to_string(),
            file_log_dir: "/Library/Logs/exvlan".to_string(),
        }
    }
    #[cfg(target_os = "windows")]
    {
        ServiceDefaultDirs {
            config_dir: r"C:\ProgramData\exvlan\config.d".to_string(),
            file_log_dir: r"C:\ProgramData\exvlan\logs".to_string(),
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        ServiceDefaultDirs {
            config_dir: "/var/lib/exvlan/config.d".to_string(),
            file_log_dir: "/var/log/exvlan".to_string(),
        }
    }
}

#[tauri::command]
fn service_default_dirs_cmd() -> ServiceDefaultDirs {
    service_default_dirs()
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
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/exvlan-elevate.log")
        {
            let _ = writeln!(f, "{line}");
        }
    }

    let code = match args.first().map(|s| s.as_str()) {
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
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn provision(req: &ProvisionPayload) -> anyhow::Result<()> {
    if let Some(src) = req.kernel_src.as_deref() {
        kernel::install_kernel(std::path::Path::new(src), &req.kernel_version)?;
        crate::error_log::record(
            "provision.kernel",
            format!("installed {}", req.kernel_version),
        );
    }
    if let Some(opts) = req.service.clone() {
        service::install(opts)?;
        crate::error_log::record("provision.service", "installed");
    }
    Ok(())
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

    error_log::record(
        "repair.start",
        format!("检测到用户目录不可写，尝试提权修复: {paths:?}"),
    );
    let json = match serde_json::to_string(&payload) {
        Ok(j) => j,
        Err(err) => {
            error_log::record("repair.failed", format!("序列化失败: {err}"));
            return;
        }
    };
    match run_elevated(&["--repair-permissions", &json]) {
        Ok(()) => {
            let fixed = log_dir.as_deref().map(dir_writable) == Some(true)
                && config_dir.as_deref().map(dir_writable) == Some(true);
            error_log::record("repair.done", format!("fixed={fixed}"));
            if fixed {
                // 修好后重新初始化日志（之前可能落到了 fallback 目录）
                ensure_error_log(app);
            }
        }
        Err(err) => error_log::record("repair.failed", err),
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
        error_log::record(
            "service.config_dir",
            format!(
                "覆盖为系统路径: {} → {}",
                opts.config_dir, defaults.config_dir
            ),
        );
        opts.config_dir = defaults.config_dir.clone();
    }
    if opts.file_log_dir != defaults.file_log_dir {
        error_log::record(
            "service.file_log_dir",
            format!(
                "覆盖为系统路径: {} → {}",
                opts.file_log_dir, defaults.file_log_dir
            ),
        );
        opts.file_log_dir = defaults.file_log_dir.clone();
    }
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
    let want_version = env!("CARGO_PKG_VERSION");
    let need_kernel =
        !kernel::is_installed() || kernel::installed_version().as_deref() != Some(want_version);
    let need_service = !service::is_current(&opts);

    if !need_kernel && !need_service {
        return Ok("current".to_string());
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

    let payload = ProvisionPayload {
        kernel_src,
        kernel_version: want_version.to_string(),
        service: need_service.then(|| opts.clone()),
    };
    let json = serde_json::to_string(&payload)
        .map_err(|e| error_log::other("cmd:ensure_kernel_service", e))?;
    error_log::record(
        "provision",
        format!("kernel={need_kernel} service={need_service}"),
    );
    run_elevated(&["--provision", &json])?;
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
        env!("CARGO_PKG_VERSION"),
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
    error_log::record("service.diagnostics", &report);
    Ok(report)
}

/// 重启服务（提权）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
fn service_restart() -> Result<(), String> {
    run_elevated(&["--service-ctl", "restart"])?;
    error_log::record("service.restart", "done");
    Ok(())
}

/// 强制重装服务（提权，含 bootout + 目录预建 + 真连自检）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn service_reinstall(opts: service::ServiceOptions) -> Result<(), String> {
    let payload =
        serde_json::to_string(&opts).map_err(|e| error_log::other("cmd:service_reinstall", e))?;
    run_elevated(&["--service-reinstall", &payload])?;
    error_log::record("service.reinstall", "done");
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
    error_log::record("kernel.install", format!("version={version}"));
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
    error_log::record("update.apply", format!("version={version}"));
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
            match ffi_kernel::poll_event() {
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
            let infos = match ffi_kernel::collect_running_network_infos(16) {
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
fn run_elevated(args: &[&str]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位自身可执行文件: {e}"))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
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
    if detail.is_empty()
        && let Ok(log) = std::fs::read_to_string("/tmp/exvlan-elevate.log")
    {
        detail = log
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
    }
    let message = format!(
        "提权命令失败（exit={}）：{}",
        output.status,
        if detail.is_empty() {
            "（无输出，详见 /tmp/exvlan-elevate.log）".to_string()
        } else {
            detail
        }
    );
    error_log::record("elevate", format!("args={args:?} {message}"));
    Err(message)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run_gui() -> std::process::ExitCode {
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
                error_log::init_events(&dir);
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
            report_boot_state,
            diagnostics_info,
            service_default_dirs_cmd,
            error_log_path,
            read_error_log,
            share_error_log,
            upload_diagnostics,
            network_diagnostics,
            ui_state::get_ui_state,
            ui_state::set_ui_state,
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
