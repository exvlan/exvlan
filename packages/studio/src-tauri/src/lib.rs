// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod elevate;
mod kernel;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod kernel_update;

use anyhow::Context;
use exvlan_core::config::api_input::{NetworkConfig, NetworkConfigExt};
use exvlan_core::config::toml::{ConfigLoader, ConfigSource, TomlConfigLoader};
use exvlan_core::management::config_source_to_rpc;
use exvlan_core::management::remote_client::{
    GetNetworkMetasResponse, ListNetworkInstanceIdsJsonResp, ListNetworkProps, RemoteClientManager,
    Storage,
};
use exvlan_proto::api::config::{
    ConfigPatchAction, ConfigRpc, ConfigRpcClientFactory, VpnPortalClientPatch,
};
use exvlan_proto::api::instance::{
    GetVpnPortalInfoRequest, InstanceIdentifier, VpnPortalInfo, VpnPortalRpc,
    VpnPortalRpcClientFactory, instance_identifier,
};
use exvlan_proto::api::manage::{
    CollectNetworkInfoResponse, ValidateConfigResponse, VpnPortalClientConfig, WebClientService,
    WebClientServiceClientFactory,
};
use exvlan_proto::rpc_types::controller::BaseController;

// 移动端仍以内核库形式在同进程内跑实例（VPN Service 插件）。
#[cfg(any(target_os = "android", target_os = "ios"))]
use exvlan::common::config::{FileLoggerConfig, LoggingConfig};
#[cfg(any(target_os = "android", target_os = "ios"))]
use exvlan::common::log;
#[cfg(target_os = "android")]
use exvlan::instance::factory::subscribe_native_instance_event;
#[cfg(any(target_os = "android", target_os = "ios"))]
use exvlan::instance::factory::{NativeInstanceManager, native_instance_manager};
#[cfg(any(target_os = "android", target_os = "ios"))]
use exvlan::proto::rpc::standalone::{runtime_rpc_dialer, runtime_rpc_listener};
#[cfg(any(target_os = "android", target_os = "ios"))]
use exvlan::rpc_service::ApiRpcServer;
#[cfg(any(target_os = "android", target_os = "ios"))]
use exvlan::utils::panic::setup_panic_handler;
#[cfg(any(target_os = "android", target_os = "ios"))]
use exvlan::web_client::{self, WebClient};
#[cfg(any(target_os = "android", target_os = "ios"))]
use exvlan_core::{
    connectivity::protocol::raw::TunnelDialer as _, process_runtime::CoreProcessRuntime,
    socket::SocketListener, tunnel::Tunnel,
};

#[cfg(any(target_os = "android", target_os = "ios"))]
use std::ops::Deref;
#[cfg(any(target_os = "android", target_os = "ios"))]
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, RwLockReadGuard};
use uuid::Uuid;

use tauri::{AppHandle, Emitter, Manager as _};

#[cfg(not(target_os = "android"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

#[cfg(any(target_os = "android", target_os = "ios"))]
static INSTANCE_MANAGER: once_cell::sync::Lazy<RwLock<Option<Arc<NativeInstanceManager>>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

#[cfg(any(target_os = "android", target_os = "ios"))]
static RPC_RING_UUID: once_cell::sync::Lazy<uuid::Uuid> =
    once_cell::sync::Lazy::new(uuid::Uuid::new_v4);

/// 移动端用进程内 ring tunnel；桌面端恒为 `()`（一律走 tcp 连本地守护进程）。
#[cfg(any(target_os = "android", target_os = "ios"))]
type LocalProcessRuntime = Option<Arc<CoreProcessRuntime>>;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
type LocalProcessRuntime = ();

static CLIENT_MANAGER: once_cell::sync::Lazy<RwLock<Option<manager::GUIClientManager>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

#[cfg(any(target_os = "android", target_os = "ios"))]
type BoxedTunnelListener = Box<dyn SocketListener<Accepted = Box<dyn Tunnel>>>;

#[cfg(any(target_os = "android", target_os = "ios"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum RpcServerKind {
    Ring,
    Tcp,
}

