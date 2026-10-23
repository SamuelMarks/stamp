//! `custom-hook` provisioner.

use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::Timeout;
use async_trait::async_trait;
use std::str::FromStr;

/// Configuration for `custom-hook`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CustomHookConfig {
    /// Optional restart timeout duration string (e.g. `"5m"`).
    pub restart_timeout: String,
    /// Optional command to execute over the communicator during the custom hook.
    pub command: Option<String>,
}

/// The `custom-hook` provisioner.
#[derive(Debug, Clone)]
pub struct CustomHookProvisioner {
    /// The config.
    pub config: CustomHookConfig,
}

impl CustomHookProvisioner {
    /// Create a new `CustomHookProvisioner`.
    #[must_use]
    pub const fn new(config: CustomHookConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provisioner for CustomHookProvisioner {
    async fn provision(
        &self,
        comm: &dyn crate::communicator::Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        if !self.config.restart_timeout.is_empty() {
            let _timeout = Timeout::from_str(&self.config.restart_timeout)?;
        }

        if let Some(ref cmd_str) = self.config.command {
            ui.say("custom-hook", &format!("Executing hook command: {cmd_str}"));
            let res = comm
                .execute(&crate::communicator::Command::new(cmd_str.clone()))
                .await?;
            if res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "custom-hook command failed with exit code {}: {}",
                    res.exit_code, res.stderr
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_hook_derived_traits() {
        let config = CustomHookConfig {
            restart_timeout: "5m".to_string(),
            command: None,
        };
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("CustomHookConfig"));
        assert_eq!(CustomHookConfig::default().restart_timeout, "");

        let p = CustomHookProvisioner::new(config);
        assert!(format!("{p:?}").contains("CustomHookProvisioner"));
        assert_eq!(p.clone().config, p.config);
    }

    #[tokio::test]
    async fn test_custom_hook_success() -> Result<(), StampError> {
        let p = CustomHookProvisioner::new(CustomHookConfig {
            restart_timeout: "5m".to_string(),
            command: Some("echo hooked".to_string()),
        });
        let mock_comm = crate::communicator::mock::MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        p.provision(&mock_comm, ui.clone()).await?;

        let p_fail_cmd = CustomHookProvisioner::new(CustomHookConfig {
            restart_timeout: String::new(),
            command: Some("fail_cmd".to_string()),
        });
        assert!(p_fail_cmd.provision(&mock_comm, ui).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_custom_hook_failure() {
        let p = CustomHookProvisioner::new(CustomHookConfig {
            restart_timeout: "fail".to_string(),
            command: None,
        });
        let mock_comm = crate::communicator::mock::MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        assert!(p.provision(&mock_comm, ui).await.is_err());
    }
}
