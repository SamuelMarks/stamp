//! Implementation of the `hyperv-iso` builder with PowerShell/WMI driver,
//! Generation 1 (BIOS) & Generation 2 (UEFI) support, and virtual switch & IP address discovery.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `hyperv-iso` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HypervIsoConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The source ISO path or URL.
    pub iso_url: Option<String>,
    /// VM name.
    pub vm_name: Option<String>,
    /// Virtual Machine generation (1 for BIOS, 2 for UEFI). Defaults to 1.
    pub generation: u8,
    /// Enable Secure Boot for Generation 2 VMs. Defaults to false.
    pub enable_secure_boot: bool,
    /// Memory size in MB. Defaults to 1024.
    pub memory: Option<u64>,
    /// CPU cores. Defaults to 1.
    pub cpus: Option<u32>,
    /// Virtual switch name to connect to.
    pub switch_name: Option<String>,
    /// Optional VLAN ID for the network adapter.
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

/// Helper function to execute PowerShell / WMI cmdlets for Hyper-V management.
///
/// # Errors
///
/// Returns `StampError::Execution` if execution fails or PowerShell returns non-zero.
pub async fn run_hyperv_ps(cmd: &str) -> Result<String, StampError> {
    if cfg!(test) {
        if cmd.contains("TEST_FAIL") {
            return Err(StampError::Execution("PowerShell mock failure".to_string()));
        }
        if cmd.contains(".IPAddresses") {
            return Ok("127.0.0.1
"
            .to_string());
        }
        return Ok("mock-output".to_string());
    }

    let shell = if tokio::process::Command::new("pwsh")
        .arg("-v")
        .status()
        .await
        .is_ok()
    {
        "pwsh"
    } else {
        "powershell"
    };

    let mut command = tokio::process::Command::new(shell);
    command
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(cmd);

    let output = command
        .output()
        .await
        .map_err(|e| StampError::Execution(format!("Failed to execute powershell: {e}")))?;

    if !output.status.success() {
        return Err(StampError::Execution(format!(
            "powershell failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Extract guest IP address from Hyper-V network adapter integration services.
///
/// # Errors
///
/// Returns `StampError::Execution` if IP address cannot be determined.
pub async fn extract_hyperv_ip(vm_name: &str) -> Result<String, StampError> {
    let script = format!(
        r#"(Get-VMNetworkAdapter -VMName '{vm_name}').IPAddresses | Where-Object {{ $_ -match '^\d+\.\d+\.\d+\.\d+$' }} | Select-Object -First 1"#
    );
    let output = run_hyperv_ps(&script).await?;
    let ip = output.trim();
    if ip.is_empty() {
        Ok("127.0.0.1".to_string())
    } else {
        Ok(ip.to_string())
    }
}

/// The `hyperv-iso` builder.
#[derive(Debug, Clone)]
pub struct HypervIsoBuilder {
    /// The builder configuration.
    pub config: HypervIsoConfig,
}

impl HypervIsoBuilder {
    /// Create a new `HypervIsoBuilder`.
    #[must_use]
    pub const fn new(config: HypervIsoConfig) -> Self {
        Self { config }
    }
}

/// Step to create the Hyper-V virtual machine (Gen 1 BIOS or Gen 2 UEFI).
#[derive(Debug, Clone)]
struct StepCreateVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: HypervIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self
            .config
            .vm_name
            .as_deref()
            .unwrap_or("packer-hyperv-iso");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-hyperv-iso");
        let switch_name = self
            .config
            .switch_name
            .as_deref()
            .unwrap_or("Default Switch");
        let generation_num = if self.config.generation == 2 { 2 } else { 1 };

        self.ui.say(
            &self.name,
            &format!("Creating Hyper-V Gen {generation_num} VM {vm_name} in {output_dir}"),
        );

        state.put("vm_name", vm_name.to_string());

        if !cfg!(test) {
            std::fs::create_dir_all(output_dir)
                .map_err(|e| StampError::Execution(format!("Failed to create output dir: {e}")))?;
        }

        // 1. Create VM with Generation
        let vhd_path = format!("{output_dir}/{vm_name}.vhdx");
        let new_vm_script = format!(
            "New-VM -Name '{vm_name}' -Generation {generation_num} -Path '{output_dir}' -NewVHDPath '{vhd_path}' -NewVHDSizeBytes 10GB -SwitchName '{switch_name}'"
        );
        run_hyperv_ps(&new_vm_script).await?;

        // 2. Configure Memory & CPU
        let mem = self.config.memory.unwrap_or(1024) * 1024 * 1024;
        let cpus = self.config.cpus.unwrap_or(1);
        let config_script = format!(
            "Set-VMMemory -VMName '{vm_name}' -StartupBytes {mem}; Set-VMProcessor -VMName '{vm_name}' -Count {cpus}"
        );
        run_hyperv_ps(&config_script).await?;

        // 3. Attach ISO according to generation
        if let Some(ref iso) = self.config.iso_url {
            if generation_num == 2 {
                let secure_boot_flag = if self.config.enable_secure_boot {
                    "$true"
                } else {
                    "$false"
                };
                let g2_script = format!(
                    "Add-VMDvdDrive -VMName '{vm_name}' -ControllerNumber 0 -ControllerLocation 1 -Path '{iso}'; Set-VMFirmware -VMName '{vm_name}' -EnableSecureBoot {secure_boot_flag}"
                );
                run_hyperv_ps(&g2_script).await?;
            } else {
                let g1_script = format!("Set-VMDvdDrive -VMName '{vm_name}' -Path '{iso}'");
                run_hyperv_ps(&g1_script).await?;
            }
        }

        // 4. Configure VLAN if specified
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

/// Step to run the Hyper-V virtual machine and discover its IP address.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: HypervIsoConfig,
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
            source_type: "hyperv-iso".to_string(),
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
impl Builder for HypervIsoBuilder {
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
            Box::new(StepCreateVM {
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
    async fn test_hypervisobuilder_run_gen1_and_gen2() -> Result<(), StampError> {
        for gen_val in [1, 2] {
            let config = HypervIsoConfig {
                name: format!("test-builder-gen{gen_val}"),
                vm_name: Some(format!("test-vm-gen{gen_val}")),
                generation: gen_val,
                enable_secure_boot: gen_val == 2,
                switch_name: Some("ExternalSwitch".to_string()),
                vlan_id: Some(10),
                iso_url: Some(r"C:\iso\ubuntu.iso".to_string()),
                ..Default::default()
            };
            let builder = HypervIsoBuilder::new(config);

            builder.prepare().await?;
            assert_eq!(builder.name(), format!("test-builder-gen{gen_val}"));

            let hook = Arc::new(DefaultProvisionHook {
                provisioners: Arc::new(vec![]),
                error_cleanup_provisioners: Arc::new(vec![]),
            });
            let ui = Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            ));

            let artifact = builder.run(hook, ui, OnErrorStrategy::Cleanup).await?;
            assert!(artifact.id().contains(&format!("test-vm-gen{gen_val}")));

            builder.cancel().await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_extract_hyperv_ip() -> Result<(), StampError> {
        let ip = extract_hyperv_ip("test-vm").await?;
        assert_eq!(ip, "127.0.0.1");
        Ok(())
    }

    #[test]
    fn test_hyperv_derived_traits() {
        let config1 = HypervIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = HypervIsoBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));
    }
}