#[cfg(any(target_os = "android", target_os = "ios"))]
struct RpcServer {
    kind: RpcServerKind,
    _server: ApiRpcServer<BoxedTunnelListener>,
    bind_url: Option<url::Url>,
}

#[cfg(any(target_os = "android", target_os = "ios"))]
static RPC_SERVER: once_cell::sync::Lazy<Mutex<Option<RpcServer>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

#[cfg(any(target_os = "android", target_os = "ios"))]
static WEB_CLIENT: once_cell::sync::Lazy<RwLock<Option<WebClient>>> =
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
        .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
    Ok(())
}

#[tauri::command]
fn parse_network_config(cfg: NetworkConfig) -> Result<String, String> {
    let toml = cfg.gen_config().map_err(|e| e.to_string())?;
    Ok(toml.dump())
}

#[tauri::command]
fn generate_network_config(toml_config: String) -> Result<NetworkConfig, String> {
    let config = TomlConfigLoader::new_from_str(&toml_config).map_err(|e| e.to_string())?;
    let cfg = NetworkConfig::new_from_config(&config).map_err(|e| e.to_string())?;
    Ok(cfg)
}

#[tauri::command]
async fn run_network_instance(
    app: AppHandle,
    cfg: NetworkConfig,
    save: bool,
) -> Result<(), String> {
    let client_manager = get_client_manager!()?;
    let toml_config = cfg.gen_config().map_err(|e| e.to_string())?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .pre_run_network_instance_hook(&app, &toml_config, manager::PersistedConfigSource::User)
        .await?;
    client_manager
        .handle_run_network_instance(app.clone(), cfg, save)
        .await
        .map_err(|e| e.to_string())?;
    client_manager
        .post_run_network_instance_hook(&app, &toml_config.get_id())
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
        .map_err(|e: uuid::Error| e.to_string())?;
    get_client_manager!()?
        .handle_collect_network_info(app, Some(vec![instance_id]))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_vpn_portal_info(instance_id: String) -> Result<Option<VpnPortalInfo>, String> {
    let instance_id = instance_id
        .parse::<uuid::Uuid>()
        .map_err(|e| e.to_string())?;
    let client_manager = get_client_manager!()?;
    let client = client_manager
        .rpc_manager
        .rpc_client()
        .scoped_client::<VpnPortalRpcClientFactory<BaseController>>(1, 1, "".to_string());
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
        .map_err(|e| e.to_string())?;
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
        .map_err(|e| e.to_string())?;
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
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_logging_level(level: String) -> Result<(), String> {
    get_client_manager!()?
        .set_logging_level(level.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn set_tun_fd(fd: i32) -> Result<(), String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let Some(instance_manager) = INSTANCE_MANAGER.read().await.clone() else {
            return Err("set_tun_fd is not supported in remote mode".to_string());
        };
        if let Some(uuid) = get_client_manager!()?
            .get_enabled_instances_with_tun_ids()
            .next()
        {
            instance_manager
                .attach_tun_fd(uuid, fd)
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        // 桌面端 TUN 由 root 内核守护进程创建，GUI 不再传递 fd。
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
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_network_instance(app: AppHandle, instance_id: String) -> Result<(), String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .handle_remove_network_instances(app.clone(), vec![instance_id])
        .await
        .map_err(|e| e.to_string())?;
    client_manager
        .post_stop_network_instances_hook(&app)
        .await?;

    Ok(())
}

#[tauri::command]
async fn update_network_config_state(
    app: AppHandle,
    instance_id: String,
    disabled: bool,
) -> Result<(), String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    if !disabled {
        let (cfg, source) = client_manager
            .handle_get_network_config_with_source(app.clone(), instance_id)
            .await
            .map_err(|e| e.to_string())?;
        let toml_config = cfg.gen_config().map_err(|e| e.to_string())?;
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
        .map_err(|e| e.to_string())?;

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
        .map_err(|e: uuid::Error| e.to_string())?;
    let client_manager = get_client_manager!()?;
    let _mutation = client_manager.config_mutation.lock().await;
    client_manager
        .handle_save_network_config(app, instance_id, cfg)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn validate_config(
    app: AppHandle,
    config: NetworkConfig,
) -> Result<ValidateConfigResponse, String> {
    get_client_manager!()?
        .handle_validate_config(app, config)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_config(app: AppHandle, instance_id: String) -> Result<NetworkConfig, String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    let cfg = get_client_manager!()?
        .handle_get_network_config(app, instance_id)
        .await
        .map_err(|e| e.to_string())?;
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
        .map_err(|e| e.to_string())?;
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
        .map_err(|e| e.to_string())
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
                std::fs::create_dir_all(&args.config_dir).map_err(|e| e.to_string())?;
            } else if !path.is_dir() {
                return Err("config_dir exists but is not a directory".to_string());
            }
            let path = std::path::Path::new(&args.file_log_dir);
            if !path.exists() {
                std::fs::create_dir_all(&args.file_log_dir).map_err(|e| e.to_string())?;
            } else if !path.is_dir() {
                return Err("file_log_dir exists but is not a directory".to_string());
            }

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                let payload = serde_json::to_string(&args).map_err(|e| e.to_string())?;
                run_elevated(&["--install-service", &payload])?;
            }
            #[cfg(any(target_os = "android", target_os = "ios"))]
            service::install(args).map_err(|e| format!("{:#}", e))?;
        }
        None => {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            run_elevated(&["--uninstall-service"])?;
            #[cfg(any(target_os = "android", target_os = "ios"))]
            service::uninstall().map_err(|e| format!("{:#}", e))?;
        }
    }
    Ok(())
}

#[tauri::command]
fn set_service_status(_enable: bool) -> Result<(), String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        run_elevated(&["--service-ctl", if _enable { "start" } else { "stop" }])?;
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        service::set_status(_enable).map_err(|e| format!("{:#}", e))?;
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
        manager::GUIClientManager::new(Some(client_url.clone()), ()),
    )
    .await
    .map_err(|_| format!("connect rpc timed out: {client_url}"))?
    .with_context(|| format!("Failed to connect rpc: {client_url}"))
    .map_err(|e| format!("{:#}", e))?;
    *client_manager_guard = Some(client_manager);

    Ok(())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
