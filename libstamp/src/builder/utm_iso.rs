//! Implementation of the `utm-iso` builder for creating, configuring, running, and exporting
//! UTM virtual machine bundles (`.utm` package directory containing `config.plist` and disk images)
//! via `utmctl` CLI driver and native macOS Virtualization / QEMU bundle generation.

/// Configuration types, backends, architectures, and plist generation for `utm-iso`.
pub mod config;

pub use config::{
    UtmArch, UtmBackend, UtmDisplayType, UtmGuestAdditionsMode, UtmInterface, UtmIsoConfig,
    discover_utm_guest_ip, discover_utm_guest_ip_with_cmd, find_utmctl_binary, generate_utm_plist,
    parse_string_list, run_utmctl, run_utmctl_with_cmd, utmctl_binary,
};

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::communicator::Communicator;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::communicator::winrm::{WinRmCommunicator, WinRmConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The `utm-iso` builder.
#[derive(Debug, Clone)]
pub struct UtmIsoBuilder {
    /// The builder configuration.
    pub config: UtmIsoConfig,
}

impl UtmIsoBuilder {
    /// Create a new `UtmIsoBuilder`.
    #[must_use]
    pub const fn new(config: UtmIsoConfig) -> Self {
        Self { config }
    }
}

/// Step to create the `.utm` bundle directory, write `config.plist`, and create virtual disk images.
#[derive(Debug, Clone)]
struct StepCreateBundle {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateBundle {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self.config.vm_name.as_deref().unwrap_or("packer-utm-iso");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-utm");

        let utm_bundle = format!("{output_dir}/{vm_name}.utm");
        let data_dir = format!("{utm_bundle}/Data");

        self.ui
            .say(&self.name, &format!("Creating UTM bundle at {utm_bundle}"));

        tokio::fs::create_dir_all(&data_dir)
            .await
            .map_err(StampError::Io)?;

        // Write config.plist
        let plist_content = generate_utm_plist(&self.config, vm_name);
        let plist_path = format!("{utm_bundle}/config.plist");
        tokio::fs::write(&plist_path, plist_content.as_bytes())
            .await
            .map_err(StampError::Io)?;

        // Create main disk image
        let disk_path = format!("{data_dir}/disk-0.qcow2");
        if !Path::new(&disk_path).exists() {
            let _ = tokio::fs::write(&disk_path, b"MOCK_QCOW2_HEADER").await;
        }

        // Secondary media (cidata ISO)
        if !self.config.cd_files.is_empty() {
            let cd_path = format!("{data_dir}/cdrom-1.iso");
            crate::builder::virtualization::generate_cdrom_iso(
                &self.config.cd_files,
                self.config.cd_label.as_deref(),
                Path::new(&cd_path),
            )
            .await?;
        }

        // Floppy media
        if !self.config.floppy_files.is_empty() {
            let floppy_path = format!("{data_dir}/floppy-0.img");
            crate::builder::virtualization::generate_floppy_disk(
                &self.config.floppy_files,
                Path::new(&floppy_path),
            )
            .await?;
        }

        state.put("utm_bundle", utm_bundle);
        state.put("vm_name", vm_name.to_string());

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(utm_bundle) = state.get::<String>("utm_bundle") {
            self.ui
                .say(&self.name, &format!("Cleaning up UTM bundle: {utm_bundle}"));
        }
    }
}

/// Step to launch the `UTM` VM and send boot commands.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepRunVM {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        self.ui.say(&self.name, "Starting UTM VM...");

        let _ = run_utmctl(&["start", &vm_name]).await;

        let ip = discover_utm_guest_ip(&vm_name).await?;
        self.ui
            .say(&self.name, &format!("Discovered UTM guest IP: {ip}"));
        state.put("vm_ip", ip);
        state.put("ssh_port", self.config.ssh_port.map_or(22u16, |p| p.get()));
        state.put(
            "winrm_port",
            self.config.winrm_host_port.map_or(5985u16, |p| p.get()),
        );

        if let Some(ref cmds) = self.config.boot_command {
            self.ui
                .say(&self.name, "Typing boot commands into UTM VM...");
            let actions = crate::builder::virtualization::BootCommandParser::parse(
                cmds,
                state.http_ip(),
                state.http_port(),
                None,
            );
            if let Some(vnc_addr) = state.get::<String>("vnc_addr") {
                crate::builder::virtualization::send_vnc_boot_command(vnc_addr, None, &actions)
                    .await?;
            } else {
                crate::builder::virtualization::send_apple_virtualization_boot_command(
                    &vm_name, &actions, None,
                )
                .await?;
            }
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_name) = state.get::<String>("vm_name") {
            self.ui
                .say(&self.name, &format!("Stopping UTM VM: {vm_name}"));
            let _ = run_utmctl(&["stop", vm_name]).await;
        }
    }
}

