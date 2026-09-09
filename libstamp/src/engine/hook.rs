#![cfg_attr(coverage_nightly, coverage(off))]
//! Orchestration hooks for the Packer engine.

use crate::communicator::Communicator;
use crate::error::StampError;
use crate::provisioner::Provisioner;
use async_trait::async_trait;
use std::sync::Arc;

/// Context passed to provision hooks and provisioners.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildContext {
    /// The build ID.
    pub build_id: String,
    /// The build name.
    pub build_name: String,
    /// The build or builder type.
    pub build_type: String,
    /// The remote host address.
    pub host: String,
    /// The remote port.
    pub port: u16,
    /// The remote user name.
    pub user: String,
    /// The remote password if configured.
    pub password: Option<String>,
    /// The communicator connection type (e.g., "ssh", "winrm", "none").
    pub conn_type: String,
    /// Connection metadata dictionary.
    pub conn_info: std::collections::HashMap<String, String>,
    /// The Packer run UUID.
    pub packer_run_uuid: String,
    /// The source name.
    pub source_name: String,
    /// The source type.
    pub source_type: String,
    /// The source AMI identifier if building on AWS EC2.
    pub source_ami: Option<String>,
    /// The source AMI name if building on AWS EC2.
    pub source_ami_name: Option<String>,
    /// Ephemeral or configured SSH public key.
    pub ssh_public_key: Option<String>,
    /// Ephemeral or configured SSH private key file path.
    pub ssh_private_key: Option<String>,
}

/// A hook that Builders call when they have created an instance and established a communicator.
/// This allows the engine to run provisioners mid-flight before the builder finalizes the image.
#[async_trait]
pub trait ProvisionHook: Send + Sync {
    /// Execute provisioners on the provided communicator.
    async fn run_provisioners(
        &self,
        comm: Arc<dyn Communicator>,
        ctx: &BuildContext,
        ui: Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError>;
    /// Execute error cleanup provisioners on the provided communicator.
    async fn run_error_cleanup_provisioners(
        &self,
        comm: Arc<dyn Communicator>,
        ctx: &BuildContext,
        ui: Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError>;
}

/// A default provision hook that actually runs a list of provisioners.
#[derive(Clone)]
pub struct DefaultProvisionHook {
    /// The provisioners to run.
    pub provisioners: Arc<Vec<Box<dyn Provisioner>>>,
    /// The error cleanup provisioners to run on failure.
    pub error_cleanup_provisioners: Arc<Vec<Box<dyn Provisioner>>>,
}

#[async_trait]
impl ProvisionHook for DefaultProvisionHook {
    async fn run_provisioners(
        &self,
        comm: Arc<dyn Communicator>,
        _ctx: &BuildContext,
        ui: Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        for provisioner in self.provisioners.iter() {
            provisioner.provision(comm.as_ref(), ui.clone()).await?;
        }
        Ok(())
    }

    async fn run_error_cleanup_provisioners(
        &self,
        comm: Arc<dyn Communicator>,
        _ctx: &BuildContext,
        ui: Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        for provisioner in self.error_cleanup_provisioners.iter() {
            provisioner.provision(comm.as_ref(), ui.clone()).await?;
        }
        Ok(())
    }
}

/// Type alias for hook callback closures.
pub type HookHandler = Arc<dyn Fn(&str, &[u8]) -> Result<(), String> + Send + Sync>;

/// Remote hook server adapter exposing lifecycle hooks over gRPC.
#[derive(Clone)]
pub struct RemoteHookServer {
    /// Handler callback for hook events.
    handler: HookHandler,
}

impl RemoteHookServer {
    /// Creates a new `RemoteHookServer` with the provided hook callback.
    #[must_use]
    pub fn new(handler: HookHandler) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl crate::r#gen::packer::hook_server::Hook for RemoteHookServer {
    async fn run(
        &self,
        request: tonic::Request<crate::r#gen::packer::HookRunRequest>,
    ) -> Result<tonic::Response<crate::r#gen::packer::HookRunResponse>, tonic::Status> {
        let req = request.into_inner();
        match (self.handler)(&req.hook_name, &req.data) {
            Ok(()) => Ok(tonic::Response::new(
                crate::r#gen::packer::HookRunResponse {
                    error: false,
                    message: String::new(),
                },
            )),
            Err(e) => Ok(tonic::Response::new(
                crate::r#gen::packer::HookRunResponse {
                    error: true,
                    message: e,
                },
            )),
        }
    }
}

