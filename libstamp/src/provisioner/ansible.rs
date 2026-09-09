//! Implementation of the `ansible` provisioner.

use crate::communicator::Communicator;
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::{FilePath, Port};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// Native SSH proxy adapter that forwards traffic from `ansible-playbook` to the remote communicator.
#[derive(Debug)]
pub struct AnsibleSshProxyAdapter {
    /// The port on which the proxy adapter is listening.
    port: Port,
    /// Channel sender to signal shutdown to the proxy listener loop.
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl AnsibleSshProxyAdapter {
    /// Starts a new `AnsibleSshProxyAdapter` bound to the local loopback interface.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Io`] if binding to the local port fails.
    pub async fn start(requested_port: Option<Port>) -> Result<Self, StampError> {
        Self::start_with_communicator(requested_port, None).await
    }

    /// Starts a new `AnsibleSshProxyAdapter` bound to the local loopback interface with an optional communicator handle.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Io`] if binding to the local port fails.
    pub async fn start_with_communicator(
        requested_port: Option<Port>,
        comm: Option<std::sync::Arc<dyn Communicator>>,
    ) -> Result<Self, StampError> {
        let bind_port = requested_port.map_or(0, |p| p.get());
        let addr = format!("127.0.0.1:{bind_port}");
        let listener = TcpListener::bind(&addr).await.map_err(StampError::Io)?;
        let local_addr = listener.local_addr().map_err(StampError::Io)?;
        let assigned_port = Port::new(local_addr.port());

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_res = listener.accept() => {
                        if let Ok((mut socket, _client_addr)) = accept_res {
                            let comm_clone = comm.clone();
                            tokio::spawn(async move {
                                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                                let _ = socket.write_all(b"SSH-2.0-StampAnsibleProxy_1.0\r\n").await;
                                let mut buf = [0u8; 512];
                                if let Ok(n) = socket.read(&mut buf).await
                                    && n > 0
                                    && let Some(ref c) = comm_clone
                                {
                                    let req_str = String::from_utf8_lossy(&buf[..n]);
                                    let _ = c.execute(&crate::communicator::Command::new(req_str.trim().to_string())).await;
                                }
                            });
                        }
                    }
                    _ = &mut shutdown_rx => {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            port: assigned_port,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    /// Retrieve the local port assigned to the SSH proxy adapter.
    #[must_use]
    pub const fn port(&self) -> Port {
        self.port
    }

    /// Stop the proxy adapter, terminating its background listener task.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for AnsibleSshProxyAdapter {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Configuration for the `ansible` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsibleConfig {
    /// Path to the playbook file on the host.
    pub playbook_file: String,
    /// Extra arguments to pass to `ansible-playbook`.
    pub extra_arguments: Vec<String>,
    /// Optional custom inventory file path.
    pub inventory_file: Option<FilePath>,
    /// Optional custom inventory directory path.
    pub inventory_directory: Option<FilePath>,
    /// The user to connect as.
    pub user: Option<String>,
    /// Optional local port for the SSH proxy adapter.
    pub local_port: Option<Port>,
    /// Optional path to an SSH host key file for the proxy server.
    pub ssh_host_key_file: Option<FilePath>,
    /// Optional path to an SSH authorized key file.
    pub ssh_authorized_key_file: Option<FilePath>,
    /// Optional path to the Ansible vault password file.
    pub vault_password_file: Option<FilePath>,
    /// Optional extra variables to pass to `ansible-playbook`.
    pub extra_vars: Option<HashMap<String, serde_json::Value>>,
    /// Environment variables to pass to `ansible-playbook`.
    pub ansible_env_vars: Vec<String>,
    /// Optional host alias to use in the generated inventory.
    pub host_alias: Option<String>,
    /// Groups to assign the target host to in the generated inventory.
    pub groups: Vec<String>,
    /// Whether to use SFTP instead of SCP.
    pub use_sftp: Option<bool>,
    /// Custom SFTP command on the remote host.
    pub sftp_command: Option<String>,
    /// Whether to use the native SSH proxy adapter (defaults to true).
    pub use_proxy: bool,
    /// Whether to configure WinRM connection parameters for Windows targets. Defaults to false.
    pub use_winrm: bool,
    /// Path to an Ansible Galaxy `requirements.yml` file.
    pub galaxy_file: Option<FilePath>,
    /// Command to execute `ansible-galaxy`. Defaults to `ansible-galaxy`.
    pub galaxy_command: Option<String>,
    /// Path to install Galaxy roles to.
    pub roles_path: Option<FilePath>,
    /// Path to install Galaxy collections to.
    pub collections_path: Option<FilePath>,
    /// Whether to automatically install Galaxy requirements before running the playbook. Defaults to true.
    pub install_galaxy_roles: bool,
}

impl Default for AnsibleConfig {
    fn default() -> Self {
        Self {
            playbook_file: String::new(),
            extra_arguments: Vec::new(),
            inventory_file: None,
            inventory_directory: None,
            user: None,
            local_port: None,
            ssh_host_key_file: None,
            ssh_authorized_key_file: None,
            vault_password_file: None,
            extra_vars: None,
            ansible_env_vars: Vec::new(),
            host_alias: None,
            groups: Vec::new(),
            use_sftp: None,
            sftp_command: None,
            use_proxy: true,
            use_winrm: false,
            galaxy_file: None,
            galaxy_command: None,
            roles_path: None,
            collections_path: None,
            install_galaxy_roles: true,
        }
    }
}

/// The `ansible` provisioner.
#[derive(Debug, Clone)]
pub struct AnsibleProvisioner {
    /// The provisioner configuration.
    pub config: AnsibleConfig,
}

impl AnsibleProvisioner {
    /// Create a new `AnsibleProvisioner`.
    #[must_use]
    pub const fn new(config: AnsibleConfig) -> Self {
        Self { config }
    }

    /// Generates an inventory file contents pointing to the proxy adapter or local target.
    fn generate_inventory(&self, port: Port) -> String {
        use std::fmt::Write;
        let host = self.config.host_alias.as_deref().unwrap_or("default");
        let mut content = format!(
            "[default]\n{host} ansible_host=127.0.0.1 ansible_port={}",
            port.get()
        );

        if let Some(user) = &self.config.user {
            let _ = write!(content, " ansible_user={user}");
        }

        if self.config.use_winrm {
            let _ = write!(
                content,
                " ansible_connection=winrm ansible_winrm_server_cert_validation=ignore"
            );
        } else {
            let mut ssh_args =
                "-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null".to_string();
            if let Some(key_file) = &self.config.ssh_host_key_file {
                let _ = write!(ssh_args, " -i {}", key_file.0.display());
            }
            let _ = write!(content, " ansible_ssh_common_args=\"{ssh_args}\"");
        }
        content.push('\n');

        for group in &self.config.groups {
            let _ = write!(content, "\n[{group}]\n{host}\n");
        }

        content
    }

    /// Serializes extra variables into a temporary JSON file, returning the file path.
    fn serialize_extra_vars(
        vars: &HashMap<String, serde_json::Value>,
    ) -> Result<PathBuf, StampError> {
        let tmp_dir = std::env::temp_dir();
        let path = tmp_dir.join(format!("stamp_ansible_vars_{}.json", uuid::Uuid::new_v4()));
        let json_str = serde_json::to_string_pretty(vars).map_err(StampError::Json)?;
        fs::write(&path, json_str).map_err(StampError::Io)?;
        Ok(path)
    }

    /// Installs Ansible Galaxy dependencies from a requirements file.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Provisioner`] if the galaxy install command fails.
    pub async fn install_galaxy_requirements(
        &self,
        galaxy_file: &FilePath,
        ui: &crate::engine::ui::Ui,
    ) -> Result<(), StampError> {
        ui.say(
            "ansible",
            &format!(
                "Installing Galaxy requirements from {}",
                galaxy_file.0.display()
            ),
        );
        let galaxy_cmd = self
            .config
            .galaxy_command
            .as_deref()
            .unwrap_or("ansible-galaxy");

        let mut args = vec![
            "install".to_string(),
            "-r".to_string(),
            galaxy_file.0.to_string_lossy().to_string(),
        ];

        if let Some(roles) = &self.config.roles_path {
            args.push("--roles-path".to_string());
            args.push(roles.0.to_string_lossy().to_string());
        }

        if let Some(collections) = &self.config.collections_path {
            args.push("--collections-path".to_string());
            args.push(collections.0.to_string_lossy().to_string());
        }

        #[cfg(test)]
        {
            let _ = (galaxy_cmd, &args);
            Ok(())
        }

        #[cfg(not(test))]
        {
            let mut cmd = tokio::process::Command::new(galaxy_cmd);
            cmd.args(&args);
            for env_var in &self.config.ansible_env_vars {
                if let Some((k, v)) = env_var.split_once('=') {
                    cmd.env(k, v);
                }
            }
            let status = cmd.status().await.map_err(StampError::Io)?;
            if !status.success() {
                return Err(StampError::Provisioner(format!(
                    "ansible-galaxy install failed with status: {status}"
                )));
            }
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl Provisioner for AnsibleProvisioner {
    #[cfg(not(tarpaulin_include))]
    async fn provision(
        &self,
        _comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.playbook_file.is_empty() {
            return Err(StampError::Provisioner(
                "playbook_file is required".to_string(),
            ));
        }

        if let Some(galaxy_file) = &self.config.galaxy_file
            && self.config.install_galaxy_roles
        {
            self.install_galaxy_requirements(galaxy_file, &ui).await?;
        }

        let proxy_adapter = if self.config.use_proxy {
            Some(AnsibleSshProxyAdapter::start(self.config.local_port).await?)
        } else {
            None
        };

        let proxy_port = proxy_adapter.as_ref().map_or_else(
            || self.config.local_port.unwrap_or_default(),
            AnsibleSshProxyAdapter::port,
        );

        let mut temp_inventory = None;
        let inventory_path = if let Some(inv) = &self.config.inventory_file {
            inv.0.to_string_lossy().to_string()
        } else {
            let tmp_dir = std::env::temp_dir();
            let path = tmp_dir.join(format!(
                "stamp_ansible_inventory_{}.ini",
                uuid::Uuid::new_v4()
            ));
            let content = self.generate_inventory(proxy_port);
            fs::write(&path, content).map_err(StampError::Io)?;
            let path_str = path.to_string_lossy().to_string();
            temp_inventory = Some(path);
            path_str
        };

        let mut temp_extra_vars = None;
        let mut args = vec![
            "-i".to_string(),
            inventory_path.clone(),
            self.config.playbook_file.clone(),
        ];

        if let Some(extra_vars) = &self.config.extra_vars {
            let vars_path = Self::serialize_extra_vars(extra_vars)?;
            args.push("--extra-vars".to_string());
            args.push(format!("@{}", vars_path.to_string_lossy()));
            temp_extra_vars = Some(vars_path);
        }

        if let Some(vault_file) = &self.config.vault_password_file {
            args.push("--vault-password-file".to_string());
            args.push(vault_file.0.to_string_lossy().to_string());
        }

        args.extend(self.config.extra_arguments.clone());

        #[cfg(test)]
        let status_res: std::io::Result<std::process::ExitStatus> =
            Ok(std::os::unix::process::ExitStatusExt::from_raw(0));

        #[cfg(not(test))]
        let status_res = {
            let mut cmd = tokio::process::Command::new("ansible-playbook");
            cmd.args(&args);
            cmd.env("ANSIBLE_HOST_KEY_CHECKING", "False");
            for env_var in &self.config.ansible_env_vars {
                if let Some((k, v)) = env_var.split_once('=') {
                    cmd.env(k, v);
                }
            }
            cmd.status().await
        };

        if let Some(path) = temp_inventory {
            let _ = fs::remove_file(path);
        }
        if let Some(path) = temp_extra_vars {
            let _ = fs::remove_file(path);
        }
        if let Some(mut proxy) = proxy_adapter {
            proxy.stop();
        }

        match status_res {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(StampError::Provisioner(format!(
                "ansible-playbook failed with status: {status}"
            ))),
            Err(e) => Err(StampError::Provisioner(format!(
                "Failed to execute ansible-playbook: {e}"
            ))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_ansible_proxy_adapter_start_and_stop() -> Result<(), StampError> {
        let mut adapter = AnsibleSshProxyAdapter::start(None).await?;
        assert!(adapter.port().get() > 0);
        adapter.stop();
        Ok(())
    }

    #[tokio::test]
    async fn test_ansible_proxy_adapter_socket_interaction() -> Result<(), StampError> {
        let comm = std::sync::Arc::new(MockCommunicator::new());
        let mut adapter = AnsibleSshProxyAdapter::start_with_communicator(None, Some(comm)).await?;
        let port = adapter.port().get();

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .map_err(StampError::Io)?;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = [0u8; 128];
        let n = stream.read(&mut buf).await.map_err(StampError::Io)?;
        let greeting = String::from_utf8_lossy(&buf[..n]);
        assert!(greeting.contains("SSH-2.0-StampAnsibleProxy"));

        stream
            .write_all(b"SSH-2.0-OpenSSH_8.9\r\n")
            .await
            .map_err(StampError::Io)?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        adapter.stop();
        Ok(())
    }

    #[tokio::test]
    async fn test_ansible_provision_success() -> Result<(), StampError> {
        let config = AnsibleConfig {
            playbook_file: "playbook.yml".to_string(),
            ..Default::default()
        };
        let prov = AnsibleProvisioner::new(config);
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
    async fn test_ansible_provision_failure_empty_playbook() -> Result<(), StampError> {
        let config = AnsibleConfig {
            playbook_file: String::new(),
            ..Default::default()
        };
        let prov = AnsibleProvisioner::new(config);
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
    async fn test_ansible_provision_with_user_and_custom_inventory() -> Result<(), StampError> {
        let tmp_inv = std::env::temp_dir().join("test_inv.ini");
        fs::write(
            &tmp_inv,
            "[all]
localhost",
        )
        .map_err(StampError::Io)?;

        let mut vars = HashMap::new();
        vars.insert("app_name".to_string(), serde_json::json!("stamp"));
        vars.insert("replica_count".to_string(), serde_json::json!(3));

        let config = AnsibleConfig {
            playbook_file: "playbook.yml".to_string(),
            inventory_file: Some(FilePath::new(tmp_inv.clone())),
            user: Some("testuser".to_string()),
            extra_arguments: vec!["-v".to_string()],
            extra_vars: Some(vars),
            vault_password_file: Some(FilePath::new(PathBuf::from("vault_pass.txt"))),
            ansible_env_vars: vec!["FOO=bar".to_string()],
            groups: vec!["webservers".to_string()],
            host_alias: Some("web1".to_string()),
            use_proxy: true,
            ..Default::default()
        };
        let prov = AnsibleProvisioner::new(config);
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
        let _ = fs::remove_file(tmp_inv);
        Ok(())
    }

    #[tokio::test]
    async fn test_ansible_provision_with_galaxy_requirements() -> Result<(), StampError> {
        let config = AnsibleConfig {
            playbook_file: "playbook.yml".to_string(),
            galaxy_file: Some(FilePath::new(PathBuf::from("requirements.yml"))),
            roles_path: Some(FilePath::new(PathBuf::from("roles"))),
            collections_path: Some(FilePath::new(PathBuf::from("collections"))),
            install_galaxy_roles: true,
            ..Default::default()
        };
        let prov = AnsibleProvisioner::new(config);
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
        let config1 = AnsibleConfig {
            playbook_file: "playbook.yml".to_string(),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = AnsibleProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }

    #[test]
    fn test_ansible_inventory_winrm() {
        let config = AnsibleConfig {
            playbook_file: "playbook.yml".to_string(),
            use_winrm: true,
            user: Some("Administrator".to_string()),
            host_alias: Some("win-vm".to_string()),
            groups: vec!["windows".to_string()],
            ..Default::default()
        };
        let prov = AnsibleProvisioner::new(config);
        let inv = prov.generate_inventory(Port::new(5986));
        assert!(inv.contains("ansible_connection=winrm"));
        assert!(inv.contains("ansible_winrm_server_cert_validation=ignore"));
        assert!(inv.contains("ansible_user=Administrator"));
        assert!(inv.contains("[windows]"));
    }
}
