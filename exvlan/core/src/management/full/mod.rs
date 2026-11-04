#[cfg(feature = "management")]
mod compiled;
mod config_patch;
mod instance_info;
#[cfg(feature = "management")]
mod logger_rpc;
#[cfg(feature = "management")]
pub(super) mod packet_proxy;
mod process_rpc;
#[cfg(feature = "management")]
pub mod remote_client;
mod web_client;

use std::sync::Arc;

#[cfg(any(not(feature = "management"), test))]
use exvlan_proto::api::config::ConfigRpcServer;
use exvlan_proto::api::manage::WebClientServiceServer;
#[cfg(feature = "management")]
use exvlan_proto::{
    api::logger::{LoggerRpc, LoggerRpcServer},
    api::manage::WebClientService,
    rpc_types::controller::BaseController,
};

use crate::{
    config::toml::ConfigSource,
    instance::{
        CoreInstance, CoreInstanceHost,
        manager::{InstanceFactory, InstanceManager},
    },
    rpc::service_registry::ServiceRegistry,
};

use super::{
    ConfigFileControl, ConfigFilePermission, DaemonGuard, resolve_optional_instance_by_name,
};

#[cfg(feature = "management")]
pub use compiled::register_instance_management_rpc;
pub use config_patch::{ConfigPatchPersistence, apply_config_patch};
pub use instance_info::network_instance_running_info;
#[cfg(feature = "management")]
pub use logger_rpc::{
    LoggerControl, LoggerManagementRpc, UnsupportedLoggerControl, log_level_name, parse_log_level,
};
pub use process_rpc::{
    ConfigFileStorage, InstanceMutationHooks, InstanceMutationResult, ProcessManagement,
    ProcessManagementRpc, UnsupportedConfigFileStorage,
};
#[cfg(target_os = "wasi")]
pub(crate) use web_client::WebClientBackend;
pub use web_client::{ConfigServerEndpoint, WebClient, WebClientConfig};

#[cfg(feature = "management")]
pub use super::instance_rpc::full::call_instance_json_rpc;

pub fn config_source_from_rpc(source: i32) -> Option<ConfigSource> {
    match exvlan_proto::api::manage::ConfigSource::try_from(source).ok() {
        Some(exvlan_proto::api::manage::ConfigSource::Web) => Some(ConfigSource::Web),
        Some(exvlan_proto::api::manage::ConfigSource::User) => Some(ConfigSource::User),
        _ => None,
    }
}

pub fn config_source_to_rpc(source: ConfigSource) -> i32 {
    match source {
        ConfigSource::User => exvlan_proto::api::manage::ConfigSource::User as i32,
        ConfigSource::Web => exvlan_proto::api::manage::ConfigSource::Web as i32,
    }
}

/// Registers the complete process-level management surface once.
#[cfg(feature = "management")]
pub fn register_management_rpc<F, H>(
    instances: Arc<InstanceManager<F>>,
    registry: &ServiceRegistry,
    hooks: Arc<dyn InstanceMutationHooks>,
    storage: Arc<dyn ConfigFileStorage>,
    logger: Arc<dyn LoggerControl>,
) where
    F: InstanceFactory<Instance = CoreInstance<H>, CreateContext = ()>,
    F::Error: std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    H: CoreInstanceHost,
{
    register_instance_management_rpc(instances.clone(), registry, storage.clone());
    registry.register(LoggerRpcServer::new(LoggerManagementRpc::new(logger)), "");
    registry.register(
        WebClientServiceServer::new(ProcessManagementRpc::<F>::new(instances, hooks, storage)),
        "",
    );
}

/// Registers the compact reverse-RPC surface required by exvlan-api.
#[cfg(any(not(feature = "management"), test))]
pub(crate) fn register_web_client_rpc<F, H>(
    instances: Arc<InstanceManager<F>>,
    registry: &ServiceRegistry,
    hooks: Arc<dyn InstanceMutationHooks>,
    storage: Arc<dyn ConfigFileStorage>,
) where
    F: InstanceFactory<Instance = CoreInstance<H>, CreateContext = ()>,
    F::Error: std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    H: CoreInstanceHost,
{
    let config_rpc = super::instance_rpc::InstanceManagementRpc::<F>::new_with_config_storage(
        instances.clone(),
        storage.clone(),
    );
    registry.register(ConfigRpcServer::new(config_rpc), "");
    registry.register(
        WebClientServiceServer::new(ProcessManagementRpc::<F>::new(instances, hooks, storage)),
        "",
    );
}

/// FFI/JNI 的 `WebClientService` 分发入口（实例管理面）。
///
/// 与 [`call_management_json_rpc`] 分开：`ProcessManagementRpc` 需要 hooks/storage，
/// 而 FFI 侧的 context 只持有一份已构造好的 rpc 实例。
#[cfg(feature = "management")]
pub async fn call_web_client_json_rpc<F, H>(
    rpc: &ProcessManagementRpc<F>,
    method_name: &str,
    payload: serde_json::Value,
) -> exvlan_proto::rpc_types::error::Result<serde_json::Value>
where
    F: InstanceFactory<Instance = CoreInstance<H>, CreateContext = ()>,
    F::Error: std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    H: CoreInstanceHost,
{
    WebClientService::json_call_method(rpc, BaseController::default(), method_name, payload).await
}

#[cfg(feature = "management")]
pub async fn call_management_json_rpc<F, H>(
    manager: &Arc<InstanceManager<F>>,
    logger: Arc<dyn LoggerControl>,
    service_name: &str,
    method_name: &str,
    domain_name: Option<&str>,
    payload: serde_json::Value,
) -> crate::proto::rpc_types::error::Result<serde_json::Value>
where
    F: InstanceFactory<Instance = CoreInstance<H>>,
    H: CoreInstanceHost,
{
    // 内部工具服务：GUI（尤其移动端）拿不到 core 时，用它做配置格式转换。
    // 转换逻辑只有这一份（TomlConfigLoader / NetworkConfigExt），避免多端实现漂移。
    if service_name == "exvlan.internal.ConfigConvert" {
        use crate::config::api_input::{NetworkConfig, NetworkConfigExt as _};
        use crate::config::toml::{ConfigLoader as _, TomlConfigLoader};

        return match method_name {
            "toml_to_config" => {
                let toml_text = payload
                    .get("tomlConfig")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let cfg = TomlConfigLoader::new_from_str(toml_text)?;
                let config = NetworkConfig::new_from_config(&cfg)?;
                serde_json::to_value(config).map_err(|e| anyhow::anyhow!("{e}").into())
            }
            "config_to_toml" => {
                let config: NetworkConfig =
                    serde_json::from_value(payload).map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(serde_json::Value::String(config.gen_config()?.dump()))
            }
            other => Err(anyhow::anyhow!("unknown ConfigConvert method: {other}").into()),
        };
    }

    if service_name == "api.manage.WebClientService" {
        return Err(anyhow::anyhow!(
            "service {service_name} is not exposed through FFI/JNI generic RPC"
        )
        .into());
    }
    if service_name == "api.logger.LoggerRpcService" {
        return LoggerRpc::json_call_method(
            &LoggerManagementRpc::new(logger),
            BaseController::default(),
            method_name,
            payload,
        )
        .await;
    }
    call_instance_json_rpc(manager, service_name, method_name, domain_name, payload).await
}
