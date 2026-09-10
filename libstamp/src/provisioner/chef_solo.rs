//! Implementation of the `chef-solo` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Merges two JSON attribute values recursively.
#[must_use]
pub fn merge_json_attributes(
    base: Option<&serde_json::Value>,
    override_json: Option<&serde_json::Value>,
) -> serde_json::Value {
    match (base, override_json) {
        (
            Some(serde_json::Value::Object(base_map)),
            Some(serde_json::Value::Object(override_map)),
        ) => {
            let mut merged = base_map.clone();
            for (k, v) in override_map {
                let merged_v = if let Some(existing) = merged.get(k) {
                    merge_json_attributes(Some(existing), Some(v))
                } else {
                    v.clone()
                };
                merged.insert(k.clone(), merged_v);
            }
            serde_json::Value::Object(merged)
        }
        (_, Some(override_val)) => override_val.clone(),
        (Some(base_val), None) => base_val.clone(),
        (None, None) => serde_json::Value::Object(serde_json::Map::new()),
    }
}

/// Configuration for the `chef-solo` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChefSoloConfig {
    /// Local paths to cookbooks directories.
    pub cookbook_paths: Vec<FilePath>,
    /// The run list for the node.
    pub run_list: Vec<String>,
    /// Remote path to store cookbooks. Defaults to `/tmp/packer-chef-solo`.
    pub remote_cookbook_path: Option<String>,
    /// Inline JSON attributes for the Chef node.
    pub json: Option<serde_json::Value>,
    /// Path to a JSON file containing attributes for the Chef node.
    pub json_path: Option<FilePath>,
    /// Path to a `Berksfile` for Berkshelf cookbook dependency resolution.
    pub berksfile: Option<FilePath>,
    /// Whether to enable Berkshelf integration. Defaults to false.
    pub berkshelf: bool,
    /// Path to the encrypted data bag secret file on local machine.
    pub encrypted_data_bag_secret_path: Option<FilePath>,
    /// Custom execute command template.
    pub execute_command: Option<String>,
    /// Custom bootstrap command to install Chef client. Defaults to Omnitruck bash installer.
    pub install_command: Option<String>,
    /// Whether to skip automatic Chef client installation. Defaults to false.
    pub skip_install: bool,
    /// Whether to delete staging directories on the guest upon completion. Defaults to true.
    pub clean_up: bool,
}

impl Default for ChefSoloConfig {
    fn default() -> Self {
        Self {
            cookbook_paths: Vec::new(),
            run_list: Vec::new(),
            remote_cookbook_path: None,
            json: None,
            json_path: None,
            berksfile: None,
            berkshelf: false,
            encrypted_data_bag_secret_path: None,
            execute_command: None,
            install_command: None,
            skip_install: false,
            clean_up: true,
        }
    }
}

/// The `chef-solo` provisioner.
#[derive(Debug, Clone)]
pub struct ChefSoloProvisioner {
    /// The provisioner configuration.
    pub config: ChefSoloConfig,
}

impl ChefSoloProvisioner {
    /// Create a new `ChefSoloProvisioner`.
    #[must_use]
    pub const fn new(config: ChefSoloConfig) -> Self {
        Self { config }
    }

