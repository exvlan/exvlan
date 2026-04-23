use std::sync::Arc;
use tokio::sync::Mutex;

use exvlan::common::PeerId;
use exvlan::proto::api::logger::LoggerRpcClientFactory;
use exvlan::proto::rpc_impl::client::Client;
use exvlan::proto::rpc_types::controller::BaseController;

pub struct LoggerClient {
    pub client: Arc<Mutex<Client>>,
    pub from_peer_id: PeerId,
    pub to_peer_id: PeerId,
    pub domain_name: String,
}

impl LoggerClient {
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

    pub async fn set_logger_config(
        &self,
        request: exvlan::proto::api::logger::SetLoggerConfigRequest,
    ) -> anyhow::Result<exvlan::proto::api::logger::SetLoggerConfigResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<LoggerRpcClientFactory<BaseController>>(
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        );
        client
            .set_logger_config(BaseController::default(), request)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn get_logger_config(
        &self,
        request: exvlan::proto::api::logger::GetLoggerConfigRequest,
    ) -> anyhow::Result<exvlan::proto::api::logger::GetLoggerConfigResponse> {
        let client = self.client.lock().await;
        let client = client.scoped_client::<LoggerRpcClientFactory<BaseController>>(
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        );
        client
            .get_logger_config(BaseController::default(), request)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_logger_client_new() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id = 0;
        let to_peer_id = 1;
        let domain_name = "example.com".to_string();

        let logger_client = LoggerClient::new(client, from_peer_id, to_peer_id, domain_name);
        assert_eq!(logger_client.from_peer_id, 0);
        assert_eq!(logger_client.to_peer_id, 1);
        assert_eq!(logger_client.domain_name, "example.com");
    }
}
