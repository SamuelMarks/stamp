//! Implementation of the `chef-client` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::provisioner::chef_solo::merge_json_attributes;
use crate::types::FilePath;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

/// Cleanup and error-handling options for Chef provisioning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChefCleanupOptions {
    /// Whether to delete the Chef node on error during provisioning.
    pub delete_node_on_error: bool,
    /// Whether to delete the Chef client on error during provisioning.
    pub delete_client_on_error: bool,
    /// Whether to delete staging directories and keys on the guest upon completion. Defaults to true.
    pub clean_up: bool,
}

impl Default for ChefCleanupOptions {
    fn default() -> Self {
        Self {
            delete_node_on_error: false,
            delete_client_on_error: false,
            clean_up: true,
        }
    }
}

/// Configuration for the `chef-client` provisioner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChefClientConfig {
    /// URL of the Chef server.
    pub server_url: String,
    /// Name of the validation client.
    pub validation_client_name: String,
    /// Path to the validation key on the local machine.
    pub validation_key_path: Option<FilePath>,
    /// Path to the existing client key if already registered.
    pub client_key: Option<FilePath>,
    /// The run list for the node.
    pub run_list: Vec<String>,
    /// Node name.
    pub node_name: Option<String>,
    /// Chef environment name.
    pub chef_environment: Option<String>,
    /// Inline JSON attributes for the Chef node.
    pub json: Option<serde_json::Value>,
    /// Path to a JSON file containing attributes for the Chef node.
    pub json_path: Option<FilePath>,
    /// Path to the encrypted data bag secret file on local machine.
    pub encrypted_data_bag_secret_path: Option<FilePath>,
    /// Guest OS platform type (e.g. `linux`, `windows`).
    pub guest_os_type: Option<String>,
    /// Custom execute command template.
    pub execute_command: Option<String>,
    /// Custom bootstrap command to install Chef client. Defaults to Omnitruck bash installer.
    pub install_command: Option<String>,
    /// Whether to skip automatic Chef client installation. Defaults to false.
    pub skip_install: bool,
    /// Cleanup options.
    pub cleanup: ChefCleanupOptions,
}

/// The `chef-client` provisioner.
#[derive(Debug, Clone)]
pub struct ChefClientProvisioner {
    /// The provisioner configuration.
    pub config: ChefClientConfig,
}

