//! Implementation of the `ansible-local` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use std::path::{Path, PathBuf};

/// Configuration for the `ansible-local` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsibleLocalConfig {
    /// Path to the playbook file on the host.
    pub playbook_file: String,
    /// Optional directory on the host to upload containing playbooks and roles.
    pub playbook_dir: Option<FilePath>,
    /// Optional paths to role directories on the host to upload.
    pub role_paths: Vec<FilePath>,
    /// Optional path to a `group_vars` directory on the host.
    pub group_vars: Option<FilePath>,
    /// Optional path to a `host_vars` directory on the host.
    pub host_vars: Option<FilePath>,
    /// Staging directory on the target guest. Defaults to `/tmp/packer-provisioner-ansible-local`.
    pub staging_directory: Option<String>,
    /// Whether to clean up the staging directory after provision completion.
    pub clean_staging_directory: bool,
    /// Optional custom inventory file path on the host to upload.
    pub inventory_file: Option<FilePath>,
    /// Extra arguments to pass to the `ansible-playbook` command.
    pub extra_arguments: Vec<String>,
    /// The command used to run ansible-playbook. Defaults to `ansible-playbook`.
    pub command: Option<String>,
    /// Optional Ansible Galaxy requirements file path on the host.
    pub galaxy_file: Option<FilePath>,
    /// Command to execute for installing Ansible Galaxy dependencies.
    pub galaxy_command: Option<String>,
}

impl Default for AnsibleLocalConfig {
    fn default() -> Self {
        Self {
            playbook_file: String::new(),
            playbook_dir: None,
            role_paths: Vec::new(),
            group_vars: None,
            host_vars: None,
            staging_directory: None,
            clean_staging_directory: true,
            inventory_file: None,
            extra_arguments: Vec::new(),
            command: None,
            galaxy_file: None,
            galaxy_command: None,
        }
    }
}

/// The `ansible-local` provisioner.
#[derive(Debug, Clone)]
pub struct AnsibleLocalProvisioner {
    /// The provisioner configuration.
    pub config: AnsibleLocalConfig,
}

