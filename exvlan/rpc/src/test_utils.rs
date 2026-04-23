//! 集成测试辅助模块
//!
//! 该模块提供了启动真实 exvlan 服务的辅助功能，用于集成测试。
//! 注意：使用此模块需要特定的环境（如 root 权限、网络命名空间支持等）

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use exvlan::common::PeerId;
use exvlan::proto::api::config::{GetConfigRequest, PatchConfigRequest};
use exvlan::proto::api::instance::{
    DumpRouteRequest, GetAclStatsRequest, GetStatsRequest, GetVpnPortalInfoRequest,
    GetWhitelistRequest, ListConnectorRequest, ListMappedListenerRequest, ListPeerRequest,
    ListPortForwardRequest, ListRouteRequest, ListTcpProxyEntryRequest,
};
use exvlan::proto::api::instance::{
    GenerateCredentialRequest, ListCredentialsRequest, RevokeCredentialRequest,
};
use exvlan::proto::api::logger::{GetLoggerConfigRequest, SetLoggerConfigRequest};
use exvlan::proto::api::manage::{
    CollectNetworkInfoRequest, DeleteNetworkInstanceRequest, GetNetworkInstanceConfigRequest,
    ListNetworkInstanceMetaRequest, ListNetworkInstanceRequest, RetainNetworkInstanceRequest,
    RunNetworkInstanceRequest, ValidateConfigRequest,
};
use exvlan::proto::peer_rpc::{GetGlobalPeerMapRequest, ReportPeersRequest};
use exvlan::proto::rpc_impl::client::Client;
use exvlan::proto::rpc_types::controller::BaseController;
use tokio::sync::Mutex;

pub fn default_controller() -> BaseController {
    BaseController::default()
}

pub async fn wait_for_condition<F, Fut>(condition: F, timeout: Duration) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if condition().await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(anyhow::anyhow!("Timeout waiting for condition"))
}

pub fn create_test_client() -> Arc<Mutex<Client>> {
    Arc::new(Mutex::new(Client::new()))
}

pub fn create_test_peer_manage_request() -> ListPeerRequest {
    ListPeerRequest::default()
}

pub fn create_test_list_route_request() -> ListRouteRequest {
    ListRouteRequest::default()
}

pub fn create_test_dump_route_request() -> DumpRouteRequest {
    DumpRouteRequest::default()
}

pub fn create_test_list_connector_request() -> ListConnectorRequest {
    ListConnectorRequest::default()
}

pub fn create_test_list_port_forward_request() -> ListPortForwardRequest {
    ListPortForwardRequest::default()
}

pub fn create_test_list_tcp_proxy_request() -> ListTcpProxyEntryRequest {
    ListTcpProxyEntryRequest::default()
}

pub fn create_test_get_vpn_portal_info_request() -> GetVpnPortalInfoRequest {
    GetVpnPortalInfoRequest::default()
}

pub fn create_test_list_mapped_listener_request() -> ListMappedListenerRequest {
    ListMappedListenerRequest::default()
}

pub fn create_test_get_stats_request() -> GetStatsRequest {
    GetStatsRequest::default()
}

pub fn create_test_get_acl_stats_request() -> GetAclStatsRequest {
    GetAclStatsRequest::default()
}

pub fn create_test_get_whitelist_request() -> GetWhitelistRequest {
    GetWhitelistRequest::default()
}

pub fn create_test_get_config_request() -> GetConfigRequest {
    GetConfigRequest::default()
}

pub fn create_test_patch_config_request() -> PatchConfigRequest {
    PatchConfigRequest::default()
}

pub fn create_test_get_logger_config_request() -> GetLoggerConfigRequest {
    GetLoggerConfigRequest::default()
}

pub fn create_test_set_logger_config_request() -> SetLoggerConfigRequest {
    SetLoggerConfigRequest::default()
}

pub fn create_test_validate_config_request() -> ValidateConfigRequest {
    ValidateConfigRequest::default()
}

pub fn create_test_run_network_instance_request() -> RunNetworkInstanceRequest {
    RunNetworkInstanceRequest::default()
}

pub fn create_test_retain_network_instance_request() -> RetainNetworkInstanceRequest {
    RetainNetworkInstanceRequest::default()
}

pub fn create_test_collect_network_info_request() -> CollectNetworkInfoRequest {
    CollectNetworkInfoRequest::default()
}

pub fn create_test_list_network_instance_request() -> ListNetworkInstanceRequest {
    ListNetworkInstanceRequest::default()
}

pub fn create_test_delete_network_instance_request() -> DeleteNetworkInstanceRequest {
    DeleteNetworkInstanceRequest::default()
}

pub fn create_test_get_network_instance_config_request() -> GetNetworkInstanceConfigRequest {
    GetNetworkInstanceConfigRequest::default()
}

pub fn create_test_list_network_instance_meta_request() -> ListNetworkInstanceMetaRequest {
    ListNetworkInstanceMetaRequest::default()
}

pub fn create_test_report_peers_request() -> ReportPeersRequest {
    ReportPeersRequest::default()
}

pub fn create_test_get_global_peer_map_request() -> GetGlobalPeerMapRequest {
    GetGlobalPeerMapRequest::default()
}

pub fn create_test_list_credentials_request() -> ListCredentialsRequest {
    ListCredentialsRequest::default()
}

pub fn create_test_generate_credential_request() -> GenerateCredentialRequest {
    GenerateCredentialRequest::default()
}

