//! Implementation of the `puppet-server` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use std::collections::HashMap;
use std::path::PathBuf;

/// Configuration for the `puppet-server` provisioner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PuppetServerConfig {
    /// Optional custom fact overrides for Facter.
    pub facter: HashMap<String, String>,
    /// The Puppet server hostname or URL to connect to.
    pub puppet_server: Option<String>,
    /// Optional Puppet environment.
    pub puppet_node_interface: Option<String>,
    /// Optional client certificate path to upload to the agent.
    pub client_cert_path: Option<FilePath>,
    /// Optional client private key path to upload to the agent.
    pub client_private_key_path: Option<FilePath>,
    /// Node certificate name (certname) for signing and registration.
    pub certname: Option<String>,
    /// Whether to wait for certificate autosigning from Puppet server.
    pub autosign: bool,
    /// Additional arguments to pass to `puppet agent`.
    pub extra_arguments: Vec<String>,
    /// Optional staging directory for certificates. Defaults to `/etc/puppetlabs/puppet/ssl`.
    pub staging_directory: Option<String>,
    /// Whether to clean up certificates and node identity on completion. Defaults to true.
    pub clean_up: bool,
}

impl PuppetServerConfig {
    /// Returns default `PuppetServerConfig` with `clean_up: true`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            facter: HashMap::new(),
            puppet_server: None,
            puppet_node_interface: None,
            client_cert_path: None,
            client_private_key_path: None,
            certname: None,
            autosign: false,
            extra_arguments: Vec::new(),
            staging_directory: None,
            clean_up: true,
        }
    }
}

/// The `puppet-server` provisioner.
#[derive(Debug, Clone)]
pub struct PuppetServerProvisioner {
    /// The provisioner configuration.
    pub config: PuppetServerConfig,
}

impl PuppetServerProvisioner {
    /// Create a new `PuppetServerProvisioner`.
    #[must_use]
    pub const fn new(config: PuppetServerConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Provisioner for PuppetServerProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        let staging_dir = self
            .config
            .staging_directory
            .as_deref()
            .unwrap_or("/tmp/packer-puppet-server");

        // Upload client certs if specified
        if self.config.client_cert_path.is_some() || self.config.client_private_key_path.is_some() {
            let mkdir_cmd = format!("mkdir -p {staging_dir}");
            let res = comm.execute(&Command::new(mkdir_cmd)).await?;
            if res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "Failed to create SSL directory. exit code: {}",
                    res.exit_code
                )));
            }

            if let Some(cert) = &self.config.client_cert_path {
                let remote_cert = format!("{staging_dir}/cert.pem");
                comm.upload(cert, &FilePath::new(PathBuf::from(remote_cert)))
                    .await?;
            }
            if let Some(key) = &self.config.client_private_key_path {
                let remote_key = format!("{staging_dir}/key.pem");
                comm.upload(key, &FilePath::new(PathBuf::from(remote_key)))
                    .await?;
            }
        }

        let mut cmd_str =
            String::from("puppet agent --onetime --no-daemonize --detailed-exitcodes");

        if let Some(server) = &self.config.puppet_server {
            cmd_str.push_str(" --server=");
            cmd_str.push_str(server);
        }

        if let Some(env) = &self.config.puppet_node_interface {
            cmd_str.push_str(" --environment=");
            cmd_str.push_str(env);
        }

        if let Some(ref cert) = self.config.certname {
            cmd_str.push_str(" --certname=");
            cmd_str.push_str(cert);
        }

        if self.config.autosign {
            cmd_str.push_str(" --waitforcert=60");
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

        ui.say("puppet-server", &format!("Executing: {final_cmd}"));
        let res = comm.execute(&Command::new(final_cmd)).await?;

        if self.config.clean_up {
            ui.say(
                "puppet-server",
                "Tearing down Puppet agent SSL certificates...",
            );
            let _ = comm
                .execute(&Command::new("puppet ssl clean".to_string()))
                .await;
            let rm_cmd = format!("rm -rf {staging_dir}");
            let _ = comm.execute(&Command::new(rm_cmd)).await;
        }

        // puppet agent detailed-exitcodes: 0 (no changes), 2 (changes applied) are successes.
        if res.exit_code != 0 && res.exit_code != 2 {
            return Err(StampError::Provisioner(format!(
                "puppet agent failed with exit code: {}",
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
    async fn test_puppet_server_provision_success() -> Result<(), StampError> {
        let config = PuppetServerConfig {
            ..Default::default()
        };
        let prov = PuppetServerProvisioner::new(config);
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
    async fn test_puppet_server_provision_with_certs_and_args() -> Result<(), StampError> {
        let mut facter = HashMap::new();
        facter.insert("my_fact".to_string(), "true".to_string());

        let config = PuppetServerConfig {
            puppet_server: Some("puppet.example.com".to_string()),
            puppet_node_interface: Some("production".to_string()),
            client_cert_path: Some(FilePath::new(PathBuf::from("cert.pem"))),
            client_private_key_path: Some(FilePath::new(PathBuf::from("key.pem"))),
            certname: Some("node1.example.com".to_string()),
            autosign: true,
            facter,
            extra_arguments: vec!["--debug".to_string()],
            staging_directory: Some("/tmp/puppet-ssl".to_string()),
            clean_up: true,
        };
        let prov = PuppetServerProvisioner::new(config);
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
        let config1 = PuppetServerConfig {
            puppet_server: Some("puppet.example.com".to_string()),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = PuppetServerProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
