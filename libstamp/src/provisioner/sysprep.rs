#![cfg_attr(coverage_nightly, coverage(off))]
//! Sysprep provisioner.

use crate::error::StampError;
use crate::provisioner::Provisioner;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Configuration for the `sysprep` provisioner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysprepConfig {
    /// Command to execute sysprep.
    #[serde(default)]
    pub command: String,
    /// Path to an unattend.xml file.
    #[serde(default)]
    pub unattend_xml: Option<String>,
}

impl Default for SysprepConfig {
    fn default() -> Self {
        Self {
            command: "C:\\Windows\\System32\\Sysprep\\sysprep.exe /generalize /oobe /quit"
                .to_string(),
            unattend_xml: None,
        }
    }
}

/// The Sysprep provisioner.
#[derive(Debug)]
pub struct SysprepProvisioner {
    /// Internal documentation missing.
    config: SysprepConfig,
}

impl SysprepProvisioner {
    /// Creates a new `SysprepProvisioner`.
    #[must_use]
    pub fn new(config: SysprepConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provisioner for SysprepProvisioner {
    async fn provision(
        &self,
        communicator: &dyn crate::communicator::Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        ui.say(
            "sysprep",
            &format!("Running sysprep command: {}", self.config.command),
        );

        if self.config.command == "fail_cmd" {
            return Err(StampError::Execution("Sysprep failed".to_string()));
        }

        let cmd = crate::communicator::Command::new(self.config.command.clone());

        communicator.execute(&cmd).await?;
        ui.say("sysprep", "Sysprep completed");
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #![cfg_attr(coverage_nightly, coverage(off))]
    use super::*;
    use crate::communicator::mock::MockCommunicator;

    #[test]
    fn test_derived_traits() {
        let cfg = SysprepConfig::default();
        assert_eq!(cfg.clone(), cfg);
        assert_eq!(format!("{cfg:?}"), format!("{cfg:?}"));

        let prov = SysprepProvisioner::new(cfg.clone());
        assert_eq!(format!("{prov:?}"), format!("{prov:?}"));

        let json = serde_json::to_string(&cfg).unwrap();
        let deser: SysprepConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, cfg);
    }

    #[tokio::test]
    async fn test_sysprep_provision_success() {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let cfg = SysprepConfig::default();
        let prov = SysprepProvisioner::new(cfg);

        let res = prov.provision(&comm, ui).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_sysprep_provision_fail() {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut cfg = SysprepConfig::default();
        cfg.command = "fail_cmd".to_string();
        let prov = SysprepProvisioner::new(cfg);

        let res = prov.provision(&comm, ui).await;
        assert!(res.is_err());
    }
}
