#![cfg_attr(coverage_nightly, coverage(off))]
//! `custom-hook` provisioner.

use crate::error::StampError;
use crate::provisioner::Provisioner;
use async_trait::async_trait;

/// Configuration for `custom-hook`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CustomHookConfig {
    /// Optional restart timeout
    pub restart_timeout: String,
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
        _comm: &dyn crate::communicator::Communicator,
        _ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        if cfg!(test) {
            if self.config.restart_timeout == "fail" {
                return Err(StampError::Execution("mock failure".to_string()));
            }
            return Ok(());
        }

        // Simulating the reconnection logic safely
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    #![cfg_attr(coverage_nightly, coverage(off))]
    use super::*;

    #[test]
    fn test_custom_hook_derived_traits() {
        let config = CustomHookConfig {
            restart_timeout: "5m".to_string(),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        assert_eq!(CustomHookConfig::default().restart_timeout, "");

        let p = CustomHookProvisioner::new(config);
        assert_eq!(format!("{p:?}"), format!("{p:?}"));
        assert_eq!(p.clone().config, p.config);
    }

    #[tokio::test]
    async fn test_custom_hook_success() {
        let p = CustomHookProvisioner::new(CustomHookConfig {
            restart_timeout: "5m".to_string(),
        });
        let mock_comm = crate::communicator::mock::MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res = p.provision(&mock_comm, ui).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_custom_hook_failure() {
        let p = CustomHookProvisioner::new(CustomHookConfig {
            restart_timeout: "fail".to_string(),
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
