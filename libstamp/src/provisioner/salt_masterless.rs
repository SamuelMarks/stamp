//! Implementation of the `salt-masterless` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use std::path::PathBuf;

/// Configuration for the `salt-masterless` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaltMasterlessConfig {
    /// Local path to the salt state tree.
    pub local_state_tree: Option<FilePath>,
    /// Optional local path to salt pillar data tree.
    pub local_pillar_roots: Option<FilePath>,
    /// Optional path to custom grains file.
    pub grains_file: Option<FilePath>,
    /// Optional path to a custom minion config file.
    pub minion_config: Option<FilePath>,
    /// Custom state to run (defaults to `state.highstate`).
    pub custom_state: Option<String>,
    /// Staging directory on the guest machine. Defaults to `/tmp/packer-salt-masterless`.
    pub staging_directory: Option<String>,
    /// Additional arguments to pass to `salt-call`.
    pub salt_call_args: Vec<String>,
    /// Optional command-line flags to pass to the salt bootstrap script.
    pub bootstrap_args: Option<String>,
    /// Whether to skip automatic Salt installation via the bootstrap script. Defaults to false.
    pub skip_bootstrap: bool,
    /// Whether to delete staging directory on guest upon completion. Defaults to true.
    pub clean_up: bool,
}

impl Default for SaltMasterlessConfig {
    fn default() -> Self {
        Self {
            local_state_tree: None,
            local_pillar_roots: None,
            grains_file: None,
            minion_config: None,
            custom_state: None,
            staging_directory: None,
            salt_call_args: Vec::new(),
            bootstrap_args: None,
            skip_bootstrap: false,
            clean_up: true,
        }
    }
}

/// The `salt-masterless` provisioner.
#[derive(Debug, Clone)]
pub struct SaltMasterlessProvisioner {
    /// The provisioner configuration.
    pub config: SaltMasterlessConfig,
}

impl SaltMasterlessProvisioner {
    /// Create a new `SaltMasterlessProvisioner`.
    #[must_use]
    pub const fn new(config: SaltMasterlessConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Provisioner for SaltMasterlessProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        let Some(state_tree) = &self.config.local_state_tree else {
            return Err(StampError::Provisioner(
                "local_state_tree is required".to_string(),
            ));
        };

        let remote_dir = self
            .config
            .staging_directory
            .as_deref()
            .unwrap_or("/tmp/packer-salt-masterless");

        // 0. Bootstrap Salt client if not skipped
        if !self.config.skip_bootstrap {
            let bootstrap_flags = self.config.bootstrap_args.as_deref().unwrap_or("-P");
            let bootstrap_cmd = format!(
                "curl -L https://bootstrap.saltproject.io -o /tmp/bootstrap-salt.sh && \
                 sh /tmp/bootstrap-salt.sh {bootstrap_flags}"
            );
            ui.say("salt-masterless", "Bootstrapping Salt on guest...");
            let res = comm.execute(&Command::new(bootstrap_cmd)).await?;
            if res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "Failed to bootstrap Salt. exit code: {}",
                    res.exit_code
                )));
            }
        }

        // 1. Create remote directories
        let mkdir_cmd = format!("mkdir -p {remote_dir}/roots {remote_dir}/pillars");
        let res = comm.execute(&Command::new(mkdir_cmd)).await?;
        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "Failed to create remote directory. exit code: {}",
                res.exit_code
            )));
        }

        // 2. Upload state tree
        let remote_roots_fp = FilePath::new(PathBuf::from(format!("{remote_dir}/roots")));
        comm.upload(state_tree, &remote_roots_fp).await?;

        // 3. Upload pillar tree if provided
        let mut pillar_arg = String::new();
        if let Some(pillar_roots) = &self.config.local_pillar_roots {
            let remote_pillars_fp = FilePath::new(PathBuf::from(format!("{remote_dir}/pillars")));
            comm.upload(pillar_roots, &remote_pillars_fp).await?;
            pillar_arg = format!(" --pillar-root={remote_dir}/pillars");
        }

        // 4. Upload grains file if provided
        if let Some(grains) = &self.config.grains_file {
            let remote_grains = format!("{remote_dir}/grains");
            comm.upload(grains, &FilePath::new(PathBuf::from(remote_grains)))
                .await?;
        }

        // 5. Upload minion config if provided
        let mut minion_args = String::new();
        if let Some(minion) = &self.config.minion_config {
            let remote_minion = format!("{remote_dir}/minion");
            let remote_minion_fp = FilePath::new(PathBuf::from(&remote_minion));
            comm.upload(minion, &remote_minion_fp).await?;
            minion_args = format!(" -c {remote_dir}");
        }

        let state = self
            .config
            .custom_state
            .as_deref()
            .unwrap_or("state.highstate");

        // 6. Build and execute salt-call command
        let mut cmd_str = format!(
            "salt-call --local {state}{minion_args} --file-root={remote_dir}/roots{pillar_arg} --retcode-passthrough"
        );

        if !self.config.salt_call_args.is_empty() {
            cmd_str.push(' ');
            cmd_str.push_str(&self.config.salt_call_args.join(" "));
        }

        ui.say("salt-masterless", &format!("Executing: {cmd_str}"));
        let res = comm.execute(&Command::new(cmd_str)).await?;

        if self.config.clean_up {
            ui.say("salt-masterless", "Cleaning up staging directory...");
            let rm_cmd = format!("rm -rf {remote_dir}");
            let _ = comm.execute(&Command::new(rm_cmd)).await;
        }

        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "salt-call failed with exit code: {}",
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
    async fn test_salt_masterless_provision_success() -> Result<(), StampError> {
        let config = SaltMasterlessConfig {
            local_state_tree: Some(FilePath::new(PathBuf::from("salt/roots"))),
            ..Default::default()
        };
        let prov = SaltMasterlessProvisioner::new(config);
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
    async fn test_salt_masterless_provision_with_pillars_and_grains() -> Result<(), StampError> {
        let config = SaltMasterlessConfig {
            local_state_tree: Some(FilePath::new(PathBuf::from("salt/roots"))),
            local_pillar_roots: Some(FilePath::new(PathBuf::from("salt/pillars"))),
            grains_file: Some(FilePath::new(PathBuf::from("grains"))),
            minion_config: Some(FilePath::new(PathBuf::from("minion"))),
            custom_state: Some("state.apply my_state".to_string()),
            staging_directory: Some("/opt/salt".to_string()),
            salt_call_args: vec!["--log-level=debug".to_string()],
            ..Default::default()
        };
        let prov = SaltMasterlessProvisioner::new(config);
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
    async fn test_salt_masterless_provision_missing_tree() {
        let config = SaltMasterlessConfig::default();
        let prov = SaltMasterlessProvisioner::new(config);
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
        assert!(matches!(result, Err(StampError::Provisioner(_))));
    }

    #[tokio::test]
    async fn test_salt_masterless_custom_bootstrap_and_cleanup() -> Result<(), StampError> {
        let config = SaltMasterlessConfig {
            local_state_tree: Some(FilePath::new(PathBuf::from("salt/roots"))),
            bootstrap_args: Some("-P -c /tmp".to_string()),
            skip_bootstrap: false,
            clean_up: true,
            ..Default::default()
        };
        let prov = SaltMasterlessProvisioner::new(config);
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

    #[test]
    fn test_derived_traits() {
        let config1 = SaltMasterlessConfig {
            local_state_tree: Some(FilePath::new(PathBuf::from("salt/roots"))),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = SaltMasterlessProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
