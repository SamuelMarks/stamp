//! Implementation of the `goss` provisioner for specification validation.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use async_trait::async_trait;
use std::path::PathBuf;

/// Configuration for the `goss` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GossConfig {
    /// Local path or paths to the Goss specification files. Defaults to `["goss.yaml"]`.
    pub tests: Vec<String>,
    /// Version of Goss to download (e.g. `v0.4.4`). Defaults to `v0.4.4`.
    pub version: Option<String>,
    /// Architecture to download (`amd64` or `arm64`). Defaults to `amd64`.
    pub arch: Option<String>,
    /// Target path where Goss will be downloaded on the guest machine. Defaults to `/tmp/goss`.
    pub download_path: Option<String>,
    /// Whether to skip downloading Goss if it is already installed.
    pub skip_install: bool,
    /// Whether to run Goss with `sudo`.
    pub use_sudo: bool,
    /// Whether to output verbose inspection details.
    pub inspect: bool,
    /// Remote path to the primary Goss test file. Defaults to `/tmp/goss/goss.yaml`.
    pub remote_folder: Option<String>,
}

impl Default for GossConfig {
    fn default() -> Self {
        Self {
            tests: vec!["goss.yaml".to_string()],
            version: Some("v0.4.4".to_string()),
            arch: Some("amd64".to_string()),
            download_path: Some("/tmp/goss".to_string()),
            skip_install: false,
            use_sudo: false,
            inspect: false,
            remote_folder: Some("/tmp/packer-goss".to_string()),
        }
    }
}

/// The `goss` provisioner.
#[derive(Debug, Clone)]
pub struct GossProvisioner {
    /// The provisioner configuration.
    pub config: GossConfig,
}

impl GossProvisioner {
    /// Creates a new `GossProvisioner`.
    #[must_use]
    pub const fn new(config: GossConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provisioner for GossProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        if self.config.tests.is_empty() {
            return Err(StampError::Parse(
                "tests array is required for goss provisioner".to_string(),
            ));
        }

        let remote_dir = self
            .config
            .remote_folder
            .as_deref()
            .unwrap_or("/tmp/packer-goss");
        let download_path = self.config.download_path.as_deref().unwrap_or("/tmp/goss");

        // 1. Create remote test folder
        let mkdir_cmd = format!("mkdir -p {remote_dir}");
        let res = comm.execute(&Command::new(mkdir_cmd)).await?;
        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "Failed to create goss test directory: exit code {}",
                res.exit_code
            )));
        }

        // 2. Download goss binary if not skipped
        if !self.config.skip_install {
            let version = self.config.version.as_deref().unwrap_or("v0.4.4");
            let arch = self.config.arch.as_deref().unwrap_or("amd64");
            let goss_url = format!(
                "https://github.com/goss-org/goss/releases/download/{version}/goss-linux-{arch}"
            );

            ui.say("goss", &format!("Downloading Goss from {goss_url}"));
            let dl_cmd =
                format!("curl -L -s -S -o {download_path} {goss_url} && chmod +x {download_path}");
            let dl_res = comm.execute(&Command::new(dl_cmd)).await?;
            if dl_res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "Failed to download Goss binary: exit code {}",
                    dl_res.exit_code
                )));
            }
        }

        // 3. Upload test files
        for test_file in &self.config.tests {
            let local_fp = FilePath::new(PathBuf::from(test_file));
            let file_name = PathBuf::from(test_file).file_name().map_or_else(
                || "goss.yaml".to_string(),
                |f| f.to_string_lossy().to_string(),
            );
            let remote_dest = format!("{remote_dir}/{file_name}");
            let remote_fp = FilePath::new(PathBuf::from(&remote_dest));
            ui.say("goss", &format!("Uploading Goss spec: {test_file}"));
            comm.upload(&local_fp, &remote_fp).await?;
        }

        // 4. Execute goss validate
        let sudo_prefix = if self.config.use_sudo { "sudo " } else { "" };
        let inspect_flag = if self.config.inspect {
            " --format documentation"
        } else {
            ""
        };
        let primary_spec = format!("{remote_dir}/goss.yaml");
        let validate_cmd = format!(
            "{sudo_prefix}{download_path} --gossfile {primary_spec} validate{inspect_flag}"
        );

        ui.say("goss", &format!("Running Goss validation: {validate_cmd}"));
        let val_res = comm.execute(&Command::new(validate_cmd)).await?;
        if val_res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "Goss validation failed with exit code: {}",
                val_res.exit_code
            )));
        }

        ui.say("goss", "Goss verification passed successfully!");
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;
    use crate::engine::packer::FeatureState;
    use crate::engine::ui::Ui;
    use std::sync::Arc;

    #[test]
    fn test_derived_traits() {
        let config = GossConfig {
            tests: vec!["goss.yaml".to_string()],
            version: Some("v0.4.4".to_string()),
            arch: Some("amd64".to_string()),
            download_path: Some("/tmp/goss".to_string()),
            skip_install: true,
            use_sudo: true,
            inspect: true,
            remote_folder: Some("/tmp/specs".to_string()),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let prov = GossProvisioner::new(config);
        assert_eq!(format!("{prov:?}"), format!("{:?}", prov.clone()));
    }

    #[tokio::test]
    async fn test_goss_provision_success() {
        let mock_comm = MockCommunicator::new();
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));

        let temp_dir = std::env::temp_dir();
        let test_spec = temp_dir.join("goss.yaml");
        let _ = tokio::fs::write(
            &test_spec,
            "port:
  tcp:22:
    listening: true
",
        )
        .await;

        let prov = GossProvisioner::new(GossConfig {
            tests: vec![test_spec.to_string_lossy().to_string()],
            skip_install: false,
            use_sudo: true,
            inspect: true,
            ..Default::default()
        });

        let res = prov.provision(&mock_comm, ui).await;
        assert!(res.is_ok());
        let _ = tokio::fs::remove_file(&test_spec).await;
    }

    #[tokio::test]
    async fn test_goss_provision_empty_tests() {
        let mock_comm = MockCommunicator::new();
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));

        let prov = GossProvisioner::new(GossConfig {
            tests: vec![],
            ..Default::default()
        });

        assert!(prov.provision(&mock_comm, ui).await.is_err());
    }

    #[tokio::test]
    async fn test_goss_provision_mkdir_fail() {
        let mock_comm = MockCommunicator::new();
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));

        let prov = GossProvisioner::new(GossConfig {
            tests: vec!["goss.yaml".to_string()],
            remote_folder: Some("/tmp/fail_mkdir".to_string()),
            ..Default::default()
        });

        assert!(prov.provision(&mock_comm, ui).await.is_err());
    }
}
