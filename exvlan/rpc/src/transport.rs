//! RPC 传输抽象（P6 ③）。
//!
//! GUI 通过该 trait 获取 RPC 客户端，运行期按平台注入不同实现：
//! - 桌面：`TunnelTransport` —— 复用现有 `BidirectRpcManager` + tunnel（二进制协议）
//! - 移动：`FfiTransport`   —— 走 `call_json_rpc`（C ABI，protobuf JSON）
//!
//! 目前只落地桌面实现；移动端实现随 P6 ③ 的 Android 改造一起接入，
//! 以免在无 CI 验证路径的情况下改动 VPN 主链路。

#![cfg(any(feature = "rpc", feature = "ffi-rpc"))]

#[cfg(feature = "rpc")]
use std::sync::Arc;

use exvlan_proto::api::config::ConfigRpc;
use exvlan_proto::api::instance::VpnPortalRpc;
use exvlan_proto::api::logger::LoggerRpc;
use exvlan_proto::api::manage::WebClientService;
use exvlan_proto::rpc_types::controller::BaseController;

/// 客户端来源。每个方法返回可直接 `await` 的 typed client。
pub trait RpcTransport: Send + Sync {
    fn logger(&self) -> Box<dyn LoggerRpc<Controller = BaseController> + Send>;
    fn config(&self) -> Box<dyn ConfigRpc<Controller = BaseController> + Send>;
    fn vpn_portal(&self) -> Box<dyn VpnPortalRpc<Controller = BaseController> + Send>;
    /// 实例管理面（`api.manage.WebClientService`）。
    fn web_client(&self) -> Box<dyn WebClientService<Controller = BaseController> + Send>;
}

/// 桌面实现：复用 `BidirectRpcManager` 上的 typed client 工厂。
#[cfg(feature = "rpc")]
pub struct TunnelTransport {
    pub rpc_manager: Arc<exvlan_core::rpc::bidirect::BidirectRpcManager>,
}

#[cfg(feature = "rpc")]
impl RpcTransport for TunnelTransport {
    fn logger(&self) -> Box<dyn LoggerRpc<Controller = BaseController> + Send> {
        use exvlan_proto::api::logger::LoggerRpcClientFactory;
        self.rpc_manager
            .rpc_client()
            .scoped_client::<LoggerRpcClientFactory<BaseController>>(1, 1, String::new())
    }

    fn config(&self) -> Box<dyn ConfigRpc<Controller = BaseController> + Send> {
        use exvlan_proto::api::config::ConfigRpcClientFactory;
        self.rpc_manager
            .rpc_client()
            .scoped_client::<ConfigRpcClientFactory<BaseController>>(1, 1, String::new())
    }

    fn vpn_portal(&self) -> Box<dyn VpnPortalRpc<Controller = BaseController> + Send> {
        use exvlan_proto::api::instance::VpnPortalRpcClientFactory;
        self.rpc_manager
            .rpc_client()
            .scoped_client::<VpnPortalRpcClientFactory<BaseController>>(1, 1, String::new())
    }

    fn web_client(&self) -> Box<dyn WebClientService<Controller = BaseController> + Send> {
        use exvlan_proto::api::manage::WebClientServiceClientFactory;
        self.rpc_manager
            .rpc_client()
            .scoped_client::<WebClientServiceClientFactory<BaseController>>(1, 1, String::new())
    }
}

// ---------------------------------------------------------------------------
// 移动端实现：走 FFI 的 call_json_rpc
// ---------------------------------------------------------------------------

#[cfg(feature = "ffi-rpc")]
mod ffi_impl {
    use super::*;
    use exvlan_proto::api::config::{
        ConfigRpc, GetConfigRequest, GetConfigResponse, PatchConfigRequest, PatchConfigResponse,
    };
    use exvlan_proto::api::instance::{
        GetVpnPortalInfoRequest, GetVpnPortalInfoResponse, VpnPortalRpc,
    };
    use exvlan_proto::api::logger::{
        GetLoggerConfigRequest, GetLoggerConfigResponse, LoggerRpc, SetLoggerConfigRequest,
        SetLoggerConfigResponse,
    };
    use exvlan_proto::api::manage::{
        CollectNetworkInfoRequest, CollectNetworkInfoResponse, DeleteNetworkInstanceRequest,
        DeleteNetworkInstanceResponse, GetNetworkInstanceConfigRequest,
        GetNetworkInstanceConfigResponse, ListNetworkInstanceMetaRequest,
        ListNetworkInstanceMetaResponse, ListNetworkInstanceRequest, ListNetworkInstanceResponse,
        RetainNetworkInstanceRequest, RetainNetworkInstanceResponse, RunNetworkInstanceRequest,
        RunNetworkInstanceResponse, ValidateConfigRequest, ValidateConfigResponse,
        WebClientService,
    };
    use exvlan_proto::rpc_types::error::{Error, Result};

    const SVC_LOGGER: &str = "api.logger.LoggerRpcService";
    const SVC_CONFIG: &str = "api.config.ConfigRpc";
    const SVC_VPN_PORTAL: &str = "api.instance.VpnPortalRpc";
    const SVC_WEB_CLIENT: &str = "api.manage.WebClientService";