impl ChefClientProvisioner {
    /// Create a new `ChefClientProvisioner`.
    #[must_use]
    pub const fn new(config: ChefClientConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Provisioner for ChefClientProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.server_url.is_empty() {
            return Err(StampError::Provisioner(
                "server_url is required".to_string(),
            ));
        }
        if self.config.validation_client_name.is_empty() {
            return Err(StampError::Provisioner(
                "validation_client_name is required".to_string(),
            ));
        }

        let remote_dir = "/tmp/packer-chef-client";

        // 0. Install Chef client if not skipped
        if !self.config.skip_install {
            let default_install = "curl -L https://omnitruck.chef.io/install.sh | sudo bash";
            let install_cmd = self
                .config
                .install_command
                .as_deref()
                .unwrap_or(default_install);
            ui.say("chef-client", "Installing Chef client on guest...");
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

        // 2. Upload validation key if provided
        let remote_key_path = format!("{remote_dir}/validation.pem");
        if let Some(key_path) = &self.config.validation_key_path {
            comm.upload(key_path, &FilePath::new(PathBuf::from(&remote_key_path)))
                .await?;
        }

        // 3. Merge JSON attributes
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
            let tmp_node_file = std::env::temp_dir()
                .join(format!("stamp_client_node_{}.json", uuid::Uuid::new_v4()));
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

        let mut secret_config_arg = String::new();
        if let Some(ref secret_file) = self.config.encrypted_data_bag_secret_path {
            let remote_secret = format!("{remote_dir}/encrypted_data_bag_secret");
            comm.upload(secret_file, &FilePath::new(PathBuf::from(&remote_secret)))
                .await?;
            secret_config_arg = format!(" --secret-file {remote_secret}");
        }

        // 4. Construct chef-client command
        let default_cmd = format!(
            "chef-client --server-url {} --validation_client_name {} --validation_key {}",
            self.config.server_url, self.config.validation_client_name, remote_key_path
        );
        let mut cmd_str = self.config.execute_command.clone().unwrap_or(default_cmd);

        if !secret_config_arg.is_empty() {
            cmd_str.push_str(&secret_config_arg);
        }

        if let Some(node_name) = &self.config.node_name {
            cmd_str.push_str(" --node-name ");
            cmd_str.push_str(node_name);
        }

        if let Some(env) = &self.config.chef_environment {
            cmd_str.push_str(" --environment ");
            cmd_str.push_str(env);
        }

        if !merged_attributes.is_null() && merged_attributes != serde_json::json!({}) {
            let _ = write!(cmd_str, " -j {node_json_remote}");
        }

        if !self.config.run_list.is_empty() {
            cmd_str.push_str(" --runlist ");
            cmd_str.push_str(&self.config.run_list.join(","));
        }

        ui.say("chef-client", &format!("Executing: {cmd_str}"));
        let res = comm.execute(&Command::new(cmd_str)).await;

        let node_id = self.config.node_name.as_deref().unwrap_or("packer-node");

        if let Err(ref e) = res {
            if self.config.cleanup.delete_node_on_error {
                ui.say(
                    "chef-client",
                    &format!("Cleaning up node {node_id} on error..."),
                );
                let _ = comm
                    .execute(&Command::new(format!("knife node delete {node_id} -y")))
                    .await;
            }
            if self.config.cleanup.delete_client_on_error {
                ui.say(
                    "chef-client",
                    &format!("Cleaning up client {node_id} on error..."),
                );
                let _ = comm
                    .execute(&Command::new(format!("knife client delete {node_id} -y")))
                    .await;
            }
            return Err(StampError::Provisioner(e.to_string()));
        }

        let exec_result = res.map_err(|e| StampError::Provisioner(e.to_string()))?;

        if self.config.cleanup.clean_up {
            ui.say(
                "chef-client",
                "Cleaning up node registration and staging directory...",
            );
            let _ = comm
                .execute(&Command::new(format!("knife node delete {node_id} -y")))
                .await;
            let _ = comm
                .execute(&Command::new(format!("knife client delete {node_id} -y")))
                .await;
            let rm_cmd = format!("rm -rf {remote_dir}");
            let _ = comm.execute(&Command::new(rm_cmd)).await;
        }

        if exec_result.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "chef-client failed with exit code: {}",
                exec_result.exit_code
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
    async fn test_chef_client_provision_success() -> Result<(), StampError> {
        let tmp_secret =
            std::env::temp_dir().join(format!("test_client_secret_{}", uuid::Uuid::new_v4()));
        fs::write(&tmp_secret, "mysecret123").map_err(StampError::Io)?;

        let config = ChefClientConfig {
            server_url: "https://chef.example.com".to_string(),
            validation_client_name: "chef-validator".to_string(),
            validation_key_path: Some(FilePath::new(PathBuf::from("validator.pem"))),
            node_name: Some("test-node-1".to_string()),
            encrypted_data_bag_secret_path: Some(FilePath::new(tmp_secret.clone())),
            cleanup: ChefCleanupOptions {
                delete_node_on_error: true,
                delete_client_on_error: true,
                clean_up: true,
            },
            ..Default::default()
        };
        let prov = ChefClientProvisioner::new(config);
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
        let _ = fs::remove_file(tmp_secret);
        Ok(())
    }

    #[tokio::test]
    async fn test_chef_client_provision_with_json_and_env() -> Result<(), StampError> {
        let tmp_json =
            std::env::temp_dir().join(format!("test_client_attrs_{}.json", uuid::Uuid::new_v4()));
        fs::write(&tmp_json, r#"{"client_ver": "1.0"}"#).map_err(StampError::Io)?;

        let config = ChefClientConfig {
            server_url: "https://chef.example.com".to_string(),
            validation_client_name: "chef-validator".to_string(),
            validation_key_path: Some(FilePath::new(PathBuf::from("validator.pem"))),
            node_name: Some("test-node".to_string()),
            chef_environment: Some("staging".to_string()),
            run_list: vec!["role[base]".to_string()],
            json_path: Some(FilePath::new(tmp_json.clone())),
            json: Some(serde_json::json!({"enabled": true})),
            ..Default::default()
        };
        let prov = ChefClientProvisioner::new(config);
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
        Ok(())
    }

    #[tokio::test]
    async fn test_chef_client_provision_failure_missing_url() {
        let config = ChefClientConfig {
            server_url: String::new(),
            validation_client_name: "chef-validator".to_string(),
            ..Default::default()
        };
        let prov = ChefClientProvisioner::new(config);
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
    async fn test_chef_client_provision_failure_missing_validator() {
        let config = ChefClientConfig {
            server_url: "https://chef.example.com".to_string(),
            validation_client_name: String::new(),
            ..Default::default()
        };
        let prov = ChefClientProvisioner::new(config);
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
    async fn test_chef_client_custom_install_and_cleanup() -> Result<(), StampError> {
        let config = ChefClientConfig {
            server_url: "https://chef.example.com".to_string(),
            validation_client_name: "chef-validator".to_string(),
            install_command: Some("custom-chef-install".to_string()),
            skip_install: false,
            cleanup: ChefCleanupOptions {
                delete_node_on_error: false,
                delete_client_on_error: false,
                clean_up: true,
            },
            ..Default::default()
        };
        let prov = ChefClientProvisioner::new(config);
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
        let config1 = ChefClientConfig {
            server_url: "https://chef.example.com".to_string(),
            validation_client_name: "chef-validator".to_string(),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = ChefClientProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
