use std::sync::Arc;
use tokio::sync::Mutex;

use exvlan::common::PeerId;
use exvlan::proto::api::manage::WebClientServiceClientFactory;
use exvlan::proto::rpc_types::controller::BaseController;
use exvlan::proto::rpc_impl::client::Client;

pub struct InstanceManageClient {
    pub client: Arc<Mutex<Client>>,
    pub from_peer_id: PeerId,
    pub to_peer_id: PeerId,
    pub domain_name: String,
}

impl InstanceManageClient {
    pub fn new(client: Arc<Mutex<Client>>, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> Self {
        Self {
            client,
            from_peer_id,
            to_peer_id,
            domain_name,
        }
    }

    pub async fn validate_config(&self, request: exvlan::proto::api::manage::ValidateConfigRequest) -> anyhow::Result<exvlan::proto::api::manage::ValidateConfigResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<WebClientServiceClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.validate_config(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn run_network_instance(&self, request: exvlan::proto::api::manage::RunNetworkInstanceRequest) -> anyhow::Result<exvlan::proto::api::manage::RunNetworkInstanceResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<WebClientServiceClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.run_network_instance(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn retain_network_instance(&self, request: exvlan::proto::api::manage::RetainNetworkInstanceRequest) -> anyhow::Result<exvlan::proto::api::manage::RetainNetworkInstanceResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<WebClientServiceClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.retain_network_instance(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn collect_network_info(&self, request: exvlan::proto::api::manage::CollectNetworkInfoRequest) -> anyhow::Result<exvlan::proto::api::manage::CollectNetworkInfoResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<WebClientServiceClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.collect_network_info(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn list_network_instance(&self, request: exvlan::proto::api::manage::ListNetworkInstanceRequest) -> anyhow::Result<exvlan::proto::api::manage::ListNetworkInstanceResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<WebClientServiceClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.list_network_instance(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn delete_network_instance(&self, request: exvlan::proto::api::manage::DeleteNetworkInstanceRequest) -> anyhow::Result<exvlan::proto::api::manage::DeleteNetworkInstanceResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<WebClientServiceClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.delete_network_instance(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn get_network_instance_config(&self, request: exvlan::proto::api::manage::GetNetworkInstanceConfigRequest) -> anyhow::Result<exvlan::proto::api::manage::GetNetworkInstanceConfigResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<WebClientServiceClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.get_network_instance_config(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn list_network_instance_meta(&self, request: exvlan::proto::api::manage::ListNetworkInstanceMetaRequest) -> anyhow::Result<exvlan::proto::api::manage::ListNetworkInstanceMetaResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<WebClientServiceClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.list_network_instance_meta(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_instance_manage_client_new() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id: PeerId = 0;
        let to_peer_id: PeerId = 1;
        let domain_name = "example.com".to_string();

        let instance_manage_client = InstanceManageClient::new(client, from_peer_id, to_peer_id, domain_name.clone());
        assert_eq!(instance_manage_client.from_peer_id, 0);
        assert_eq!(instance_manage_client.to_peer_id, 1);
        assert_eq!(instance_manage_client.domain_name, domain_name);
    }

    #[tokio::test]
    async fn test_instance_manage_client_new_with_different_ids() {
        let client = Arc::new(Mutex::new(Client::new()));
        let instance_manage_client = InstanceManageClient::new(
            client,
            42,
            84,
            "test.domain.com".to_string(),
        );
        assert_eq!(instance_manage_client.from_peer_id, 42);
        assert_eq!(instance_manage_client.to_peer_id, 84);
        assert_eq!(instance_manage_client.domain_name, "test.domain.com");
    }

    #[tokio::test]
    async fn test_instance_manage_client_new_zero_ids() {
        let client = Arc::new(Mutex::new(Client::new()));
        let instance_manage_client = InstanceManageClient::new(
            client,
            0,
            0,
            String::new(),
        );
        assert_eq!(instance_manage_client.from_peer_id, 0);
        assert_eq!(instance_manage_client.to_peer_id, 0);
        assert_eq!(instance_manage_client.domain_name, "");
    }

    #[tokio::test]
    async fn test_instance_manage_client_new_max_u32_ids() {
        let client = Arc::new(Mutex::new(Client::new()));
        let max_id = u32::MAX;
        let instance_manage_client = InstanceManageClient::new(
            client,
            max_id,
            max_id,
            "max.test".to_string(),
        );
        assert_eq!(instance_manage_client.from_peer_id, max_id);
        assert_eq!(instance_manage_client.to_peer_id, max_id);
    }

    #[tokio::test]
    async fn test_instance_manage_client_special_chars_domain() {
        let client = Arc::new(Mutex::new(Client::new()));
        let domain = "instance-domain_123.test.com".to_string();
        let instance_manage_client = InstanceManageClient::new(
            client,
            0,
            0,
            domain.clone(),
        );
        assert_eq!(instance_manage_client.domain_name, domain);
    }

    #[tokio::test]
    async fn test_instance_manage_client_arc_shared() {
        let client = Arc::new(Mutex::new(Client::new()));
        let client_clone = client.clone();
        let instance_manage_client = InstanceManageClient::new(
            client,
            10,
            20,
            "shared.test".to_string(),
        );
        assert!(Arc::ptr_eq(&instance_manage_client.client, &client_clone));
    }

    #[tokio::test]
    async fn test_instance_manage_validate_config_error() {
        let client = Arc::new(Mutex::new(Client::new()));
        let instance_manage_client = InstanceManageClient::new(
            client,
            u32::MAX,
            0,
            "error.test".to_string(),
        );
        let request = exvlan::proto::api::manage::ValidateConfigRequest::default();
        let result = instance_manage_client.validate_config(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_instance_manage_run_network_instance_error() {
        let client = Arc::new(Mutex::new(Client::new()));
        let instance_manage_client = InstanceManageClient::new(
            client,
            0,
            u32::MAX,
            "error.test".to_string(),
        );
        let request = exvlan::proto::api::manage::RunNetworkInstanceRequest::default();
        let result = instance_manage_client.run_network_instance(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_instance_manage_retain_network_instance_error() {
        let client = Arc::new(Mutex::new(Client::new()));
        let instance_manage_client = InstanceManageClient::new(
            client,
            100,
            200,
            "retain.test".to_string(),
        );
        let request = exvlan::proto::api::manage::RetainNetworkInstanceRequest::default();
        let result = instance_manage_client.retain_network_instance(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_instance_manage_collect_network_info_error() {
        let client = Arc::new(Mutex::new(Client::new()));
        let instance_manage_client = InstanceManageClient::new(
            client,
            100,
            200,
            "collect.test".to_string(),
        );
        let request = exvlan::proto::api::manage::CollectNetworkInfoRequest::default();
        let result = instance_manage_client.collect_network_info(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_instance_manage_list_network_instance_error() {
        let client = Arc::new(Mutex::new(Client::new()));
        let instance_manage_client = InstanceManageClient::new(
            client,
            100,
            200,
            "list.test".to_string(),
        );
        let request = exvlan::proto::api::manage::ListNetworkInstanceRequest::default();
        let result = instance_manage_client.list_network_instance(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_instance_manage_delete_network_instance_error() {
        let client = Arc::new(Mutex::new(Client::new()));
        let instance_manage_client = InstanceManageClient::new(
            client,
            100,
            200,
            "delete.test".to_string(),
        );
        let request = exvlan::proto::api::manage::DeleteNetworkInstanceRequest::default();
        let result = instance_manage_client.delete_network_instance(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_instance_manage_get_network_instance_config_error() {
        let client = Arc::new(Mutex::new(Client::new()));
        let instance_manage_client = InstanceManageClient::new(
            client,
            100,
            200,
            "config.test".to_string(),
        );
        let request = exvlan::proto::api::manage::GetNetworkInstanceConfigRequest::default();
        let result = instance_manage_client.get_network_instance_config(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_instance_manage_list_network_instance_meta_error() {
        let client = Arc::new(Mutex::new(Client::new()));
        let instance_manage_client = InstanceManageClient::new(
            client,
            100,
            200,
            "meta.test".to_string(),
        );
        let request = exvlan::proto::api::manage::ListNetworkInstanceMetaRequest::default();
        let result = instance_manage_client.list_network_instance_meta(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_instance_manage_client_different_domain_names() {
        let domains = vec![
            "node1.example.com",
            "node2.example.com",
            "instance-1.region.test",
            "",
        ];

        for domain in domains {
            let client = Arc::new(Mutex::new(Client::new()));
            let instance_manage_client = InstanceManageClient::new(
                client,
                0,
                0,
                domain.to_string(),
            );
            assert_eq!(instance_manage_client.domain_name, domain);
        }
    }

    #[tokio::test]
    async fn test_instance_manage_multiple_clients_independence() {
        let client1 = Arc::new(Mutex::new(Client::new()));
        let client2 = Arc::new(Mutex::new(Client::new()));

        let inst1 = InstanceManageClient::new(
            client1.clone(),
            1,
            2,
            "first.test".to_string(),
        );
        let inst2 = InstanceManageClient::new(
            client2.clone(),
            3,
            4,
            "second.test".to_string(),
        );

        assert_ne!(inst1.from_peer_id, inst2.from_peer_id);
        assert_ne!(inst1.to_peer_id, inst2.to_peer_id);
        assert_ne!(inst1.domain_name, inst2.domain_name);
    }
}