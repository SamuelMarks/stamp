//! Implementation of the `puppet-masterless` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Configuration for the `puppet-masterless` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PuppetMasterlessConfig {
    /// The main manifest file to apply.
    pub manifest_file: String,
    /// Optional directory containing manifests.
    pub manifest_dir: Option<FilePath>,
    /// Paths to directories containing Puppet modules.
    pub module_paths: Vec<FilePath>,
    /// Path to a `hiera.yaml` configuration file.
    pub hiera_config_path: Option<FilePath>,
    /// Custom facts to expose to Puppet.
    pub facter: HashMap<String, String>,
    /// Staging directory on the guest machine. Defaults to `/tmp/packer-puppet-masterless`.
    pub staging_directory: Option<String>,
    /// Additional arguments to pass to `puppet apply`.
    pub extra_arguments: Vec<String>,
    /// Custom execute command.
    pub execute_command: Option<String>,
    /// Custom bootstrap command to install Puppet client on guest.
    pub install_command: Option<String>,
    /// Whether to skip automatic Puppet installation. Defaults to false.
    pub skip_install: bool,
    /// Whether to delete staging directory on guest upon completion. Defaults to true.
    pub clean_up: bool,
}

impl Default for PuppetMasterlessConfig {
    fn default() -> Self {
        Self {
            manifest_file: String::new(),
            manifest_dir: None,
            module_paths: Vec::new(),
            hiera_config_path: None,
            facter: HashMap::new(),
            staging_directory: None,
            extra_arguments: Vec::new(),
            execute_command: None,
            install_command: None,
            skip_install: false,
            clean_up: true,
        }
    }
}

/// The `puppet-masterless` provisioner.
#[derive(Debug, Clone)]
pub struct PuppetMasterlessProvisioner {
    /// The provisioner configuration.
    pub config: PuppetMasterlessConfig,
}

