use std::sync::Arc;
use tokio::sync::Mutex;

use exvlan::common::PeerId;
use exvlan::proto::api::instance::PeerManageRpcClientFactory;
use exvlan::proto::rpc_types::controller::BaseController;
use exvlan::proto::rpc_impl::client::Client;

pub struct PeerManageClient {
    pub client: Arc<Mutex<Client>>,
    pub from_peer_id: PeerId,
    pub to_peer_id: PeerId,
    pub domain_name: String,
}

impl PeerManageClient {
    pub fn new(client: Arc<Mutex<Client>>, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> Self {
        Self {
            client,
            from_peer_id,
            to_peer_id,
            domain_name,
        }
    }

    pub async fn list_peer(&self, request: exvlan::proto::api::instance::ListPeerRequest) -> anyhow::Result<exvlan::proto::api::instance::ListPeerResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<PeerManageRpcClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.list_peer(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn list_route(&self, request: exvlan::proto::api::instance::ListRouteRequest) -> anyhow::Result<exvlan::proto::api::instance::ListRouteResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<PeerManageRpcClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.list_route(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn dump_route(&self, request: exvlan::proto::api::instance::DumpRouteRequest) -> anyhow::Result<exvlan::proto::api::instance::DumpRouteResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<PeerManageRpcClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.dump_route(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_peer_manage_client_new() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id = 0;
        let to_peer_id = 1;
        let domain_name = "example.com".to_string();

        let peer_manage_client = PeerManageClient::new(client, from_peer_id, to_peer_id, domain_name);
        assert_eq!(peer_manage_client.from_peer_id, 0);
        assert_eq!(peer_manage_client.to_peer_id, 1);
        assert_eq!(peer_manage_client.domain_name, "example.com");
    }
}