use std::sync::Arc;

use exvlan::instance::factory::{
    NativeInstanceFactory, NativeInstanceManager, NativeProcessManagement,
    native_instance_manager_with_runtime, native_process_management,
};
use tokio::runtime::{Builder, Runtime};

struct FfiOwnedInstanceHooks;

#[async_trait::async_trait]
impl exvlan_core::management::InstanceMutationHooks for FfiOwnedInstanceHooks {
    async fn post_remove_network_instances(
        &self,
        instance_ids: &[uuid::Uuid],
    ) -> Result<(), String> {
        crate::config_server::remove_config_server_tracked_instance_ids(instance_ids);
        crate::data_plane::remove_data_plane_sessions_by_instance_ids(instance_ids);
        Ok(())
    }
}

pub(crate) struct FfiContext {
    pub(crate) runtime: Runtime,
    pub(crate) manager: Arc<NativeInstanceManager>,
    pub(crate) process_management: NativeProcessManagement,
    /// `api.manage.WebClientService` 的 JSON-RPC 分发目标（移动端 GUI 的实例管理面）。
    pub(crate) web_client_rpc: exvlan_core::management::ProcessManagementRpc<NativeInstanceFactory>,
}

impl FfiContext {
    fn new() -> Self {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime for exvlan-ffi");
        let manager = Arc::new(native_instance_manager_with_runtime(
            runtime.handle().clone(),
        ));
        let hooks: Arc<dyn exvlan_core::management::InstanceMutationHooks> =
            Arc::new(FfiOwnedInstanceHooks);
        let process_management = native_process_management(manager.clone(), hooks.clone());
        let web_client_rpc = exvlan_core::management::ProcessManagementRpc::new(
            manager.clone(),
            hooks,
            Arc::new(exvlan_core::management::UnsupportedConfigFileStorage),
        );
        Self {
            runtime,
            manager,
            process_management,
            web_client_rpc,
        }
    }
}

static FFI_CONTEXT: once_cell::sync::Lazy<FfiContext> = once_cell::sync::Lazy::new(FfiContext::new);

pub(crate) fn ffi_context() -> &'static FfiContext {
    &FFI_CONTEXT
}

pub(crate) fn resolve_instance_id_by_name(inst_name: &str) -> Result<Option<uuid::Uuid>, String> {
    exvlan_core::management::resolve_optional_instance_by_name(
        ffi_context().manager.as_ref(),
        inst_name,
    )
    .map(|instance| instance.map(|instance| instance.instance_id()))
    .map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) fn find_instance_id_by_name(inst_name: &str) -> Option<uuid::Uuid> {
    resolve_instance_id_by_name(inst_name).ok().flatten()
}
