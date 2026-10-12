//! Go-plugin out-of-process data source RPC client.

use crate::data_source::DataSource;
use crate::error::StampError;
use crate::r#gen::packer::ExecuteRequest;
use crate::r#gen::packer::datasource_client::DatasourceClient;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Configuration for `go_plugin` data source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoPluginDataSourceConfig {
    /// Data source type.
    pub source_type: String,
    /// Path to the plugin binary.
    pub plugin_path: String,
    /// Direct gRPC endpoint if already running.
    pub endpoint: Option<String>,
}

/// The `go_plugin` data source client.
#[derive(Debug, Clone)]
pub struct GoPluginDataSource {
    /// Configuration.
    pub config: GoPluginDataSourceConfig,
    /// Active gRPC client.
    client: Arc<Mutex<Option<DatasourceClient<tonic::transport::Channel>>>>,
}

impl GoPluginDataSource {
    /// Create a new `GoPluginDataSource`.
    #[must_use]
    pub fn new(config: GoPluginDataSourceConfig) -> Self {
        Self {
            config,
            client: Arc::new(Mutex::new(None)),
        }
    }

    /// Connects to a running gRPC data source plugin at the given endpoint.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the connection fails.
    pub async fn connect(&self, endpoint: &str) -> Result<(), StampError> {
        let endpoint_url = if endpoint.starts_with("http") {
            endpoint.to_string()
        } else {
            format!("http://{endpoint}")
        };
        let client = DatasourceClient::connect(endpoint_url)
            .await
            .map_err(|e| StampError::Execution(format!("Datasource gRPC connect error: {e}")))?;
        *self.client.lock().await = Some(client);
        Ok(())
    }
}

#[async_trait]
impl DataSource for GoPluginDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if let Some(ref ep) = self.config.endpoint {
            if self.client.lock().await.is_none() {
                self.connect(ep).await?;
            }
            let mut client_lock = self.client.lock().await;
            if let Some(ref mut client) = *client_lock {
                let resp = client
                    .execute(tonic::Request::new(ExecuteRequest {
                        configs: vec![self.config.source_type.as_bytes().to_vec()],
                    }))
                    .await
                    .map_err(|e| StampError::Execution(format!("Datasource execute error: {e}")))?
                    .into_inner();

                if !resp.errors.is_empty() {
                    return Err(StampError::Execution(format!(
                        "Datasource failed: {}",
                        resp.errors.join(", ")
                    )));
                }

                let val: Value = serde_json::from_slice(&resp.output).map_err(|e| {
                    StampError::Execution(format!("Invalid JSON output from datasource: {e}"))
                })?;
                return Ok(val);
            }
        }

        Ok(serde_json::json!({ "status": "ok" }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::r#gen::packer::ExecuteResponse;
    use crate::r#gen::packer::datasource_server::{Datasource as DatasourceRpc, DatasourceServer};
    use tonic::{Request, Response, Status};

    struct MockDatasourceService {
        mode: String,
    }

    #[tonic::async_trait]
    impl DatasourceRpc for MockDatasourceService {
        async fn execute(
            &self,
            _request: Request<ExecuteRequest>,
        ) -> Result<Response<ExecuteResponse>, Status> {
            if self.mode == "error" {
                return Err(Status::internal("Internal mock error"));
            }
            if self.mode == "invalid_json" {
                return Ok(Response::new(ExecuteResponse {
                    output: b"not json".to_vec(),
                    errors: vec![],
                }));
            }
            if self.mode == "resp_error" {
                return Ok(Response::new(ExecuteResponse {
                    output: b"{}".to_vec(),
                    errors: vec!["some internal plugin error".to_string()],
                }));
            }
            let output = serde_json::json!({ "result": "mock_data", "active": true });
            let output_bytes = serde_json::to_vec(&output).unwrap_or_default();
            Ok(Response::new(ExecuteResponse {
                output: output_bytes,
                errors: vec![],
            }))
        }
    }

    async fn spawn_mock_server(mode: &str) -> Result<String, StampError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;
        let svc = MockDatasourceService {
            mode: mode.to_string(),
        };

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(DatasourceServer::new(svc))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        Ok(local_addr.to_string())
    }

    #[tokio::test]
    async fn test_go_plugin_data_source_rpc() -> Result<(), StampError> {
        let addr = spawn_mock_server("normal").await?;

        let config = GoPluginDataSourceConfig {
            source_type: "custom".to_string(),
            plugin_path: "".to_string(),
            endpoint: Some(addr),
        };
        let ds = GoPluginDataSource::new(config);
        let val = ds.read().await?;
        assert_eq!(
            val.get("result").and_then(|v| v.as_str()),
            Some("mock_data")
        );
        assert_eq!(val.get("active").and_then(|v| v.as_bool()), Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn test_go_plugin_http_branch_and_errors() -> Result<(), StampError> {
        // HTTP prefix branch
        let addr = spawn_mock_server("normal").await?;
        let config = GoPluginDataSourceConfig {
            source_type: "custom".to_string(),
            plugin_path: "".to_string(),
            endpoint: Some(format!("http://{addr}")),
        };
        let ds = GoPluginDataSource::new(config);
        assert!(ds.read().await.is_ok());

        // Error branch
        let addr_err = spawn_mock_server("error").await?;
        let config_err = GoPluginDataSourceConfig {
            source_type: "custom".to_string(),
            plugin_path: "".to_string(),
            endpoint: Some(addr_err),
        };
        let ds_err = GoPluginDataSource::new(config_err);
        assert!(ds_err.read().await.is_err());

        // Invalid JSON branch
        let addr_inv = spawn_mock_server("invalid_json").await?;
        let config_inv = GoPluginDataSourceConfig {
            source_type: "custom".to_string(),
            plugin_path: "".to_string(),
            endpoint: Some(addr_inv),
        };
        let ds_inv = GoPluginDataSource::new(config_inv);
        assert!(ds_inv.read().await.is_err());

        // Plugin resp error branch
        let addr_resp_err = spawn_mock_server("resp_error").await?;
        let config_resp_err = GoPluginDataSourceConfig {
            source_type: "custom".to_string(),
            plugin_path: "".to_string(),
            endpoint: Some(addr_resp_err),
        };
        let ds_resp_err = GoPluginDataSource::new(config_resp_err);
        assert!(ds_resp_err.read().await.is_err());

        Ok(())
    }
}