async fn init_rpc_connection(
    _app: AppHandle,
    is_normal_mode: bool,
    url: Option<String>,
) -> Result<(), String> {
    let mut client_manager_guard =
        tokio::time::timeout(std::time::Duration::from_secs(5), CLIENT_MANAGER.write())
            .await
            .map_err(|_| "Failed to acquire write lock for client manager")?;
    let mut instance_manager_guard = INSTANCE_MANAGER
        .try_write()
        .map_err(|_| "Failed to acquire write lock for instance manager")?;
    let mut rpc_server_guard = RPC_SERVER
        .try_lock()
        .map_err(|_| "Failed to acquire lock for rpc server")?;

    let mut client_url = url.clone();
    let mut local_process_runtime = None;
    if is_normal_mode {
        let instance_manager = if let Some(im) = instance_manager_guard.take() {
            im
        } else {
            Arc::new(native_instance_manager())
        };

        let portal = url.and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

        let (desired_kind, bind_url, connect_url) = if let Some(portal) = portal {
            let (bind_url, connect_url) = normalize_normal_mode_rpc_portal(&portal)?;
            (RpcServerKind::Tcp, Some(bind_url), Some(connect_url))
        } else {
            (RpcServerKind::Ring, None, None)
        };

        let need_restart = rpc_server_guard
            .as_ref()
            .map(|x| x.kind != desired_kind || x.bind_url != bind_url)
            .unwrap_or(true);

        if need_restart {
            *rpc_server_guard = None;

            let tunnel: BoxedTunnelListener = match desired_kind {
                RpcServerKind::Ring => instance_manager
                    .process_runtime()
                    .bind_ring_tunnel(*RPC_RING_UUID.deref())
                    .map_err(|error| error.to_string())?,
                RpcServerKind::Tcp => {
                    let bind_url = bind_url.as_ref().expect("tcp rpc must have bind url");
                    Box::new(runtime_rpc_listener(resolve_rpc_bind_url(bind_url).await?))
                }
            };

            let rpc_server = ApiRpcServer::from_tunnel(tunnel, instance_manager.clone())
                .with_rx_timeout(None)
                .serve()
                .await
                .map_err(|e| e.to_string())?;
            *rpc_server_guard = Some(RpcServer {
                kind: desired_kind,
                _server: rpc_server,
                bind_url,
            });
        }

        local_process_runtime = Some(instance_manager.process_runtime());
        *instance_manager_guard = Some(instance_manager);
        client_url = connect_url.map(|u| u.to_string());
    } else {
        *rpc_server_guard = None;
    }

    let client_manager = tokio::time::timeout(
        std::time::Duration::from_millis(1000),
        manager::GUIClientManager::new(client_url, local_process_runtime),
    )
    .await
    .map_err(|_| "connect remote rpc timed out".to_string())?
    .with_context(|| "Failed to connect remote rpc")
    .map_err(|e| format!("{:#}", e))?;
    *client_manager_guard = Some(client_manager);

    if !is_normal_mode {
        drop(WEB_CLIENT.write().await.take());
        if let Some(instance_manager) = instance_manager_guard.take() {
            instance_manager
                .retain_network_instances(&[])
                .await
                .map_err(|e| e.to_string())?;
            drop(instance_manager);
        }
    }

    Ok(())
}

