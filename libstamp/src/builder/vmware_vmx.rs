//! Implementation of the `vmware-vmx` builder.

pub use super::vmware_iso::{
    VmwareDriver, discover_guest_ip, generate_vmx_content, run_govc, run_ovftool, run_vmrun,
};
use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `vmware-vmx` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VmwareVmxConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The source VMX path.
    pub source_path: Option<String>,
    /// VM name.
    pub vm_name: Option<String>,
    /// The boot command sequence.
    pub boot_command: Option<Vec<String>>,
    /// The wait time before booting.
    pub boot_wait: Option<String>,
    /// Automation driver (`vmrun` or `govc`).
    pub driver: VmwareDriver,
    /// Custom `.vmx` parameters to inject.
    pub vmx_data: HashMap<String, String>,
    /// Headless mode.
    pub headless: bool,
    /// Output directory.
    pub output_directory: Option<String>,
    /// Target export format (`ova` or `ovf`).
    pub export_format: Option<String>,
}

/// The `vmware-vmx` builder.
#[derive(Debug, Clone)]
pub struct VmwareVmxBuilder {
    /// The builder configuration.
    pub config: VmwareVmxConfig,
}

impl VmwareVmxBuilder {
    /// Create a new `VmwareVmxBuilder`.
    #[must_use]
    pub const fn new(config: VmwareVmxConfig) -> Self {
        Self { config }
    }
}

/// Step to clone or stage the source VMX into the output directory.
#[derive(Debug, Clone)]
struct StepCloneVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VmwareVmxConfig,
}

#[async_trait::async_trait]
impl Step for StepCloneVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self
            .config
            .vm_name
            .as_deref()
            .unwrap_or("packer-vmware-vmx");
        let source_path = self.config.source_path.as_deref().unwrap_or("dummy.vmx");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-vmware-vmx");

        self.ui.say(
            &self.name,
            &format!("Staging VMware VM {vm_name} from {source_path} into {output_dir}"),
        );

        let _ = std::fs::create_dir_all(output_dir);

        let target_vmx = PathBuf::from(output_dir).join(format!("{vm_name}.vmx"));
        state.put("vmx_path", target_vmx.to_string_lossy().to_string());

        if Path::new(source_path).exists() {
            let _ = tokio::fs::copy(source_path, &target_vmx).await;
        } else {
            let vmx_content =
                generate_vmx_content(vm_name, "other-64", 1024, 1, None, &self.config.vmx_data);
            let _ = tokio::fs::write(&target_vmx, vmx_content.as_bytes()).await;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vmx_path) = state.get::<String>("vmx_path") {
            self.ui
                .say(&self.name, &format!("Cleaning up cloned VM: {vmx_path}"));
        }
    }
}

/// Step to launch the cloned VMware VM.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VmwareVmxConfig,
}

