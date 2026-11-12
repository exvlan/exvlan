//! 内核 RPC 客户端（薄）。
//!
//! 只做「连内核 → 调四个接口」：列表 / 读配置 / 下发 / 删除。
//! 不做热更新决策、不做端口转发 diff —— 那些在内核与 `exvlan-service` 里。

use std::sync::Arc;

use anyhow::{Context as _, bail};
use exvlan_rpc::exvlan_core::connectivity::protocol::raw::TunnelDialer as _;
use exvlan_rpc::exvlan_core::rpc::bidirect::BidirectRpcManager;
use exvlan_rpc::exvlan_proto::api::manage::{
    DeleteNetworkInstanceRequest, GetNetworkInstanceConfigRequest, ListNetworkInstanceRequest,
    NetworkConfig, RunNetworkInstanceRequest,
};
use exvlan_rpc::exvlan_proto::rpc_types::controller::BaseController;
use exvlan_rpc::transport::{RpcTransport as _, TunnelTransport};
use uuid::Uuid;

pub struct Kernel {
    transport: TunnelTransport,
    _manager: Arc<BidirectRpcManager>,
}

impl Kernel {
    /// 连接内核 RPC portal（默认 `tcp://127.0.0.1:1995`）。
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let parsed = url
            .parse()
            .with_context(|| format!("非法 RPC 地址: {url}"))?;
        let tunnel = exvlan_rpc::rpc::runtime_rpc_dialer(parsed)
            .connect()
            .await
            .with_context(|| format!("连接内核 RPC 失败: {url}"))?;
        let manager = Arc::new(BidirectRpcManager::new());
        manager.run_with_tunnel(tunnel);
        let transport = TunnelTransport {
            rpc_manager: manager.clone(),
        };
        Ok(Self {
            transport,
            _manager: manager,
        })
    }

    /// 运行中的实例 id。
    pub async fn list_ids(&self) -> anyhow::Result<Vec<String>> {
        let resp = self
            .transport
            .web_client()
            .list_network_instance(BaseController::default(), ListNetworkInstanceRequest {})
            .await
            .map_err(|e| anyhow::anyhow!("list_network_instance: {e:#}"))?;
        Ok(resp
            .inst_ids
            .into_iter()
            .map(|id| Uuid::from(id).to_string())
            .collect())
    }

    /// 读实例配置。
    pub async fn get_config(&self, id: &str) -> anyhow::Result<Option<NetworkConfig>> {
        let inst = parse_id(id)?;
        let resp = self
            .transport
            .web_client()
            .get_network_instance_config(
                BaseController::default(),
                GetNetworkInstanceConfigRequest {
                    inst_id: Some(inst.into()),
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("get_network_instance_config: {e:#}"))?;
        Ok(resp.config)
    }

    /// 下发配置（`overwrite=true` 时整实例重建）。
    pub async fn run(&self, config: NetworkConfig, overwrite: bool) -> anyhow::Result<String> {
        let resp = self
            .transport
            .web_client()
            .run_network_instance(
                BaseController::default(),
                RunNetworkInstanceRequest {
                    inst_id: None,
                    config: Some(config),
                    overwrite,
                    source: 0, // ConfigSource::User
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("run_network_instance: {e:#}"))?;
        Ok(resp
            .inst_id
            .map(|id| Uuid::from(id).to_string())
            .unwrap_or_default())
    }

    /// 删除实例。
    pub async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let inst = parse_id(id)?;
        self.transport
            .web_client()
            .delete_network_instance(
                BaseController::default(),
                DeleteNetworkInstanceRequest {
                    inst_ids: vec![inst.into()],
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("delete_network_instance: {e:#}"))?;
        Ok(())
    }
}

fn parse_id(id: &str) -> anyhow::Result<Uuid> {
    Uuid::parse_str(id).or_else(|e| bail!("非法实例 id {id}: {e}"))
}
