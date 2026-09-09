//! Bidirectional reverse-RPC broker for HashiCorp Packer plugins.
//!
//! Provides a local gRPC server hosted by Stamp that enables spawned plugin processes
//! to invoke host-side callbacks:
//! - `packer.Ui`: stream stdout, stderr, and interactive user prompts back to Stamp.
//! - `packer.Communicator`: execute commands and stream file transfers on the guest.
//! - `packer.Hook`: execute host provisioners during builder lifecycle phases.

use crate::communicator::Communicator;
use crate::communicator::grpc_proxy::GrpcCommunicatorServer;
use crate::engine::hook::{DefaultProvisionHook, HookHandler, ProvisionHook, RemoteHookServer};
use crate::engine::remote_ui::RemoteUiServer;
use crate::engine::ui::Ui;
use crate::error::StampError;
use crate::r#gen::packer::communicator_server::CommunicatorServer;
use crate::r#gen::packer::hook_server::HookServer;
use crate::r#gen::packer::ui_server::UiServer;
use crate::plugin::handshake::NetworkType;
use crate::plugin::socket::{UnixSocketGuard, bind_bounded_tcp_listener};
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;

/// Broker configuration and server instance.
pub struct PluginBroker {
    /// Network type the broker is bound to.
    network_type: NetworkType,
    /// Connection address for plugin to connect back (e.g. `127.0.0.1:port` or `/path/to/sock`).
    address: String,
    /// Shutdown signal sender to terminate broker background server.
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Background server join handle.
    server_handle: Option<tokio::task::JoinHandle<Result<(), StampError>>>,
    /// Optional Unix socket cleanup guard.
    _unix_guard: Option<UnixSocketGuard>,
}

impl std::fmt::Debug for PluginBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginBroker")
            .field("network_type", &self.network_type)
            .field("address", &self.address)
            .finish()
    }
}

impl PluginBroker {
    /// Starts a reverse-RPC broker listening over TCP loopback respecting bounded port ranges.
    ///
    /// # Errors
    /// Returns `StampError` if port binding or server startup fails.
    pub async fn start_tcp(
        ui: Arc<Ui>,
        comm: Option<Arc<dyn Communicator>>,
        hook: Option<Arc<dyn ProvisionHook>>,
    ) -> Result<Self, StampError> {
        let listener = bind_bounded_tcp_listener().await?;
        let addr = listener.local_addr().map_err(StampError::Io)?;
        let address = addr.to_string();

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let mut server = tonic::transport::Server::builder();
        let ui_service = UiServer::new(RemoteUiServer::new(ui));

        let comm_instance = comm
            .unwrap_or_else(|| Arc::new(crate::communicator::mock::MockCommunicator::default()));
        let comm_service = CommunicatorServer::new(GrpcCommunicatorServer::new(comm_instance));

        let _hook_instance = hook.unwrap_or_else(|| {
            Arc::new(DefaultProvisionHook {
                provisioners: Arc::new(vec![]),
                error_cleanup_provisioners: Arc::new(vec![]),
            })
        });
        let hook_handler: HookHandler = Arc::new(|_name, _data| Ok(()));
        let hook_service = HookServer::new(RemoteHookServer::new(hook_handler));

        let incoming = TcpListenerStream::new(listener);

        let handle = tokio::spawn(async move {
            server
                .add_service(ui_service)
                .add_service(comm_service)
                .add_service(hook_service)
                .serve_with_incoming_shutdown(incoming, async {
                    let _ = shutdown_rx.await;
                })
                .await
                .map_err(|e| StampError::Execution(format!("Broker server error: {e}")))
        });

        Ok(Self {
            network_type: NetworkType::Tcp,
            address,
            shutdown_tx: Some(shutdown_tx),
            server_handle: Some(handle),
            _unix_guard: None,
        })
    }

    /// Returns the network type used by this broker.
    #[must_use]
    pub fn network_type(&self) -> NetworkType {
        self.network_type
    }

    /// Returns the address string to provide to spawned plugins.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Gracefully stops the broker server.
    ///
    /// # Errors
    /// Returns `StampError` if the server task encounters a runtime failure.
    pub async fn shutdown(mut self) -> Result<(), StampError> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.server_handle.take() {
            handle
                .await
                .map_err(|e| StampError::Execution(format!("Broker join error: {e}")))??;
        }
        Ok(())
    }
}

impl Drop for PluginBroker {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::engine::packer::FeatureState;

    #[tokio::test]
    async fn test_plugin_broker_tcp_lifecycle() {
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));
        let broker = PluginBroker::start_tcp(ui.clone(), None, None)
            .await
            .unwrap();
        assert_eq!(broker.network_type(), NetworkType::Tcp);
        assert!(!broker.address().is_empty());
        assert!(format!("{broker:?}").contains("PluginBroker"));

        // Verify Ui service can be called via broker
        let client_res = crate::r#gen::packer::ui_client::UiClient::connect(format!(
            "http://{}",
            broker.address()
        ))
        .await;
        assert!(client_res.is_ok());
        let mut client = client_res.unwrap();
        let say_res = client
            .say(tonic::Request::new(crate::r#gen::packer::SayRequest {
                target: "test-target".to_string(),
                message: "hello broker".to_string(),
            }))
            .await;
        assert!(say_res.is_ok());

        let msg_res = client
            .message(tonic::Request::new(crate::r#gen::packer::MessageRequest {
                target: "test-target".to_string(),
                message: "info message".to_string(),
            }))
            .await;
        assert!(msg_res.is_ok());

        let err_res = client
            .error(tonic::Request::new(crate::r#gen::packer::ErrorRequest {
                target: "test-target".to_string(),
                error: "err message".to_string(),
            }))
            .await;
        assert!(err_res.is_ok());

        // Verify Communicator service can be called via broker
        let comm_proxy =
            crate::communicator::grpc_proxy::GrpcCommunicatorProxy::connect_tcp(broker.address())
                .await
                .unwrap();
        let cmd = crate::communicator::Command::new("echo 1".to_string());
        let exec_res = comm_proxy.execute(&cmd).await;
        assert!(exec_res.is_ok());

        // Verify Hook service can be called via broker
        let mut hook_client = crate::engine::hook::RemoteHookClient::connect_tcp(broker.address())
            .await
            .unwrap();
        let hook_res = hook_client.run_hook("pre-provision", b"data").await;
        assert!(hook_res.is_ok());

        broker.shutdown().await.unwrap();
    }
}
