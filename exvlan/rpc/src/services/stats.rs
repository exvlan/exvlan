use std::sync::Arc;
use tokio::sync::Mutex;

use exvlan::common::PeerId;
use exvlan::proto::api::instance::StatsRpcClientFactory;
use exvlan::proto::rpc_types::controller::BaseController;
use exvlan::proto::rpc_impl::client::Client;

pub struct StatsClient {
    pub client: Arc<Mutex<Client>>,
    pub from_peer_id: PeerId,
    pub to_peer_id: PeerId,
    pub domain_name: String,
}

impl StatsClient {
    pub fn new(client: Arc<Mutex<Client>>, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> Self {
        Self {
            client,
            from_peer_id,
            to_peer_id,
            domain_name,
        }
    }

    pub async fn get_stats(&self, request: exvlan::proto::api::instance::GetStatsRequest) -> anyhow::Result<exvlan::proto::api::instance::GetStatsResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<StatsRpcClientFactory<BaseController>>(self.from_peer_id, self.to_peer_id, self.domain_name.clone());
        client.get_stats(BaseController::default(), request).await.map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stats_client_new() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id: PeerId = 0;
        let to_peer_id: PeerId = 1;
        let domain_name = "example.com".to_string();

        let stats_client = StatsClient::new(client, from_peer_id, to_peer_id, domain_name);
        assert_eq!(stats_client.from_peer_id, 0);
        assert_eq!(stats_client.to_peer_id, 1);
        assert_eq!(stats_client.domain_name, "example.com");
    }
}