//! 移动端（Android / iOS）客户端管理器：不链接 `exvlan-core`。
//!
//! 桌面版见 `lib.rs` 内的 `mod manager`——它复用内核的 `Storage` / `PersistentConfig`
//! trait 与 TOML 转换（`NetworkConfigExt::gen_config`）。移动端内核在应用进程内，
//! 所有 RPC 走 [`exvlan_rpc::transport::FfiTransport`]（C ABI JSON-RPC），配置的
//! TOML 化由内核侧完成，所以这里只需要：
//!
//! 1. 一份内存存储（前端通过 `save_configs` / `save_enabled_networks` 事件落盘）；
//! 2. 把桌面 `RemoteClientManager` 的语义用**固有方法**实现一遍（命令层按名字调用）。
//!
//! 未在移动端实现的命令由命令层直接返回「该功能仅桌面端支持」。

use super::*;
use dashmap::{DashMap, DashSet};
use exvlan_rpc::exvlan_proto::api::config::{ConfigRpc, InstanceConfigPatch, PatchConfigRequest};
use exvlan_rpc::exvlan_proto::api::instance::{InstanceIdentifier, instance_identifier};
use exvlan_rpc::exvlan_proto::api::manage::{
    CollectNetworkInfoRequest, ConfigSource as RpcConfigSource, DeleteNetworkInstanceRequest,
    GetNetworkInstanceConfigRequest, ListNetworkInstanceMetaRequest, ListNetworkInstanceRequest,
    NetworkConfig, NetworkMeta, RunNetworkInstanceRequest, ValidateConfigRequest,
    ValidateConfigResponse, WebClientService,
};
use exvlan_rpc::exvlan_proto::rpc_types::controller::BaseController;

/// 与桌面版保持同一份 serde 表示（`user` / `web` / `legacy`），前端存储格式不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum PersistedConfigSource {
    User,
    #[serde(alias = "webhook")]
    Web,
    #[serde(other)]
    #[default]
    Legacy,
}

impl PersistedConfigSource {
    fn to_rpc(self) -> i32 {
        match self {
            Self::Web => RpcConfigSource::Web as i32,
            Self::User | Self::Legacy => RpcConfigSource::User as i32,
        }
    }

    fn from_rpc(source: i32) -> Option<Self> {
        match RpcConfigSource::try_from(source).ok()? {
            RpcConfigSource::User => Some(Self::User),
            RpcConfigSource::Web => Some(Self::Web),
            RpcConfigSource::Unspecified => None,
        }
    }

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

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(super) struct ListNetworkInstanceIdsJsonResp {
    pub(super) running_inst_ids: Vec<exvlan_rpc::exvlan_proto::common::Uuid>,
    pub(super) disabled_inst_ids: Vec<exvlan_rpc::exvlan_proto::common::Uuid>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(super) struct GetNetworkMetasResponse {
    pub(super) metas: std::collections::HashMap<uuid::Uuid, NetworkMeta>,
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

    fn upsert(
        &self,
        app: &AppHandle,
        inst_id: Uuid,
        config: NetworkConfig,
        source: PersistedConfigSource,
    ) -> anyhow::Result<()> {
        self.network_configs
            .insert(inst_id, GUIConfig::new(inst_id.to_string(), config, source));
        self.save_configs(app)
    }

    fn insert_enabled(
        &self,
        app: &AppHandle,
        inst_id: Uuid,
        config: NetworkConfig,
        source: PersistedConfigSource,
    ) -> anyhow::Result<()> {
        self.upsert(app, inst_id, config, source)?;
        self.enabled_networks.insert(inst_id);
        self.save_enabled_networks(app)
    }

    fn delete_configs(&self, app: &AppHandle, inst_ids: &[Uuid]) -> anyhow::Result<()> {
        for inst_id in inst_ids {
            self.network_configs.remove(inst_id);
            self.enabled_networks.remove(inst_id);
        }
        self.save_configs(app)?;
        self.save_enabled_networks(app)
    }

    fn set_enabled(&self, app: &AppHandle, inst_id: Uuid, disabled: bool) -> anyhow::Result<()> {
        if disabled {
            self.enabled_networks.remove(&inst_id);
        } else {
            self.enabled_networks.insert(inst_id);
        }
        self.save_enabled_networks(app)
    }

    fn get(&self, inst_id: &str) -> Option<GUIConfig> {
        let uuid = Uuid::parse_str(inst_id).ok()?;
        self.network_configs
            .get(&uuid)
            .map(|entry| entry.value().clone())
    }

    fn all(&self) -> Vec<GUIConfig> {
        self.network_configs
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }
}

pub(super) struct GUIClientManager {
    pub(super) storage: GUIStorage,
    /// RPC 客户端来源：移动端恒为 `FfiTransport`（call_json_rpc）。
    pub(super) transport: Box<dyn exvlan_rpc::transport::RpcTransport>,
    pub(super) config_mutation: Mutex<()>,
}

fn instance_id_of(config: &NetworkConfig) -> String {
    config.instance_id.clone().unwrap_or_default()
}

fn no_tun_of(config: &NetworkConfig) -> bool {
    config.no_tun.unwrap_or_default()
}

impl GUIClientManager {
    pub fn new_ffi() -> Self {
        Self {
            storage: GUIStorage::new(),
            transport: Box::new(exvlan_rpc::transport::FfiTransport),
            config_mutation: Mutex::new(()),
        }
    }

    fn web(&self) -> Box<dyn WebClientService<Controller = BaseController> + Send> {
        self.transport.web_client()
    }

    pub fn get_enabled_instances_with_tun_ids(&self) -> impl Iterator<Item = uuid::Uuid> + '_ {
        self.storage
            .network_configs
            .iter()
            .filter(|v| self.storage.enabled_networks.contains(v.key()))
            .filter(|v| !no_tun_of(&v.config))
            .filter_map(|c| instance_id_of(&c.config).parse::<uuid::Uuid>().ok())
    }

    #[cfg(target_os = "android")]
    pub fn get_enabled_instances_with_web_like_tun_ids(
        &self,
    ) -> impl Iterator<Item = uuid::Uuid> + '_ {
        self.storage
            .network_configs
            .iter()
            .filter(|v| self.storage.enabled_networks.contains(v.key()))
            .filter(|v| !no_tun_of(&v.config))
            .filter(|v| v.source.is_web_like())
            .filter_map(|c| instance_id_of(&c.config).parse::<uuid::Uuid>().ok())
    }

