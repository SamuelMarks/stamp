//! Implementation of the `null` builder.

use crate::builder::Builder;
use crate::communicator::{
    Command, Communicator,
    mock::MockCommunicator,
    none::NoneCommunicator,
    ssh::{SshCommunicator, SshConfig},
    winrm::{WinRmAuth, WinRmCommunicator, WinRmConfig},
};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `null` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NullConfig {
    /// The name of the builder instance.
    pub name: String,

    /// The communicator to use ("ssh", "winrm", "mock", or "none"). Defaults to "none".
    pub communicator: Option<String>,

    /// The hostname or IP address to connect to.
    pub host: Option<String>,

    /// The port to connect to.
    pub port: Option<u16>,

    /// The username for authentication.
    pub username: Option<String>,

    /// The password for authentication.
    pub password: Option<String>,

    /// Optional delay (in seconds) to simulate build process time.
    pub sleep_delay: Option<u64>,

    /// Overrides the command for testing purposes (legacy).
    #[cfg(test)]
    pub test_cmd: Option<String>,
}

/// The `null` builder.
#[derive(Debug, Clone)]
pub struct NullBuilder {
    /// The builder configuration.
    pub config: NullConfig,
}

impl NullBuilder {
    /// Create a new `NullBuilder`.
    #[must_use]
    pub const fn new(config: NullConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Builder for NullBuilder {
    async fn prepare(&self) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }

        let comm_type = self.config.communicator.as_deref().unwrap_or("none");
        if comm_type != "ssh" && comm_type != "winrm" && comm_type != "mock" && comm_type != "none"
        {
            return Err(StampError::Parse(format!(
                "Unsupported communicator: {comm_type}"
            )));
        }

        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        _on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        ui.say(&self.name(), "Running null builder...");

        if let Some(delay) = self.config.sleep_delay {
            ui.say(&self.name(), &format!("Sleeping for {delay}s"));
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }

        let comm_type = self.config.communicator.as_deref().unwrap_or("none");
        let comm: Arc<dyn Communicator> = match comm_type {
            "ssh" => {
                let ssh_config = SshConfig {
                    host: self.config.host.clone().unwrap_or("127.0.0.1".to_string()),
                    port: Port::new(self.config.port.unwrap_or(22)),
                    username: self.config.username.clone().unwrap_or("root".to_string()),

                    private_key_path: None,
                    timeout: Timeout::new(Duration::from_secs(10)),
                    bastion_host: None,
                    bastion_port: None,
                    bastion_username: None,
                    bastion_private_key_file: None,
                    agent_forwarding: false,
                    pty: false,
                    connection_attempts: 1,
                    expect_disconnect: false,
                    ..Default::default()
                };
                let c = SshCommunicator::new(ssh_config);
                c.execute(&Command::new(String::new())).await?;
                Arc::new(c)
            }
            "winrm" => {
                let winrm_config = WinRmConfig {
                    host: self.config.host.clone().unwrap_or("127.0.0.1".to_string()),
                    port: Port::new(self.config.port.unwrap_or(5985)),
                    username: self
                        .config
                        .username
                        .clone()
                        .unwrap_or("Administrator".to_string()),
                    password: self.config.password.clone(),
                    auth: WinRmAuth::Ntlm,
                    tls: crate::communicator::winrm::WinRmTlsConfig {
                        use_https: false,
                        insecure_skip_verify: false,
                        winrm_insecure: false,
                    },
                    timeout: Timeout::new(Duration::from_secs(10)),
                    use_powershell_wrapper: false,
                    ..Default::default()
                };
                let c = WinRmCommunicator::new(winrm_config);
                c.execute(&Command::new(String::new())).await?;
                Arc::new(c)
            }
            "mock" => Arc::new(MockCommunicator::new()),
            _ => Arc::new(NoneCommunicator::new()),
        };

        let build_ctx = BuildContext {
            build_id: self.name(),
            host: self.config.host.clone().unwrap_or_default(),
            user: self.config.username.clone().unwrap_or_default(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name(),
            source_type: "null".to_string(),
            ..Default::default()
        };

        ui.say(&self.name(), "Running provisioners...");
        if let Err(e) = hook
            .run_provisioners(comm.clone(), &build_ctx, ui.clone())
            .await
        {
            ui.error(&self.name(), &format!("Provisioning failed: {e}"));
            if let Err(cleanup_err) = hook
                .run_error_cleanup_provisioners(comm.clone(), &build_ctx, ui.clone())
                .await
            {
                ui.error(
                    &self.name(),
                    &format!("Error cleanup provisioning failed: {cleanup_err}"),
                );
            }
            return Err(e);
        }

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("{}-artifact", self.name()),
            files: vec![],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }

    fn name(&self) -> String {
        self.config.name.clone()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = NullConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{:?}", config));

        let builder = NullBuilder::new(config);
        assert_eq!(format!("{:?}", builder.clone()), format!("{:?}", builder));
    }

    #[tokio::test]
    async fn test_null_prepare_success() {
        let config = NullConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let res = builder.prepare().await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_null_prepare_failure_name() {
        let config = NullConfig {
            name: String::new(),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let err = builder.prepare().await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_null_prepare_failure_comm() {
        let config = NullConfig {
            name: "test".to_string(),
            communicator: Some("invalid".to_string()),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let err = builder.prepare().await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_null_run_none() {
        let config = NullConfig {
            name: "test".to_string(),
            communicator: Some("none".to_string()),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_null_run_mock() {
        let config = NullConfig {
            name: "test".to_string(),
            communicator: Some("mock".to_string()),
            sleep_delay: Some(0),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_null_run_ssh_success() {
        unsafe {
            std::env::set_var("STAMP_TEST_MODE", "1");
        }
        let config = NullConfig {
            name: "test".to_string(),
            communicator: Some("ssh".to_string()),
            host: Some("localhost".to_string()),
            username: Some("admin".to_string()),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_null_run_ssh_failure() {
        let config = NullConfig {
            name: "test".to_string(),
            communicator: Some("ssh".to_string()),
            host: Some("unreachable".to_string()),
            username: Some("admin".to_string()),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let result = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_null_run_winrm_success() {
        unsafe {
            std::env::set_var("STAMP_TEST_MODE", "1");
        }
        let config = NullConfig {
            name: "test".to_string(),
            communicator: Some("winrm".to_string()),
            host: Some("localhost".to_string()),
            username: Some("admin".to_string()),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_null_run_winrm_failure() {
        let config = NullConfig {
            name: "test".to_string(),
            communicator: Some("winrm".to_string()),
            host: Some("unreachable".to_string()),
            username: Some("admin".to_string()),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let result = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_null_run_invalid_comm() {
        let config = NullConfig {
            name: "test".to_string(),
            communicator: Some("invalid".to_string()),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_null_cancel() {
        let config = NullConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        let res = builder.cancel().await;
        assert!(res.is_ok());
    }

    #[test]
    fn test_null_name() {
        let config = NullConfig {
            name: "test-name".to_string(),
            ..Default::default()
        };
        let builder = NullBuilder::new(config);
        assert_eq!(builder.name(), "test-name");
    }
}