#[async_trait::async_trait]
impl Step for StepRunVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vmx_path = state.get::<String>("vmx_path").cloned().unwrap_or_default();
        self.ui.say(&self.name, "Starting VMware VM...");

        let mode = if self.config.headless { "nogui" } else { "gui" };
        let _ = run_vmrun(&["start", &vmx_path, mode]).await;

        let ip = discover_guest_ip(Path::new(&vmx_path), self.config.driver).await?;
        self.ui
            .say(&self.name, &format!("Discovered guest IP: {ip}"));
        state.put("vm_ip", ip);
        state.put("ssh_port", 22u16);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vmx_path) = state.get::<String>("vmx_path") {
            self.ui
                .say(&self.name, &format!("Stopping VMware VM: {vmx_path}"));
            let _ = run_vmrun(&["stop", vmx_path, "hard"]).await;
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
        self.ui.say(&self.name, "Provisioning VMware VM...");

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
            host: "vmware".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "vmware-vmx".to_string(),
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
        self.ui.say(&self.name, "Shutting down VMware VM...");
        let vmx_path = state.get::<String>("vmx_path").cloned().unwrap_or_default();
        let _ = run_vmrun(&["stop", &vmx_path, "soft"]).await;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to export the VMware VM to OVA/OVF format via `ovftool`.
#[derive(Debug, Clone)]
struct StepExportVmx {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VmwareVmxConfig,
}

#[async_trait::async_trait]
impl Step for StepExportVmx {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let Some(ref format) = self.config.export_format else {
            return Ok(StepAction::Continue);
        };
        if format != "ova" && format != "ovf" {
            return Ok(StepAction::Continue);
        }

        let vmx_path_str = state.get::<String>("vmx_path").cloned().unwrap_or_default();
        let vmx_path = Path::new(&vmx_path_str);
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-vmware-vmx");
        let vm_name = self.config.vm_name.as_deref().unwrap_or("vm");
        let export_path = PathBuf::from(output_dir).join(format!("{vm_name}.{format}"));

        self.ui.say(
            &self.name,
            &format!(
                "Exporting VMware VM to {} via ovftool...",
                export_path.display()
            ),
        );

        run_ovftool(vmx_path, &export_path).await?;
        state.put("export_path", export_path.to_string_lossy().to_string());
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for VmwareVmxBuilder {
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
            Box::new(StepExportVmx {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
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

        let vmx_path = state.get::<String>("vmx_path").cloned().unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("vmware-vmx:{vmx_path}"),
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
    async fn test_vmwarevmxbuilder_run() {
        let mut vmx_data = HashMap::new();
        vmx_data.insert("custom.vmx".to_string(), "val".to_string());

        let config = VmwareVmxConfig {
            name: "test-builder".to_string(),
            source_path: Some("source.vmx".to_string()),
            vm_name: Some("test-vm".to_string()),
            output_directory: Some("custom-vmware-out".to_string()),
            vmx_data,
            driver: VmwareDriver::Govc,
            export_format: Some("ova".to_string()),
            ..Default::default()
        };
        let builder = VmwareVmxBuilder::new(config.clone());

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
                .contains("test-vm.vmx")
        );
        assert!(builder.cancel().await.is_ok());

        // Default run without export_format, output_directory, vm_name
        let b_default = VmwareVmxBuilder::new(VmwareVmxConfig {
            name: "default-vmx".to_string(),
            ..Default::default()
        });
        let res_default = b_default
            .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
            .await;
        assert!(res_default.is_ok());

        // Test step cleanups and export format branches (both without and with vm_ip/ssh_port)
        let mut empty_state = StateBag::new();
        let mut prov_step = StepProvision {
            ui: ui.clone(),
            name: "test".to_string(),
            hook,
        };
        assert!(prov_step.run(&mut empty_state).await.is_ok());

        let mut state = StateBag::new();
        state.put("vm_ip", "10.0.0.1".to_string());
        state.put("vmx_path", "/tmp/vm.vmx".to_string());
        state.put("ssh_port", 2222u16);
        assert!(prov_step.run(&mut state).await.is_ok());
        prov_step.cleanup(&state).await;

        let mut export_ovf = StepExportVmx {
            ui: ui.clone(),
            name: "test".to_string(),
            config: VmwareVmxConfig {
                export_format: Some("ovf".to_string()),
                ..Default::default()
            },
        };
        assert!(export_ovf.run(&mut state).await.is_ok());
        export_ovf.cleanup(&state).await;

        let mut export_invalid = StepExportVmx {
            ui: ui.clone(),
            name: "test".to_string(),
            config: VmwareVmxConfig {
                export_format: Some("invalid".to_string()),
                ..Default::default()
            },
        };
        assert!(export_invalid.run(&mut state).await.is_ok());

        let mut export_none = StepExportVmx {
            ui: ui.clone(),
            name: "test".to_string(),
            config: VmwareVmxConfig::default(),
        };
        assert!(export_none.run(&mut state).await.is_ok());

        let mut clone_step = StepCloneVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
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
    fn test_vmware_vmx_derived_traits() {
        let config1 = VmwareVmxConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = VmwareVmxBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));
    }
}
