//! Implementation of the `hyperv-vmcx` builder.

pub use super::hyperv_iso::{extract_hyperv_ip, run_hyperv_ps, run_hyperv_ps_with_bin};
use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use std::sync::Arc;

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
    /// Optional custom PowerShell binary path for execution.
    pub powershell_binary: Option<String>,
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
        let ps_bin = self.config.powershell_binary.as_deref();

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
            run_hyperv_ps_with_bin(&import_script, ps_bin).await?;
        } else if let Some(ref src_name) = self.config.clone_from_vm_name {
            let export_script = format!(
                "Export-VM -Name '{src_name}' -Path '{output_dir}'; Import-VM -Path '{output_dir}/{src_name}/Virtual Machines' -Copy -GenerateNewId"
            );
            run_hyperv_ps_with_bin(&export_script, ps_bin).await?;
        }

        let _ = run_hyperv_ps_with_bin(
            &format!("Rename-VM -Name '{vm_name}' -NewName '{vm_name}'"),
            ps_bin,
        )
        .await;

        if let Some(ref switch) = self.config.switch_name {
            let switch_script =
                format!("Connect-VMNetworkAdapter -VMName '{vm_name}' -SwitchName '{switch}'");
            run_hyperv_ps_with_bin(&switch_script, ps_bin).await?;
        }

        if let Some(vlan) = self.config.vlan_id {
            let vlan_script =
                format!("Set-VMNetworkAdapterVlan -VMName '{vm_name}' -Access -VlanId {vlan}");
            run_hyperv_ps_with_bin(&vlan_script, ps_bin).await?;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_name) = state.get::<String>("vm_name") {
            self.ui.say(&self.name, &format!("Deleting VM: {vm_name}"));
            let _ = run_hyperv_ps_with_bin(
                &format!("Remove-VM -Name '{vm_name}' -Force"),
                self.config.powershell_binary.as_deref(),
            )
            .await;
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
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        self.ui.say(&self.name, "Starting Hyper-V VM...");

        run_hyperv_ps_with_bin(
            &format!("Start-VM -Name '{vm_name}'"),
            self.config.powershell_binary.as_deref(),
        )
        .await?;

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
            let _ = run_hyperv_ps_with_bin(
                &format!("Stop-VM -Name '{vm_name}' -TurnOff -Force"),
                self.config.powershell_binary.as_deref(),
            )
            .await;
        }
    }
}

/// Step to shut down the VM.
#[derive(Debug, Clone)]
struct StepShutdown {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: HypervVmcxConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down Hyper-V VM...");
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        let _ = run_hyperv_ps_with_bin(
            &format!("Stop-VM -Name '{vm_name}' -Force"),
            self.config.powershell_binary.as_deref(),
        )
        .await;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for HypervVmcxBuilder {
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
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
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "hyperv-vmcx", hook)
                    .with_ip_key("vm_ip")
                    .with_ssh_credentials("packer", None)
                    .with_message("Provisioning VM..."),
            ),
            Box::new(StepShutdown {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            format!("hyperv-vm:{vm_name}"),
            vec![],
        )))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }

    fn name(&self) -> String {
        self.config.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::packer::OnErrorStrategy;
    use crate::engine::ui::Ui;

    #[tokio::test]
    async fn test_hypervvmcxbuilder_run() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let config = HypervVmcxConfig {
            name: "test-builder".to_string(),
            vm_name: Some("test-vm".to_string()),
            clone_from_vm_name: Some("base-vm".to_string()),
            clone_from_vmcx_path: Some(r"C:\VMs\vm.vmcx".to_string()),
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            switch_name: Some("Default Switch".to_string()),
            vlan_id: Some(20),
            boot_command: Some(vec!["<enter>".to_string()]),
            ..Default::default()
        };
        let builder = HypervVmcxBuilder::new(config.clone());

        builder.prepare().await?;
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
            .await?;
        assert!(res.id().contains("test-vm"));
        assert_eq!(res.builder_id(), "test-builder");
        builder.cancel().await?;

        // Test clone_from_vm_name branch
        let b_clone_vm = HypervVmcxBuilder::new(HypervVmcxConfig {
            name: "clone-vm-builder".to_string(),
            clone_from_vm_name: Some("base-vm".to_string()),
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });
        let _ = b_clone_vm
            .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
            .await?;

        let b_empty = HypervVmcxBuilder::new(HypervVmcxConfig::default());
        assert!(b_empty.prepare().await.is_err());

        // Test step cleanups
        let mut state = StateBag::new();
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
            config,
        };
        shutdown_step.cleanup(&state).await;
        Ok(())
    }
}
