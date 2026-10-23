//! Implementation of the `ansible-local` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use std::path::{Path, PathBuf};

/// Configuration for the `ansible-local` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsibleLocalConfig {
    /// Playbook files to upload and execute.
    pub playbook_files: Vec<FilePath>,
    /// Optional playbook directories to upload onto the guest.
    pub playbook_paths: Vec<FilePath>,
    /// Optional role paths to upload onto the guest.
    pub role_paths: Vec<FilePath>,
    /// Optional group\_vars directory to upload onto the guest.
    pub group_vars: Option<FilePath>,
    /// Optional host\_vars directory to upload onto the guest.
    pub host_vars: Option<FilePath>,
    /// Optional inventory file to upload.
    pub inventory_file: Option<FilePath>,
    /// Inventory groups for the local host.
    pub inventory_groups: Vec<String>,
    /// Optional Ansible Galaxy requirements file (roles/collections) to install via `ansible-galaxy`.
    pub galaxy_file: Option<FilePath>,
    /// Command to use for galaxy installation. Defaults to `ansible-galaxy`.
    pub galaxy_command: Option<String>,
    /// Additional arguments to pass to `ansible-playbook`.
    pub extra_arguments: Vec<String>,
    /// Command to run `ansible-playbook`. Defaults to `ansible-playbook`.
    pub command: Option<String>,
    /// Staging directory on the guest. Defaults to `/tmp/packer-provisioner-ansible-local`.
    pub staging_directory: Option<String>,
    /// Whether to clean up the staging directory upon completion. Defaults to true.
    pub clean_staging_directory: bool,
}

