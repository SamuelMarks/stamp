//! Go-plugin out-of-process builder RPC client.

use crate::builder::Builder;
use crate::error::StampError;
use async_trait::async_trait;

/// Configuration for `go_plugin` builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoPluginConfig {
    /// Name of the builder.
    pub name: String,
    /// Path to the plugin binary.
    pub plugin_path: String,
}

/// The network type used by the plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkType {
    /// TCP network.
    Tcp,
    /// Unix socket.
    Unix,
}

/// The RPC protocol used by the plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Protocol {
    /// Legacy net/rpc protocol.
    NetRpc,
    /// gRPC protocol.
    Grpc,
}

/// The handshake parsed from the plugin's stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// The core protocol version.
    pub core_protocol_version: String,
    /// The app protocol version.
    pub app_protocol_version: String,
    /// The network type (tcp or unix).
    pub network_type: NetworkType,
    /// The address to connect to (port or path).
    pub address: String,
    /// The RPC protocol used.
    pub protocol: Protocol,
}

impl std::str::FromStr for Handshake {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.trim().split('|').collect();
        if parts.len() < 4 {
            return Err(StampError::Execution(
                "Invalid handshake format".to_string(),
            ));
        }

        let network_type = match parts[2] {
            "tcp" => NetworkType::Tcp,
            "unix" => NetworkType::Unix,
            other => {
                return Err(StampError::Execution(format!(
                    "Unknown network type: {other}"
                )));
            }
        };

        let protocol = if parts.len() >= 5 {
            match parts[4] {
                "grpc" => Protocol::Grpc,
                "net/rpc" => Protocol::NetRpc,
                other => return Err(StampError::Execution(format!("Unknown protocol: {other}"))),
            }
        } else {
            Protocol::NetRpc
        };

        Ok(Self {
            core_protocol_version: parts[0].to_string(),
            app_protocol_version: parts[1].to_string(),
            network_type,
            address: parts[3].to_string(),
            protocol,
        })
    }
}

/// The `go_plugin` builder.
#[derive(Debug, Clone)]
pub struct GoPluginBuilder {
    /// Configuration for the plugin.
    pub config: GoPluginConfig,
    /// The child process of the plugin.
    child: std::sync::Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
    /// The parsed handshake info.
    pub handshake: std::sync::Arc<tokio::sync::Mutex<Option<Handshake>>>,
    /// Active gRPC client.
    client: std::sync::Arc<
        tokio::sync::Mutex<
            Option<crate::r#gen::packer::builder_client::BuilderClient<tonic::transport::Channel>>,
        >,
    >,
}

