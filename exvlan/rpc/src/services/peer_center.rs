use std::sync::Arc;
use tokio::sync::Mutex;

use exvlan::common::PeerId;
use exvlan::proto::peer_rpc::PeerCenterRpcClientFactory;
use exvlan::proto::rpc_impl::client::Client;
use exvlan::proto::rpc_types::controller::BaseController;

pub struct PeerCenterClient {
    pub client: Arc<Mutex<Client>>,
    pub from_peer_id: PeerId,
    pub to_peer_id: PeerId,
    pub domain_name: String,
}

impl PeerCenterClient {
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

    pub async fn report_peers(
        &self,
        request: exvlan::proto::peer_rpc::ReportPeersRequest,
    ) -> anyhow::Result<exvlan::proto::peer_rpc::ReportPeersResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<PeerCenterRpcClientFactory<BaseController>>(
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        );
        client
            .report_peers(BaseController::default(), request)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn get_global_peer_map(
        &self,
        request: exvlan::proto::peer_rpc::GetGlobalPeerMapRequest,
    ) -> anyhow::Result<exvlan::proto::peer_rpc::GetGlobalPeerMapResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<PeerCenterRpcClientFactory<BaseController>>(
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        );
        client
            .get_global_peer_map(BaseController::default(), request)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_peer_center_client_new() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id = 0;
        let to_peer_id = 1;
        let domain_name = "example.com".to_string();

        let peer_center_client =
            PeerCenterClient::new(client, from_peer_id, to_peer_id, domain_name);
        assert_eq!(peer_center_client.from_peer_id, 0);
        assert_eq!(peer_center_client.to_peer_id, 1);
        assert_eq!(peer_center_client.domain_name, "example.com");
    }
}