pub fn create_test_revoke_credential_request() -> RevokeCredentialRequest {
    RevokeCredentialRequest::default()
}

pub fn create_mock_peer_manage_client() -> super::services::PeerManageClient {
    super::services::PeerManageClient::new(
        create_test_client(),
        0,
        0,
        "test.example.com".to_string(),
    )
}

pub fn create_mock_connector_manage_client() -> super::services::ConnectorManageClient {
    super::services::ConnectorManageClient::new(
        create_test_client(),
        0,
        0,
        "test.example.com".to_string(),
    )
}

pub fn create_mock_stats_client() -> super::services::StatsClient {
    super::services::StatsClient::new(create_test_client(), 0, 0, "test.example.com".to_string())
}

pub fn create_mock_acl_manage_client() -> super::services::AclManageClient {
    super::services::AclManageClient::new(
        create_test_client(),
        0,
        0,
        "test.example.com".to_string(),
    )
}

pub fn create_mock_port_forward_manage_client() -> super::services::PortForwardManageClient {
    super::services::PortForwardManageClient::new(
        create_test_client(),
        0,
        0,
        "test.example.com".to_string(),
    )
}

pub fn create_mock_vpn_portal_client() -> super::services::VpnPortalClient {
    super::services::VpnPortalClient::new(
        create_test_client(),
        0,
        0,
        "test.example.com".to_string(),
    )
}

pub fn create_mock_config_client() -> super::services::ConfigClient {
    super::services::ConfigClient::new(create_test_client(), 0, 0, "test.example.com".to_string())
}

pub fn create_mock_logger_client() -> super::services::LoggerClient {
    super::services::LoggerClient::new(create_test_client(), 0, 0, "test.example.com".to_string())
}

pub fn create_mock_mapped_listener_manage_client() -> super::services::MappedListenerManageClient {
    super::services::MappedListenerManageClient::new(
        create_test_client(),
        0,
        0,
        "test.example.com".to_string(),
    )
}

pub fn create_mock_peer_center_client() -> super::services::PeerCenterClient {
    super::services::PeerCenterClient::new(
        create_test_client(),
        0,
        0,
        "test.example.com".to_string(),
    )
}

pub fn create_mock_credential_manage_client() -> super::services::CredentialManageClient {
    super::services::CredentialManageClient::new(
        create_test_client(),
        0,
        0,
        "test.example.com".to_string(),
    )
}

pub fn create_mock_instance_manage_client() -> super::services::InstanceManageClient {
    super::services::InstanceManageClient::new(
        create_test_client(),
        0,
        0,
        "test.example.com".to_string(),
    )
}

pub fn create_mock_tcp_proxy_client() -> super::services::TcpProxyClient {
    super::services::TcpProxyClient::new(
        create_test_client(),
        0,
        0,
        "test.example.com".to_string(),
        "test_type".to_string(),
    )
}

pub fn create_rpc_client_with_peers(
    from_peer_id: PeerId,
    to_peer_id: PeerId,
) -> super::client::RpcClient {
    let client = create_test_client();
    super::client::RpcClient::new(
        client,
        from_peer_id,
        to_peer_id,
        "test.example.com".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_controller() {
        let controller = default_controller();
        assert!(std::mem::size_of_val(&controller) > 0);
    }

    #[tokio::test]
    async fn test_wait_for_condition_success() {
        let result = wait_for_condition(|| async { true }, Duration::from_millis(10)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_wait_for_condition_timeout() {
        let result = wait_for_condition(|| async { false }, Duration::from_millis(50)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_test_client() {
        let client = create_test_client();
        assert!(Arc::ptr_eq(&client, &client));
    }

    #[tokio::test]
    async fn test_create_mock_peer_manage_client() {
        let client = create_mock_peer_manage_client();
        assert_eq!(client.from_peer_id, 0);
        assert_eq!(client.to_peer_id, 0);
        assert_eq!(client.domain_name, "test.example.com");
    }

    #[tokio::test]
    async fn test_create_rpc_client_with_peers() {
        let client = create_rpc_client_with_peers(100, 200);
        assert_eq!(client.from_peer_id(), 100);
        assert_eq!(client.to_peer_id(), 200);
    }

    #[test]
    fn test_create_test_requests() {
        assert!(create_test_peer_manage_request().eq(&ListPeerRequest::default()));
        assert!(create_test_list_route_request().eq(&ListRouteRequest::default()));
        assert!(create_test_dump_route_request().eq(&DumpRouteRequest::default()));
        assert!(create_test_list_connector_request().eq(&ListConnectorRequest::default()));
    }

    #[test]
    fn test_create_test_config_requests() {
        assert!(create_test_get_config_request().eq(&GetConfigRequest::default()));
        assert!(create_test_patch_config_request().eq(&PatchConfigRequest::default()));
    }

    #[test]
    fn test_create_test_logger_requests() {
        assert!(create_test_get_logger_config_request().eq(&GetLoggerConfigRequest::default()));
        assert!(create_test_set_logger_config_request().eq(&SetLoggerConfigRequest::default()));
    }

    #[test]
    fn test_create_test_instance_manage_requests() {
        assert!(create_test_validate_config_request().eq(&ValidateConfigRequest::default()));
        assert!(
            create_test_run_network_instance_request().eq(&RunNetworkInstanceRequest::default())
        );
        assert!(create_test_delete_network_instance_request()
            .eq(&DeleteNetworkInstanceRequest::default()));
    }
}