/// Remote hook client proxy invoking lifecycle hooks over gRPC.
#[derive(Debug, Clone)]
pub struct RemoteHookClient {
    /// Inner tonic client.
    client: crate::r#gen::packer::hook_client::HookClient<tonic::transport::Channel>,
}

impl RemoteHookClient {
    /// Creates a new `RemoteHookClient` connected to the given channel.
    #[must_use]
    pub fn new(channel: tonic::transport::Channel) -> Self {
        Self {
            client: crate::r#gen::packer::hook_client::HookClient::new(channel),
        }
    }

    /// Connect to a remote hook service over TCP.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if connection fails.
    pub async fn connect_tcp(address: &str) -> Result<Self, StampError> {
        let url = if address.starts_with("http") {
            address.to_string()
        } else {
            format!("http://{address}")
        };
        let client = crate::r#gen::packer::hook_client::HookClient::connect(url)
            .await
            .map_err(|e| StampError::Execution(format!("Hook connect error: {e}")))?;
        Ok(Self { client })
    }

    /// Dispatch a lifecycle hook event over gRPC.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if hook execution fails.
    pub async fn run_hook(&mut self, hook_name: &str, data: &[u8]) -> Result<(), StampError> {
        let req = tonic::Request::new(crate::r#gen::packer::HookRunRequest {
            hook_name: hook_name.to_string(),
            data: data.to_vec(),
        });
        let resp = self
            .client
            .run(req)
            .await
            .map_err(|e| StampError::Execution(format!("Hook RPC error: {e}")))?
            .into_inner();

        if resp.error {
            return Err(StampError::Execution(format!(
                "Hook error: {}",
                resp.message
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::communicator::{Command, mock::MockCommunicator};

    #[derive(Clone)]
    struct MockProvisioner;
    #[async_trait]
    impl Provisioner for MockProvisioner {
        async fn provision(
            &self,
            comm: &dyn Communicator,
            _ui: std::sync::Arc<crate::engine::ui::Ui>,
        ) -> Result<(), StampError> {
            comm.execute(&Command::new("mock".to_string())).await?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_default_provision_hook() -> Result<(), StampError> {
        let provs: Vec<Box<dyn Provisioner>> = vec![Box::new(MockProvisioner)];
        let hook = DefaultProvisionHook {
            provisioners: Arc::new(provs),
            error_cleanup_provisioners: Arc::new(vec![]),
        };
        let comm = Arc::new(MockCommunicator::default());
        let ctx = BuildContext {
            build_id: "".into(),
            host: "".into(),
            user: "".into(),
            packer_run_uuid: "".into(),
            source_name: "".into(),
            source_type: "".into(),
            ..Default::default()
        };
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        hook.run_provisioners(comm.clone(), &ctx, ui.clone())
            .await?;
        hook.run_error_cleanup_provisioners(comm, &ctx, ui).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_remote_hook_grpc_roundtrip() -> Result<(), StampError> {
        use crate::r#gen::packer::hook_server::HookServer;

        let server = RemoteHookServer::new(Arc::new(|name, data| {
            if name == "step_error" {
                Err("simulated hook failure".to_string())
            } else {
                assert_eq!(data, b"payload");
                Ok(())
            }
        }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(StampError::Io)?;
        let local_addr = listener.local_addr().map_err(StampError::Io)?;

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(HookServer::new(server))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let mut client = RemoteHookClient::connect_tcp(&local_addr.to_string()).await?;
        assert!(client.run_hook("step_pre", b"payload").await.is_ok());
        assert!(client.run_hook("step_error", b"payload").await.is_err());

        Ok(())
    }
}
