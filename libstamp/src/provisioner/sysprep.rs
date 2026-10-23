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
            command: r"C:\Windows\System32\Sysprep\sysprep.exe /generalize /oobe /quit".to_string(),
            unattend_xml: None,
        }
    }
}

/// The Sysprep provisioner.
#[derive(Debug)]
pub struct SysprepProvisioner {
    /// Configuration for the `sysprep` provisioner.
    config: SysprepConfig,
}

impl SysprepProvisioner {
    /// Creates a new `SysprepProvisioner`.
    #[must_use]
    pub const fn new(config: SysprepConfig) -> Self {
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

        let cmd = crate::communicator::Command::new(self.config.command.clone());

        let res = communicator.execute(&cmd).await?;
        if res.exit_code != 0 {
            return Err(StampError::Execution(format!(
                "Sysprep failed with exit code {}",
                res.exit_code
            )));
        }
        ui.say("sysprep", "Sysprep completed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;

    #[test]
    fn test_derived_traits() -> Result<(), StampError> {
        let cfg = SysprepConfig::default();
        let cloned = cfg.clone();
        assert_eq!(cloned, cfg);
        assert!(format!("{cfg:?}").contains("SysprepConfig"));

        let prov = SysprepProvisioner::new(cfg.clone());
        assert!(format!("{prov:?}").contains("SysprepProvisioner"));

        let json = serde_json::to_string(&cfg)?;
        let deser: SysprepConfig = serde_json::from_str(&json)?;
        assert_eq!(deser, cfg);
        Ok(())
    }

    #[tokio::test]
    async fn test_sysprep_provision_success() -> Result<(), StampError> {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let cfg = SysprepConfig::default();
        let prov = SysprepProvisioner::new(cfg);

        prov.provision(&comm, ui).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_sysprep_provision_fail() {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let cfg = SysprepConfig {
            command: "fail_cmd".to_string(),
            unattend_xml: None,
        };
        let prov = SysprepProvisioner::new(cfg);

        let res = prov.provision(&comm, ui).await;
        assert!(res.is_err());
    }
}