#[tauri::command]
async fn is_client_running() -> Result<bool, String> {
    Ok(get_client_manager!()?.rpc_manager.is_running())
}

#[tauri::command]
async fn init_web_client(app: AppHandle, url: Option<String>) -> Result<(), String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let mut web_client_guard = WEB_CLIENT.write().await;
        let Some(url) = url else {
            *web_client_guard = None;
            return Ok(());
        };
        let instance_manager = INSTANCE_MANAGER
            .try_read()
            .map_err(|_| "Failed to acquire read lock for instance manager")?
            .clone()
            .ok_or_else(|| "Instance manager is not available".to_string())?;

        let hooks = Arc::new(manager::GuiHooks { app: app.clone() });
        let machine_id_state_dir = app
            .path()
            .app_data_dir()
            .with_context(|| "Failed to resolve machine id state directory")
            .map_err(|e| format!("{:#}", e))?;

        let web_client = web_client::run_web_client(
            url.as_str(),
            exvlan::common::MachineIdOptions {
                explicit_machine_id: None,
                state_dir: Some(machine_id_state_dir),
            },
            None,
            false,
            instance_manager,
            Some(hooks),
        )
        .await
        .with_context(|| "Failed to initialize web client")
        .map_err(|e| format!("{:#}", e))?;
        *web_client_guard = Some(web_client);
    }
    // 桌面端：config-server 同步由内核守护进程负责（`--config-server`），GUI 不再自建 web client。
    let _ = (app, url);
    Ok(())
}

#[tauri::command]
async fn is_web_client_connected() -> Result<bool, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let web_client_guard = WEB_CLIENT.read().await;
        if let Some(web_client) = web_client_guard.as_ref() {
            return Ok(web_client.is_connected());
        }
    }
    Ok(false)
}

/// GUI 侧日志初始化：与内核共用 `exvlan-rpc` 的日志栈（控制台 + 滚动文件）。
fn init_gui_logging(app: &tauri::AppHandle) {
    let Ok(log_dir) = get_log_dir(app) else {
        return;
    };
    let config = exvlan_core::config::toml::LoggingConfig::builder()
        .file_logger(exvlan_core::config::toml::FileLoggerConfig {
            dir: Some(log_dir.to_string_lossy().to_string()),
            level: None,
            file: None,
            size_mb: None,
            count: None,
        })
        .build();
    if let Err(err) = exvlan_rpc::log::init(&config, true) {
        eprintln!("初始化 GUI 日志失败: {err:#}");
    }
}

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

