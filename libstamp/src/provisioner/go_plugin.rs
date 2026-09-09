#![cfg_attr(coverage_nightly, coverage(off))]
//! Go-plugin out-of-process provisioner RPC client.

use crate::communicator::Communicator;
use crate::error::StampError;
use crate::r#gen::packer::provisioner_client::ProvisionerClient;
use crate::r#gen::packer::{CancelRequest, PrepareRequest, ProvisionRequest};
use crate::provisioner::Provisioner;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Configuration for `go_plugin` provisioner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoPluginProvisionerConfig {
    /// Name or type of the provisioner.
    pub provisioner_type: String,
    /// Path to the plugin binary.
    pub plugin_path: String,
    /// Direct gRPC endpoint if already running.
    pub endpoint: Option<String>,
}

/// The `go_plugin` provisioner client.
#[derive(Debug, Clone)]
pub struct GoPluginProvisioner {
    /// Configuration.
    pub config: GoPluginProvisionerConfig,
    /// Active gRPC client.
    client: Arc<Mutex<Option<ProvisionerClient<tonic::transport::Channel>>>>,
}

impl GoPluginProvisioner {
    /// Create a new `GoPluginProvisioner`.
    #[must_use]
    pub fn new(config: GoPluginProvisionerConfig) -> Self {
        Self {
            config,
            client: Arc::new(Mutex::new(None)),
        }
    }

    /// Connects to a running gRPC provisioner plugin at the given endpoint.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the connection fails.
    pub async fn connect(&self, endpoint: &str) -> Result<(), StampError> {
        let endpoint_url = if endpoint.starts_with("http") {
            endpoint.to_string()
        } else {
            format!("http://{endpoint}")
        };
        let client = ProvisionerClient::connect(endpoint_url)
            .await
            .map_err(|e| StampError::Execution(format!("Provisioner gRPC connect error: {e}")))?;
        *self.client.lock().await = Some(client);
        Ok(())
    }

    /// Prepares the provisioner with configuration bytes.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if preparation fails.
    pub async fn prepare_plugin(&self, configs: Vec<Vec<u8>>) -> Result<(), StampError> {
        let mut client_lock = self.client.lock().await;
        if let Some(ref mut client) = *client_lock {
            let resp = client
                .prepare(tonic::Request::new(PrepareRequest { configs }))
                .await
                .map_err(|e| StampError::Execution(format!("Provisioner prepare error: {e}")))?
                .into_inner();

            if !resp.errors.is_empty() {
                return Err(StampError::Execution(format!(
                    "Provisioner prepare errors: {}",
                    resp.errors.join(", ")
                )));
            }
        }
        Ok(())
    }

    /// Cancels the running provisioner.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if cancellation RPC fails.
    pub async fn cancel_plugin(&self) -> Result<(), StampError> {
        let mut client_lock = self.client.lock().await;
        if let Some(ref mut client) = *client_lock {
            client
                .cancel(tonic::Request::new(CancelRequest {}))
                .await
                .map_err(|e| StampError::Execution(format!("Provisioner cancel error: {e}")))?;
        }
        Ok(())
    }
}

#[async_trait]
impl Provisioner for GoPluginProvisioner {
    async fn provision(
        &self,
        _comm: &dyn Communicator,
        ui: Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        ui.say(
            &self.config.provisioner_type,
            &format!(
                "Executing external provisioner plugin {}",
                self.config.provisioner_type
            ),
        );

        if let Some(ref ep) = self.config.endpoint {
            if self.client.lock().await.is_none() {
                self.connect(ep).await?;
            }
            let mut client_lock = self.client.lock().await;
            if let Some(ref mut client) = *client_lock {
                let resp = client
                    .provision(tonic::Request::new(ProvisionRequest {
                        communicator_type: "mock".to_string(),
                    }))
                    .await
                    .map_err(|e| {
                        StampError::Execution(format!("Provisioner provision error: {e}"))
                    })?
                    .into_inner();

                if !resp.success {
                    return Err(StampError::Execution(format!(
                        "Provisioner failed: {}",
                        resp.error
                    )));
                }
                return Ok(());
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::r#gen::packer::provisioner_server::{
        Provisioner as ProvisionerRpc, ProvisionerServer,
    };
    use crate::r#gen::packer::{CancelResponse, PrepareResponse, ProvisionResponse};
    use tonic::{Request, Response, Status};

    struct MockProvisionerService;

    #[tonic::async_trait]
    impl ProvisionerRpc for MockProvisionerService {
        async fn prepare(
            &self,
            _request: Request<PrepareRequest>,
        ) -> Result<Response<PrepareResponse>, Status> {
            Ok(Response::new(PrepareResponse {
                warnings: vec![],
                errors: vec![],
            }))
        }

        async fn provision(
            &self,
            _request: Request<ProvisionRequest>,
        ) -> Result<Response<ProvisionResponse>, Status> {
            Ok(Response::new(ProvisionResponse {
                success: true,
                error: String::new(),
            }))
        }

        async fn cancel(
            &self,
            _request: Request<CancelRequest>,
        ) -> Result<Response<CancelResponse>, Status> {
            Ok(Response::new(CancelResponse {}))
        }
    }

    #[tokio::test]
    async fn test_go_plugin_provisioner_rpc() -> Result<(), StampError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(ProvisionerServer::new(MockProvisionerService))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let config = GoPluginProvisionerConfig {
            provisioner_type: "custom".to_string(),
            plugin_path: "".to_string(),
            endpoint: Some(local_addr.to_string()),
        };
        let prov = GoPluginProvisioner::new(config);
        prov.connect(&local_addr.to_string()).await?;
        prov.prepare_plugin(vec![b"config".to_vec()]).await?;

        let comm = crate::communicator::mock::MockCommunicator::default();
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        prov.provision(&comm, ui).await?;
        prov.cancel_plugin().await?;
        Ok(())
    }
}
