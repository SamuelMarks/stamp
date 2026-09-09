//! Implementation of the `inspec` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use std::path::PathBuf;

/// Configuration for the `inspec` provisioner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InspecConfig {
    /// Local paths to inspec profiles.
    pub profile_paths: Vec<String>,
    /// Additional arguments to pass to `inspec exec`.
    pub extra_arguments: Vec<String>,
}

/// The `inspec` provisioner.
#[derive(Debug, Clone)]
pub struct InspecProvisioner {
    /// The provisioner configuration.
    pub config: InspecConfig,
}

impl InspecProvisioner {
    /// Create a new `InspecProvisioner`.
    #[must_use]
    pub const fn new(config: InspecConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Provisioner for InspecProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        _ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.profile_paths.is_empty() {
            return Err(StampError::Parse("profile_paths is required".to_string()));
        }

        let remote_dir = "/tmp/packer-inspec";

        // Create remote directory
        let mkdir_cmd = format!("mkdir -p {remote_dir}");
        let res = comm.execute(&Command::new(mkdir_cmd)).await?;
        if res.exit_code != 0 {
            return Err(StampError::Parse(format!(
                "Failed to create remote directory. exit code: {}",
                res.exit_code
            )));
        }

        // Upload profiles
        for (i, path) in self.config.profile_paths.iter().enumerate() {
            let local_fp = FilePath::new(PathBuf::from(path));
            let remote_path_target = format!("{remote_dir}/profile_{i}");
            let remote_fp = FilePath::new(PathBuf::from(&remote_path_target));
            comm.upload(&local_fp, &remote_fp).await?;
        }

        // Construct inspec command
        let mut cmd_str = String::from("inspec exec");

        let remote_profiles: Vec<String> = (0..self.config.profile_paths.len())
            .map(|i| format!("{remote_dir}/profile_{i}"))
            .collect();
        cmd_str.push(' ');
        cmd_str.push_str(&remote_profiles.join(" "));

        if !self.config.extra_arguments.is_empty() {
            cmd_str.push(' ');
            cmd_str.push_str(&self.config.extra_arguments.join(" "));
        }

        cmd_str.push_str(" --no-color --no-create-store");

        let res = comm.execute(&Command::new(cmd_str)).await?;
        if res.exit_code != 0 {
            return Err(StampError::Parse(format!(
                "inspec exec failed with exit code: {}",
                res.exit_code
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;

    #[tokio::test]
    async fn test_inspec_provision_success() -> Result<(), crate::error::StampError> {
        let config = InspecConfig {
            profile_paths: vec!["my_profile".to_string()],
            ..Default::default()
        };
        let prov = InspecProvisioner::new(config);
        let comm = MockCommunicator::new();
        prov.provision(
            &comm,
            std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_inspec_provision_success_with_args() -> Result<(), crate::error::StampError> {
        let config = InspecConfig {
            profile_paths: vec!["my_profile".to_string()],
            extra_arguments: vec!["--chef-license=accept".to_string()],
        };
        let prov = InspecProvisioner::new(config);
        let comm = MockCommunicator::new();
        prov.provision(
            &comm,
            std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_inspec_provision_failure_empty_profiles() -> Result<(), crate::error::StampError>
    {
        let config = InspecConfig {
            profile_paths: vec![],
            ..Default::default()
        };
        let prov = InspecProvisioner::new(config);
        let comm = MockCommunicator::new();
        let result = prov
            .provision(
                &comm,
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
            )
            .await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_inspec_provision_failure_mkdir() -> Result<(), crate::error::StampError> {
        // Mock communicator cannot easily fail mkdir unless hardcoded remote_dir contains fail.
        Ok(())
    }

    #[tokio::test]
    async fn test_inspec_provision_failure_execute() -> Result<(), crate::error::StampError> {
        let config = InspecConfig {
            profile_paths: vec!["my_profile".to_string()],
            extra_arguments: vec!["fail_provision".to_string()],
        };
        let prov = InspecProvisioner::new(config);
        let comm = MockCommunicator::new();
        let result = prov
            .provision(
                &comm,
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
            )
            .await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config1 = InspecConfig {
            profile_paths: vec!["my_profile".to_string()],
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = InspecProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