mod manager {
    use super::*;
    use async_trait::async_trait;
    use dashmap::{DashMap, DashSet};
    #[cfg(any(target_os = "android", target_os = "ios"))]
    use exvlan::web_client::WebClientHooks;
    use exvlan_core::config::api_input::{NetworkConfig, NetworkConfigExt};
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    use exvlan_core::connectivity::protocol::raw::TunnelDialer as _;
    use exvlan_core::management::remote_client::PersistentConfig;
    use exvlan_core::rpc::bidirect::BidirectRpcManager;
    use exvlan_proto::api::logger::{LoggerRpc, LoggerRpcClientFactory, SetLoggerConfigRequest};
    use exvlan_proto::api::manage::RunNetworkInstanceRequest;
    use exvlan_proto::rpc_types::controller::BaseController;

    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub(super) struct GuiHooks {
        pub(super) app: AppHandle,
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    #[async_trait]
    impl WebClientHooks for GuiHooks {
        async fn pre_run_network_instance(
            &self,
            cfg: &exvlan_core::config::toml::TomlConfigLoader,
        ) -> Result<(), String> {
            let client_manager = get_client_manager!()?;
            client_manager
                .pre_run_network_instance_hook(
                    &self.app,
                    cfg,
                    PersistedConfigSource::from_runtime_source(cfg.get_network_config_source()),
                )
                .await
        }

        async fn post_run_network_instance(&self, instance_id: &uuid::Uuid) -> Result<(), String> {
            let client_manager = get_client_manager!()?;
            client_manager
                .post_run_network_instance_hook(&self.app, instance_id)
                .await
        }

        async fn post_remove_network_instances(&self, ids: &[uuid::Uuid]) -> Result<(), String> {
            let client_manager = get_client_manager!()?;
            client_manager
                .post_remote_remove_network_instances_hook(&self.app, ids)
                .await
        }
    }

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
        pub(super) rpc_manager: BidirectRpcManager,
        pub(super) config_mutation: Mutex<()>,
    }
    impl GUIClientManager {
        pub async fn new(
            rpc_url: Option<String>,
            _local_process_runtime: LocalProcessRuntime,
        ) -> Result<Self, anyhow::Error> {
            let tunnel = if let Some(url) = rpc_url {
                exvlan_rpc::rpc::runtime_rpc_dialer(url.parse()?)
                    .connect()
                    .await?
            } else {
                #[cfg(any(target_os = "android", target_os = "ios"))]
                {
                    _local_process_runtime
                        .context("local RPC requires a core process runtime")?
                        .connect_ring_tunnel(*RPC_RING_UUID.deref())?
                }
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                {
                    anyhow::bail!("desktop 端必须提供 rpc url（本地内核守护进程）")
                }
            };

            let rpc_manager = BidirectRpcManager::new();
            rpc_manager.run_with_tunnel(tunnel);

            Ok(Self {
                storage: GUIStorage::new(),
                rpc_manager,
                config_mutation: Mutex::new(()),
            })
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
        ) -> Result<(), exvlan_core::management::remote_client::RemoteClientError<anyhow::Error>>
        {
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
                app.emit("vpn_service_stop", "")
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        }

