use std::sync::Arc;
use tokio::sync::Mutex;

use exvlan::common::PeerId;
use exvlan::proto::api::config::ConfigRpcClientFactory;
use exvlan::proto::rpc_impl::client::Client;
use exvlan::proto::rpc_types::controller::BaseController;

pub struct ConfigClient {
    pub client: Arc<Mutex<Client>>,
    pub from_peer_id: PeerId,
    pub to_peer_id: PeerId,
    pub domain_name: String,
}

impl ConfigClient {
    pub fn new(
        client: Arc<Mutex<Client>>,
        from_peer_id: PeerId,
        to_peer_id: PeerId,
        domain_name: String,
    ) -> Self {
        Self {
            client,
            from_peer_id,
            to_peer_id,
            domain_name,
        }
    }

    pub async fn get_config(
        &self,
        request: exvlan::proto::api::config::GetConfigRequest,
    ) -> anyhow::Result<exvlan::proto::api::config::GetConfigResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<ConfigRpcClientFactory<BaseController>>(
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        );
        client
            .get_config(BaseController::default(), request)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn patch_config(
        &self,
        request: exvlan::proto::api::config::PatchConfigRequest,
    ) -> anyhow::Result<exvlan::proto::api::config::PatchConfigResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<ConfigRpcClientFactory<BaseController>>(
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        );
        client
            .patch_config(BaseController::default(), request)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_config_client_new() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id = 0;
        let to_peer_id = 1;
        let domain_name = "example.com".to_string();

        let config_client = ConfigClient::new(client, from_peer_id, to_peer_id, domain_name);
        assert_eq!(config_client.from_peer_id, 0);
        assert_eq!(config_client.to_peer_id, 1);
        assert_eq!(config_client.domain_name, "example.com");
    }
}
