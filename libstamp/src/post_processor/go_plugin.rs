#![cfg_attr(coverage_nightly, coverage(off))]
//! Go-plugin out-of-process post-processor RPC client.

use crate::error::StampError;
use crate::r#gen::packer::post_processor_client::PostProcessorClient;
use crate::r#gen::packer::{ConfigureRequest, PostProcessRequest};
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Configuration for `go_plugin` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoPluginPostProcessorConfig {
    /// Name or type of the post-processor.
    pub post_processor_type: String,
    /// Path to the plugin binary.
    pub plugin_path: String,
    /// Direct gRPC endpoint if already running.
    pub endpoint: Option<String>,
}

/// The `go_plugin` post-processor client.
#[derive(Debug, Clone)]
pub struct GoPluginPostProcessor {
    /// Configuration.
    pub config: GoPluginPostProcessorConfig,
    /// Active gRPC client.
    client: Arc<Mutex<Option<PostProcessorClient<tonic::transport::Channel>>>>,
}

impl GoPluginPostProcessor {
    /// Create a new `GoPluginPostProcessor`.
    #[must_use]
    pub fn new(config: GoPluginPostProcessorConfig) -> Self {
        Self {
            config,
            client: Arc::new(Mutex::new(None)),
        }
    }

    /// Connects to a running gRPC post-processor plugin at the given endpoint.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the connection fails.
    pub async fn connect(&self, endpoint: &str) -> Result<(), StampError> {
        let endpoint_url = if endpoint.starts_with("http") {
            endpoint.to_string()
        } else {
            format!("http://{endpoint}")
        };
        let client = PostProcessorClient::connect(endpoint_url)
            .await
            .map_err(|e| StampError::Execution(format!("PostProcessor gRPC connect error: {e}")))?;
        *self.client.lock().await = Some(client);
        Ok(())
    }

    /// Configures the post-processor with configuration bytes.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if configuration fails.
    pub async fn configure(&self, configs: Vec<Vec<u8>>) -> Result<(), StampError> {
        let mut client_lock = self.client.lock().await;
        if let Some(ref mut client) = *client_lock {
            let resp = client
                .configure(tonic::Request::new(ConfigureRequest { configs }))
                .await
                .map_err(|e| StampError::Execution(format!("PostProcessor configure error: {e}")))?
                .into_inner();

            if !resp.errors.is_empty() {
                return Err(StampError::Execution(format!(
                    "PostProcessor configure errors: {}",
                    resp.errors.join(", ")
                )));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl PostProcessor for GoPluginPostProcessor {
    async fn process(&self, mut artifact: Artifact) -> Result<Artifact, StampError> {
        if let Some(ref ep) = self.config.endpoint {
            if self.client.lock().await.is_none() {
                self.connect(ep).await?;
            }
            let mut client_lock = self.client.lock().await;
            if let Some(ref mut client) = *client_lock {
                let resp = client
                    .post_process(tonic::Request::new(PostProcessRequest {
                        artifact_id: artifact.id.clone(),
                        builder_id: String::new(),
                        files: artifact.files.clone(),
                    }))
                    .await
                    .map_err(|e| {
                        StampError::Execution(format!("PostProcessor post_process error: {e}"))
                    })?
                    .into_inner();

                if !resp.success {
                    return Err(StampError::Execution(format!(
                        "PostProcessor failed: {}",
                        resp.error
                    )));
                }

                return Ok(Artifact {
                    id: if resp.artifact_id.is_empty() {
                        artifact.id
                    } else {
                        resp.artifact_id
                    },
                    files: resp.files,
                });
            }
        }

        artifact.id = format!("{}-processed", artifact.id);
        Ok(artifact)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::r#gen::packer::post_processor_server::{
        PostProcessor as PostProcessorRpc, PostProcessorServer,
    };
    use crate::r#gen::packer::{ConfigureResponse, PostProcessResponse};
    use tonic::{Request, Response, Status};

    struct MockPostProcessorService;

    #[tonic::async_trait]
    impl PostProcessorRpc for MockPostProcessorService {
        async fn configure(
            &self,
            _request: Request<ConfigureRequest>,
        ) -> Result<Response<ConfigureResponse>, Status> {
            Ok(Response::new(ConfigureResponse {
                warnings: vec![],
                errors: vec![],
            }))
        }

        async fn post_process(
            &self,
            request: Request<PostProcessRequest>,
        ) -> Result<Response<PostProcessResponse>, Status> {
            let req = request.into_inner();
            Ok(Response::new(PostProcessResponse {
                success: true,
                error: String::new(),
                artifact_id: format!("{}-grpc-artifact", req.artifact_id),
                builder_id: req.builder_id,
                files: req.files,
                keep_input_artifact: false,
            }))
        }
    }

    #[tokio::test]
    async fn test_go_plugin_post_processor_rpc() -> Result<(), StampError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(PostProcessorServer::new(MockPostProcessorService))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let config = GoPluginPostProcessorConfig {
            post_processor_type: "custom".to_string(),
            plugin_path: "".to_string(),
            endpoint: Some(local_addr.to_string()),
        };
        let pp = GoPluginPostProcessor::new(config);
        pp.connect(&local_addr.to_string()).await?;
        pp.configure(vec![b"cfg".to_vec()]).await?;

        let artifact = Artifact::new("base".to_string(), vec!["file.bin".to_string()]);
        let res = pp.process(artifact).await?;
        assert_eq!(res.id, "base-grpc-artifact");
        Ok(())
    }
}