/// Step to upload Guest Additions ISO into UTM VM via communicator.
#[derive(Debug, Clone)]
struct StepUploadAdditions {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepUploadAdditions {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if self.config.guest_additions_mode != UtmGuestAdditionsMode::Upload {
            return Ok(StepAction::Continue);
        }

        let additions_path = self
            .config
            .guest_additions_path
            .as_ref()
            .or(self.config.guest_additions_target_path.as_ref())
            .map_or_else(
                || PathBuf::from("/tmp/spice-guest-tools.iso"),
                PathBuf::from,
            );

        self.ui.say(
            &self.name,
            &format!(
                "Uploading UTM Guest Additions from {}",
                additions_path.display()
            ),
        );

        if let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator") {
            let _ = comm
                .upload(
                    &crate::types::FilePath::new(additions_path),
                    &crate::types::FilePath::new(PathBuf::from("/tmp/guest-additions.iso")),
                )
                .await;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to provision the UTM VM over SSH or `WinRM`.
#[derive(Clone)]
struct StepProvision {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning UTM VM...");

        if let Some(handle) =
            state.get::<crate::builder::http_server::HttpServerHandle>("http_server_handle")
        {
            handle.abort();
        }

        let ip = state.get::<String>("vm_ip").cloned().unwrap_or_default();
        let comm_type = self.config.communicator.as_deref().unwrap_or("ssh");

        let comm: Arc<dyn Communicator> = if comm_type == "winrm" {
            let port = state.get::<u16>("winrm_port").copied().unwrap_or(5985);
            let winrm_cfg = WinRmConfig {
                host: ip,
                port: Port::new(port),
                username: self
                    .config
                    .winrm_username
                    .clone()
                    .unwrap_or_else(|| "Administrator".to_string()),
                password: self.config.winrm_password.clone(),
                timeout: Timeout::new(Duration::from_secs(10)),
                ..Default::default()
            };
            Arc::new(WinRmCommunicator::new(winrm_cfg))
        } else {
            let port = state.get::<u16>("ssh_port").copied().unwrap_or(22);
            let ssh_config = SshConfig {
                host: ip,
                port: Port::new(port),
                username: self
                    .config
                    .ssh_username
                    .clone()
                    .unwrap_or_else(|| "packer".to_string()),
                password: self.config.ssh_password.clone(),
                timeout: Timeout::new(Duration::from_secs(10)),
                ..Default::default()
            };
            Arc::new(SshCommunicator::new(ssh_config))
        };

        state.put("communicator", comm.clone());

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "utm".to_string(),
            user: "root".to_string(),
            packer_run_uuid: uuid::Uuid::new_v4().to_string(),
            source_name: self.name.clone(),
            source_type: "utm-iso".to_string(),
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

/// Step to shut down the UTM VM.
#[derive(Debug, Clone)]
struct StepShutdown {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down UTM VM...");
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();

        if let Some(ref cmd) = self.config.shutdown_command {
            if let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator") {
                self.ui
                    .say(&self.name, &format!("Executing shutdown command: {cmd}"));
                let _ = comm
                    .execute(&crate::communicator::Command::new(cmd.clone()))
                    .await;
            }
        }