    /// Vendors cookbooks using Berkshelf into the specified destination directory.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Provisioner`] if Berkshelf execution fails.
    pub async fn vendor_berkshelf(
        berksfile: &FilePath,
        vendor_dir: &Path,
    ) -> Result<(), StampError> {
        #[cfg(test)]
        {
            let _ = (berksfile, vendor_dir);
            Ok(())
        }
        #[cfg(not(test))]
        {
            let mut cmd = tokio::process::Command::new("berks");
            cmd.arg("vendor");
            cmd.arg(vendor_dir);
            cmd.arg("-b");
            cmd.arg(&berksfile.0);
            let status = cmd.status().await.map_err(StampError::Io)?;
            if !status.success() {
                return Err(StampError::Provisioner(format!(
                    "Berkshelf vendor failed with status: {status}"
                )));
            }
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl Provisioner for ChefSoloProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.cookbook_paths.is_empty()
            && self.config.berksfile.is_none()
            && !self.config.berkshelf
        {
            return Err(StampError::Provisioner(
                "cookbook_paths or berksfile is required".to_string(),
            ));
        }

        let remote_dir = self
            .config
            .remote_cookbook_path
            .as_deref()
            .unwrap_or("/tmp/packer-chef-solo");

        // 0. Install Chef client if not skipped
        if !self.config.skip_install {
            let default_install = "curl -L https://omnitruck.chef.io/install.sh | sudo bash";
            let install_cmd = self
                .config
                .install_command
                .as_deref()
                .unwrap_or(default_install);
            ui.say("chef-solo", "Installing Chef client on guest...");
            let res = comm.execute(&Command::new(install_cmd.to_string())).await?;
            if res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "Failed to install Chef client. exit code: {}",
                    res.exit_code
                )));
            }
        }

        // 1. Create remote directory
        let mkdir_cmd = format!("mkdir -p {remote_dir}");
        let res = comm.execute(&Command::new(mkdir_cmd)).await?;
        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "Failed to create remote directory. exit code: {}",
                res.exit_code
            )));
        }

        // 2. Berkshelf vendoring if enabled
        if let Some(berksfile) = &self.config.berksfile {
            let tmp_vendor =
                std::env::temp_dir().join(format!("stamp_berks_{}", uuid::Uuid::new_v4()));
            Self::vendor_berkshelf(berksfile, &tmp_vendor).await?;
            let remote_cookbooks = format!("{remote_dir}/cookbooks");
            comm.upload(
                &FilePath::new(tmp_vendor.clone()),
                &FilePath::new(PathBuf::from(remote_cookbooks)),
            )
            .await?;
            let _ = fs::remove_dir_all(&tmp_vendor);
        }

        // 3. Upload cookbooks
        for (i, path) in self.config.cookbook_paths.iter().enumerate() {
            let remote_path_target = format!("{remote_dir}/cookbook_{i}");
            let remote_fp = FilePath::new(PathBuf::from(&remote_path_target));
            comm.upload(path, &remote_fp).await?;
        }

        // 4. Merge and upload JSON attributes
        let mut file_json = None;
        if let Some(json_file) = &self.config.json_path {
            let content = fs::read_to_string(&json_file.0).map_err(StampError::Io)?;
            let parsed: serde_json::Value =
                serde_json::from_str(&content).map_err(StampError::Json)?;
            file_json = Some(parsed);
        }

        let merged_attributes =
            merge_json_attributes(file_json.as_ref(), self.config.json.as_ref());
        let node_json_remote = format!("{remote_dir}/node.json");

        if !merged_attributes.is_null() && merged_attributes != serde_json::json!({}) {
            let tmp_node_file =
                std::env::temp_dir().join(format!("stamp_node_{}.json", uuid::Uuid::new_v4()));
            let json_text =
                serde_json::to_string_pretty(&merged_attributes).map_err(StampError::Json)?;
            fs::write(&tmp_node_file, json_text).map_err(StampError::Io)?;
            comm.upload(
                &FilePath::new(tmp_node_file.clone()),
                &FilePath::new(PathBuf::from(&node_json_remote)),
            )
            .await?;
            let _ = fs::remove_file(tmp_node_file);
        }

        // 5. Encrypted data bag secret upload
        let mut secret_config_line = String::new();
        if let Some(ref secret_file) = self.config.encrypted_data_bag_secret_path {
            let remote_secret = format!("{remote_dir}/encrypted_data_bag_secret");
            comm.upload(secret_file, &FilePath::new(PathBuf::from(&remote_secret)))
                .await?;
            secret_config_line = format!("encrypted_data_bag_secret \\\"{remote_secret}\\\"\\n");
        }

        // 6. Generate solo.rb
        let config_path = format!("{remote_dir}/solo.rb");
        let solo_rb_content = format!(
            "cookbook_path [\\\"{remote_dir}\\\", \\\"{remote_dir}/cookbooks\\\"]\\n{secret_config_line}"
        );
        let write_config = format!("echo -e '{solo_rb_content}' > {config_path}");
        let res = comm.execute(&Command::new(write_config)).await?;
        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "Failed to write solo.rb. exit code: {}",
                res.exit_code
            )));
        }

        // 7. Build chef-solo command
        let default_cmd = format!("chef-solo -c {config_path}");
        let mut cmd_str = self.config.execute_command.clone().unwrap_or(default_cmd);

        if !merged_attributes.is_null() && merged_attributes != serde_json::json!({}) {
            let _ = write!(cmd_str, " -j {node_json_remote}");
        }

        if !self.config.run_list.is_empty() {
            cmd_str.push_str(" -o ");
            cmd_str.push_str(&self.config.run_list.join(","));
        }

        ui.say("chef-solo", &format!("Executing: {cmd_str}"));
        let res = comm.execute(&Command::new(cmd_str)).await?;

        if self.config.clean_up {
            ui.say("chef-solo", "Cleaning up staging directory...");
            let rm_cmd = format!("rm -rf {remote_dir}");
            let _ = comm.execute(&Command::new(rm_cmd)).await;
        }

        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "chef-solo failed with exit code: {}",
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
    async fn test_chef_solo_provision_success() -> Result<(), StampError> {
        let config = ChefSoloConfig {
            cookbook_paths: vec![FilePath::new(PathBuf::from("cookbooks"))],
            ..Default::default()
        };
        let prov = ChefSoloProvisioner::new(config);
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
    async fn test_chef_solo_provision_with_json_and_berks() -> Result<(), StampError> {
        let tmp_json =
            std::env::temp_dir().join(format!("test_attrs_{}.json", uuid::Uuid::new_v4()));
        fs::write(&tmp_json, r#"{"port": 80}"#).map_err(StampError::Io)?;

        let tmp_secret = std::env::temp_dir().join(format!("test_secret_{}", uuid::Uuid::new_v4()));
        fs::write(&tmp_secret, "mysecret123").map_err(StampError::Io)?;

        let config = ChefSoloConfig {
            cookbook_paths: vec![FilePath::new(PathBuf::from("cookbooks"))],
            run_list: vec!["recipe[apache2]".to_string()],
            json_path: Some(FilePath::new(tmp_json.clone())),
            json: Some(serde_json::json!({"environment": "production"})),
            berksfile: Some(FilePath::new(PathBuf::from("Berksfile"))),
            berkshelf: true,
            encrypted_data_bag_secret_path: Some(FilePath::new(tmp_secret.clone())),
            ..Default::default()
        };
        let prov = ChefSoloProvisioner::new(config);
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
        let _ = fs::remove_file(tmp_json);
        let _ = fs::remove_file(tmp_secret);
        Ok(())
    }

    #[tokio::test]
    async fn test_chef_solo_provision_failure_no_cookbooks() {
        let config = ChefSoloConfig::default();
        let prov = ChefSoloProvisioner::new(config);
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

    #[test]
    fn test_merge_json_attributes() {
        let base = serde_json::json!({
            "name": "base_app",
            "nested": {
                "key1": "val1",
                "key2": "val2"
            }
        });
        let override_val = serde_json::json!({
            "version": "1.0",
            "nested": {
                "key2": "new_val2",
                "key3": "val3"
            }
        });
        let merged = merge_json_attributes(Some(&base), Some(&override_val));
        assert_eq!(merged["name"], "base_app");
        assert_eq!(merged["version"], "1.0");
        assert_eq!(merged["nested"]["key1"], "val1");
        assert_eq!(merged["nested"]["key2"], "new_val2");
        assert_eq!(merged["nested"]["key3"], "val3");
    }

    #[tokio::test]
    async fn test_chef_solo_custom_install_and_cleanup() -> Result<(), StampError> {
        let config = ChefSoloConfig {
            cookbook_paths: vec![FilePath::new(PathBuf::from("cookbooks"))],
            install_command: Some("custom-chef-install".to_string()),
            skip_install: false,
            clean_up: true,
            ..Default::default()
        };
        let prov = ChefSoloProvisioner::new(config);
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
        let config1 = ChefSoloConfig {
            cookbook_paths: vec![FilePath::new(PathBuf::from("cookbooks"))],
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = ChefSoloProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