        pub(super) async fn pre_run_network_instance_hook(
            &self,
            app: &AppHandle,
            cfg: &exvlan_core::config::toml::TomlConfigLoader,
            source: PersistedConfigSource,
        ) -> Result<(), String> {
            let instance_id = cfg.get_id();
            app.emit("pre_run_network_instance", instance_id.to_string())
                .map_err(|e| e.to_string())?;

            #[cfg(target_os = "android")]
            if !cfg.get_flags().no_tun {
                match source {
                    PersistedConfigSource::User | PersistedConfigSource::Legacy => {
                        self.disable_instances_with_tun(app, false)
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                    PersistedConfigSource::Web => {
                        self.disable_instances_with_tun(app, true)
                            .await
                            .map_err(|e| e.to_string())?;
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
                    NetworkConfig::new_from_config(cfg).map_err(|e| e.to_string())?,
                    source,
                )
                .map_err(|e| e.to_string())?;

            Ok(())
        }

        pub(super) async fn post_run_network_instance_hook(
            &self,
            app: &AppHandle,
            instance_id: &uuid::Uuid,
        ) -> Result<(), String> {
            #[cfg(target_os = "android")]
            if let Some(instance_manager) = super::INSTANCE_MANAGER.read().await.as_ref() {
                let instance_uuid = *instance_id;
                if let Some(instance) = instance_manager.instance(instance_uuid) {
                    if let Some(mut event_receiver) = subscribe_native_instance_event(&instance) {
                        let app_clone = app.clone();
                        let instance_id_clone = *instance_id;
                        tokio::spawn(async move {
                            let instance_id_str = instance_id_clone.to_string();
                            loop {
                                match event_receiver.recv().await {
                                    Ok(exvlan::common::global_ctx::GlobalCtxEvent::DhcpIpv4Changed(_, _)) => {
                                        let _ = app_clone.emit("dhcp_ip_changed", &instance_id_str);
                                    }
                                    Ok(exvlan::common::global_ctx::GlobalCtxEvent::ProxyCidrsUpdated(_, _)) => {
                                        let _ = app_clone.emit("proxy_cidrs_updated", &instance_id_str);
                                    }
                                    Ok(_) => {}
                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                        break;
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                        let _ = app_clone.emit("event_lagged", &instance_id_str);
                                        event_receiver = event_receiver.resubscribe();
                                    }
                                }
                            }
                        });
                    }
                }
            }

            self.storage.enabled_networks.insert(*instance_id);

            app.emit("post_run_network_instance", instance_id.to_string())
                .map_err(|e| e.to_string())?;

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
                .map_err(|e| e.to_string())?;
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
            Some(
                self.rpc_manager
                    .rpc_client()
                    .scoped_client::<LoggerRpcClientFactory<BaseController>>(1, 1, "".to_string()),
            )
        }

        pub(super) async fn set_logging_level(&self, level: String) -> Result<(), anyhow::Error> {
            let logger_rpc = self
                .get_logger_rpc_client()
                .ok_or_else(|| anyhow::anyhow!("Logger RPC client not available"))?;
            logger_rpc
                .set_logger_config(
                    BaseController::default(),
                    SetLoggerConfigRequest {
                        level: exvlan_core::management::parse_log_level(&level).into(),
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
    impl RemoteClientManager<AppHandle, GUIConfig, anyhow::Error> for GUIClientManager {
        fn get_config_rpc_client(
            &self,
            _: AppHandle,
        ) -> Option<Box<dyn ConfigRpc<Controller = BaseController> + Send>> {
            Some(
                self.rpc_manager
                    .rpc_client()
                    .scoped_client::<ConfigRpcClientFactory<BaseController>>(1, 1, String::new()),
            )
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
        use exvlan_proto::api::manage::NetworkConfig;

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

    fn marker_path() -> std::path::PathBuf {
        crate::kernel::kernel_dir().join("service.fingerprint")
    }

    /// 当前已安装的服务是否与本配置一致（不需要 root：marker 可读）。
    pub fn is_current(opts: &ServiceOptions) -> bool {
        std::fs::read_to_string(marker_path())
            .map(|s| s == fingerprint(opts))
            .unwrap_or(false)
    }

    fn write_marker(opts: &ServiceOptions) -> anyhow::Result<()> {
        std::fs::write(marker_path(), fingerprint(opts))
            .map_err(|e| anyhow::anyhow!("写入服务指纹失败: {e}"))
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
        service
            .install(&options)
            .with_context(|| "Failed to install service")?;
        write_marker(&opts)?;
        Ok(())
    }

    pub fn uninstall() -> anyhow::Result<()> {
        let service =
            exvlan_rpc::service_manager::Service::new(env!("CARGO_PKG_NAME").to_string())?;
        service.uninstall()?;
        Ok(())
    }

    pub fn set_status(enable: bool) -> anyhow::Result<()> {
        use exvlan_rpc::service_manager::*;
        let service = Service::new(env!("CARGO_PKG_NAME").to_string())?;
        let status = service.status()?;
        if enable && status != ServiceStatus::Running {
            service.start().with_context(|| "Failed to start service")?;
        } else if !enable && status == ServiceStatus::Running {
            service.stop().with_context(|| "Failed to stop service")?;
        } else if status == ServiceStatus::NotInstalled {
            return Err(anyhow::anyhow!("Service not installed"));
        }
        Ok(())
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

/// 提权子命令入口（由 `main.rs` 在解析到内部参数时调用）。
///
/// 这些命令只会在通过 `elevate::Command` 以 root 拉起时执行；普通用户直接运行
/// 同名参数没有任何跳过校验的捷径（写内核目录本身就要 root）。
pub fn run_elevated_cli(args: &[String]) -> Option<std::process::ExitCode> {
    use std::process::ExitCode;

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
                    eprintln!("[kernel] 安装失败: {err:#}");
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
                        eprintln!("[service] 安装失败: {err:#}");
                        ExitCode::FAILURE
                    }
                },
                Err(err) => {
                    eprintln!("[service] 参数解析失败: {err:#}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--uninstall-service") => match service::uninstall() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("[service] 卸载失败: {err:#}");
                ExitCode::FAILURE
            }
        },
        Some("--service-ctl") => {
            let enable = args.get(1).map(|s| s.as_str()) == Some("start");
            match service::set_status(enable) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("[service] 启停失败: {err:#}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => return None,
    };
    Some(code)
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
    if !kernel::is_installed() {
        let src = kernel::bundled_kernel(&app)
            .ok_or_else(|| "未找到随包内核资源（resources/kernel/），无法安装内核".to_string())?;
        let version = env!("CARGO_PKG_VERSION");
        run_elevated(&["--install-kernel", &src.display().to_string(), version])?;
    }

    if !service::is_current(&opts) {
        let payload = serde_json::to_string(&opts).map_err(|e| e.to_string())?;
        run_elevated(&["--install-service", &payload])?;
        return Ok("installed".to_string());
    }

    Ok("current".to_string())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
async fn ensure_kernel_service(
    _app: AppHandle,
    _opts: serde_json::Value,
) -> Result<String, String> {
    Ok("current".to_string())
}

/// 内核更新信息（当前版本 / 最新版本 / 可用版本列表 / 分发源）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn kernel_update_info(source: Option<String>) -> Result<kernel_update::UpdateInfo, String> {
    let base = source.unwrap_or_else(kernel_update::base_url_override);
    kernel_update::check_update(&base, env!("CARGO_PKG_VERSION"))
        .await
        .map_err(|e| format!("{e:#}"))
}

/// 安装指定内核版本（下载 → 校验 → 提权写入系统目录 → 重启服务）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
async fn kernel_install_version(
    version: String,
    source: Option<String>,
) -> Result<(), String> {
    let base = source.unwrap_or_else(kernel_update::base_url_override);
    kernel_update::install_version(&base, &version, env!("CARGO_PKG_VERSION"))
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

/// 以 root 重新拉起自身执行内部子命令（桌面端）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn run_elevated(args: &[&str]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位自身可执行文件: {e}"))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    let output = elevate::Command::new(cmd)
        .output()
        .map_err(|e| format!("提权执行失败: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "提权命令执行失败（exit={}），可能是用户取消了授权",
            output.status
        ))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run_gui() -> std::process::ExitCode {
    // 桌面端不再自动提权：内核以独立 root 守护进程运行，GUI 始终是普通用户。
    #[cfg(any(target_os = "android", target_os = "ios"))]
    setup_panic_handler();

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

    let app = builder
        .setup(|app| {
            init_gui_logging(app.app_handle());

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
            kernel_install_version,
            set_service_status,
            get_service_status,
            init_rpc_connection,
            is_client_running,
            init_web_client,
            is_web_client_connected,
            get_log_dir_path,
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
