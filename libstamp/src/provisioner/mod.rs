#![cfg_attr(coverage_nightly, coverage(off))]
//! Provisioners for modifying machine images.

use crate::communicator::Communicator;
use crate::error::StampError;
use async_trait::async_trait;

/// The main trait for all provisioners.
#[async_trait]
pub trait Provisioner: Send + Sync {
    /// Provision the machine using the provided communicator.
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError>;
}

pub mod ansible;
pub mod ansible_local;
pub mod breakpoint;
pub mod chef_client;
pub mod chef_solo;
pub mod converge;
pub mod custom_hook;
pub mod fabric;
pub mod file;
pub mod goss;
pub mod inspec;
pub mod powershell;
pub mod puppet_masterless;
pub mod puppet_server;
pub mod salt_masterless;
pub mod shell;
pub mod shell_local;
pub mod sysprep;
pub mod windows_restart;
pub mod windows_shell;
pub mod windows_update;

use crate::template::ProvisionerConfig;

/// Creates a new provisioner instance from configuration.
///
/// # Errors
///
/// Returns a `StampError` if the provisioner type is unknown.
pub fn create_provisioner(config: &ProvisionerConfig) -> Result<Box<dyn Provisioner>, StampError> {
    match config.provisioner_type.as_str() {
        "ansible" => Ok(Box::new(ansible::AnsibleProvisioner::new(
            ansible::AnsibleConfig::default(),
        ))),
        "ansible-local" => Ok(Box::new(ansible_local::AnsibleLocalProvisioner::new(
            ansible_local::AnsibleLocalConfig::default(),
        ))),
        "breakpoint" => Ok(Box::new(breakpoint::BreakpointProvisioner::new(
            breakpoint::BreakpointConfig::default(),
        ))),
        "chef-client" => Ok(Box::new(chef_client::ChefClientProvisioner::new(
            chef_client::ChefClientConfig::default(),
        ))),
        "chef-solo" => Ok(Box::new(chef_solo::ChefSoloProvisioner::new(
            chef_solo::ChefSoloConfig::default(),
        ))),
        "converge" => Ok(Box::new(converge::ConvergeProvisioner::new(
            converge::ConvergeConfig::default(),
        ))),
        "fabric" => Ok(Box::new(fabric::FabricProvisioner::new(
            fabric::FabricConfig::default(),
        ))),
        "file" => Ok(Box::new(file::FileProvisioner::new(
            file::FileConfig::default(),
        ))),
        "goss" => Ok(Box::new(goss::GossProvisioner::new(
            goss::GossConfig::default(),
        ))),
        "inspec" => Ok(Box::new(inspec::InspecProvisioner::new(
            inspec::InspecConfig::default(),
        ))),
        "powershell" => Ok(Box::new(powershell::PowershellProvisioner::new(
            powershell::PowershellConfig::default(),
        ))),
        "puppet-masterless" => Ok(Box::new(
            puppet_masterless::PuppetMasterlessProvisioner::new(
                puppet_masterless::PuppetMasterlessConfig::default(),
            ),
        )),
        "puppet-server" => Ok(Box::new(puppet_server::PuppetServerProvisioner::new(
            puppet_server::PuppetServerConfig::default(),
        ))),
        "salt-masterless" => Ok(Box::new(salt_masterless::SaltMasterlessProvisioner::new(
            salt_masterless::SaltMasterlessConfig::default(),
        ))),
        "shell" => Ok(Box::new(shell::ShellProvisioner::new(
            shell::ShellConfig::default(),
        ))),
        "shell-local" => Ok(Box::new(shell_local::ShellLocalProvisioner::new(
            shell_local::ShellLocalConfig::default(),
        ))),
        "sysprep" => Ok(Box::new(sysprep::SysprepProvisioner::new(
            sysprep::SysprepConfig::default(),
        ))),
        "windows-restart" => Ok(Box::new(windows_restart::WindowsRestartProvisioner::new(
            windows_restart::WindowsRestartConfig::default(),
        ))),
        "windows-shell" => Ok(Box::new(windows_shell::WindowsShellProvisioner::new(
            windows_shell::WindowsShellConfig::default(),
        ))),
        "windows-update" => Ok(Box::new(windows_update::WindowsUpdateProvisioner::new(
            windows_update::WindowsUpdateConfig::default(),
        ))),
        "custom-hook" => Ok(Box::new(custom_hook::CustomHookProvisioner::new(
            custom_hook::CustomHookConfig::default(),
        ))),
        "plugin" | "go-plugin" => Ok(Box::new(go_plugin::GoPluginProvisioner::new(
            go_plugin::GoPluginProvisionerConfig {
                provisioner_type: config.provisioner_type.clone(),
                plugin_path: config
                    .config
                    .get("plugin_path")
                    .cloned()
                    .unwrap_or_default(),
                endpoint: config.config.get("endpoint").cloned(),
            },
        ))),
        _ => Err(StampError::Parse(format!(
            "Unknown provisioner type: {}",
            config.provisioner_type
        ))),
    }
}

pub mod go_plugin;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_create_provisioner_known() {
        let types = vec![
            "ansible",
            "ansible-local",
            "breakpoint",
            "chef-client",
            "chef-solo",
            "converge",
            "fabric",
            "file",
            "goss",
            "inspec",
            "powershell",
            "puppet-masterless",
            "puppet-server",
            "salt-masterless",
            "shell",
            "shell-local",
            "sysprep",
            "windows-restart",
            "windows-shell",
            "windows-update",
            "custom-hook",
            "plugin",
            "go-plugin",
        ];

        for p_type in types {
            let c = ProvisionerConfig {
                provisioner_type: p_type.to_string(),
                ..Default::default()
            };
            assert!(create_provisioner(&c).is_ok());
        }
    }

    #[test]
    fn test_create_provisioner_unknown() {
        let c = ProvisionerConfig {
            provisioner_type: "unknown".to_string(),
            ..Default::default()
        };
        assert!(create_provisioner(&c).is_err());
    }
}