    #[cfg(target_os = "android")]
    pub(super) async fn disable_instances_with_tun(
        &self,
        app: &AppHandle,
        web_only: bool,
    ) -> anyhow::Result<()> {
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
        cfg: &NetworkConfig,
        source: PersistedConfigSource,
    ) -> Result<(), String> {
        let instance_id = instance_id_of(cfg);
        let Ok(instance_id) = instance_id.parse::<Uuid>() else {
            return Err(format!("invalid instance id: {instance_id}"));
        };
        app.emit("pre_run_network_instance", instance_id.to_string())
            .map_err(|e| e.to_string())?;

        #[cfg(target_os = "android")]
        if !no_tun_of(cfg) {
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
            .upsert(app, instance_id, cfg.clone(), source)
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub(super) async fn post_run_network_instance_hook(
        &self,
        app: &AppHandle,
        instance_id: &uuid::Uuid,
    ) -> Result<(), String> {
        self.storage.enabled_networks.insert(*instance_id);

        app.emit("post_run_network_instance", instance_id.to_string())
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub(super) async fn post_remote_remove_network_instances_hook(
        &self,
        app: &AppHandle,
        ids: &[uuid::Uuid],
    ) -> Result<(), String> {
        self.storage
            .delete_configs(app, ids)
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

    pub(super) async fn set_logging_level(&self, level: String) -> Result<(), anyhow::Error> {
        self.transport
            .logger()
            .set_logger_config(
                BaseController::default(),
                exvlan_rpc::exvlan_proto::api::logger::SetLoggerConfigRequest {
                    level: parse_log_level(&level).into(),
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
            let instance_id = instance_id_of(&stored.config);
            self.storage.network_configs.insert(
                instance_id.parse()?,
                GUIConfig::new(instance_id, stored.config, stored.source),
            );
        }

        self.storage.enabled_networks.clear();
        for id in enabled_networks {
            let Ok(uuid) = id.parse() else {
                continue;
            };
            if self.storage.enabled_networks.contains(&uuid) {
                continue;
            }
            let Some(stored) = self.storage.get(&id) else {
                continue;
            };
            self.pre_run_network_instance_hook(&app, &stored.config, stored.source)
                .await
                .map_err(|e| anyhow::anyhow!(e))?;
            self.web()
                .run_network_instance(
                    BaseController::default(),
                    RunNetworkInstanceRequest {
                        inst_id: None,
                        config: Some(stored.config),
                        overwrite: false,
                        source: stored.source.to_rpc(),
                    },
                )
                .await?;
            self.post_run_network_instance_hook(&app, &uuid)
                .await
                .map_err(|e| anyhow::anyhow!(e))?;
        }
        Ok(())
    }

    pub(super) async fn handle_validate_config(
        &self,
        _app: AppHandle,
        config: NetworkConfig,
    ) -> Result<ValidateConfigResponse, anyhow::Error> {
        Ok(self
            .web()
            .validate_config(
                BaseController::default(),
                ValidateConfigRequest {
                    config: Some(config),
                },
            )
            .await?)
    }

    pub(super) async fn handle_run_network_instance(
        &self,
        app: AppHandle,
        config: NetworkConfig,
        save: bool,
    ) -> Result<(), anyhow::Error> {
        let resp = self
            .web()
            .run_network_instance(
                BaseController::default(),
                RunNetworkInstanceRequest {
                    inst_id: None,
                    config: Some(config.clone()),
                    overwrite: true,
                    source: PersistedConfigSource::User.to_rpc(),
                },
            )
            .await?;

        if save {
            self.storage.insert_enabled(
                &app,
                resp.inst_id.unwrap_or_default().into(),
                config,
                PersistedConfigSource::User,
            )?;
        }

        Ok(())
    }

    pub(super) async fn handle_collect_network_info(
        &self,
        _app: AppHandle,
        inst_ids: Option<Vec<uuid::Uuid>>,
    ) -> Result<exvlan_rpc::exvlan_proto::api::manage::CollectNetworkInfoResponse, anyhow::Error>
    {
        Ok(self
            .web()
            .collect_network_info(
                BaseController::default(),
                CollectNetworkInfoRequest {
                    inst_ids: inst_ids
                        .unwrap_or_default()
                        .into_iter()
                        .map(|id| id.into())
                        .collect(),
                },
            )
            .await?)
    }

    pub(super) async fn handle_list_network_instance_ids(
        &self,
        _app: AppHandle,
    ) -> Result<ListNetworkInstanceIdsJsonResp, anyhow::Error> {
        let ret = self
            .web()
            .list_network_instance(BaseController::default(), ListNetworkInstanceRequest {})
            .await?;

        let running_inst_ids = ret.inst_ids.clone();
        let disabled_inst_ids = self
            .storage
            .all()
            .iter()
            .map(|cfg| Into::<exvlan_rpc::exvlan_proto::common::Uuid>::into(cfg.inst_id.clone()))
            .filter(|id| !ret.inst_ids.contains(id))
            .collect::<Vec<_>>();

        Ok(ListNetworkInstanceIdsJsonResp {
            running_inst_ids,
            disabled_inst_ids,
        })
    }

    pub(super) async fn handle_remove_network_instances(
        &self,
        app: AppHandle,
        inst_ids: Vec<uuid::Uuid>,
    ) -> Result<(), anyhow::Error> {
        if inst_ids.is_empty() {
            return Ok(());
        }
        self.storage.delete_configs(&app, &inst_ids)?;
        self.web()
            .delete_network_instance(
                BaseController::default(),
                DeleteNetworkInstanceRequest {
                    inst_ids: inst_ids.into_iter().map(|id| id.into()).collect(),
                },
            )
            .await?;
        Ok(())
    }

    pub(super) async fn handle_update_network_state(
        &self,
        app: AppHandle,
        inst_id: uuid::Uuid,
        disabled: bool,
    ) -> Result<(), anyhow::Error> {
        let (cfg, source) = self
            .handle_get_network_config_with_source(app.clone(), inst_id)
            .await?;

        if disabled {
            self.storage.insert_enabled(&app, inst_id, cfg, source)?;
            self.web()
                .delete_network_instance(
                    BaseController::default(),
                    DeleteNetworkInstanceRequest {
                        inst_ids: vec![inst_id.into()],
                    },
                )
                .await?;
        } else {
            self.web()
                .run_network_instance(
                    BaseController::default(),
                    RunNetworkInstanceRequest {
                        inst_id: Some(inst_id.into()),
                        config: Some(cfg),
                        overwrite: true,
                        source: source.to_rpc(),
                    },
                )
                .await?;
        }

        self.storage.set_enabled(&app, inst_id, disabled)?;
        Ok(())
    }

    pub(super) async fn handle_get_network_metas(
        &self,
        app: AppHandle,
        inst_ids: Vec<uuid::Uuid>,
    ) -> Result<GetNetworkMetasResponse, anyhow::Error> {
        let mut metas = std::collections::HashMap::new();

        if let Ok(resp) = self
            .web()
            .list_network_instance_meta(
                BaseController::default(),
                ListNetworkInstanceMetaRequest {
                    inst_ids: inst_ids.iter().cloned().map(|id| id.into()).collect(),
                },
            )
            .await
        {
            for meta in resp.metas {
                if let Some(inst_id) = meta.inst_id.as_ref() {
                    let inst_id: uuid::Uuid = (*inst_id).into();
                    metas.insert(inst_id, meta);
                }
            }
        }

        for instance_id in inst_ids {
            if metas.contains_key(&instance_id) {
                continue;
            }
            let (config, source) = self
                .handle_get_network_config_with_source(app.clone(), instance_id)
                .await?;
            let network_name = config.network_name.unwrap_or_default();
            metas.insert(
                instance_id,
                NetworkMeta {
                    inst_id: Some(instance_id.into()),
                    network_name: network_name.clone(),
                    config_permission: 0,
                    instance_name: network_name,
                    source: source.to_rpc(),
                },
            );
        }

        Ok(GetNetworkMetasResponse { metas })
    }

    pub(super) async fn handle_save_network_config(
        &self,
        app: AppHandle,
        inst_id: uuid::Uuid,
        config: NetworkConfig,
    ) -> Result<(), anyhow::Error> {
        self.storage
            .insert_enabled(&app, inst_id, config, PersistedConfigSource::User)?;
        self.storage.set_enabled(&app, inst_id, true)?;
        Ok(())
    }

    pub(super) async fn handle_get_network_config(
        &self,
        app: AppHandle,
        inst_id: uuid::Uuid,
    ) -> Result<NetworkConfig, anyhow::Error> {
        self.handle_get_network_config_with_source(app, inst_id)
            .await
            .map(|(config, _)| config)
    }

    pub(super) async fn handle_get_network_config_with_source(
        &self,
        _app: AppHandle,
        inst_id: uuid::Uuid,
    ) -> Result<(NetworkConfig, PersistedConfigSource), anyhow::Error> {
        if let Ok(resp) = self
            .web()
            .get_network_instance_config(
                BaseController::default(),
                GetNetworkInstanceConfigRequest {
                    inst_id: Some(inst_id.into()),
                },
            )
            .await
            && let Some(config) = resp.config
        {
            let source = PersistedConfigSource::from_rpc(resp.source)
                .or_else(|| self.storage.get(&inst_id.to_string()).map(|cfg| cfg.source))
                .unwrap_or(PersistedConfigSource::User);
            return Ok((config, source));
        }

        let stored = self
            .storage
            .get(&inst_id.to_string())
            .ok_or_else(|| anyhow::anyhow!("No such network instance: {inst_id}"))?;

        Ok((stored.config, stored.source))
    }

    pub(super) async fn handle_patch_vpn_portal_clients(
        &self,
        app: AppHandle,
        inst_id: uuid::Uuid,
        patches: Vec<exvlan_rpc::exvlan_proto::api::config::VpnPortalClientPatch>,
    ) -> Result<(), anyhow::Error> {
        if patches.is_empty() {
            return Ok(());
        }
        self.transport
            .config()
            .patch_config(
                BaseController::default(),
                PatchConfigRequest {
                    instance: Some(InstanceIdentifier {
                        selector: Some(instance_identifier::Selector::Id(inst_id.into())),
                    }),
                    patch: Some(InstanceConfigPatch {
                        vpn_portal_clients: patches,
                        ..Default::default()
                    }),
                },
            )
            .await?;

        // PatchConfig 会把改动落进内核的 TOML；这里同步 GUI 侧的已保存配置。
        let (config, source) = self
            .handle_get_network_config_with_source(app.clone(), inst_id)
            .await?;
        self.storage.upsert(&app, inst_id, config, source)?;
        Ok(())
    }
}

fn parse_log_level(level: &str) -> exvlan_rpc::exvlan_proto::api::logger::LogLevel {
    use exvlan_rpc::exvlan_proto::api::logger::LogLevel;
    match level.to_ascii_lowercase().as_str() {
        "off" | "disabled" => LogLevel::Disabled,
        "error" => LogLevel::Error,
        "warn" | "warning" => LogLevel::Warning,
        "debug" => LogLevel::Debug,
        "trace" => LogLevel::Trace,
        _ => LogLevel::Info,
    }
}
