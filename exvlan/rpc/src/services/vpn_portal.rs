use std::sync::Arc;
use tokio::sync::Mutex;

use exvlan::common::PeerId;
use exvlan::proto::api::instance::VpnPortalRpcClientFactory;
use exvlan::proto::rpc_impl::client::Client;
use exvlan::proto::rpc_types::controller::BaseController;

pub struct VpnPortalClient {
    pub client: Arc<Mutex<Client>>,
    pub from_peer_id: PeerId,
    pub to_peer_id: PeerId,
    pub domain_name: String,
}

impl VpnPortalClient {
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

    pub async fn get_vpn_portal_info(
        &self,
        request: exvlan::proto::api::instance::GetVpnPortalInfoRequest,
    ) -> anyhow::Result<exvlan::proto::api::instance::GetVpnPortalInfoResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<VpnPortalRpcClientFactory<BaseController>>(
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        );
        client
            .get_vpn_portal_info(BaseController::default(), request)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_vpn_portal_client_new() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id = 0;
        let to_peer_id = 1;
        let domain_name = "example.com".to_string();

        let vpn_portal_client = VpnPortalClient::new(client, from_peer_id, to_peer_id, domain_name);
        assert_eq!(vpn_portal_client.from_peer_id, 0);
        assert_eq!(vpn_portal_client.to_peer_id, 1);
        assert_eq!(vpn_portal_client.domain_name, "example.com");
    }
}
