//! 内核 RPC 客户端（薄）。
//!
//! 只做「连内核 → 调四个接口」：列表 / 读配置 / 下发 / 删除。
//! 不做热更新决策、不做端口转发 diff —— 那些在内核与 `exvlan-service` 里。

use std::sync::Arc;

use anyhow::{Context as _, bail};
use exvlan_rpc::exvlan_core::connectivity::protocol::raw::TunnelDialer as _;
use exvlan_rpc::exvlan_core::rpc::bidirect::BidirectRpcManager;
use exvlan_rpc::exvlan_proto::api::config::{
    ConfigPatchAction, PatchConfigRequest, VpnPortalClientPatch,
};
use exvlan_rpc::exvlan_proto::api::manage::{
    CollectNetworkInfoRequest, DeleteNetworkInstanceRequest, GetNetworkInstanceConfigRequest,
    ListNetworkInstanceMetaRequest, ListNetworkInstanceRequest, NetworkConfig,
    RetainNetworkInstanceRequest, RunNetworkInstanceRequest, ValidateConfigRequest,
    VpnPortalClientConfig,
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

    /// 停止实例但**不动配置目录**（`retain` 只保留给定 id，其余被停掉）。
    ///
    /// 为什么要单独一个：`delete` 在「实例由配置目录加载」时会顺手删掉 `<id>.toml`，
    /// 用它做「停用」会直接丢配置（R1 实测）。停用应该：先改名成 `.disabled`，
    /// 再用这里停掉实例。
    pub async fn stop(&self, id: &str) -> anyhow::Result<()> {
        let inst = parse_id(id)?;
        let keep: Vec<_> = self
            .list_ids()
            .await?
            .into_iter()
            .filter_map(|other| parse_id(&other).ok())
            .filter(|other| *other != inst)
            .map(Into::into)
            .collect();
        self.transport
            .web_client()
            .retain_network_instance(
                BaseController::default(),
                RetainNetworkInstanceRequest { inst_ids: keep },
            )
            .await
            .map_err(|e| anyhow::anyhow!("retain_network_instance: {e:#}"))?;
        Ok(())
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

impl Kernel {
    /// 运行信息（collect_network_info）。
    pub async fn collect_info(&self, id: &str) -> anyhow::Result<serde_json::Value> {
        let inst = parse_id(id)?;
        let resp = self
            .transport
            .web_client()
            .collect_network_info(
                BaseController::default(),
                CollectNetworkInfoRequest {
                    inst_ids: vec![inst.into()],
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("collect_network_info: {e:#}"))?;
        Ok(serde_json::to_value(resp).unwrap_or(serde_json::Value::Null))
    }

    /// 实例元信息（名字/来源等）。
    pub async fn meta(&self, id: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let inst = parse_id(id)?;
        let resp = self
            .transport
            .web_client()
            .list_network_instance_meta(
                BaseController::default(),
                ListNetworkInstanceMetaRequest {
                    inst_ids: vec![inst.into()],
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("list_network_instance_meta: {e:#}"))?;
        Ok(resp
            .metas
            .into_iter()
            .next()
            .and_then(|m| serde_json::to_value(m).ok()))
    }

    /// 校验配置并返回内核 TOML（内核自己 `gen_config().dump()`）。
    pub async fn validate(&self, config: NetworkConfig) -> anyhow::Result<String> {
        let resp = self
            .transport
            .web_client()
            .validate_config(
                BaseController::default(),
                ValidateConfigRequest {
                    config: Some(config),
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("validate_config: {e:#}"))?;
        Ok(resp.toml_config)
    }

    /// 改单条 VPN portal 客户端（add/remove/clear）。
    pub async fn patch_portal(
        &self,
        id: &str,
        action: &str,
        client: Option<VpnPortalClientConfig>,
    ) -> anyhow::Result<()> {
        let inst = parse_id(id)?;
        let action = match action {
            "add" => ConfigPatchAction::Add as i32,
            "remove" => ConfigPatchAction::Remove as i32,
            "clear" => ConfigPatchAction::Clear as i32,
            other => bail!("未知 portal action: {other}（add|remove|clear）"),
        };
        let patch = VpnPortalClientPatch { action, client };
        self.transport
            .config()
            .patch_config(
                BaseController::default(),
                PatchConfigRequest {
                    instance: Some(exvlan_rpc::exvlan_proto::api::instance::InstanceIdentifier {
                        selector: Some(
                            exvlan_rpc::exvlan_proto::api::instance::instance_identifier::Selector::Id(
                                inst.into(),
                            ),
                        ),
                    }),
                    patch: Some(exvlan_rpc::exvlan_proto::api::config::InstanceConfigPatch {
                        vpn_portal_clients: vec![patch],
                        ..Default::default()
                    }),
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("patch_config: {e:#}"))?;
        Ok(())
    }
}

fn parse_id(id: &str) -> anyhow::Result<Uuid> {
    Uuid::parse_str(id).or_else(|e| bail!("非法实例 id {id}: {e}"))
}
