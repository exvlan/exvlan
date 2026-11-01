//! RPC 传输抽象（P6 ③）。
//!
//! GUI 通过该 trait 获取 RPC 客户端，运行期按平台注入不同实现：
//! - 桌面：`TunnelTransport` —— 复用现有 `BidirectRpcManager` + tunnel（二进制协议）
//! - 移动：`FfiTransport`   —— 走 `call_json_rpc`（C ABI，protobuf JSON）
//!
//! 目前只落地桌面实现；移动端实现随 P6 ③ 的 Android 改造一起接入，
//! 以免在无 CI 验证路径的情况下改动 VPN 主链路。

#![cfg(feature = "gui-client")]

use std::sync::Arc;

use exvlan_proto::api::config::ConfigRpc;
use exvlan_proto::api::instance::VpnPortalRpc;
use exvlan_proto::api::logger::LoggerRpc;
use exvlan_proto::rpc_types::controller::BaseController;

/// 客户端来源。每个方法返回可直接 `await` 的 typed client。
pub trait RpcTransport: Send + Sync {
    fn logger(&self) -> Box<dyn LoggerRpc<Controller = BaseController> + Send>;
    fn config(&self) -> Box<dyn ConfigRpc<Controller = BaseController> + Send>;
    fn vpn_portal(&self) -> Box<dyn VpnPortalRpc<Controller = BaseController> + Send>;
}

/// 桌面实现：复用 `BidirectRpcManager` 上的 typed client 工厂。
pub struct TunnelTransport {
    pub rpc_manager: Arc<exvlan_core::rpc::bidirect::BidirectRpcManager>,
}

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
}
