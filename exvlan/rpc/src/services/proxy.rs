use std::sync::Arc;
use tokio::sync::Mutex;

use exvlan::common::PeerId;
use exvlan::proto::api::instance::TcpProxyRpcClientFactory;
use exvlan::proto::rpc_impl::client::Client;
use exvlan::proto::rpc_types::controller::BaseController;

pub struct TcpProxyClient {
    pub client: Arc<Mutex<Client>>,
    pub from_peer_id: PeerId,
    pub to_peer_id: PeerId,
    pub domain_name: String,
    pub client_type: String,
}

impl TcpProxyClient {
    pub fn new(
        client: Arc<Mutex<Client>>,
        from_peer_id: PeerId,
        to_peer_id: PeerId,
        domain_name: String,
        client_type: String,
    ) -> Self {
        Self {
            client,
            from_peer_id,
            to_peer_id,
            domain_name,
            client_type,
        }
    }

    pub fn client_type(&self) -> &str {
        &self.client_type
    }

    pub fn from_peer_id(&self) -> PeerId {
        self.from_peer_id
    }

    pub fn to_peer_id(&self) -> PeerId {
        self.to_peer_id
    }

    pub fn domain_name(&self) -> &str {
        &self.domain_name
    }

    pub async fn list_tcp_proxy_entry(
        &self,
        request: exvlan::proto::api::instance::ListTcpProxyEntryRequest,
    ) -> anyhow::Result<exvlan::proto::api::instance::ListTcpProxyEntryResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<TcpProxyRpcClientFactory<BaseController>>(
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        );
        client
            .list_tcp_proxy_entry(BaseController::default(), request)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tcp_proxy_client_new() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id = 0;
        let to_peer_id = 1;
        let domain_name = "example.com".to_string();
        let client_type = "test".to_string();

        let tcp_proxy_client = TcpProxyClient::new(
            client,
            from_peer_id,
            to_peer_id,
            domain_name.clone(),
            client_type.clone(),
        );

        assert_eq!(tcp_proxy_client.from_peer_id, from_peer_id);
        assert_eq!(tcp_proxy_client.to_peer_id, to_peer_id);
        assert_eq!(tcp_proxy_client.domain_name, domain_name);
        assert_eq!(tcp_proxy_client.client_type, client_type);
    }

    #[tokio::test]
    async fn test_tcp_proxy_client_type() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id = 0;
        let to_peer_id = 1;
        let domain_name = "example.com".to_string();
        let client_type = "test".to_string();

        let tcp_proxy_client =
            TcpProxyClient::new(client, from_peer_id, to_peer_id, domain_name, client_type);
        assert_eq!(tcp_proxy_client.client_type(), "test");
    }

    #[tokio::test]
    async fn test_tcp_proxy_client_accessors() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id = 42;
        let to_peer_id = 100;
        let domain_name = "test.example.com".to_string();
        let client_type = "proxy_type".to_string();

        let tcp_proxy_client = TcpProxyClient::new(
            client,
            from_peer_id,
            to_peer_id,
            domain_name.clone(),
            client_type.clone(),
        );

        assert_eq!(tcp_proxy_client.from_peer_id(), from_peer_id);
        assert_eq!(tcp_proxy_client.to_peer_id(), to_peer_id);
        assert_eq!(tcp_proxy_client.domain_name(), domain_name.as_str());
        assert_eq!(tcp_proxy_client.client_type(), client_type.as_str());
    }

    #[tokio::test]
    async fn test_tcp_proxy_client_empty_type() {
        let client = Arc::new(Mutex::new(Client::new()));
        let tcp_proxy_client = TcpProxyClient::new(client, 0, 0, String::new(), String::new());

        assert_eq!(tcp_proxy_client.client_type(), "");
        assert_eq!(tcp_proxy_client.domain_name(), "");
    }

    #[tokio::test]
    async fn test_tcp_proxy_client_max_peer_ids() {
        let client = Arc::new(Mutex::new(Client::new()));
        let max_u32 = u32::MAX;

        let tcp_proxy_client = TcpProxyClient::new(
            client,
            max_u32,
            max_u32,
            "max.test".to_string(),
            "max_type".to_string(),
        );

        assert_eq!(tcp_proxy_client.from_peer_id(), max_u32);
        assert_eq!(tcp_proxy_client.to_peer_id(), max_u32);
    }

    #[tokio::test]
    async fn test_tcp_proxy_client_special_chars_type() {
        let client = Arc::new(Mutex::new(Client::new()));
        let client_type = "type-with-special-chars_123".to_string();

        let tcp_proxy_client =
            TcpProxyClient::new(client, 0, 0, "test.com".to_string(), client_type.clone());

        assert_eq!(tcp_proxy_client.client_type(), client_type);
    }

    #[tokio::test]
    async fn test_tcp_proxy_client_client_arc() {
        let client = Arc::new(Mutex::new(Client::new()));
        let tcp_proxy_client = TcpProxyClient::new(
            client.clone(),
            0,
            0,
            "test.com".to_string(),
            "type".to_string(),
        );

        let client_ref = &tcp_proxy_client.client;
        assert!(Arc::ptr_eq(&tcp_proxy_client.client, client_ref));
    }

    #[tokio::test]
    async fn test_tcp_proxy_client_different_types() {
        let client = Arc::new(Mutex::new(Client::new()));
        let domain_name = "test.com".to_string();

        let tcp_proxy1 = TcpProxyClient::new(
            client.clone(),
            0,
            0,
            domain_name.clone(),
            "type_a".to_string(),
        );
        let tcp_proxy2 = TcpProxyClient::new(
            client.clone(),
            0,
            0,
            domain_name.clone(),
            "type_b".to_string(),
        );
        let tcp_proxy3 = TcpProxyClient::new(
            client.clone(),
            0,
            0,
            domain_name.clone(),
            "type_c".to_string(),
        );

        assert_eq!(tcp_proxy1.client_type(), "type_a");
        assert_eq!(tcp_proxy2.client_type(), "type_b");
        assert_eq!(tcp_proxy3.client_type(), "type_c");
    }
}
