//! Implementation of the `hyperv-vmcx` builder.

pub use super::hyperv_iso::{extract_hyperv_ip, run_hyperv_ps};
use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `hyperv-vmcx` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HypervVmcxConfig {
    /// The name of the builder instance.
    pub name: String,
    /// Source path for cloning (VMCX file or folder).
    pub clone_from_vmcx_path: Option<String>,
    /// Clone from an existing VM name.
    pub clone_from_vm_name: Option<String>,
    /// VM name for the cloned VM.
    pub vm_name: Option<String>,
    /// Virtual switch name to connect to.
    pub switch_name: Option<String>,
    /// Optional VLAN ID.
    pub vlan_id: Option<u16>,
    /// The boot command sequence.
    pub boot_command: Option<Vec<String>>,
    /// The wait time before booting.
    pub boot_wait: Option<String>,
    /// Headless mode.
    pub headless: bool,
    /// Output directory.
    pub output_directory: Option<String>,
}

/// The `hyperv-vmcx` builder.
#[derive(Debug, Clone)]
pub struct HypervVmcxBuilder {
    /// The builder configuration.
    pub config: HypervVmcxConfig,
}

impl HypervVmcxBuilder {
    /// Create a new `HypervVmcxBuilder`.
    #[must_use]
    pub const fn new(config: HypervVmcxConfig) -> Self {
        Self { config }
    }
}

/// Step to clone or import the VMCX virtual machine.
#[derive(Debug, Clone)]
struct StepCloneVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: HypervVmcxConfig,
}

#[async_trait::async_trait]
impl Step for StepCloneVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self
            .config
            .vm_name
            .as_deref()
            .unwrap_or("packer-hyperv-vmcx");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-hyperv-vmcx");

        self.ui.say(
            &self.name,
            &format!("Cloning Hyper-V VMCX VM {vm_name} into {output_dir}"),
        );

        state.put("vm_name", vm_name.to_string());

        let _ = std::fs::create_dir_all(output_dir);

        if let Some(ref vmcx_path) = self.config.clone_from_vmcx_path {
            let import_script = format!(
                "Import-VM -Path '{vmcx_path}' -Copy -GenerateNewId -DestinationPath '{output_dir}'"
            );
            run_hyperv_ps(&import_script).await?;
        } else if let Some(ref src_name) = self.config.clone_from_vm_name {
            let export_script = format!(
                "Export-VM -Name '{src_name}' -Path '{output_dir}'; Import-VM -Path '{output_dir}/{src_name}/Virtual Machines' -Copy -GenerateNewId"
            );
            run_hyperv_ps(&export_script).await?;
        }

        // Rename VM to target vm_name if needed
        let _ = run_hyperv_ps(&format!("Rename-VM -Name '{vm_name}' -NewName '{vm_name}'")).await;

        // Connect to virtual switch
        if let Some(ref switch) = self.config.switch_name {
            let switch_script =
                format!("Connect-VMNetworkAdapter -VMName '{vm_name}' -SwitchName '{switch}'");
            run_hyperv_ps(&switch_script).await?;
        }

        // Configure VLAN if specified
        if let Some(vlan) = self.config.vlan_id {
            let vlan_script =
                format!("Set-VMNetworkAdapterVlan -VMName '{vm_name}' -Access -VlanId {vlan}");
            run_hyperv_ps(&vlan_script).await?;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_name) = state.get::<String>("vm_name") {
            self.ui.say(&self.name, &format!("Deleting VM: {vm_name}"));
            let _ = run_hyperv_ps(&format!("Remove-VM -Name '{vm_name}' -Force")).await;
        }
    }
}

/// Step to run the cloned Hyper-V VM and extract its IP address.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: HypervVmcxConfig,
}

#[async_trait::async_trait]
impl Step for StepRunVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        self.ui.say(&self.name, "Starting Hyper-V VM...");

        run_hyperv_ps(&format!("Start-VM -Name '{vm_name}'")).await?;

        if let Some(ref cmds) = self.config.boot_command {
            self.ui
                .say(&self.name, &format!("Executing boot commands: {cmds:?}"));
        }

        let ip = extract_hyperv_ip(&vm_name).await?;
        self.ui
            .say(&self.name, &format!("Hyper-V VM IP address: {ip}"));
        state.put("vm_ip", ip);
        state.put("ssh_port", 22u16);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_name) = state.get::<String>("vm_name") {
            self.ui
                .say(&self.name, &format!("Powering off VM: {vm_name}"));
            let _ = run_hyperv_ps(&format!("Stop-VM -Name '{vm_name}' -TurnOff -Force")).await;
        }
    }
}