    fn call<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        service: &str,
        method: &str,
        payload: &T,
    ) -> Result<R> {
        let value = serde_json::to_value(payload)
            .map_err(|e| Error::ExecutionError(anyhow::anyhow!("序列化请求失败: {e}")))?;
        let resp = crate::ffi_rpc::call(service, method, "", value)
            .map_err(|e| Error::ExecutionError(anyhow::anyhow!(e)))?;
        serde_json::from_value(resp)
            .map_err(|e| Error::ExecutionError(anyhow::anyhow!("解析响应失败: {e}")))
    }

    /// 移动端传输：内核在应用进程内，只能经 C ABI 的 JSON-RPC 调用。
    #[derive(Default)]
    pub struct FfiTransport;

    #[async_trait::async_trait]
    impl LoggerRpc for FfiTransport {
        type Controller = BaseController;

        async fn set_logger_config(
            &self,
            _ctrl: Self::Controller,
            input: SetLoggerConfigRequest,
        ) -> Result<SetLoggerConfigResponse> {
            call(SVC_LOGGER, "set_logger_config", &input)
        }

        async fn get_logger_config(
            &self,
            _ctrl: Self::Controller,
            input: GetLoggerConfigRequest,
        ) -> Result<GetLoggerConfigResponse> {
            call(SVC_LOGGER, "get_logger_config", &input)
        }
    }

    #[async_trait::async_trait]
    impl ConfigRpc for FfiTransport {
        type Controller = BaseController;

        async fn patch_config(
            &self,
            _ctrl: Self::Controller,
            input: PatchConfigRequest,
        ) -> Result<PatchConfigResponse> {
            call(SVC_CONFIG, "patch_config", &input)
        }

        async fn get_config(
            &self,
            _ctrl: Self::Controller,
            input: GetConfigRequest,
        ) -> Result<GetConfigResponse> {
            call(SVC_CONFIG, "get_config", &input)
        }
    }

    #[async_trait::async_trait]
    impl VpnPortalRpc for FfiTransport {
        type Controller = BaseController;

        async fn get_vpn_portal_info(
            &self,
            _ctrl: Self::Controller,
            input: GetVpnPortalInfoRequest,
        ) -> Result<GetVpnPortalInfoResponse> {
            call(SVC_VPN_PORTAL, "get_vpn_portal_info", &input)
        }
    }

    #[async_trait::async_trait]
    impl WebClientService for FfiTransport {
        type Controller = BaseController;

        async fn validate_config(
            &self,
            _ctrl: Self::Controller,
            input: ValidateConfigRequest,
        ) -> Result<ValidateConfigResponse> {
            call(SVC_WEB_CLIENT, "validate_config", &input)
        }

        async fn run_network_instance(
            &self,
            _ctrl: Self::Controller,
            input: RunNetworkInstanceRequest,
        ) -> Result<RunNetworkInstanceResponse> {
            call(SVC_WEB_CLIENT, "run_network_instance", &input)
        }

        async fn retain_network_instance(
            &self,
            _ctrl: Self::Controller,
            input: RetainNetworkInstanceRequest,
        ) -> Result<RetainNetworkInstanceResponse> {
            call(SVC_WEB_CLIENT, "retain_network_instance", &input)
        }

        async fn collect_network_info(
            &self,
            _ctrl: Self::Controller,
            input: CollectNetworkInfoRequest,
        ) -> Result<CollectNetworkInfoResponse> {
            call(SVC_WEB_CLIENT, "collect_network_info", &input)
        }

        async fn list_network_instance(
            &self,
            _ctrl: Self::Controller,
            input: ListNetworkInstanceRequest,
        ) -> Result<ListNetworkInstanceResponse> {
            call(SVC_WEB_CLIENT, "list_network_instance", &input)
        }

        async fn delete_network_instance(
            &self,
            _ctrl: Self::Controller,
            input: DeleteNetworkInstanceRequest,
        ) -> Result<DeleteNetworkInstanceResponse> {
            call(SVC_WEB_CLIENT, "delete_network_instance", &input)
        }

        async fn get_network_instance_config(
            &self,
            _ctrl: Self::Controller,
            input: GetNetworkInstanceConfigRequest,
        ) -> Result<GetNetworkInstanceConfigResponse> {
            call(SVC_WEB_CLIENT, "get_network_instance_config", &input)
        }

        async fn list_network_instance_meta(
            &self,
            _ctrl: Self::Controller,
            input: ListNetworkInstanceMetaRequest,
        ) -> Result<ListNetworkInstanceMetaResponse> {
            call(SVC_WEB_CLIENT, "list_network_instance_meta", &input)
        }
    }

    impl RpcTransport for FfiTransport {
        fn logger(&self) -> Box<dyn LoggerRpc<Controller = BaseController> + Send> {
            Box::new(FfiTransport)
        }
        fn config(&self) -> Box<dyn ConfigRpc<Controller = BaseController> + Send> {
            Box::new(FfiTransport)
        }
        fn vpn_portal(&self) -> Box<dyn VpnPortalRpc<Controller = BaseController> + Send> {
            Box::new(FfiTransport)
        }
        fn web_client(&self) -> Box<dyn WebClientService<Controller = BaseController> + Send> {
            Box::new(FfiTransport)
        }
    }
}

#[cfg(feature = "ffi-rpc")]
pub use ffi_impl::FfiTransport;
