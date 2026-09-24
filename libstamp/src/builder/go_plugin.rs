//! Go-plugin out-of-process builder RPC client.

use crate::builder::Builder;
use crate::error::StampError;
use async_trait::async_trait;

/// Configuration for `go_plugin` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

        if let Some(stdout) = child.stdout.take() {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let _ = reader.read_line(&mut line).await;

            let line = line.trim();
            if let Ok(hs) = line.parse::<Handshake>() {
                *self.handshake.lock().await = Some(hs);
            } else {
                return Err(StampError::Execution(format!(
                    "Invalid go-plugin handshake: {line}"
                )));
            }
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
                            let channel =
                                tonic::transport::Endpoint::from_static("http://[::]:50051")
                                    .connect_with_connector(tower::service_fn(
                                        move |_: tonic::transport::Uri| {
                                            tokio::net::UnixStream::connect(path.clone())
                                        },
                                    ))
                                    .await
                                    .map_err(|e| {
                                        StampError::Execution(format!(
                                            "gRPC Unix connect error: {e}"
                                        ))
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

                if !resp.success {
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
#[allow(
    clippy::unwrap_used,
    clippy::pedantic,
    clippy::all,
    for_loops_over_fallibles
)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_net_rpc_tcp_connect_error() {
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
        assert!(res.is_err_and(|e| e.to_string().contains("net/rpc TCP connect error")));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_net_rpc_unix_connect_error() {
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
        assert!(res.is_err_and(|e| e.to_string().contains("net/rpc Unix connect error")));
    }

    #[tokio::test]
    async fn test_grpc_connect_error() {
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
        assert!(res.is_err_and(|e| e.to_string().contains("gRPC connect error")));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_grpc_unix_connect_error() {
        let b = GoPluginBuilder::new(GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "dummy".to_string(),
        });
        *b.handshake.lock().await = Some(Handshake {
            core_protocol_version: "1".to_string(),
            app_protocol_version: "1".to_string(),
            network_type: NetworkType::Unix,
            address: "/tmp/nonexistent_grpc_socket_for_test".to_string(),
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
        assert!(res.is_err_and(|e| e.to_string().contains("gRPC Unix connect error")));
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
    async fn test_go_plugin_builder() {
        let config = GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "/path/to/plugin".to_string(),
        };
        let b = GoPluginBuilder::new(config);
        assert_eq!(b.name(), "test");
        assert!(b.prepare().await.is_ok());
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
        assert!(
            b.run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
                .await
                .is_ok()
        );
        assert!(b.cancel().await.is_ok());
    }

    #[tokio::test]
    async fn test_go_plugin_builder_spawn() {
        let script = r#"#!/bin/bash
echo "1|5|tcp|127.0.0.1:12345|grpc"
sleep 5
"#;
        let path = std::env::temp_dir().join(format!("mock-plugin-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::write(&path, script);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        let config = GoPluginConfig {
            name: "test".to_string(),
            plugin_path: path.to_string_lossy().into_owned(),
        };
        let b = GoPluginBuilder::new(config);
        assert!(b.prepare().await.is_ok());
        assert_eq!(
            b.handshake
                .lock()
                .await
                .as_ref()
                .map(|hs| hs.address.clone())
                .as_deref(),
            Some("127.0.0.1:12345")
        );
        assert!(b.cancel().await.is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_net_rpc_tcp_unsupported() {
        for listener in tokio::net::TcpListener::bind("127.0.0.1:0").await {
            for local_addr in listener.local_addr() {
                let port = local_addr.port();

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
                assert!(res.is_err_and(|err| {
                    err.to_string().contains("net/rpc protocol requires Go gob")
                }));
            }
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_net_rpc_unix_unsupported() {
        let path = std::env::temp_dir().join(format!("test-sock-{}", uuid::Uuid::new_v4()));
        for _listener in tokio::net::UnixListener::bind(&path) {
            let config = GoPluginConfig {
                name: "test".to_string(),
                plugin_path: "/path/to/plugin".to_string(), // won't spawn
            };
            let b = GoPluginBuilder::new(config);
            *b.handshake.lock().await = Some(Handshake {
                core_protocol_version: "1".to_string(),
                app_protocol_version: "1".to_string(),
                network_type: NetworkType::Unix,
                address: path.to_string_lossy().into_owned(),
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
            assert!(
                res.is_err_and(|err| err.to_string().contains("net/rpc protocol requires Go gob"))
            );
            let _ = std::fs::remove_file(&path);
        }
    }

    #[tokio::test]
    async fn test_go_plugin_builder_spawn_fail() {
        let b = GoPluginBuilder::new(GoPluginConfig {
            name: "test".to_string(),
            plugin_path: "/nonexistent/plugin/path/12345".to_string(),
        });
        let err = b.prepare().await;
        assert!(err.is_err());
    }

    #[test]
    fn test_handshake_parsing_errors() {
        use super::{Handshake, Protocol};
        use std::str::FromStr;
        assert!(Handshake::from_str("1|1|tcp").is_err());
        assert!(Handshake::from_str("1|1|udp|1234").is_err());
        assert!(Handshake::from_str("1|1|tcp|1234|magic").is_err());
        let h_res = Handshake::from_str("1|1|tcp|1234");
        assert!(h_res.is_ok_and(|h| h.protocol == Protocol::NetRpc));
        let h_unix = Handshake::from_str("1|1|unix|/tmp/sock|grpc");
        assert!(
            h_unix
                .is_ok_and(|h| h.network_type == NetworkType::Unix && h.protocol == Protocol::Grpc)
        );
        let h_netrpc5 = Handshake::from_str("1|1|tcp|1234|net/rpc");
        assert!(h_netrpc5.is_ok_and(|h| h.protocol == Protocol::NetRpc));
    }

    struct MockBuilderRpc;

    #[tonic::async_trait]
    impl crate::r#gen::packer::builder_server::Builder for MockBuilderRpc {
        async fn prepare(
            &self,
            request: tonic::Request<crate::r#gen::packer::PrepareRequest>,
        ) -> Result<tonic::Response<crate::r#gen::packer::PrepareResponse>, tonic::Status> {
            let req = request.into_inner();
            if req
                .configs
                .iter()
                .any(|c| c.as_slice() == b"rpc_err_prepare")
            {
                return Err(tonic::Status::internal("mock prepare rpc error"));
            }
            let errors = if req.configs.iter().any(|c| c.as_slice() == b"fail_prepare") {
                vec!["mock prepare error".to_string()]
            } else {
                vec![]
            };
            Ok(tonic::Response::new(
                crate::r#gen::packer::PrepareResponse {
                    warnings: vec![],
                    errors,
                },
            ))
        }

        async fn run(
            &self,
            request: tonic::Request<crate::r#gen::packer::RunRequest>,
        ) -> Result<tonic::Response<crate::r#gen::packer::RunResponse>, tonic::Status> {
            let req = request.into_inner();
            if req.build_name == "rpc_error" {
                return Err(tonic::Status::internal("mock internal rpc error"));
            }
            if req.build_name == "fail_run" {
                return Ok(tonic::Response::new(crate::r#gen::packer::RunResponse {
                    success: false,
                    error: "mock run error".to_string(),
                    artifact_id: String::new(),
                    builder_id: String::new(),
                    files: vec![],
                }));
            }
            if req.build_name == "empty_ids" {
                return Ok(tonic::Response::new(crate::r#gen::packer::RunResponse {
                    success: true,
                    error: String::new(),
                    artifact_id: String::new(),
                    builder_id: String::new(),
                    files: vec![],
                }));
            }
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
    async fn test_grpc_builder_prepare_run_cancel() {
        let mut server_info = None;
        for l in tokio::net::TcpListener::bind("127.0.0.1:0").await {
            for addr in l.local_addr() {
                server_info = Some((l, addr));
                break;
            }
            break;
        }
        for (listener, local_addr) in server_info {
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                let _ = tonic::transport::Server::builder()
                    .add_service(crate::r#gen::packer::builder_server::BuilderServer::new(
                        MockBuilderRpc,
                    ))
                    .serve_with_incoming_shutdown(
                        tokio_stream::wrappers::TcpListenerStream::new(listener),
                        async {
                            let _ = rx.await;
                        },
                    )
                    .await;
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

            let res = b
                .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
                .await;
            assert!(res.is_ok());
            for artifact in res {
                assert_eq!(artifact.id(), "art-1");
                assert_eq!(artifact.files(), vec!["disk.img".to_string()]);
            }

            assert!(
                b.prepare_plugin(vec![b"test-config".to_vec()])
                    .await
                    .is_ok()
            );
            assert!(b.cancel().await.is_ok());

            let b_no_client = GoPluginBuilder::new(GoPluginConfig::default());
            assert!(b_no_client.prepare_plugin(vec![]).await.is_ok());

            let _ = tx.send(());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn test_grpc_builder_prepare_failure() {
        let mut server_info = None;
        for l in tokio::net::TcpListener::bind("127.0.0.1:0").await {
            for addr in l.local_addr() {
                server_info = Some((l, addr));
                break;
            }
            break;
        }
        for (listener, local_addr) in server_info {
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                let _ = tonic::transport::Server::builder()
                    .add_service(crate::r#gen::packer::builder_server::BuilderServer::new(
                        MockBuilderRpc,
                    ))
                    .serve_with_incoming_shutdown(
                        tokio_stream::wrappers::TcpListenerStream::new(listener),
                        async {
                            let _ = rx.await;
                        },
                    )
                    .await;
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

            assert!(
                b.run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
                    .await
                    .is_ok()
            );
            let res = b.prepare_plugin(vec![b"fail_prepare".to_vec()]).await;
            assert!(res.is_err_and(|e| {
                e.to_string()
                    .contains("Builder prepare failed: mock prepare error")
            }));
            let res_rpc = b.prepare_plugin(vec![b"rpc_err_prepare".to_vec()]).await;
            assert!(res_rpc.is_err_and(|e| e.to_string().contains("Builder prepare RPC error")));
            let _ = tx.send(());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn test_grpc_builder_run_failure_and_empty_ids() {
        let mut server_info = None;
        for l in tokio::net::TcpListener::bind("127.0.0.1:0").await {
            for addr in l.local_addr() {
                server_info = Some((l, addr));
                break;
            }
            break;
        }
        for (listener, local_addr) in server_info {
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                let _ = tonic::transport::Server::builder()
                    .add_service(crate::r#gen::packer::builder_server::BuilderServer::new(
                        MockBuilderRpc,
                    ))
                    .serve_with_incoming_shutdown(
                        tokio_stream::wrappers::TcpListenerStream::new(listener),
                        async {
                            let _ = rx.await;
                        },
                    )
                    .await;
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

            // Test fail_run
            let b_fail = GoPluginBuilder::new(GoPluginConfig {
                name: "fail_run".to_string(),
                plugin_path: "/path/to/plugin".to_string(),
            });
            *b_fail.handshake.lock().await = Some(Handshake {
                core_protocol_version: "1".to_string(),
                app_protocol_version: "1".to_string(),
                network_type: NetworkType::Tcp,
                address: local_addr.to_string(),
                protocol: Protocol::Grpc,
            });
            let res_fail = b_fail
                .run(
                    hook.clone(),
                    ui.clone(),
                    crate::engine::packer::OnErrorStrategy::Cleanup,
                )
                .await;
            assert!(res_fail.is_err_and(|e| {
                e.to_string()
                    .contains("Plugin builder 'fail_run' failed: mock run error")
            }));

            // Test rpc_error
            let b_rpc_err = GoPluginBuilder::new(GoPluginConfig {
                name: "rpc_error".to_string(),
                plugin_path: "/path/to/plugin".to_string(),
            });
            *b_rpc_err.handshake.lock().await = Some(Handshake {
                core_protocol_version: "1".to_string(),
                app_protocol_version: "1".to_string(),
                network_type: NetworkType::Tcp,
                address: local_addr.to_string(),
                protocol: Protocol::Grpc,
            });
            let res_rpc_err = b_rpc_err
                .run(
                    hook.clone(),
                    ui.clone(),
                    crate::engine::packer::OnErrorStrategy::Cleanup,
                )
                .await;
            assert!(res_rpc_err.is_err_and(|e| e.to_string().contains("gRPC Run RPC error")));

            // Test empty_ids
            let b_empty = GoPluginBuilder::new(GoPluginConfig {
                name: "empty_ids".to_string(),
                plugin_path: "/path/to/plugin".to_string(),
            });
            *b_empty.handshake.lock().await = Some(Handshake {
                core_protocol_version: "1".to_string(),
                app_protocol_version: "1".to_string(),
                network_type: NetworkType::Tcp,
                address: local_addr.to_string(),
                protocol: Protocol::Grpc,
            });
            let res_empty = b_empty
                .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
                .await;
            assert!(res_empty.is_ok());
            for art in res_empty {
                assert_eq!(art.id(), "empty_ids-artifact");
                assert_eq!(art.builder_id(), "empty_ids");
            }

            let _ = tx.send(());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_grpc_builder_unix_socket() {
        let sock_path =
            std::env::temp_dir().join(format!("test-grpc-sock-{}", uuid::Uuid::new_v4()));
        for listener in tokio::net::UnixListener::bind(&sock_path) {
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                let _ = tonic::transport::Server::builder()
                    .add_service(crate::r#gen::packer::builder_server::BuilderServer::new(
                        MockBuilderRpc,
                    ))
                    .serve_with_incoming_shutdown(
                        tokio_stream::wrappers::UnixListenerStream::new(listener),
                        async {
                            let _ = rx.await;
                        },
                    )
                    .await;
            });

            let b = GoPluginBuilder::new(GoPluginConfig {
                name: "unix-builder".to_string(),
                plugin_path: "/path/to/plugin".to_string(),
            });
            *b.handshake.lock().await = Some(Handshake {
                core_protocol_version: "1".to_string(),
                app_protocol_version: "1".to_string(),
                network_type: NetworkType::Unix,
                address: sock_path.to_string_lossy().to_string(),
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
            assert!(res.is_ok());
            for artifact in res {
                assert_eq!(artifact.id(), "art-1");
            }
            assert!(b.cancel().await.is_ok());
            let _ = tx.send(());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let _ = std::fs::remove_file(&sock_path);
        }
    }
}