impl AnsibleLocalProvisioner {
    /// Create a new `AnsibleLocalProvisioner`.
    #[must_use]
    pub const fn new(config: AnsibleLocalConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Provisioner for AnsibleLocalProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        _ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.playbook_file.is_empty() {
            return Err(StampError::Provisioner(
                "playbook_file is required".to_string(),
            ));
        }

        let staging_dir = self
            .config
            .staging_directory
            .as_deref()
            .unwrap_or("/tmp/packer-provisioner-ansible-local");

        let remote_playbook = format!(
            "{staging_dir}/{}",
            Path::new(&self.config.playbook_file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );

        // 1. Create remote staging directory
        let mkdir_cmd = format!("mkdir -p {staging_dir}");
        let res = comm.execute(&Command::new(mkdir_cmd)).await?;
        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "Failed to create remote staging directory. exit code: {}",
                res.exit_code
            )));
        }

        // 2. Upload playbook directory or playbook file
        if let Some(dir) = &self.config.playbook_dir {
            let remote_fp = FilePath::new(PathBuf::from(staging_dir));
            comm.upload(dir, &remote_fp).await?;
        } else {
            let local_fp = FilePath::new(PathBuf::from(&self.config.playbook_file));
            let remote_fp = FilePath::new(PathBuf::from(&remote_playbook));
            comm.upload(&local_fp, &remote_fp).await?;
        }

        // 3. Upload role paths if specified
        if !self.config.role_paths.is_empty() {
            let mkdir_roles = format!("mkdir -p {staging_dir}/roles");
            let _ = comm.execute(&Command::new(mkdir_roles)).await?;
            for role_path in &self.config.role_paths {
                let role_name = role_path
                    .0
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                let remote_role = format!("{staging_dir}/roles/{role_name}");
                let remote_fp = FilePath::new(PathBuf::from(remote_role));
                comm.upload(role_path, &remote_fp).await?;
            }
        }

        // 4. Upload group_vars / host_vars if specified
        if let Some(gv) = &self.config.group_vars {
            let remote_gv = format!("{staging_dir}/group_vars");
            let _ = comm
                .execute(&Command::new(format!("mkdir -p {remote_gv}")))
                .await?;
            comm.upload(gv, &FilePath::new(PathBuf::from(remote_gv)))
                .await?;
        }
        if let Some(hv) = &self.config.host_vars {
            let remote_hv = format!("{staging_dir}/host_vars");
            let _ = comm
                .execute(&Command::new(format!("mkdir -p {remote_hv}")))
                .await?;
            comm.upload(hv, &FilePath::new(PathBuf::from(remote_hv)))
                .await?;
        }

        // 5. Install Galaxy dependencies if galaxy_file is provided
        if let Some(galaxy_file) = &self.config.galaxy_file {
            let remote_galaxy_path = format!("{staging_dir}/requirements.yml");
            comm.upload(
                galaxy_file,
                &FilePath::new(PathBuf::from(&remote_galaxy_path)),
            )
            .await?;

            let default_galaxy_cmd =
                format!("ansible-galaxy install -r {remote_galaxy_path} -p {staging_dir}/roles");
            let galaxy_cmd = self
                .config
                .galaxy_command
                .as_deref()
                .unwrap_or(&default_galaxy_cmd);
            let galaxy_res = comm.execute(&Command::new(galaxy_cmd.to_string())).await?;
            if galaxy_res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "ansible-galaxy failed with exit code: {}",
                    galaxy_res.exit_code
                )));
            }
        }

        // 6. Handle inventory file
        let remote_inventory = format!("{staging_dir}/hosts");
        if let Some(inv_file) = &self.config.inventory_file {
            comm.upload(inv_file, &FilePath::new(PathBuf::from(&remote_inventory)))
                .await?;
        } else {
            let inv_content = "localhost ansible_connection=local\\n";
            let create_inv_cmd = format!("echo -e '{inv_content}' > {remote_inventory}");
            let _ = comm.execute(&Command::new(create_inv_cmd)).await?;
        }

        // 7. Execute ansible-playbook
        let ansible_bin = self.config.command.as_deref().unwrap_or("ansible-playbook");
        let mut cmd_str =
            format!("cd {staging_dir} && {ansible_bin} -i {remote_inventory} {remote_playbook}");
        if !self.config.extra_arguments.is_empty() {
            cmd_str.push(' ');
            cmd_str.push_str(&self.config.extra_arguments.join(" "));
        }

        let run_res = comm.execute(&Command::new(cmd_str)).await?;

        // 8. Clean staging directory if enabled
        if self.config.clean_staging_directory {
            let cleanup_cmd = format!("rm -rf {staging_dir}");
            let _ = comm.execute(&Command::new(cleanup_cmd)).await;
        }

        if run_res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "ansible-playbook failed with exit code: {}",
                run_res.exit_code
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
    async fn test_ansible_local_provision_success() -> Result<(), StampError> {
        let config = AnsibleLocalConfig {
            playbook_file: "playbook.yml".to_string(),
            ..Default::default()
        };
        let prov = AnsibleLocalProvisioner::new(config);
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
    async fn test_ansible_local_provision_success_with_dir_and_args() -> Result<(), StampError> {
        let config = AnsibleLocalConfig {
            playbook_file: "playbook.yml".to_string(),
            playbook_dir: Some(FilePath::new(PathBuf::from("playbook_dir"))),
            role_paths: vec![FilePath::new(PathBuf::from("roles/common"))],
            group_vars: Some(FilePath::new(PathBuf::from("group_vars"))),
            host_vars: Some(FilePath::new(PathBuf::from("host_vars"))),
            inventory_file: Some(FilePath::new(PathBuf::from("custom_hosts"))),
            galaxy_file: Some(FilePath::new(PathBuf::from("requirements.yml"))),
            galaxy_command: Some("ansible-galaxy install -r requirements.yml".to_string()),
            staging_directory: Some("/opt/ansible".to_string()),
            clean_staging_directory: true,
            extra_arguments: vec!["--tags".to_string(), "setup".to_string()],
            command: Some("ansible-playbook".to_string()),
        };
        let prov = AnsibleLocalProvisioner::new(config);
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
    async fn test_ansible_local_provision_failure_empty_playbook() -> Result<(), StampError> {
        let config = AnsibleLocalConfig {
            playbook_file: String::new(),
            ..Default::default()
        };
        let prov = AnsibleLocalProvisioner::new(config);
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
        Ok(())
    }

    #[tokio::test]
    async fn test_ansible_local_provision_failure_mkdir() -> Result<(), StampError> {
        let config = AnsibleLocalConfig {
            playbook_file: "fail_provision_mkdir.yml".to_string(),
            ..Default::default()
        };
        let prov = AnsibleLocalProvisioner::new(config);
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
        Ok(())
    }

    #[tokio::test]
    async fn test_ansible_local_provision_failure_playbook() -> Result<(), StampError> {
        let config = AnsibleLocalConfig {
            playbook_file: "fail_provision.yml".to_string(),
            ..Default::default()
        };
        let prov = AnsibleLocalProvisioner::new(config);
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
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config1 = AnsibleLocalConfig {
            playbook_file: "playbook.yml".to_string(),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = AnsibleLocalProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