impl PuppetMasterlessProvisioner {
    /// Create a new `PuppetMasterlessProvisioner`.
    #[must_use]
    pub const fn new(config: PuppetMasterlessConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Provisioner for PuppetMasterlessProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.manifest_file.is_empty() {
            return Err(StampError::Provisioner(
                "manifest_file is required".to_string(),
            ));
        }

        let remote_dir = self
            .config
            .staging_directory
            .as_deref()
            .unwrap_or("/tmp/packer-puppet-masterless");

        // 0. Install Puppet client if not skipped
        if !self.config.skip_install {
            let default_install =
                "apt-get update && apt-get install -y puppet || yum install -y puppet";
            let install_cmd = self
                .config
                .install_command
                .as_deref()
                .unwrap_or(default_install);
            ui.say("puppet-masterless", "Installing Puppet client on guest...");
            let res = comm.execute(&Command::new(install_cmd.to_string())).await?;
            if res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "Failed to install Puppet client. exit code: {}",
                    res.exit_code
                )));
            }
        }

        // 1. Create remote directory
        let mkdir_cmd = format!("mkdir -p {remote_dir}/modules {remote_dir}/manifests");
        let res = comm.execute(&Command::new(mkdir_cmd)).await?;
        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "Failed to create remote directory. exit code: {}",
                res.exit_code
            )));
        }

        // 2. Upload modules
        let mut remote_module_dirs = Vec::new();
        for (i, path) in self.config.module_paths.iter().enumerate() {
            let module_name = path.0.file_name().map_or_else(
                || format!("module_{i}"),
                |n| n.to_string_lossy().to_string(),
            );
            let remote_path_target = format!("{remote_dir}/modules/{module_name}");
            let remote_fp = FilePath::new(PathBuf::from(&remote_path_target));
            comm.upload(path, &remote_fp).await?;
            remote_module_dirs.push(remote_path_target);
        }

        // 3. Upload manifest directory if provided
        if let Some(m_dir) = &self.config.manifest_dir {
            let remote_m_dir = format!("{remote_dir}/manifests");
            comm.upload(m_dir, &FilePath::new(PathBuf::from(remote_m_dir)))
                .await?;
        }

        // 4. Upload main manifest
        let manifest_name = Path::new(&self.config.manifest_file)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let remote_manifest = format!("{remote_dir}/{manifest_name}");
        let local_fp = FilePath::new(PathBuf::from(&self.config.manifest_file));
        let remote_fp = FilePath::new(PathBuf::from(&remote_manifest));
        comm.upload(&local_fp, &remote_fp).await?;

        // 5. Upload hiera.yaml if specified
        let mut hiera_arg = String::new();
        if let Some(hiera_file) = &self.config.hiera_config_path {
            let remote_hiera = format!("{remote_dir}/hiera.yaml");
            comm.upload(hiera_file, &FilePath::new(PathBuf::from(&remote_hiera)))
                .await?;
            hiera_arg = format!(" --hiera_config={remote_hiera}");
        }

        // 6. Build puppet command
        let default_cmd = format!("puppet apply {remote_manifest}");
        let mut cmd_str = self.config.execute_command.clone().unwrap_or(default_cmd);

        if !remote_module_dirs.is_empty() {
            cmd_str.push_str(" --modulepath=");
            cmd_str.push_str(&remote_module_dirs.join(":"));
        }

        if !hiera_arg.is_empty() {
            cmd_str.push_str(&hiera_arg);
        }

        if !self.config.extra_arguments.is_empty() {
            cmd_str.push(' ');
            cmd_str.push_str(&self.config.extra_arguments.join(" "));
        }

        let mut env_str = String::new();
        for (k, v) in &self.config.facter {
            use std::fmt::Write;
            let _ = write!(env_str, "FACTER_{k}=\"{v}\" ");
        }

        let final_cmd = if env_str.is_empty() {
            cmd_str
        } else {
            format!("{env_str}{cmd_str}")
        };

        ui.say("puppet-masterless", &format!("Executing: {final_cmd}"));
        let res = comm.execute(&Command::new(final_cmd)).await?;

        if self.config.clean_up {
            ui.say("puppet-masterless", "Cleaning up staging directory...");
            let rm_cmd = format!("rm -rf {remote_dir}");
            let _ = comm.execute(&Command::new(rm_cmd)).await;
        }

        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "puppet apply failed with exit code: {}",
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
    async fn test_puppet_masterless_provision_success() -> Result<(), StampError> {
        let config = PuppetMasterlessConfig {
            manifest_file: "site.pp".to_string(),
            ..Default::default()
        };
        let prov = PuppetMasterlessProvisioner::new(config);
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
    async fn test_puppet_masterless_provision_with_hiera_and_modules() -> Result<(), StampError> {
        let mut facter = HashMap::new();
        facter.insert("role".to_string(), "database".to_string());

        let config = PuppetMasterlessConfig {
            manifest_file: "site.pp".to_string(),
            manifest_dir: Some(FilePath::new(PathBuf::from("manifests"))),
            module_paths: vec![
                FilePath::new(PathBuf::from("modules/nginx")),
                FilePath::new(PathBuf::from("/")),
            ],
            hiera_config_path: Some(FilePath::new(PathBuf::from("hiera.yaml"))),
            facter,
            extra_arguments: vec!["--verbose".to_string()],
            staging_directory: Some("/opt/puppet".to_string()),
            ..Default::default()
        };
        let prov = PuppetMasterlessProvisioner::new(config);
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
    async fn test_puppet_masterless_provision_failure_empty_manifest() {
        let config = PuppetMasterlessConfig::default();
        let prov = PuppetMasterlessProvisioner::new(config);
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
    async fn test_puppet_masterless_custom_install_and_cleanup() -> Result<(), StampError> {
        let config = PuppetMasterlessConfig {
            manifest_file: "site.pp".to_string(),
            install_command: Some("custom-puppet-install".to_string()),
            skip_install: false,
            clean_up: true,
            ..Default::default()
        };
        let prov = PuppetMasterlessProvisioner::new(config);
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
        let config1 = PuppetMasterlessConfig {
            manifest_file: "site.pp".to_string(),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = PuppetMasterlessProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
