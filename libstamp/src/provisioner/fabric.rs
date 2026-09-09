//! Fabric provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the `fabric` provisioner.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FabricConfig {
    /// Optional path to the local fabfile to upload and run.
    pub fabfile: Option<FilePath>,
    /// Tasks to execute.
    #[serde(default)]
    pub tasks: Vec<String>,
    /// Additional arguments to pass to the `fab` CLI.
    #[serde(default)]
    pub extra_arguments: Vec<String>,
    /// Staging directory on the remote machine. Defaults to `/tmp/packer-fabric`.
    pub staging_directory: Option<String>,
}

/// The Fabric provisioner.
#[derive(Debug, Clone)]
pub struct FabricProvisioner {
    /// Configuration for the provisioner.
    pub config: FabricConfig,
}

impl FabricProvisioner {
    /// Creates a new `FabricProvisioner`.
    #[must_use]
    pub const fn new(config: FabricConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provisioner for FabricProvisioner {
    async fn provision(
        &self,
        communicator: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        let staging_dir = self
            .config
            .staging_directory
            .as_deref()
            .unwrap_or("/tmp/packer-fabric");

        let mut fabfile_path = String::from("fabfile.py");

        if let Some(fabfile) = &self.config.fabfile {
            let mkdir_cmd = format!("mkdir -p {staging_dir}");
            let res = communicator.execute(&Command::new(mkdir_cmd)).await?;
            if res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "Failed to create staging directory for Fabric. exit code: {}",
                    res.exit_code
                )));
            }

            let remote_fabfile = format!("{staging_dir}/fabfile.py");
            communicator
                .upload(fabfile, &FilePath::new(PathBuf::from(&remote_fabfile)))
                .await?;
            fabfile_path = remote_fabfile;
        }

        ui.say(
            "fabric",
            &format!("Running Fabric tasks: {:?}", self.config.tasks),
        );

        let mut cmd_str = format!("fab -f {fabfile_path} {}", self.config.tasks.join(" "));
        if !self.config.extra_arguments.is_empty() {
            cmd_str.push(' ');
            cmd_str.push_str(&self.config.extra_arguments.join(" "));
        }

        let cmd = Command::new(cmd_str);
        let res = communicator.execute(&cmd).await?;

        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "Fabric execution failed with exit code: {}",
                res.exit_code
            )));
        }

        ui.say("fabric", "Fabric execution completed successfully");
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    #![cfg_attr(coverage_nightly, coverage(off))]
    use super::*;
    use crate::communicator::mock::MockCommunicator;

    #[tokio::test]
    async fn test_fabric_provision_success() {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let cfg = FabricConfig {
            fabfile: Some(FilePath::new(PathBuf::from("fabfile.py"))),
            tasks: vec!["deploy".into()],
            extra_arguments: vec!["--show=debug".to_string()],
            staging_directory: Some("/tmp/custom-fabric".to_string()),
        };
        let prov = FabricProvisioner::new(cfg);

        let res = prov.provision(&comm, ui).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_fabric_staging_dir_fail() {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let cfg = FabricConfig {
            fabfile: Some(FilePath::new(PathBuf::from("fabfile.py"))),
            tasks: vec!["deploy".into()],
            extra_arguments: vec![],
            staging_directory: Some("/tmp/fail_staging".to_string()),
        };
        let prov = FabricProvisioner::new(cfg);

        let res = prov.provision(&comm, ui).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_fabric_provision_failure() {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let cfg = FabricConfig {
            fabfile: None,
            tasks: vec!["fail_provision".into()],
            ..Default::default()
        };
        let prov = FabricProvisioner::new(cfg);

        let res = prov.provision(&comm, ui).await;
        assert!(res.is_err());
    }

    #[test]
    fn test_fabric_derived_traits() {
        let config1 = FabricConfig {
            tasks: vec!["test".to_string()],
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let p1 = FabricProvisioner::new(config1.clone());
        let p2 = p1.clone();
        assert_eq!(format!("{p1:?}"), format!("{p2:?}"));

        let json = serde_json::to_string(&config1).unwrap();
        let deser: FabricConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, config1);
    }
}