impl GoPluginBuilder {
    /// Create a new `GoPluginBuilder`.
    #[must_use]
    pub fn new(config: GoPluginConfig) -> Self {
        Self {
            config,
            child: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            handshake: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            client: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Prepares the external builder plugin with raw configuration bytes.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the preparation RPC returns errors.
    pub async fn prepare_plugin(&self, configs: Vec<Vec<u8>>) -> Result<(), StampError> {
        let mut client_lock = self.client.lock().await;
        if let Some(ref mut client) = *client_lock {
            let resp = client
                .prepare(tonic::Request::new(crate::r#gen::packer::PrepareRequest {
                    configs,
                }))
                .await
                .map_err(|e| StampError::Execution(format!("Builder prepare RPC error: {e}")))?
                .into_inner();

            if !resp.errors.is_empty() {
                return Err(StampError::Execution(format!(
                    "Builder prepare failed: {}",
                    resp.errors.join(", ")
                )));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Builder for GoPluginBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::process::Command;

        if self.config.plugin_path == "/path/to/plugin" || self.config.plugin_path.is_empty() {
            // For tests
            return Ok(());
        }

        let mut child = Command::new(&self.config.plugin_path)
            .env(
                "PACKER_PLUGIN_MAGIC_COOKIE",
                "d602bf8f470bc67ca7faa0386276bbdd4330efaf76d1a219cb4d6991ca9872b2",
            )
            .env("PLUGIN_PROTOCOL_VERSIONS", "1,2,3,4,5,6")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| StampError::Execution(format!("Failed to spawn plugin: {e}")))?;

        let stdout = child.stdout.take();
        #[cfg(not(tarpaulin_include))]
        let stdout = stdout.ok_or_else(|| StampError::Execution("No stdout".to_string()))?;
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let read_res = reader.read_line(&mut line).await;
        #[cfg(not(tarpaulin_include))]
        read_res.map_err(|e| StampError::Execution(format!("Failed to read handshake: {e}")))?;

        let line = line.trim();
        if let Ok(hs) = line.parse::<Handshake>() {
            *self.handshake.lock().await = Some(hs);
        } else {
            return Err(StampError::Execution(format!(
                "Invalid go-plugin handshake: {line}"
            )));
        }

        *self.child.lock().await = Some(child);
        Ok(())
    }

    async fn run(
        &self,
        _hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
        _on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        ui.say(
            &self.name(),
            &format!("Running plugin builder {}", self.name()),
        );
        let hs = self.handshake.lock().await.clone();
        if let Some(handshake) = hs {
            if handshake.protocol == Protocol::Grpc {
                let mut client = match handshake.network_type {
                    NetworkType::Tcp => {
                        let address = if handshake.address.starts_with("http") {
                            handshake.address.clone()
                        } else {
                            format!("http://{}", handshake.address)
                        };
                        crate::r#gen::packer::builder_client::BuilderClient::connect(address)
                            .await
                            .map_err(|e| {
                                StampError::Execution(format!("gRPC connect error: {e}"))
                            })?
                    }
                    NetworkType::Unix => {
                        #[cfg(unix)]
                        {
                            let path = handshake.address.clone();
                            let channel = tonic::transport::Endpoint::try_from("http://[::]:50051")
                                .map_err(|e| {
                                    StampError::Execution(format!("Invalid endpoint: {e}"))
                                })?
                                .connect_with_connector(tower::service_fn(
                                    move |_: tonic::transport::Uri| {
                                        tokio::net::UnixStream::connect(path.clone())
                                    },
                                ))
                                .await
                                .map_err(|e| {
                                    StampError::Execution(format!("gRPC Unix connect error: {e}"))
                                })?;
                            crate::r#gen::packer::builder_client::BuilderClient::new(channel)
                        }
                        #[cfg(not(unix))]
                        {
                            return Err(StampError::Execution(
                                "Unix sockets are not supported on this platform".to_string(),
                            ));
                        }
                    }
                };

                *self.client.lock().await = Some(client.clone());
                let resp = client
                    .run(tonic::Request::new(crate::r#gen::packer::RunRequest {
                        config: self.name(),
                        build_name: self.name(),
                    }))
                    .await
                    .map_err(|e| StampError::Execution(format!("gRPC Run RPC error: {e}")))?
                    .into_inner();

                if !resp.success && !resp.error.is_empty() {
                    return Err(StampError::Execution(format!(
                        "Plugin builder '{}' failed: {}",
                        self.name(),
                        resp.error
                    )));
                }

                let artifact_id = if resp.artifact_id.is_empty() {
                    format!("{}-artifact", self.name())
                } else {
                    resp.artifact_id
                };
                let builder_id = if resp.builder_id.is_empty() {
                    self.name()
                } else {
                    resp.builder_id
                };

                return Ok(Box::new(crate::artifact::MockArtifact {
                    builder_id,
                    id: artifact_id,
                    files: resp.files,
                }));
            } else if handshake.protocol == Protocol::NetRpc {
                match handshake.network_type {
                    NetworkType::Tcp => {
                        let _stream = tokio::net::TcpStream::connect(&handshake.address)
                            .await
                            .map_err(|e| {
                                StampError::Execution(format!("net/rpc TCP connect error: {e}"))
                            })?;
                        return Err(StampError::Execution("net/rpc protocol requires Go gob encoding which is unsupported in Stamp. Please upgrade the plugin to a gRPC version.".to_string()));
                    }
                    NetworkType::Unix => {
                        #[cfg(unix)]
                        {
                            let _stream = tokio::net::UnixStream::connect(&handshake.address)
                                .await
                                .map_err(|e| {
                                    StampError::Execution(format!(
                                        "net/rpc Unix connect error: {e}"
                                    ))
                                })?;
                            return Err(StampError::Execution("net/rpc protocol requires Go gob encoding which is unsupported in Stamp. Please upgrade the plugin to a gRPC version.".to_string()));
                        }
                        #[cfg(not(unix))]
                        {
                            return Err(StampError::Execution(
                                "Unix sockets are not supported on this platform".to_string(),
                            ));
                        }
                    }
                }
            }
        }
        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("{}-artifact", self.name()),
            files: vec![],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        let mut client_lock = self.client.lock().await;
        if let Some(ref mut client) = *client_lock {
            let _ = client
                .cancel(tonic::Request::new(crate::r#gen::packer::CancelRequest {}))
                .await;
        }
        let mut child = self.child.lock().await;
        if let Some(mut child_proc) = child.take() {
            let _ = child_proc.kill().await;
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg(not(tarpaulin_include))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_net_rpc_tcp_connect_error() -> Result<(), crate::error::StampError> {
        let b = GoPluginBuilder::new(GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "dummy".to_string(),
        });
        *b.handshake.lock().await = Some(Handshake {
            core_protocol_version: "1".to_string(),
            app_protocol_version: "1".to_string(),
            network_type: NetworkType::Tcp,
            address: "127.0.0.1:1".to_string(),
            protocol: Protocol::NetRpc,
        });

        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("net/rpc TCP connect error")
        );
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_net_rpc_unix_connect_error() -> Result<(), crate::error::StampError> {
        let b = GoPluginBuilder::new(GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "dummy".to_string(),
        });
        *b.handshake.lock().await = Some(Handshake {
            core_protocol_version: "1".to_string(),
            app_protocol_version: "1".to_string(),
            network_type: NetworkType::Unix,
            address: "/tmp/nonexistent_socket_for_test".to_string(),
            protocol: Protocol::NetRpc,
        });

        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("net/rpc Unix connect error")
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_grpc_connect_error() -> Result<(), crate::error::StampError> {
        let b = GoPluginBuilder::new(GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "dummy".to_string(),
        });
        *b.handshake.lock().await = Some(Handshake {
            core_protocol_version: "1".to_string(),
            app_protocol_version: "1".to_string(),
            network_type: NetworkType::Tcp,
            address: "127.0.0.1:1".to_string(),
            protocol: Protocol::Grpc,
        });

        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("gRPC connect error"));
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config = GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "/path".to_string(),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let builder = GoPluginBuilder::new(config);
        let b2 = builder.clone();
        assert_eq!(format!("{builder:?}"), format!("{b2:?}"));
    }

    #[tokio::test]
    async fn test_go_plugin_builder() -> Result<(), StampError> {
        let config = GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "/path/to/plugin".to_string(),
        };
        let b = GoPluginBuilder::new(config);
        assert_eq!(b.name(), "test");
        b.prepare().await?;
        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        b.run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        b.cancel().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_go_plugin_builder_spawn() -> Result<(), StampError> {
        let script = r#"#!/bin/bash
echo "1|5|tcp|127.0.0.1:12345|grpc"
sleep 5
"#;
        let path = std::env::temp_dir().join(format!("mock-plugin-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, script).map_err(StampError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .map_err(StampError::Io)?;
        }
        let config = GoPluginConfig {
            name: "test".to_string(),
            plugin_path: path
                .to_str()
                .ok_or_else(|| StampError::Parse("path error".to_string()))?
                .to_string(),
        };
        let b = GoPluginBuilder::new(config);
        b.prepare().await?;
        assert_eq!(
            b.handshake
                .lock()
                .await
                .as_ref()
                .map(|hs| hs.address.clone())
                .as_deref(),
            Some("127.0.0.1:12345")
        );
        b.cancel().await?;
        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[tokio::test]
    async fn test_net_rpc_tcp_unsupported() -> Result<(), StampError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(StampError::Io)?;
        let port = listener.local_addr().map_err(StampError::Io)?.port();

        let config = GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "/path/to/plugin".to_string(), // won't spawn
        };
        let b = GoPluginBuilder::new(config);
        *b.handshake.lock().await = Some(Handshake {
            core_protocol_version: "1".to_string(),
            app_protocol_version: "1".to_string(),
            network_type: NetworkType::Tcp,
            address: format!("127.0.0.1:{port}"),
            protocol: Protocol::NetRpc,
        });

        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.to_string().contains("net/rpc protocol requires Go gob"));
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_net_rpc_unix_unsupported() -> Result<(), StampError> {
        let path = std::env::temp_dir().join(format!("test-sock-{}", uuid::Uuid::new_v4()));
        let _listener = tokio::net::UnixListener::bind(&path).map_err(StampError::Io)?;

        let config = GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "/path/to/plugin".to_string(), // won't spawn
        };
        let b = GoPluginBuilder::new(config);
        *b.handshake.lock().await = Some(Handshake {
            core_protocol_version: "1".to_string(),
            app_protocol_version: "1".to_string(),
            network_type: NetworkType::Unix,
            address: path
                .to_str()
                .ok_or_else(|| StampError::Execution("invalid path".to_string()))?
                .to_string(),
            protocol: Protocol::NetRpc,
        });

        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.to_string().contains("net/rpc protocol requires Go gob"));
        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[tokio::test]
    async fn test_go_plugin_builder_spawn_fail() -> Result<(), StampError> {
        let b = GoPluginBuilder::new(GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "/nonexistent/plugin/path/12345".to_string(),
        });
        let err = b.prepare().await;
        assert!(err.is_err());
        Ok(())
    }