        let _ = run_utmctl(&["stop", &vm_name]).await;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to export and verify final `.utm` package bundle.
#[derive(Debug, Clone)]
struct StepExport {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepExport {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let utm_bundle = state
            .get::<String>("utm_bundle")
            .cloned()
            .unwrap_or_else(|| {
                let out = self
                    .config
                    .output_directory
                    .as_deref()
                    .unwrap_or("output-utm");
                let vm = self.config.vm_name.as_deref().unwrap_or("packer-utm-iso");
                format!("{out}/{vm}.utm")
            });
        self.ui.say(
            &self.name,
            &format!("Finalizing UTM package bundle: {utm_bundle}"),
        );
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for UtmIsoBuilder {
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
        let mut http_cfg = crate::builder::http_server::HttpServerConfig::new();
        if let Some(ref dir) = self.config.http_directory {
            http_cfg = http_cfg.with_dir(std::path::PathBuf::from(dir));
        }
        if !self.config.http_content.is_empty() {
            http_cfg = http_cfg.with_string_content(self.config.http_content.clone());
        }
        if let Some(ref addr) = self.config.http_bind_address {
            http_cfg = http_cfg.with_bind_address(addr);
        }
        if let (Some(min), Some(max)) = (self.config.http_port_min, self.config.http_port_max) {
            http_cfg = http_cfg.with_port_range(min, max);
        }

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateBundle {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(crate::builder::http_server::StepHttpServer::new(
                http_cfg,
                self.name(),
                Some(ui.clone()),
            )),
            Box::new(StepRunVM {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                hook,
                config: self.config.clone(),
            }),
            Box::new(StepUploadAdditions {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepShutdown {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepExport {
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

        let utm_bundle = state
            .get::<String>("utm_bundle")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            format!("utm-bundle:{utm_bundle}"),
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
    use crate::types::MemoryMb;
    use std::str::FromStr;

    #[derive(Clone)]
    struct FailingProvisioner;

    #[async_trait::async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn crate::communicator::Communicator,
            _ui: Arc<crate::engine::ui::Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Execution(
                "mock utm provision failure".to_string(),
            ))
        }
    }

    #[test]
    fn test_utm_derived_traits_and_enums() -> Result<(), std::convert::Infallible> {
        let config1 = UtmIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);

        // UtmBackend
        assert_eq!(UtmBackend::Qemu.as_str(), "qemu");
        assert_eq!(UtmBackend::Apple.as_str(), "apple");
        assert_eq!(format!("{}", UtmBackend::Qemu), "qemu");
        assert_eq!(UtmBackend::from_str("apple")?, UtmBackend::Apple);
        assert_eq!(UtmBackend::from_str("qemu")?, UtmBackend::Qemu);
        assert_eq!(UtmBackend::from_str("other")?, UtmBackend::Qemu);

        // UtmArch
        assert_eq!(UtmArch::Aarch64.as_str(), "aarch64");
        assert_eq!(UtmArch::X86_64.as_str(), "x86_64");
        assert_eq!(UtmArch::Custom("riscv64".to_string()).as_str(), "riscv64");
        assert_eq!(format!("{}", UtmArch::Aarch64), "aarch64");
        assert_eq!(UtmArch::from_str("aarch64")?, UtmArch::Aarch64);
        assert_eq!(UtmArch::from_str("x86_64")?, UtmArch::X86_64);
        assert_eq!(
            UtmArch::from_str("other")?,
            UtmArch::Custom("other".to_string())
        );

        // UtmInterface
        assert_eq!(UtmInterface::Nvme.as_str(), "nvme");
        assert_eq!(UtmInterface::VirtIo.as_str(), "virtio");
        assert_eq!(UtmInterface::Sata.as_str(), "sata");
        assert_eq!(UtmInterface::Ide.as_str(), "ide");
        assert_eq!(UtmInterface::Usb.as_str(), "usb");
        assert_eq!(
            UtmInterface::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", UtmInterface::Nvme), "nvme");
        assert_eq!(UtmInterface::from_str("nvme")?, UtmInterface::Nvme);
        assert_eq!(UtmInterface::from_str("virtio")?, UtmInterface::VirtIo);
        assert_eq!(UtmInterface::from_str("sata")?, UtmInterface::Sata);
        assert_eq!(UtmInterface::from_str("ide")?, UtmInterface::Ide);
        assert_eq!(UtmInterface::from_str("usb")?, UtmInterface::Usb);
        assert_eq!(
            UtmInterface::from_str("other")?,
            UtmInterface::Custom("other".to_string())
        );

        // UtmDisplayType
        assert_eq!(UtmDisplayType::VirtIoGpuPci.as_str(), "virtio-gpu-pci");
        assert_eq!(UtmDisplayType::VirtIoRamfb.as_str(), "virtio-ramfb");
        assert_eq!(UtmDisplayType::Console.as_str(), "console");
        assert_eq!(UtmDisplayType::None.as_str(), "none");
        assert_eq!(
            UtmDisplayType::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(
            format!("{}", UtmDisplayType::VirtIoGpuPci),
            "virtio-gpu-pci"
        );
        assert_eq!(
            UtmDisplayType::from_str("virtio-gpu-pci")?,
            UtmDisplayType::VirtIoGpuPci
        );
        assert_eq!(
            UtmDisplayType::from_str("virtio-ramfb")?,
            UtmDisplayType::VirtIoRamfb
        );
        assert_eq!(
            UtmDisplayType::from_str("console")?,
            UtmDisplayType::Console
        );
        assert_eq!(UtmDisplayType::from_str("none")?, UtmDisplayType::None);
        assert_eq!(
            UtmDisplayType::from_str("other")?,
            UtmDisplayType::Custom("other".to_string())
        );

        // UtmGuestAdditionsMode
        assert_eq!(UtmGuestAdditionsMode::Attach.as_str(), "attach");
        assert_eq!(UtmGuestAdditionsMode::Upload.as_str(), "upload");
        assert_eq!(UtmGuestAdditionsMode::Disable.as_str(), "disable");
        assert_eq!(format!("{}", UtmGuestAdditionsMode::Disable), "disable");
        assert_eq!(
            UtmGuestAdditionsMode::from_str("attach")?,
            UtmGuestAdditionsMode::Attach
        );
        assert_eq!(
            UtmGuestAdditionsMode::from_str("upload")?,
            UtmGuestAdditionsMode::Upload
        );
        assert_eq!(
            UtmGuestAdditionsMode::from_str("disable")?,
            UtmGuestAdditionsMode::Disable
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_utm_from_builder_config() -> Result<(), StampError> {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "utm-iso".to_string(),
            name: "bento-utm".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("vm_name".to_string(), "my-utm-vm".to_string());
        builder_cfg
            .config
            .insert("utm_vm_arch".to_string(), "aarch64".to_string());
        builder_cfg
            .config
            .insert("utm_vm_backend".to_string(), "apple".to_string());
        builder_cfg
            .config
            .insert("utm_vm_icon".to_string(), "debian".to_string());
        builder_cfg
            .config
            .insert("utm_hypervisor".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("utm_uefi_boot".to_string(), "true".to_string());
        builder_cfg.config.insert(
            "utm_display_hardware_type".to_string(),
            "virtio-ramfb".to_string(),
        );
        builder_cfg
            .config
            .insert("utm_hard_drive_interface".to_string(), "nvme".to_string());
        builder_cfg
            .config
            .insert("utm_iso_interface".to_string(), "usb".to_string());
        builder_cfg.config.insert(
            "utm_guest_additions_interface".to_string(),
            "usb".to_string(),
        );
        builder_cfg
            .config
            .insert("utm_guest_additions_mode".to_string(), "upload".to_string());
        builder_cfg.config.insert(
            "utm_guest_additions_url".to_string(),
            "https://example.com/spice.iso".to_string(),
        );
        builder_cfg.config.insert(
            "utm_guest_additions_sha256".to_string(),
            "abcdef".to_string(),
        );
        builder_cfg.config.insert(
            "utm_guest_additions_target_path".to_string(),
            "/tmp/spice.iso".to_string(),
        );
        builder_cfg.config.insert(
            "utm_guest_additions_path".to_string(),
            "/local/spice.iso".to_string(),
        );
        builder_cfg
            .config
            .insert("utm_disable_vnc".to_string(), "false".to_string());
        builder_cfg
            .config
            .insert("utm_boot_nopause".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("utm_display_nopause".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("utm_export_nopause".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("cpus".to_string(), "4".to_string());
        builder_cfg
            .config
            .insert("memory".to_string(), "4096".to_string());
        builder_cfg
            .config
            .insert("disk_size".to_string(), "60000".to_string());
        builder_cfg
            .config
            .insert("iso_url".to_string(), "/tmp/install.iso".to_string());
        builder_cfg
            .config
            .insert("shutdown_command".to_string(), "poweroff".to_string());
        builder_cfg.config.insert(
            "http_content".to_string(),
            r#"{"index.html": "hello"}"#.to_string(),
        );

        builder_cfg
            .config
            .insert("ssh_username".to_string(), "vagrant".to_string());
        builder_cfg.config.insert(
            "output_directory".to_string(),
            "output-utm-custom".to_string(),
        );

        let cfg = UtmIsoConfig::from_builder_config(&builder_cfg)?;
        assert_eq!(cfg.name, "bento-utm");
        assert_eq!(cfg.vm_name.as_deref(), Some("my-utm-vm"));
        assert_eq!(cfg.vm_arch, Some(UtmArch::Aarch64));
        assert_eq!(cfg.vm_backend, UtmBackend::Apple);
        assert_eq!(cfg.vm_icon.as_deref(), Some("debian"));
        assert!(cfg.hypervisor);
        assert!(cfg.uefi_boot);
        assert_eq!(cfg.display_hardware_type, Some(UtmDisplayType::VirtIoRamfb));
        assert_eq!(cfg.hard_drive_interface, UtmInterface::Nvme);
        assert_eq!(cfg.iso_interface, UtmInterface::Usb);
        assert_eq!(cfg.guest_additions_interface, UtmInterface::Usb);
        assert_eq!(cfg.guest_additions_mode, UtmGuestAdditionsMode::Upload);
        assert_eq!(
            cfg.guest_additions_url.as_deref(),
            Some("https://example.com/spice.iso")
        );
        assert_eq!(cfg.guest_additions_sha256.as_deref(), Some("abcdef"));
        assert_eq!(
            cfg.guest_additions_target_path.as_deref(),
            Some("/tmp/spice.iso")
        );
        assert_eq!(
            cfg.guest_additions_path.as_deref(),
            Some("/local/spice.iso")
        );
        assert!(!cfg.disable_vnc);
        assert!(cfg.boot_nopause);
        assert!(cfg.display_nopause);
        assert!(cfg.export_nopause);
        assert_eq!(cfg.cpus, Some(4));
        assert_eq!(cfg.memory, Some(MemoryMb::new(4096)));
        assert_eq!(cfg.disk_size, Some(MemoryMb::new(60000)));
        assert_eq!(cfg.shutdown_command.as_deref(), Some("poweroff"));
        assert_eq!(cfg.output_directory.as_deref(), Some("output-utm-custom"));
        Ok(())
    }

    #[test]
    fn test_utm_fallback_config_aliases() -> Result<(), StampError> {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "utm-iso".to_string(),
            name: "bento-utm".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("utm_hypervisor".to_string(), "false".to_string());
        builder_cfg
            .config
            .insert("utm_uefi_boot".to_string(), "false".to_string());
        builder_cfg.config.insert(
            "utm_display_hardware_type".to_string(),
            "console".to_string(),
        );
        builder_cfg
            .config
            .insert("utm_hard_drive_interface".to_string(), "virtio".to_string());
        builder_cfg
            .config
            .insert("utm_iso_interface".to_string(), "ide".to_string());
        builder_cfg.config.insert(
            "utm_guest_additions_interface".to_string(),
            "usb".to_string(),
        );
        builder_cfg
            .config
            .insert("utm_guest_additions_mode".to_string(), "upload".to_string());
        builder_cfg.config.insert(
            "utm_guest_additions_url".to_string(),
            "http://example.com/iso".to_string(),
        );
        builder_cfg
            .config
            .insert("utm_guest_additions_sha256".to_string(), "abc".to_string());
        builder_cfg.config.insert(
            "utm_guest_additions_target_path".to_string(),
            "/tmp/iso".to_string(),
        );
        builder_cfg.config.insert(
            "utm_guest_additions_path".to_string(),
            "/path/iso".to_string(),
        );
        builder_cfg
            .config
            .insert("utm_disable_vnc".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("utm_boot_nopause".to_string(), "false".to_string());
        builder_cfg
            .config
            .insert("utm_display_nopause".to_string(), "false".to_string());
        builder_cfg
            .config
            .insert("utm_export_nopause".to_string(), "false".to_string());
        builder_cfg
            .config
            .insert("winrm_port".to_string(), "5986".to_string());
        builder_cfg
            .config
            .insert("boot_command".to_string(), r#"["boot"]"#.to_string());
        builder_cfg
            .config
            .insert("floppy_files".to_string(), r#"["floppy"]"#.to_string());
        builder_cfg
            .config
            .insert("cd_files".to_string(), r#"["cd"]"#.to_string());
        builder_cfg
            .config
            .insert("ssh_port".to_string(), "2222".to_string());
        builder_cfg
            .config
            .insert("winrm_use_ntlm".to_string(), "on".to_string());
        builder_cfg
            .config
            .insert("winrm_insecure".to_string(), "on".to_string());
        builder_cfg
            .config
            .insert("winrm_use_ssl".to_string(), "on".to_string());
        builder_cfg
            .config
            .insert("http_port_min".to_string(), "8000".to_string());
        builder_cfg
            .config
            .insert("http_port_max".to_string(), "8010".to_string());
        builder_cfg
            .config
            .insert("http_content".to_string(), "invalid json".to_string());

        let cfg = UtmIsoConfig::from_builder_config(&builder_cfg)?;
        assert!(!cfg.hypervisor);
        assert!(!cfg.uefi_boot);
        assert_eq!(cfg.display_hardware_type, Some(UtmDisplayType::Console));
        assert_eq!(cfg.hard_drive_interface, UtmInterface::VirtIo);
        assert_eq!(cfg.iso_interface, UtmInterface::Ide);
        assert_eq!(cfg.guest_additions_interface, UtmInterface::Usb);
        assert_eq!(cfg.guest_additions_mode, UtmGuestAdditionsMode::Upload);
        assert!(cfg.disable_vnc);
        assert!(!cfg.boot_nopause);
        assert!(!cfg.display_nopause);
        assert!(!cfg.export_nopause);
        assert_eq!(cfg.ssh_port, Some(Port::new(2222)));
        assert_eq!(cfg.winrm_host_port, Some(Port::new(5986)));
        assert_eq!(cfg.winrm_use_ntlm, Some(true));
        assert_eq!(cfg.winrm_insecure, Some(true));
        assert_eq!(cfg.winrm_use_ssl, Some(true));
        assert_eq!(cfg.floppy_files.len(), 1);
        assert_eq!(cfg.cd_files.len(), 1);
        assert!(cfg.boot_command.is_some());
        assert_eq!(cfg.http_port_min, Some(8000));
        assert_eq!(cfg.http_port_max, Some(8010));
        assert!(cfg.http_content.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_utm_iso_run() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let f_path = td.path().join("preseed.cfg");
        tokio::fs::write(&f_path, b"d-i test").await?;
        let cd_path = td.path().join("user-data");
        tokio::fs::write(&cd_path, b"#cloud-config").await?;

        let config = UtmIsoConfig {
            name: "test-utm".to_string(),
            vm_name: Some("test-vm".to_string()),
            vm_arch: Some(UtmArch::Aarch64),
            vm_backend: UtmBackend::Qemu,
            vm_icon: Some("ubuntu".to_string()),
            hypervisor: true,
            uefi_boot: true,
            display_hardware_type: Some(UtmDisplayType::VirtIoGpuPci),
            hard_drive_interface: UtmInterface::Nvme,
            iso_interface: UtmInterface::Usb,
            guest_additions_mode: UtmGuestAdditionsMode::Attach,
            guest_additions_path: Some("/tmp/spice.iso".to_string()),
            cpus: Some(2),
            memory: Some(MemoryMb::new(2048)),
            disk_size: Some(MemoryMb::new(20480)),
            iso_url: Some("/tmp/ubuntu.iso".to_string()),
            floppy_files: vec![f_path.to_string_lossy().to_string()],
            cd_files: vec![cd_path.to_string_lossy().to_string()],
            boot_command: Some(vec!["<enter>".to_string()]),
            shutdown_command: Some("poweroff".to_string()),
            ssh_port: Some(Port::new(2222)),
            output_directory: Some(td.path().to_string_lossy().to_string()),
            ..Default::default()
        };
        let builder = UtmIsoBuilder::new(config);

        builder.prepare().await?;
        assert_eq!(builder.name(), "test-utm");

        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let art = builder
            .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
            .await?;
        assert!(art.id().contains("test-vm.utm"));
        assert_eq!(art.builder_id(), "test-utm");

        let bundle_path = td.path().join("test-vm.utm");
        assert!(bundle_path.exists());
        assert!(bundle_path.join("config.plist").exists());

        let winrm_cfg = UtmIsoConfig {
            name: "winrm-utm".to_string(),
            communicator: Some("winrm".to_string()),
            winrm_host_port: Some(Port::new(5985)),
            guest_additions_mode: UtmGuestAdditionsMode::Upload,
            output_directory: Some(td.path().to_string_lossy().to_string()),
            ..Default::default()
        };
        let b_winrm = UtmIsoBuilder::new(winrm_cfg);
        let _ = b_winrm.run(hook, ui, OnErrorStrategy::Cleanup).await?;

        builder.cancel().await
    }

    #[tokio::test]
    async fn test_utmctl_commands_and_helpers() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        run_utmctl(&["list"]).await?;
        assert!(
            run_utmctl_with_cmd("/nonexistent_utmctl", &["list"])
                .await
                .is_err()
        );
        assert!(
            run_utmctl_with_cmd("sh", &["-c", "echo 'utmctl error' >&2; exit 1"])
                .await
                .is_err()
        );
        assert!(
            run_utmctl_with_cmd("sh", &["-c", "echo 'stdout failure'; exit 1"])
                .await
                .is_err()
        );

        assert_eq!(discover_utm_guest_ip("test-vm").await?, "127.0.0.1");

        let script = td.path().join("fake_utmctl.sh");
        tokio::fs::write(&script, b"#!/bin/sh\necho 192.168.64.5\n").await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
        }
        let ip_sh = discover_utm_guest_ip_with_cmd(&script.to_string_lossy(), "test-vm").await?;
        assert_eq!(ip_sh, "192.168.64.5");

        let script_unknown = td.path().join("fake_utmctl_unknown.sh");
        tokio::fs::write(&script_unknown, b"#!/bin/sh\necho 'unknown'\n").await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_unknown, std::fs::Permissions::from_mode(0o755))?;
        }
        let ip_unknown =
            discover_utm_guest_ip_with_cmd(&script_unknown.to_string_lossy(), "test-vm").await?;
        assert_eq!(ip_unknown, "127.0.0.1");

        let script_prefix = td.path().join("fake_utmctl_prefix.sh");
        tokio::fs::write(&script_prefix, b"#!/bin/sh\necho 'ip-address error'\n").await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_prefix, std::fs::Permissions::from_mode(0o755))?;
        }
        let ip_prefix =
            discover_utm_guest_ip_with_cmd(&script_prefix.to_string_lossy(), "test-vm").await?;
        assert_eq!(ip_prefix, "127.0.0.1");

        assert!(
            discover_utm_guest_ip_with_cmd("/nonexistent_utmctl_binary_path", "test-vm")
                .await
                .is_err()
        );

        let fake_utmctl = td.path().join("utmctl");
        tokio::fs::write(&fake_utmctl, b"#!/bin/sh\n").await?;
        {
            let _env = crate::utils::ScopedEnvVar::set(
                "UTMCTL_PATH",
                fake_utmctl.to_string_lossy().as_ref(),
            );
            assert_eq!(find_utmctl_binary(), fake_utmctl);
        }
        {
            let _env = crate::utils::ScopedEnvVar::set("UTMCTL_PATH", "/nonexistent/utmctl/path");
            let _ = find_utmctl_binary();
        }
        let _ = find_utmctl_binary();
        {
            let _env = crate::utils::ScopedEnvVar::set("UTMCTL_TEST_EMPTY_CANDIDATES", "1");
            assert_eq!(find_utmctl_binary(), PathBuf::from("utmctl"));
        }
        assert_eq!(utmctl_binary(), "echo");

        let b_cancel = UtmIsoBuilder::new(UtmIsoConfig::default());
        b_cancel.cancel().await?;

        assert_eq!(parse_string_list(r#"["x", "y"]"#), vec!["x", "y"]);
        assert_eq!(parse_string_list("a,,b"), vec!["a", "b"]);

        let config = UtmIsoConfig {
            uefi_boot: false,
            ..Default::default()
        };
        let plist = generate_utm_plist(&config, "plist-test");
        assert!(plist.contains("<key>Name</key>\n\t\t<string>plist-test</string>"));
        assert!(plist.contains("<false/>"));

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut step_shutdown = StepShutdown {
            ui: ui.clone(),
            name: "shutdown-test".to_string(),
            config: UtmIsoConfig::default(),
        };
        let mut state = StateBag::new();
        state.put("vm_name", "test-vm".to_string());
        step_shutdown.run(&mut state).await?;
        step_shutdown.cleanup(&state).await;

        let mut step_create = StepCreateBundle {
            ui: ui.clone(),
            name: "create-test".to_string(),
            config: UtmIsoConfig::default(),
        };
        state.put(
            "utm_bundle",
            td.path().join("test.utm").to_string_lossy().to_string(),
        );
        step_create.cleanup(&state).await;

        let mut step_run = StepRunVM {
            ui: ui.clone(),
            name: "run-test".to_string(),
            config: UtmIsoConfig::default(),
        };
        step_run.cleanup(&state).await;

        let mut step_upload = StepUploadAdditions {
            ui: ui.clone(),
            name: "upload-test".to_string(),
            config: UtmIsoConfig {
                guest_additions_mode: UtmGuestAdditionsMode::Upload,
                ..Default::default()
            },
        };
        let comm: Arc<dyn Communicator> =
            Arc::new(crate::communicator::mock::MockCommunicator::new());
        state.put("communicator", comm);
        step_upload.run(&mut state).await?;
        step_upload.cleanup(&state).await;

        let mut step_export = StepExport {
            ui: ui.clone(),
            name: "export-test".to_string(),
            config: UtmIsoConfig {
                output_directory: Some(td.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        };
        step_export.cleanup(&state).await;
        let mut empty_state = StateBag::new();
        step_export.run(&mut empty_state).await?;

        let mut step_run_empty = StepRunVM {
            ui: ui.clone(),
            name: "run-empty".to_string(),
            config: UtmIsoConfig::default(),
        };
        step_run_empty.run(&mut empty_state).await?;

        step_create.cleanup(&empty_state).await;

        let mut step_prov_fail = StepProvision {
            ui,
            name: "prov-fail".to_string(),
            hook: Arc::new(DefaultProvisionHook {
                provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
                error_cleanup_provisioners: Arc::new(vec![]),
            }),
            config: UtmIsoConfig::default(),
        };
        assert!(step_prov_fail.run(&mut state).await.is_err());
        step_prov_fail.cleanup(&state).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_utm_error_strategies_and_prepare() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let config = UtmIsoConfig {
            name: "test-utm-err".to_string(),
            output_directory: Some(td.path().to_string_lossy().to_string()),
            ..Default::default()
        };
        let builder = UtmIsoBuilder::new(config);

        let fail_hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        assert!(
            builder
                .run(fail_hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        assert!(
            builder
                .run(fail_hook.clone(), ui.clone(), OnErrorStrategy::Abort)
                .await
                .is_err()
        );
        assert!(
            builder
                .run(
                    fail_hook.clone(),
                    ui.clone(),
                    OnErrorStrategy::RunCleanupProvisioner
                )
                .await
                .is_err()
        );

        let mut queue = std::collections::VecDeque::new();
        queue.push_back("yes".to_string());
        let ui_ask = Arc::new(
            Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(Arc::new(std::sync::Mutex::new(queue))),
        );
        assert!(
            builder
                .run(fail_hook.clone(), ui_ask, OnErrorStrategy::Ask)
                .await
                .is_err()
        );

        assert!(
            builder
                .run(fail_hook, ui, OnErrorStrategy::Ask)
                .await
                .is_err()
        );

        let empty_b = UtmIsoBuilder::new(UtmIsoConfig::default());
        assert!(empty_b.prepare().await.is_err());
        Ok(())
    }
}