impl Default for AnsibleLocalConfig {
    fn default() -> Self {
        Self {
            playbook_files: Vec::new(),
            playbook_paths: Vec::new(),
            role_paths: Vec::new(),
            group_vars: None,
            host_vars: None,
            inventory_file: None,
            inventory_groups: Vec::new(),
            galaxy_file: None,
            galaxy_command: None,
            extra_arguments: Vec::new(),
            command: None,
            staging_directory: None,
            clean_staging_directory: true,
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
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        if self.config.playbook_files.is_empty() {
            return Err(StampError::Parse("playbook_files is required".to_string()));
        }

        let staging_dir = self
            .config
            .staging_directory
            .as_deref()
            .unwrap_or("/tmp/packer-provisioner-ansible-local");

        // 1. Create staging directory
        let mkdir_cmd = format!(
            "mkdir -p {staging_dir}/playbooks {staging_dir}/roles {staging_dir}/group_vars {staging_dir}/host_vars"
        );
        let res = comm.execute(&Command::new(mkdir_cmd)).await?;
        if res.exit_code != 0 {
            return Err(StampError::Parse(format!(
                "Failed to create staging directory. exit code: {}",
                res.exit_code
            )));
        }

        // 2. Upload playbook directories, role paths, group_vars, and host_vars
        for (idx, pb_dir) in self.config.playbook_paths.iter().enumerate() {
            let remote_pb_dir = format!("{staging_dir}/playbooks/dir_{idx}");
            comm.upload(pb_dir, &FilePath::new(PathBuf::from(remote_pb_dir)))
                .await?;
        }

        for role_dir in &self.config.role_paths {
            let dir_name = role_dir
                .0
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("role");
            let remote_role = format!("{staging_dir}/roles/{dir_name}");
            comm.upload(role_dir, &FilePath::new(PathBuf::from(remote_role)))
                .await?;
        }

        if let Some(ref gv) = self.config.group_vars {
            let remote_gv = format!("{staging_dir}/group_vars");
            comm.upload(gv, &FilePath::new(PathBuf::from(remote_gv)))
                .await?;
        }

        if let Some(ref hv) = self.config.host_vars {
            let remote_hv = format!("{staging_dir}/host_vars");
            comm.upload(hv, &FilePath::new(PathBuf::from(remote_hv)))
                .await?;
        }

        // 3. Upload galaxy requirements and install if present
        if let Some(ref galaxy) = self.config.galaxy_file {
            let remote_galaxy = format!("{staging_dir}/requirements.yml");
            comm.upload(galaxy, &FilePath::new(PathBuf::from(&remote_galaxy)))
                .await?;

            let galaxy_bin = self
                .config
                .galaxy_command
                .as_deref()
                .unwrap_or("ansible-galaxy");
            let galaxy_cmd =
                format!("{galaxy_bin} install -r {remote_galaxy} -p {staging_dir}/roles");
            ui.say(
                "ansible-local",
                &format!("Installing Galaxy roles: {galaxy_cmd}"),
            );
            let g_res = comm.execute(&Command::new(galaxy_cmd)).await?;
            if g_res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "ansible-galaxy install failed with exit code: {}",
                    g_res.exit_code
                )));
            }
        }

        // 4. Upload or generate inventory file
        let remote_inventory_path = format!("{staging_dir}/hosts");
        if let Some(inv) = &self.config.inventory_file {
            let remote_fp = FilePath::new(PathBuf::from(&remote_inventory_path));
            comm.upload(inv, &remote_fp).await?;
        } else {
            let mut inv_lines = vec!["127.0.0.1 ansible_connection=local".to_string()];
            for group in &self.config.inventory_groups {
                inv_lines.push(format!("[{group}]"));
                inv_lines.push("127.0.0.1 ansible_connection=local".to_string());
            }
            let inv_body = inv_lines.join(r"\n");
            let default_inv_cmd = format!(r#"echo -e "{inv_body}" > {remote_inventory_path}"#);
            let res = comm.execute(&Command::new(default_inv_cmd)).await?;
            if res.exit_code != 0 {
                return Err(StampError::Parse(format!(
                    "Failed to create default inventory file. exit code: {}",
                    res.exit_code
                )));
            }
        }

        // 5. Upload and execute playbooks
        let ansible_cmd = self
            .config
            .command
            .as_deref()
            .unwrap_or("ansible-playbook")
            .to_string();

        for (i, playbook) in self.config.playbook_files.iter().enumerate() {
            let filename = Path::new(&playbook.0)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("playbook.yml");
            let remote_playbook_path = format!("{staging_dir}/{i}_{filename}");

            let remote_fp = FilePath::new(PathBuf::from(&remote_playbook_path));
            comm.upload(playbook, &remote_fp).await?;

            let mut cmd_str = format!(
                "ANSIBLE_ROLES_PATH={staging_dir}/roles {ansible_cmd} {remote_playbook_path} -c local -i {remote_inventory_path}"
            );
            if !self.config.extra_arguments.is_empty() {
                cmd_str.push(' ');
                cmd_str.push_str(&self.config.extra_arguments.join(" "));
            }

            ui.say("ansible-local", &format!("Executing: {cmd_str}"));
            let res = comm.execute(&Command::new(cmd_str)).await?;
            if res.exit_code != 0 {
                if self.config.clean_staging_directory {
                    let _ = comm
                        .execute(&Command::new(format!("rm -rf {staging_dir}")))
                        .await;
                }
                return Err(StampError::Parse(format!(
                    "ansible-playbook failed with exit code: {}",
                    res.exit_code
                )));
            }
        }

        // 6. Clean staging directory if requested
        if self.config.clean_staging_directory {
            let rm_cmd = format!("rm -rf {staging_dir}");
            let _ = comm.execute(&Command::new(rm_cmd)).await;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;

    #[tokio::test]
    async fn test_ansible_local_provision_success() -> Result<(), crate::error::StampError> {
        let config = AnsibleLocalConfig {
            playbook_files: vec![FilePath::new(PathBuf::from("site.yml"))],
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
    async fn test_ansible_local_provision_with_inventory_and_args()
    -> Result<(), crate::error::StampError> {
        let config = AnsibleLocalConfig {
            playbook_files: vec![FilePath::new(PathBuf::from("site.yml"))],
            playbook_paths: vec![FilePath::new(PathBuf::from("playbooks"))],
            role_paths: vec![FilePath::new(PathBuf::from("roles/common"))],
            group_vars: Some(FilePath::new(PathBuf::from("group_vars"))),
            host_vars: Some(FilePath::new(PathBuf::from("host_vars"))),
            galaxy_file: Some(FilePath::new(PathBuf::from("requirements.yml"))),
            inventory_file: Some(FilePath::new(PathBuf::from("hosts"))),
            extra_arguments: vec![r#"--extra-vars "foo=bar""#.to_string()],
            command: Some("/usr/local/bin/ansible-playbook".to_string()),
            staging_directory: Some("/var/tmp/ansible".to_string()),
            clean_staging_directory: true,
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
    async fn test_ansible_local_inventory_groups() -> Result<(), crate::error::StampError> {
        let config = AnsibleLocalConfig {
            playbook_files: vec![FilePath::new(PathBuf::from("site.yml"))],
            inventory_groups: vec!["webservers".to_string(), "dbservers".to_string()],
            clean_staging_directory: false,
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
    async fn test_ansible_local_provision_failure_empty_playbooks()
    -> Result<(), crate::error::StampError> {
        let config = AnsibleLocalConfig {
            playbook_files: vec![],
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
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_ansible_local_provision_failure_execute() -> Result<(), crate::error::StampError>
    {
        let config = AnsibleLocalConfig {
            playbook_files: vec![FilePath::new(PathBuf::from("site.yml"))],
            extra_arguments: vec!["fail_provision".to_string()],
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
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config1 = AnsibleLocalConfig {
            playbook_files: vec![FilePath::new(PathBuf::from("site.yml"))],
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert!(format!("{config1:?}").contains("AnsibleLocalConfig"));
        let st1 = AnsibleLocalProvisioner::new(config1);
        let st2 = st1.clone();
        assert!(format!("{st1:?}").contains("AnsibleLocalProvisioner"));
        assert_eq!(st1.config, st2.config);
    }
}