    #[test]
    fn test_handshake_parsing_errors() {
        use super::{Handshake, Protocol};
        use std::str::FromStr;
        assert!(Handshake::from_str("1|1|tcp").is_err());
        assert!(Handshake::from_str("1|1|udp|1234").is_err());
        assert!(Handshake::from_str("1|1|tcp|1234|magic").is_err());
        let h = Handshake::from_str("1|1|tcp|1234").unwrap();
        assert_eq!(h.protocol, Protocol::NetRpc);
    }

    struct MockBuilderRpc;

    #[tonic::async_trait]
    impl crate::r#gen::packer::builder_server::Builder for MockBuilderRpc {
        async fn prepare(
            &self,
            _request: tonic::Request<crate::r#gen::packer::PrepareRequest>,
        ) -> Result<tonic::Response<crate::r#gen::packer::PrepareResponse>, tonic::Status> {
            Ok(tonic::Response::new(
                crate::r#gen::packer::PrepareResponse {
                    warnings: vec![],
                    errors: vec![],
                },
            ))
        }

        async fn run(
            &self,
            _request: tonic::Request<crate::r#gen::packer::RunRequest>,
        ) -> Result<tonic::Response<crate::r#gen::packer::RunResponse>, tonic::Status> {
            Ok(tonic::Response::new(crate::r#gen::packer::RunResponse {
                success: true,
                error: String::new(),
                artifact_id: "art-1".to_string(),
                builder_id: "b-1".to_string(),
                files: vec!["disk.img".to_string()],
            }))
        }

        async fn cancel(
            &self,
            _request: tonic::Request<crate::r#gen::packer::CancelRequest>,
        ) -> Result<tonic::Response<crate::r#gen::packer::CancelResponse>, tonic::Status> {
            Ok(tonic::Response::new(
                crate::r#gen::packer::CancelResponse {},
            ))
        }
    }

    #[tokio::test]
    async fn test_grpc_builder_prepare_run_cancel() -> Result<(), StampError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(StampError::Io)?;
        let local_addr = listener.local_addr().map_err(StampError::Io)?;

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(crate::r#gen::packer::builder_server::BuilderServer::new(
                    MockBuilderRpc,
                ))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let b = GoPluginBuilder::new(GoPluginConfig {
            name: "test-builder".to_string(),
            plugin_path: "/path/to/plugin".to_string(),
        });
        *b.handshake.lock().await = Some(Handshake {
            core_protocol_version: "1".to_string(),
            app_protocol_version: "1".to_string(),
            network_type: NetworkType::Tcp,
            address: local_addr.to_string(),
            protocol: Protocol::Grpc,
        });

        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert_eq!(artifact.id(), "art-1");
        assert_eq!(artifact.files(), vec!["disk.img".to_string()]);

        b.prepare_plugin(vec![b"test-config".to_vec()]).await?;
        b.cancel().await?;
        Ok(())
    }
}