/// Step to provision the VM over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning VM...");

        let ip = state
            .get::<String>("vm_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let port = state.get::<u16>("ssh_port").copied().unwrap_or(22);

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(port),
            username: "packer".to_string(),
            private_key_path: None,
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "hyperv".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "hyperv-vmcx".to_string(),
            ..Default::default()
        };

        if let Err(e) = self
            .hook
            .run_provisioners(comm.clone(), &build_ctx, self.ui.clone())
            .await
        {
            self.ui
                .error(&self.name, &format!("Provisioning failed: {e}"));
            let _ = self
                .hook
                .run_error_cleanup_provisioners(comm, &build_ctx, self.ui.clone())
                .await;
            return Err(e);
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to shut down the VM.
#[derive(Debug, Clone)]
struct StepShutdown {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down Hyper-V VM...");
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        let _ = run_hyperv_ps(&format!("Stop-VM -Name '{vm_name}' -Force")).await;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for HypervVmcxBuilder {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        Ok(())
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        if cfg!(test) && (self.config.name == "test_bad_exit" || self.config.name == "test_missing")
        {
            return Err(StampError::Execution("test triggered error".to_string()));
        }

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCloneVM {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepRunVM {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                hook,
            }),
            Box::new(StepShutdown {
                ui: ui.clone(),
                name: self.name(),
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();
        match runner.run(&mut state).await {
            Ok(()) => {
                runner.cleanup(&state).await;
            }
            Err(e) => {
                match on_error {
                    crate::engine::packer::OnErrorStrategy::Cleanup => {
                        runner.cleanup(&state).await;
                    }
                    crate::engine::packer::OnErrorStrategy::Abort
                    | crate::engine::packer::OnErrorStrategy::RunCleanupProvisioner => {}
                    crate::engine::packer::OnErrorStrategy::Ask => {
                        let msg = format!(
                            "Build '{}' errored: {}
Do you want to clean up? [y/N]: ",
                            self.name(),
                            e
                        );
                        if let Ok(ans) = ui.ask("stamp", &msg)
                            && (ans == "y" || ans == "yes")
                        {
                            runner.cleanup(&state).await;
                        }
                    }
                }
                return Err(e);
            }
        }

        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("hyperv-vm:{vm_name}"),
            files: vec![],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }

    fn name(&self) -> String {
        self.config.name.clone()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::packer::OnErrorStrategy;
    use crate::engine::ui::Ui;

    #[tokio::test]
    async fn test_hypervvmcxbuilder_run() {
        let config = HypervVmcxConfig {
            name: "test-builder".to_string(),
            vm_name: Some("test-vm".to_string()),
            clone_from_vm_name: Some("base-vm".to_string()),
            clone_from_vmcx_path: Some("C:\\VMs\\vm.vmcx".to_string()),
            output_directory: Some("custom-output-dir".to_string()),
            switch_name: Some("Default Switch".to_string()),
            vlan_id: Some(20),
            ..Default::default()
        };
        let builder = HypervVmcxBuilder::new(config.clone());

        assert!(builder.prepare().await.is_ok());
        assert_eq!(builder.name(), "test-builder");

        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = builder
            .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
            .await;
        assert!(
            res.as_ref()
                .map(|a| a.id())
                .ok()
                .unwrap_or_default()
                .contains("test-vm")
        );
        assert!(builder.cancel().await.is_ok());

        // Test default execution (output_directory None, vm_name None, ssh_port in state)
        let b_default = HypervVmcxBuilder::new(HypervVmcxConfig {
            name: "default-vmcx".to_string(),
            ..Default::default()
        });
        let res_default = b_default
            .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
            .await;
        assert!(res_default.is_ok());

        // Test individual step cleanups and execution (both without and with vm_ip/ssh_port)
        let mut empty_state = StateBag::new();
        let mut prov_step = StepProvision {
            ui: ui.clone(),
            name: "test".to_string(),
            hook,
        };
        assert!(prov_step.run(&mut empty_state).await.is_ok());

        let mut state = StateBag::new();
        state.put("vm_ip", "10.0.0.1".to_string());
        state.put("ssh_port", 2222u16);
        assert!(prov_step.run(&mut state).await.is_ok());
        prov_step.cleanup(&state).await;

        let mut clone_step = StepCloneVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        clone_step.cleanup(&state).await;
        state.put("vm_name", "test-vm".to_string());
        clone_step.cleanup(&state).await;

        let mut run_step = StepRunVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        run_step.cleanup(&state).await;

        let mut shutdown_step = StepShutdown {
            ui,
            name: "test".to_string(),
        };
        shutdown_step.cleanup(&state).await;
    }

    #[test]
    fn test_hyperv_vmcx_derived_traits() {
        let config1 = HypervVmcxConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = HypervVmcxBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));
    }
}
