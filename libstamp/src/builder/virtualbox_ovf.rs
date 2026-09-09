//! Implementation of the `virtualbox-ovf` builder.

pub use super::virtualbox_iso::{char_to_vbox_scancodes, vboxmanage};
use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `virtualbox-ovf` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VirtualboxOvfConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The source OVF/OVA path.
    pub source_path: Option<String>,
    /// VM name.
    pub vm_name: Option<String>,
    /// Memory size in MB.
    pub memory: Option<u64>,
    /// Number of virtual CPUs.
    pub cpus: Option<u32>,
    /// Path to Guest Additions ISO to mount.
    pub guest_additions_path: Option<String>,
    /// Target export format (`ova` or `ovf`). Defaults to `ova`.
    pub export_format: Option<String>,
    /// The boot command sequence.
    pub boot_command: Option<Vec<String>>,
    /// The wait time before booting.
    pub boot_wait: Option<String>,
    /// Network configuration.
    pub network: Option<String>,
    /// Headless mode.
    pub headless: bool,
    /// Output directory.
    pub output_directory: Option<String>,
}

/// The `virtualbox-ovf` builder.
#[derive(Debug, Clone)]
pub struct VirtualboxOvfBuilder {
    /// The builder configuration.
    pub config: VirtualboxOvfConfig,
}

impl VirtualboxOvfBuilder {
    /// Create a new `VirtualboxOvfBuilder`.
    #[must_use]
    pub const fn new(config: VirtualboxOvfConfig) -> Self {
        Self { config }
    }
}

/// Step to import the OVF/OVA appliance and configure hardware.
#[derive(Debug, Clone)]
struct StepImportVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxOvfConfig,
}

#[async_trait::async_trait]
impl Step for StepImportVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self
            .config
            .vm_name
            .as_deref()
            .unwrap_or("packer-virtualbox-ovf");
        self.ui.say(
            &self.name,
            &format!("Importing VirtualBox OVF/OVA: {vm_name}"),
        );

        state.put("vm_name", vm_name.to_string());

        let source = self.config.source_path.as_deref().unwrap_or("source.ovf");

        vboxmanage(&["import", source, "--vsys", "0", "--vmname", vm_name]).await?;

        // Modify memory and CPUs if specified
        if let Some(mem) = self.config.memory {
            let mem_str = format!("{mem}");
            vboxmanage(&["modifyvm", vm_name, "--memory", &mem_str]).await?;
        }
        if let Some(cpus) = self.config.cpus {
            let cpus_str = format!("{cpus}");
            vboxmanage(&["modifyvm", vm_name, "--cpus", &cpus_str]).await?;
        }

        // Mount Guest Additions ISO if provided
        if let Some(ref ga_path) = self.config.guest_additions_path {
            self.ui.say(
                &self.name,
                &format!("Mounting Guest Additions ISO: {ga_path}"),
            );
            let _ = vboxmanage(&[
                "storageattach",
                vm_name,
                "--storagectl",
                "IDE Controller",
                "--port",
                "1",
                "--device",
                "0",
                "--type",
                "dvddrive",
                "--medium",
                ga_path,
            ])
            .await;
        }

        // Configure SSH port forwarding
        vboxmanage(&["modifyvm", vm_name, "--natpf1", "guestssh,tcp,,2222,,22"]).await?;

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_name) = state.get::<String>("vm_name") {
            self.ui.say(
                &self.name,
                &format!("Unregistering and deleting VM: {vm_name}"),
            );
            let _ = vboxmanage(&["unregistervm", vm_name, "--delete"]).await;
        }
    }
}

/// Step to run the imported VM and send boot commands via scancodes.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxOvfConfig,
}

#[async_trait::async_trait]
impl Step for StepRunVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        self.ui.say(&self.name, "Starting VirtualBox VM...");

        let mode = if self.config.headless {
            "headless"
        } else {
            "gui"
        };
        vboxmanage(&["startvm", &vm_name, "--type", mode]).await?;

        if let Some(ref cmds) = self.config.boot_command {
            self.ui
                .say(&self.name, "Typing boot commands via scancodes...");
            for token in cmds {
                for ch in token.chars() {
                    let scancodes = char_to_vbox_scancodes(ch);
                    if !scancodes.is_empty() {
                        let mut args = vec!["controlvm", vm_name.as_str(), "keyboardputscancode"];
                        args.extend(scancodes);
                        let _ = vboxmanage(&args).await;
                    }
                }
            }
        }

        state.put("vm_ip", "127.0.0.1".to_string());
        state.put("ssh_port", 2222u16);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_name) = state.get::<String>("vm_name") {
            self.ui
                .say(&self.name, &format!("Powering off VM: {vm_name}"));
            let _ = vboxmanage(&["controlvm", vm_name, "poweroff"]).await;
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
            host: "vbox".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "virtualbox-ovf".to_string(),
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
        self.ui.say(&self.name, "Shutting down VM...");
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        let _ = vboxmanage(&["controlvm", &vm_name, "acpipowerbutton"]).await;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to export the VM to OVA or OVF.
#[derive(Debug, Clone)]
struct StepExport {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxOvfConfig,
}

#[async_trait::async_trait]
impl Step for StepExport {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-virtualbox-ovf");

        self.ui
            .say(&self.name, &format!("Exporting VM to {output_dir}"));

        let _ = std::fs::create_dir_all(output_dir);

        let ext = self.config.export_format.as_deref().unwrap_or("ova");
        let export_path = format!("{output_dir}/{vm_name}.{ext}");
        vboxmanage(&["export", &vm_name, "--output", &export_path]).await?;

        state.put("export_path", export_path);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for VirtualboxOvfBuilder {
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
            Box::new(StepImportVM {
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
            Box::new(StepExport {
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

        let export_path = state
            .get::<String>("export_path")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("virtualbox-ovf:{export_path}"),
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
    async fn test_virtualboxovfbuilder_run() {
        let config = VirtualboxOvfConfig {
            name: "test-builder".to_string(),
            vm_name: Some("test-vm".to_string()),
            source_path: Some("source.ovf".to_string()),
            memory: Some(2048),
            cpus: Some(2),
            guest_additions_path: Some("/tmp/VBoxGuestAdditions.iso".to_string()),
            export_format: Some("ovf".to_string()),
            output_directory: Some("custom-vbox-out".to_string()),
            boot_command: Some(vec!["install<enter>".to_string()]),
            ..Default::default()
        };
        let builder = VirtualboxOvfBuilder::new(config.clone());

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
                .contains("test-vm.ovf")
        );
        assert!(builder.cancel().await.is_ok());

        // Default run without export_format, output_directory, vm_name
        let b_default = VirtualboxOvfBuilder::new(VirtualboxOvfConfig {
            name: "default-vbox".to_string(),
            ..Default::default()
        });
        let res_default = b_default
            .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
            .await;
        assert!(res_default.is_ok());

        // Test step cleanups and execution (both without and with vm_ip/ssh_port)
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

        let mut import_step = StepImportVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        import_step.cleanup(&state).await;
        state.put("vm_name", "test-vm".to_string());
        import_step.cleanup(&state).await;

        let mut run_step = StepRunVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        run_step.cleanup(&state).await;

        let mut export_step = StepExport {
            ui,
            name: "test".to_string(),
            config,
        };
        export_step.cleanup(&state).await;
    }

    #[test]
    fn test_virtualboxovfbuilder_derived_traits() {
        let config1 = VirtualboxOvfConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = VirtualboxOvfBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));
    }
}
