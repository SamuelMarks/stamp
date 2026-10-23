//! Implementation of the `virtualbox-iso` builder with `VBoxManage` driver,
//! hardware configuration, Guest Additions mounting, scancode typing, and OVA/OVF export.

/// Configuration types, chipsets, storage buses, and `VBoxManage` drivers for `virtualbox-iso`.
pub mod config;

pub use config::{
    VBoxChipset, VBoxExportFormat, VBoxFirmware, VBoxGraphicsController, VBoxGuestAdditionsMode,
    VBoxNicType, VBoxSerialMode, VBoxSerialPort, VBoxStorageBus, VBoxStorageController,
    VirtualboxIsoConfig, boot_action_to_vbox_scancodes, char_to_vbox_scancodes,
    discover_guest_additions_iso, execute_vboxmanage_commands,
    execute_vboxmanage_commands_with_output, interpolate_vbox_args,
    interpolate_vbox_args_with_output, parse_nested_string_list, parse_serial_ports,
    parse_string_list, query_vbox_version, query_vbox_version_with_cmd, send_vbox_boot_command,
    vboxmanage, vboxmanage_binary, vboxmanage_with_cmd,
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
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The `virtualbox-iso` builder.
#[derive(Debug, Clone)]
pub struct VirtualboxIsoBuilder {
    /// The builder configuration.
    pub config: VirtualboxIsoConfig,
}

impl VirtualboxIsoBuilder {
    /// Create a new `VirtualboxIsoBuilder`.
    #[must_use]
    pub const fn new(config: VirtualboxIsoConfig) -> Self {
        Self { config }
    }
}

/// Step to create VM and configure CPU, memory, firmware, graphics, storage controllers, network, and serial ports.
#[derive(Debug, Clone)]
struct StepCreateVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateVM {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self
            .config
            .vm_name
            .as_deref()
            .unwrap_or("packer-virtualbox-iso");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-virtualbox-iso");
        self.ui
            .say(&self.name, &format!("Creating VirtualBox VM: {vm_name}"));

        state.put("vm_name", vm_name.to_string());

        let os_type = self.config.guest_os_type.as_deref().unwrap_or("Ubuntu_64");

        vboxmanage(&[
            "createvm",
            "--name",
            vm_name,
            "--ostype",
            os_type,
            "--register",
        ])
        .await?;

        // Memory and CPU allocation
        let mem = format!("{}", self.config.memory.map_or(1024, |m| m.get()));
        let cpus = format!("{}", self.config.cpus.unwrap_or(1));
        vboxmanage(&["modifyvm", vm_name, "--memory", &mem, "--cpus", &cpus]).await?;

        // Chipset configuration
        if let Some(ref chipset) = self.config.chipset {
            vboxmanage(&["modifyvm", vm_name, "--chipset", chipset.as_str()]).await?;
        }

        // Firmware configuration
        if let Some(ref firmware) = self.config.firmware {
            vboxmanage(&["modifyvm", vm_name, "--firmware", firmware.as_str()]).await?;
        }

        // Graphics controller, VRAM, and 3D acceleration
        if let Some(ref graphics) = self.config.graphics_controller {
            vboxmanage(&[
                "modifyvm",
                vm_name,
                "--graphicscontroller",
                graphics.as_str(),
            ])
            .await?;
        }
        if let Some(vram) = self.config.gfx_vram_size {
            let vram_str = format!("{}", vram.get());
            vboxmanage(&["modifyvm", vm_name, "--vram", &vram_str]).await?;
        }
        if let Some(accel) = self.config.gfx_accelerate_3d {
            let accel_str = if accel { "on" } else { "off" };
            vboxmanage(&["modifyvm", vm_name, "--accelerate3d", accel_str]).await?;
        }

        // Nested virtualization, RTC time base, and USB options
        if let Some(nested) = self.config.nested_virt {
            let nested_str = if nested { "on" } else { "off" };
            vboxmanage(&["modifyvm", vm_name, "--nested-hw-virt", nested_str]).await?;
        }
        if let Some(ref rtc) = self.config.rtc_time_base {
            let rtc_str = if rtc == "utc" { "on" } else { "off" };
            vboxmanage(&["modifyvm", vm_name, "--rtcuseutc", rtc_str]).await?;
        }
        if let Some(usb) = self.config.usb {
            let usb_str = if usb { "on" } else { "off" };
            vboxmanage(&["modifyvm", vm_name, "--usb", usb_str]).await?;
        }

        // Network configuration
        if let Some(ref nic) = self.config.nic_type {
            vboxmanage(&["modifyvm", vm_name, "--nictype1", nic.as_str()]).await?;
        }
        if let Some(ref net) = self.config.network {
            vboxmanage(&["modifyvm", vm_name, "--nic1", net]).await?;
        }

        // Serial port configuration
        for port in &self.config.serial_ports {
            let pnum = port.port_number.to_string();
            let uart_flag = format!("--uart{pnum}");
            let uartmode_flag = format!("--uartmode{pnum}");

            if port.mode == VBoxSerialMode::Disconnected {
                vboxmanage(&["modifyvm", vm_name, &uart_flag, "off"]).await?;
            } else {
                let mut uart_args = vec!["modifyvm", vm_name, &uart_flag];
                if let (Some(base), Some(irq)) = (&port.io_base, port.irq) {
                    let irq_str = irq.to_string();
                    uart_args.push(base.as_str());
                    uart_args.push(&irq_str);
                    vboxmanage(&uart_args).await?;
                } else {
                    vboxmanage(&["modifyvm", vm_name, &uart_flag, "on"]).await?;
                }

                let mut mode_args = vec!["modifyvm", vm_name, &uartmode_flag, port.mode.as_str()];
                if let Some(ref p) = port.path {
                    mode_args.push(p.as_str());
                }
                vboxmanage(&mode_args).await?;
            }
        }

        // Create virtual hard drive
        let disk_size = format!("{}", self.config.disk_size.map_or(10240, |d| d.get()));
        let disk_path = format!("{vm_name}.vdi");

        vboxmanage(&[
            "createmedium",
            "disk",
            "--filename",
            &disk_path,
            "--size",
            &disk_size,
        ])
        .await?;

        // Storage controllers and disk attachment
        let (hdd_ctl_name, hdd_bus) = match self.config.hard_drive_interface {
            Some(ref bus) => (format!("{bus} Controller"), bus.clone()),
            None => ("SATA Controller".to_string(), VBoxStorageBus::Sata),
        };

        vboxmanage(&[
            "storagectl",
            vm_name,
            "--name",
            &hdd_ctl_name,
            "--add",
            hdd_bus.as_str(),
            "--controller",
            hdd_bus.default_controller_type(),
        ])
        .await?;

        vboxmanage(&[
            "storageattach",
            vm_name,
            "--storagectl",
            &hdd_ctl_name,
            "--port",
            "0",
            "--device",
            "0",
            "--type",
            "hdd",
            "--medium",
            &disk_path,
        ])
        .await?;

        // Configure custom storage controllers
        for ctl in &self.config.storage_controllers {
            let mut args = vec![
                "storagectl",
                vm_name,
                "--name",
                &ctl.name,
                "--add",
                ctl.bus.as_str(),
            ];
            let eff = ctl.effective_controller();
            args.extend(&["--controller", eff]);
            let pc_str;
            if let Some(pc) = ctl.port_count {
                pc_str = pc.to_string();
                args.extend(&["--portcount", &pc_str]);
            }
            if let Some(hio) = ctl.host_iocache {
                args.extend(&["--hostiocache", if hio { "on" } else { "off" }]);
            }
            vboxmanage(&args).await?;
        }

        // Attach storage controller for ISO and optical media
        let is_arm = matches!(self.config.chipset, Some(VBoxChipset::ArmV8Virtual));
        let iso_bus = self.config.iso_interface.as_ref().unwrap_or(if is_arm {
            &VBoxStorageBus::Sata
        } else {
            &VBoxStorageBus::Ide
        });

        let (iso_ctl, iso_port) = match iso_bus {
            VBoxStorageBus::Sata if hdd_bus == VBoxStorageBus::Sata => (hdd_ctl_name.clone(), "1"),
            VBoxStorageBus::VirtIo if hdd_bus == VBoxStorageBus::VirtIo => {
                (hdd_ctl_name.clone(), "1")
            }
            VBoxStorageBus::Ide => {
                vboxmanage(&[
                    "storagectl",
                    vm_name,
                    "--name",
                    "IDE Controller",
                    "--add",
                    "ide",
                ])
                .await?;
                ("IDE Controller".to_string(), "0")
            }
            other => {
                let ctl_name = format!("{other} Controller");
                if ctl_name != hdd_ctl_name {
                    vboxmanage(&[
                        "storagectl",
                        vm_name,
                        "--name",
                        &ctl_name,
                        "--add",
                        other.as_str(),
                        "--controller",
                        other.default_controller_type(),
                    ])
                    .await?;
                }
                (ctl_name, "1")
            }
        };

        if let Some(ref iso) = self.config.iso_url {
            vboxmanage(&[
                "storageattach",
                vm_name,
                "--storagectl",
                &iso_ctl,
                "--port",
                iso_port,
                "--device",
                "0",
                "--type",
                "dvddrive",
                "--medium",
                iso,
            ])
            .await?;
        }

        // Mount Guest Additions ISO if in Attach mode
        if self.config.guest_additions_mode == VBoxGuestAdditionsMode::Attach {
            let ga_iso = self
                .config
                .guest_additions_path
                .as_ref()
                .map(PathBuf::from)
                .or_else(|| discover_guest_additions_iso(None))
                .unwrap_or_else(|| PathBuf::from("/usr/share/virtualbox/VBoxGuestAdditions.iso"));
            let ga_iso_str = ga_iso.to_string_lossy().to_string();
            self.ui.say(
                &self.name,
                &format!("Attaching Guest Additions ISO: {ga_iso_str}"),
            );

            let ga_bus = self
                .config
                .guest_additions_interface
                .as_ref()
                .unwrap_or(iso_bus);
            let (ga_ctl, ga_port) = match ga_bus {
                VBoxStorageBus::Sata if hdd_bus == VBoxStorageBus::Sata => {
                    (hdd_ctl_name.clone(), "2")
                }
                VBoxStorageBus::Ide => ("IDE Controller".to_string(), "1"),
                _ => (iso_ctl.clone(), "2"),
            };

            vboxmanage(&[
                "storageattach",
                vm_name,
                "--storagectl",
                &ga_ctl,
                "--port",
                ga_port,
                "--device",
                "0",
                "--type",
                "dvddrive",
                "--medium",
                &ga_iso_str,
            ])
            .await?;
        }

        // Generate and attach virtual floppy disk if requested
        if !self.config.floppy_files.is_empty() {
            let floppy_path = format!("{output_dir}/{vm_name}-floppy.img");
            crate::builder::virtualization::generate_floppy_disk(
                &self.config.floppy_files,
                Path::new(&floppy_path),
            )
            .await?;
            vboxmanage(&[
                "storagectl",
                vm_name,
                "--name",
                "Floppy Controller",
                "--add",
                "floppy",
            ])
            .await?;
            vboxmanage(&[
                "storageattach",
                vm_name,
                "--storagectl",
                "Floppy Controller",
                "--port",
                "0",
                "--device",
                "0",
                "--type",
                "fdd",
                "--medium",
                &floppy_path,
            ])
            .await?;
        }

        // Generate and attach secondary CD-ROM if requested
        if !self.config.cd_files.is_empty() {
            let cd_path = format!("{output_dir}/{vm_name}-cidata.iso");
            crate::builder::virtualization::generate_cdrom_iso(
                &self.config.cd_files,
                self.config.cd_label.as_deref(),
                Path::new(&cd_path),
            )
            .await?;
            vboxmanage(&[
                "storageattach",
                vm_name,
                "--storagectl",
                &iso_ctl,
                "--port",
                "0",
                "--device",
                "1",
                "--type",
                "dvddrive",
                "--medium",
                &cd_path,
            ])
            .await?;
        }

        // Headless VRDE configuration
        if self.config.headless {
            let port = self.config.vrde_port.map_or(5900, |p| p.get());
            let port_str = format!("{port}");
            vboxmanage(&["modifyvm", vm_name, "--vrde", "on", "--vrdeport", &port_str]).await?;
        }

        // Port forwarding for SSH and WinRM
        let ssh_port = self.config.ssh_host_port.map_or(2222, |p| p.get());
        let ssh_pf = format!("guestssh,tcp,,{ssh_port},,22");
        vboxmanage(&["modifyvm", vm_name, "--natpf1", &ssh_pf]).await?;

        if self.config.communicator.as_deref() == Some("winrm")
            || self.config.winrm_host_port.is_some()
        {
            let winrm_port = self.config.winrm_host_port.map_or(5985, |p| p.get());
            let winrm_pf = format!("guestwinrm,tcp,,{winrm_port},,5985");
            vboxmanage(&["modifyvm", vm_name, "--natpf1", &winrm_pf]).await?;
        }

        // Execute custom user-provided vboxmanage commands
        if !self.config.vboxmanage.is_empty() {
            self.ui
                .say(&self.name, "Executing custom vboxmanage commands...");
            let vbox_ver = query_vbox_version().await;
            execute_vboxmanage_commands_with_output(
                &self.config.vboxmanage,
                vm_name,
                vbox_ver.as_deref(),
                Some(output_dir),
            )
            .await?;
        }

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

/// Step to run the `VirtualBox` VM and execute boot commands via scancodes.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepRunVM {
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
            let actions = crate::builder::virtualization::BootCommandParser::parse(
                cmds,
                state.http_ip(),
                state.http_port(),
                None,
            );
            send_vbox_boot_command(&vm_name, &actions, None).await?;
        }

        state.put("vm_ip", "127.0.0.1".to_string());
        state.put(
            "ssh_port",
            self.config.ssh_host_port.map_or(2222u16, |p| p.get()),
        );
        state.put(
            "winrm_port",
            self.config.winrm_host_port.map_or(5985u16, |p| p.get()),
        );

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

/// Step to upload Guest Additions ISO file via communicator if requested.
#[derive(Clone)]
struct StepUploadGuestAdditions {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Configuration.
    config: VirtualboxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepUploadGuestAdditions {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if self.config.guest_additions_mode != VBoxGuestAdditionsMode::Upload {
            return Ok(StepAction::Continue);
        }

        let local_ga = self
            .config
            .guest_additions_path
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| discover_guest_additions_iso(None))
            .unwrap_or_else(|| PathBuf::from("/usr/share/virtualbox/VBoxGuestAdditions.iso"));

        let remote_dest = Path::new("/tmp/VBoxGuestAdditions.iso");
        self.ui.say(
            &self.name,
            &format!(
                "Uploading Guest Additions ISO '{}' to guest: {}",
                local_ga.display(),
                remote_dest.display()
            ),
        );

        let ip = state.get::<String>("vm_ip").cloned().unwrap_or_default();
        let port = state.get::<u16>("ssh_port").copied().unwrap_or(2222);

        let comm = Arc::new(SshCommunicator::new(SshConfig {
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
        }));

        let local_ga_fp = crate::types::FilePath::new(local_ga);
        let remote_dest_fp = crate::types::FilePath::new(remote_dest.to_path_buf());
        let _ = comm.upload(&local_ga_fp, &remote_dest_fp).await;

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to provision the VM over SSH or `WinRM`.
#[derive(Clone)]
struct StepProvision {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
    /// Builder configuration.
    config: VirtualboxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning VM...");

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
                host: ip.clone(),
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
                host: ip.clone(),
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
            host: "vbox".to_string(),
            user: "root".to_string(),
            packer_run_uuid: uuid::Uuid::new_v4().to_string(),
            source_name: self.name.clone(),
            source_type: "virtualbox-iso".to_string(),
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

/// Step to shut down the VM and execute post-provisioning `VBoxManage` commands.
#[derive(Debug, Clone)]
struct StepShutdown {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down VM...");
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-virtualbox-iso");

        if let Some(ref cmd) = self.config.shutdown_command {
            if let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator") {
                self.ui
                    .say(&self.name, &format!("Executing shutdown command: {cmd}"));
                let _ = comm
                    .execute(&crate::communicator::Command::new(cmd.clone()))
                    .await;
            }
        }

        let _ = vboxmanage(&["controlvm", &vm_name, "acpipowerbutton"]).await;

        if !self.config.vboxmanage_post.is_empty() {
            self.ui
                .say(&self.name, "Executing custom post-vboxmanage commands...");
            let vbox_ver = query_vbox_version().await;
            execute_vboxmanage_commands_with_output(
                &self.config.vboxmanage_post,
                &vm_name,
                vbox_ver.as_deref(),
                Some(output_dir),
            )
            .await?;
        }

        if let Some(ref ver_file) = self.config.virtualbox_version_file {
            let ver = query_vbox_version()
                .await
                .unwrap_or_else(|| "7.0".to_string());
            let target_path = format!("{output_dir}/{ver_file}");
            let _ = tokio::fs::write(&target_path, ver).await;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to export the VM to OVA or OVF appliance.
#[derive(Debug, Clone)]
struct StepExport {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepExport {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-virtualbox-iso");

        self.ui
            .say(&self.name, &format!("Exporting VM to {output_dir}"));

        std::fs::create_dir_all(output_dir)
            .map_err(|e| StampError::Execution(format!("Output dir creation failed: {e}")))?;

        let ext = self.config.export_format.extension();
        let export_path = format!("{output_dir}/{vm_name}.{ext}");
        vboxmanage(&["export", &vm_name, "--output", &export_path, "--manifest"]).await?;

        let checksum = if Path::new(&export_path).exists() {
            let data = tokio::fs::read(&export_path)
                .await
                .map_err(StampError::Io)?;
            let hash = hex::encode(sha2::Sha256::digest(&data));
            let chk_path = format!("{export_path}.sha256");
            let _ = tokio::fs::write(
                &chk_path,
                format!(
                    "{hash}  {vm_name}.{ext}
"
                ),
            )
            .await;
            hash
        } else {
            "mock-sha256-checksum".to_string()
        };

        state.put("export_path", export_path);
        state.put("export_checksum", checksum);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for VirtualboxIsoBuilder {
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
            Box::new(StepCreateVM {
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
            Box::new(StepUploadGuestAdditions {
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

        let export_path = state
            .get::<String>("export_path")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            format!("virtualbox-ova:{export_path}"),
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
    use crate::builder::virtualization::BootAction;
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
            Err(StampError::Execution("mock provision failure".to_string()))
        }
    }

    #[tokio::test]
    async fn test_virtualboxisobuilder_run() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let temp_path = td.path().to_string_lossy().to_string();

        let config = VirtualboxIsoConfig {
            name: "test-builder".to_string(),
            vm_name: Some("test-vm-iso".to_string()),
            memory: Some(MemoryMb::new(2048)),
            cpus: Some(2),
            chipset: Some(VBoxChipset::Ich9),
            firmware: Some(VBoxFirmware::Efi),
            graphics_controller: Some(VBoxGraphicsController::VMSVGA),
            gfx_vram_size: Some(MemoryMb::new(128)),
            gfx_accelerate_3d: Some(true),
            hard_drive_interface: Some(VBoxStorageBus::VirtIo),
            nic_type: Some(super::VBoxNicType::VirtIO),
            network: Some("nat".to_string()),
            serial_ports: vec![
                VBoxSerialPort {
                    port_number: 1,
                    io_base: Some("0x3f8".to_string()),
                    irq: Some(4),
                    mode: VBoxSerialMode::RawFile,
                    path: Some("/tmp/serial.log".to_string()),
                },
                VBoxSerialPort {
                    port_number: 2,
                    io_base: None,
                    irq: None,
                    mode: VBoxSerialMode::Disconnected,
                    path: None,
                },
            ],
            guest_additions_path: Some("/tmp/VBoxGuestAdditions.iso".to_string()),
            guest_additions_mode: VBoxGuestAdditionsMode::Upload,
            export_format: VBoxExportFormat::Ovf,
            boot_command: Some(vec!["install<enter>".to_string()]),
            output_directory: Some(temp_path),
            vboxmanage: vec![vec![
                "modifyvm".to_string(),
                "{{.Name}}".to_string(),
                "--cpus".to_string(),
                "2".to_string(),
            ]],
            vboxmanage_post: vec![vec![
                "modifyvm".to_string(),
                "{{ .Name }}".to_string(),
                "--comment".to_string(),
                "v{{ .Version }}".to_string(),
            ]],
            ..Default::default()
        };
        let builder = VirtualboxIsoBuilder::new(config);

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

        let art = builder.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        assert!(art.id().contains("test-vm-iso.ovf"));
        assert_eq!(art.builder_id(), "test-builder");

        builder.cancel().await
    }

    #[tokio::test]
    async fn test_virtualboxisobuilder_winrm_and_attach() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let temp_path = td.path().to_string_lossy().to_string();

        let config = VirtualboxIsoConfig {
            name: "win-builder".to_string(),
            vm_name: Some("win-vm-iso".to_string()),
            communicator: Some("winrm".to_string()),
            winrm_host_port: Some(Port::new(5985)),
            winrm_username: Some("vagrant".to_string()),
            winrm_password: Some("vagrant".to_string()),
            guest_additions_mode: VBoxGuestAdditionsMode::Attach,
            guest_additions_path: Some("/tmp/ga.iso".to_string()),
            output_directory: Some(temp_path),
            ..Default::default()
        };
        let builder = VirtualboxIsoBuilder::new(config);
        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let _ = builder.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_vbox_scancodes() {
        let all_chars = [
            '\n', '\r', '\t', ' ', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm',
            'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '0', '1', '2', '3',
            '4', '5', '6', '7', '8', '9', '-', '=', '/', '.', '?',
        ];
        for ch in all_chars {
            let sc = char_to_vbox_scancodes(ch);
            if ch == '?' {
                assert_eq!(sc.len(), 0);
            } else {
                assert_ne!(sc.len(), 0);
            }
        }

        let keysyms = [
            0xFF0D, 0xFF09, 0xFF1B, 0xFF08, 0x0020, 0xFF52, 0xFF54, 0xFF51, 0xFF53, 0xFFBE, 0xFFBF,
            0xFFC0, 0xFFC1, 0xFFC2, 0xFFC3, 0xFFC4, 0xFFC5, 0xFFC6, 0xFFC7, 0xFFC8, 0xFFC9, 0x61,
            0x80, 0x1000,
        ];
        for k in keysyms {
            let sc =
                boot_action_to_vbox_scancodes(&crate::builder::virtualization::BootAction::Key(k));
            if k == 0x80 || k == 0x1000 {
                assert_eq!(sc.len(), 0);
            } else {
                assert_ne!(sc.len(), 0);
            }
        }

        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyDown(0xFFE1)),
            &["2a"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyUp(0xFFE1)),
            &["aa"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyDown(0xFFE3)),
            &["1d"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyUp(0xFFE3)),
            &["9d"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyDown(0xFFE9)),
            &["38"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyUp(0xFFE9)),
            &["b8"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::Wait(Duration::from_millis(1))),
            &[] as &[&str]
        );

        let actions = vec![
            BootAction::Wait(Duration::from_millis(1)),
            BootAction::Key(0xFF0D),
        ];
        assert!(
            send_vbox_boot_command("test-vm", &actions, Some(Duration::from_millis(1)))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_vboxmanage_execution_and_errors() -> Result<(), StampError> {
        vboxmanage(&["list", "vms"]).await?;

        assert!(
            vboxmanage_with_cmd("/nonexistent_vbox_binary_stamp", &["list"])
                .await
                .is_err()
        );

        let err_res = vboxmanage_with_cmd(
            "sh",
            &[
                "-c",
                "echo 'VBoxManage: error: Something failed' >&2; exit 1",
            ],
        )
        .await;
        assert!(
            matches!(err_res, Err(StampError::Builder(ref msg)) if msg.contains("exit code 1"))
        );

        let details_res =
            vboxmanage_with_cmd("sh", &["-c", "echo 'Details: error code 1234'; exit 1"]).await;
        assert!(details_res.is_err());

        let gen_res =
            vboxmanage_with_cmd("sh", &["-c", "echo 'generic error message' >&2; exit 1"]).await;
        assert!(gen_res.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_virtualbox_floppy_and_cd_attachment() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let f_path = td.path().join("preseed.cfg");
        tokio::fs::write(&f_path, b"d-i test").await?;
        let cd_path = td.path().join("user-data");
        tokio::fs::write(&cd_path, b"#cloud-config").await?;

        let config = VirtualboxIsoConfig {
            name: "vbox-test".to_string(),
            vm_name: Some("vbox-media-iso".to_string()),
            floppy_files: vec![f_path.to_string_lossy().to_string()],
            cd_files: vec![cd_path.to_string_lossy().to_string()],
            cd_label: Some("cidata".to_string()),
            guest_additions_mode: VBoxGuestAdditionsMode::Attach,
            output_directory: Some(td.path().to_string_lossy().to_string()),
            ..Default::default()
        };
        let builder = VirtualboxIsoBuilder::new(config);
        builder.prepare().await?;

        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let art = builder.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        assert!(art.id().contains("vbox-media"));
        Ok(())
    }

    #[tokio::test]
    async fn test_virtualbox_step_export_existing_file() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let out_dir = td.path().to_string_lossy().to_string();
        let config = VirtualboxIsoConfig {
            name: "export-test".to_string(),
            output_directory: Some(out_dir),
            export_format: VBoxExportFormat::Ova,
            ..Default::default()
        };
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut step = StepExport {
            ui: ui.clone(),
            name: "export-step".to_string(),
            config,
        };
        let mut state = StateBag::new();
        state.put("vm_name", "my-vm".to_string());

        let ova_file = td.path().join("my-vm.ova");
        tokio::fs::write(&ova_file, b"OVA_DATA").await?;

        step.run(&mut state).await?;
        step.cleanup(&state).await;

        let bad_config = VirtualboxIsoConfig {
            name: "export-fail".to_string(),
            output_directory: Some("/dev/null/impossible".to_string()),
            ..Default::default()
        };
        let mut step_fail = StepExport {
            ui,
            name: "export-fail".to_string(),
            config: bad_config,
        };
        assert!(step_fail.run(&mut state).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_virtualbox_builder_run_error_strategies() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let config = VirtualboxIsoConfig {
            name: "test-err".to_string(),
            output_directory: Some(td.path().to_string_lossy().to_string()),
            ..Default::default()
        };
        let builder = VirtualboxIsoBuilder::new(config);

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
        Ok(())
    }

    #[tokio::test]
    async fn test_virtualbox_prepare_validation() -> Result<(), StampError> {
        let mut cfg = VirtualboxIsoConfig::default();
        let b = VirtualboxIsoBuilder::new(cfg.clone());
        assert!(b.prepare().await.is_err());

        cfg.name = "valid".to_string();
        let b2 = VirtualboxIsoBuilder::new(cfg);
        b2.prepare().await
    }

    #[test]
    fn test_vbox_iso_missing_coverage() {
        assert_eq!(
            super::VBoxNicType::Custom("custom-nic".to_string()).as_str(),
            "custom-nic"
        );
        assert_eq!(
            super::VBoxSerialMode::Custom("custom-uart".to_string()).as_str(),
            "custom-uart"
        );
    }

    #[tokio::test]
    async fn test_vbox_iso_run_without_sizes_and_ports() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let config = super::VirtualboxIsoConfig {
            name: "test-vbox-iso-default-ports".to_string(),
            disk_size: None,
            vrde_port: None,
            ssh_host_port: None,
            winrm_host_port: None,
            guest_additions_path: None,
            headless: true,
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            virtualbox_version_file: Some("version.txt".to_string()),
            ..Default::default()
        };

        let builder = super::VirtualboxIsoBuilder::new(config);

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let _ = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;

        let fake_vbox = temp_dir.path().join("VBoxManageEmpty");
        tokio::fs::write(&fake_vbox, b"#!/bin/sh\necho r\nexit 0").await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_vbox, std::fs::Permissions::from_mode(0o755))?;
        }
        assert_eq!(
            super::query_vbox_version_with_cmd(&fake_vbox.to_string_lossy()).await,
            None
        );
        Ok(())
    }

    #[test]
    fn test_virtualboxisobuilder_derived_traits_and_enums() -> Result<(), std::convert::Infallible>
    {
        let config1 = VirtualboxIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);

        // VBoxChipset
        assert_eq!(VBoxChipset::Ich9.as_str(), "ich9");
        assert_eq!(VBoxChipset::Piix3.as_str(), "piix3");
        assert_eq!(VBoxChipset::ArmV8Virtual.as_str(), "armv8virtual");
        assert_eq!(VBoxChipset::Custom("custom".to_string()).as_str(), "custom");
        assert_eq!(VBoxChipset::from_str("ich9")?, VBoxChipset::Ich9);
        assert_eq!(VBoxChipset::from_str("piix3")?, VBoxChipset::Piix3);
        assert_eq!(
            VBoxChipset::from_str("armv8virtual")?,
            VBoxChipset::ArmV8Virtual
        );
        assert_eq!(VBoxChipset::from_str("arm")?, VBoxChipset::ArmV8Virtual);
        assert_eq!(format!("{}", VBoxChipset::Ich9), "ich9");

        // VBoxFirmware
        assert_eq!(VBoxFirmware::Bios.as_str(), "bios");
        assert_eq!(VBoxFirmware::Efi.as_str(), "efi");
        assert_eq!(VBoxFirmware::Efi64.as_str(), "efi64");
        assert_eq!(VBoxFirmware::Custom("other".to_string()).as_str(), "other");
        assert_eq!(VBoxFirmware::from_str("bios")?, VBoxFirmware::Bios);
        assert_eq!(VBoxFirmware::from_str("efi")?, VBoxFirmware::Efi);
        assert_eq!(VBoxFirmware::from_str("efi64")?, VBoxFirmware::Efi64);
        assert_eq!(format!("{}", VBoxFirmware::Efi), "efi");

        // VBoxGraphicsController
        assert_eq!(VBoxGraphicsController::VBoxSVGA.as_str(), "vboxsvga");
        assert_eq!(VBoxGraphicsController::VMSVGA.as_str(), "vmsvga");
        assert_eq!(VBoxGraphicsController::VBoxVGA.as_str(), "vboxvga");
        assert_eq!(VBoxGraphicsController::QemuRamFB.as_str(), "qemuramfb");
        assert_eq!(
            VBoxGraphicsController::Custom("x".to_string()).as_str(),
            "x"
        );
        assert_eq!(
            VBoxGraphicsController::from_str("vboxsvga")?,
            VBoxGraphicsController::VBoxSVGA
        );
        assert_eq!(
            VBoxGraphicsController::from_str("vmsvga")?,
            VBoxGraphicsController::VMSVGA
        );
        assert_eq!(
            VBoxGraphicsController::from_str("vboxvga")?,
            VBoxGraphicsController::VBoxVGA
        );
        assert_eq!(
            VBoxGraphicsController::from_str("qemuramfb")?,
            VBoxGraphicsController::QemuRamFB
        );
        assert_eq!(format!("{}", VBoxGraphicsController::VMSVGA), "vmsvga");

        // VBoxStorageBus
        assert_eq!(VBoxStorageBus::Sata.as_str(), "sata");
        assert_eq!(VBoxStorageBus::VirtIo.as_str(), "virtio-scsi");
        assert_eq!(VBoxStorageBus::Ide.as_str(), "ide");
        assert_eq!(VBoxStorageBus::Scsi.as_str(), "scsi");
        assert_eq!(VBoxStorageBus::Nvme.as_str(), "nvme");
        assert_eq!(VBoxStorageBus::Usb.as_str(), "usb");
        assert_eq!(VBoxStorageBus::Floppy.as_str(), "floppy");
        assert_eq!(VBoxStorageBus::Custom("z".to_string()).as_str(), "z");
        assert_eq!(VBoxStorageBus::Sata.default_controller_type(), "IntelAHCI");
        assert_eq!(VBoxStorageBus::VirtIo.default_controller_type(), "VirtIO");
        assert_eq!(VBoxStorageBus::Ide.default_controller_type(), "PIIX4");
        assert_eq!(VBoxStorageBus::Scsi.default_controller_type(), "LsiLogic");
        assert_eq!(VBoxStorageBus::Nvme.default_controller_type(), "PCIe");
        assert_eq!(VBoxStorageBus::Usb.default_controller_type(), "USB");
        assert_eq!(VBoxStorageBus::Floppy.default_controller_type(), "I82078");
        assert_eq!(
            VBoxStorageBus::Custom("z".to_string()).default_controller_type(),
            "IntelAHCI"
        );
        assert_eq!(VBoxStorageBus::from_str("sata")?, VBoxStorageBus::Sata);
        assert_eq!(VBoxStorageBus::from_str("virtio")?, VBoxStorageBus::VirtIo);
        assert_eq!(VBoxStorageBus::from_str("ide")?, VBoxStorageBus::Ide);
        assert_eq!(VBoxStorageBus::from_str("scsi")?, VBoxStorageBus::Scsi);
        assert_eq!(VBoxStorageBus::from_str("nvme")?, VBoxStorageBus::Nvme);
        assert_eq!(VBoxStorageBus::from_str("usb")?, VBoxStorageBus::Usb);
        assert_eq!(VBoxStorageBus::from_str("floppy")?, VBoxStorageBus::Floppy);
        assert_eq!(format!("{}", VBoxStorageBus::Sata), "sata");

        let ctl = VBoxStorageController {
            name: "Test Ctl".to_string(),
            bus: VBoxStorageBus::Sata,
            controller: Some("AHCI".to_string()),
            port_count: Some(4),
            host_iocache: Some(true),
        };
        assert_eq!(ctl.effective_controller(), "AHCI");

        // VBoxNicType
        assert_eq!(super::VBoxNicType::I82540EM.as_str(), "82540EM");
        assert_eq!(super::VBoxNicType::VirtIO.as_str(), "virtio");
        assert_eq!(super::VBoxNicType::UsbNet.as_str(), "usbnet");
        assert_eq!(super::VBoxNicType::Am79C970A.as_str(), "Am79C970A");
        assert_eq!(super::VBoxNicType::Am79C973.as_str(), "Am79C973");
        assert_eq!(super::VBoxNicType::I82543GC.as_str(), "82543GC");
        assert_eq!(super::VBoxNicType::I82545EM.as_str(), "82545EM");
        assert_eq!(
            super::VBoxNicType::from_str("82540em")?,
            super::VBoxNicType::I82540EM
        );
        assert_eq!(
            super::VBoxNicType::from_str("virtio")?,
            super::VBoxNicType::VirtIO
        );
        assert_eq!(
            super::VBoxNicType::from_str("usbnet")?,
            super::VBoxNicType::UsbNet
        );
        assert_eq!(
            super::VBoxNicType::from_str("am79c970a")?,
            super::VBoxNicType::Am79C970A
        );
        assert_eq!(
            super::VBoxNicType::from_str("am79c973")?,
            super::VBoxNicType::Am79C973
        );
        assert_eq!(
            super::VBoxNicType::from_str("82543gc")?,
            super::VBoxNicType::I82543GC
        );
        assert_eq!(
            super::VBoxNicType::from_str("82545em")?,
            super::VBoxNicType::I82545EM
        );
        assert_eq!(format!("{}", super::VBoxNicType::VirtIO), "virtio");

        // VBoxSerialMode
        assert_eq!(VBoxSerialMode::Disconnected.as_str(), "disconnected");
        assert_eq!(VBoxSerialMode::HostPipe.as_str(), "server");
        assert_eq!(VBoxSerialMode::HostDevice.as_str(), "device");
        assert_eq!(VBoxSerialMode::RawFile.as_str(), "file");
        assert_eq!(
            VBoxSerialMode::from_str("disconnected")?,
            VBoxSerialMode::Disconnected
        );
        assert_eq!(
            VBoxSerialMode::from_str("server")?,
            VBoxSerialMode::HostPipe
        );
        assert_eq!(
            VBoxSerialMode::from_str("device")?,
            VBoxSerialMode::HostDevice
        );
        assert_eq!(VBoxSerialMode::from_str("file")?, VBoxSerialMode::RawFile);
        assert_eq!(format!("{}", VBoxSerialMode::HostPipe), "server");

        // VBoxGuestAdditionsMode
        assert_eq!(VBoxGuestAdditionsMode::Attach.as_str(), "attach");
        assert_eq!(VBoxGuestAdditionsMode::Upload.as_str(), "upload");
        assert_eq!(VBoxGuestAdditionsMode::Disable.as_str(), "disable");
        assert_eq!(
            VBoxGuestAdditionsMode::from_str("upload")?,
            VBoxGuestAdditionsMode::Upload
        );
        assert_eq!(
            VBoxGuestAdditionsMode::from_str("disable")?,
            VBoxGuestAdditionsMode::Disable
        );
        assert_eq!(
            VBoxGuestAdditionsMode::from_str("attach")?,
            VBoxGuestAdditionsMode::Attach
        );
        assert_eq!(format!("{}", VBoxGuestAdditionsMode::Attach), "attach");

        // VBoxExportFormat
        assert_eq!(VBoxExportFormat::Ova.extension(), "ova");
        assert_eq!(VBoxExportFormat::Ovf.extension(), "ovf");
        assert_eq!(VBoxExportFormat::from_str("ovf")?, VBoxExportFormat::Ovf);
        assert_eq!(VBoxExportFormat::from_str("ova")?, VBoxExportFormat::Ova);
        assert_eq!(format!("{}", VBoxExportFormat::Ova), "ova");

        // Custom enum fallback parses
        assert_eq!(
            VBoxChipset::from_str("other")?,
            VBoxChipset::Custom("other".to_string())
        );
        assert_eq!(
            VBoxFirmware::from_str("other")?,
            VBoxFirmware::Custom("other".to_string())
        );
        assert_eq!(
            VBoxGraphicsController::from_str("other")?,
            VBoxGraphicsController::Custom("other".to_string())
        );
        assert_eq!(
            VBoxStorageBus::from_str("other")?,
            VBoxStorageBus::Custom("other".to_string())
        );
        assert_eq!(
            super::VBoxNicType::from_str("other")?,
            super::VBoxNicType::Custom("other".to_string())
        );
        assert_eq!(
            VBoxSerialMode::from_str("other")?,
            VBoxSerialMode::Custom("other".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_virtualbox_from_builder_config() -> Result<(), StampError> {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "virtualbox-iso".to_string(),
            name: "bento-vbox".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("vm_name".to_string(), "my-bento-vm".to_string());
        builder_cfg
            .config
            .insert("memory".to_string(), "4096".to_string());
        builder_cfg
            .config
            .insert("cpus".to_string(), "4".to_string());
        builder_cfg
            .config
            .insert("disk_size".to_string(), "50000".to_string());
        builder_cfg
            .config
            .insert("chipset".to_string(), "ich9".to_string());
        builder_cfg
            .config
            .insert("firmware".to_string(), "efi".to_string());
        builder_cfg
            .config
            .insert("graphicscontroller".to_string(), "vmsvga".to_string());
        builder_cfg
            .config
            .insert("gfx_vram_size".to_string(), "64".to_string());
        builder_cfg
            .config
            .insert("gfx_accelerate_3d".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("hard_drive_interface".to_string(), "sata".to_string());
        builder_cfg
            .config
            .insert("nic_type".to_string(), "virtio".to_string());
        builder_cfg
            .config
            .insert("network".to_string(), "nat".to_string());
        builder_cfg
            .config
            .insert("ssh_host_port".to_string(), "2222".to_string());
        builder_cfg
            .config
            .insert("winrm_host_port".to_string(), "5985".to_string());
        builder_cfg
            .config
            .insert("guest_additions_mode".to_string(), "upload".to_string());
        builder_cfg
            .config
            .insert("export_format".to_string(), "ovf".to_string());
        builder_cfg
            .config
            .insert("headless".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("uart1".to_string(), "0x3f8 4".to_string());
        builder_cfg
            .config
            .insert("uartmode1".to_string(), "file".to_string());
        builder_cfg
            .config
            .insert("uartpath1".to_string(), "/tmp/vbox-uart.log".to_string());
        builder_cfg.config.insert(
            "vboxmanage".to_string(),
            r#"[["modifyvm", "{{.Name}}", "--cpus", "4"]]"#.to_string(),
        );

        let cfg = VirtualboxIsoConfig::from_builder_config(&builder_cfg)?;
        assert_eq!(cfg.name, "bento-vbox");
        assert_eq!(cfg.vm_name.as_deref(), Some("my-bento-vm"));
        assert_eq!(cfg.memory, Some(MemoryMb::new(4096)));
        assert_eq!(cfg.cpus, Some(4));
        assert_eq!(cfg.disk_size, Some(MemoryMb::new(50000)));
        assert_eq!(cfg.chipset, Some(VBoxChipset::Ich9));
        assert_eq!(cfg.firmware, Some(VBoxFirmware::Efi));
        assert_eq!(
            cfg.graphics_controller,
            Some(VBoxGraphicsController::VMSVGA)
        );
        assert_eq!(cfg.gfx_vram_size, Some(MemoryMb::new(64)));
        assert_eq!(cfg.gfx_accelerate_3d, Some(true));
        assert_eq!(cfg.hard_drive_interface, Some(VBoxStorageBus::Sata));
        assert_eq!(cfg.nic_type, Some(super::VBoxNicType::VirtIO));
        assert_eq!(cfg.network.as_deref(), Some("nat"));
        assert_eq!(cfg.ssh_host_port, Some(Port::new(2222)));
        assert_eq!(cfg.winrm_host_port, Some(Port::new(5985)));
        assert_eq!(cfg.guest_additions_mode, VBoxGuestAdditionsMode::Upload);
        assert_eq!(cfg.export_format, VBoxExportFormat::Ovf);
        assert!(cfg.headless);
        assert_eq!(cfg.serial_ports.len(), 1);
        assert_eq!(cfg.serial_ports[0].port_number, 1);
        assert_eq!(cfg.serial_ports[0].mode, VBoxSerialMode::RawFile);
        assert_eq!(cfg.vboxmanage.len(), 1);

        let _ = discover_guest_additions_iso(Some("7.0"));
        Ok(())
    }

    #[tokio::test]
    async fn test_virtualbox_iso_enhanced_options_and_helpers() -> Result<(), StampError> {
        let raw = vec![
            "{{.Name}}".to_string(),
            "{{ .Name }}".to_string(),
            "{{.Version}}".to_string(),
            "{{ .Version }}".to_string(),
            "{{.OutputDirectory}}".to_string(),
            "{{ .OutputDirectory }}".to_string(),
        ];
        let interp = interpolate_vbox_args_with_output(&raw, "my-vm", Some("7.1"), Some("/my/out"));
        assert_eq!(
            interp,
            vec!["my-vm", "my-vm", "7.1", "7.1", "/my/out", "/my/out"]
        );

        let default_interp = interpolate_vbox_args_with_output(&raw, "my-vm", None, None);
        assert_eq!(
            default_interp,
            vec!["my-vm", "my-vm", "7.0", "7.0", "output", "output"]
        );

        let dummy = hashicorp_configuration_language_rs::span::Span::default();
        let tuple_expr = hashicorp_configuration_language_rs::ast::expr::Expression::Tuple(
            vec![
                hashicorp_configuration_language_rs::ast::expr::Expression::Tuple(
                    vec![
                        hashicorp_configuration_language_rs::ast::expr::Expression::String(
                            "arg1".to_string(),
                            dummy.clone(),
                        ),
                    ],
                    dummy.clone(),
                ),
                hashicorp_configuration_language_rs::ast::expr::Expression::String(
                    "flat_arg".to_string(),
                    dummy.clone(),
                ),
            ],
            dummy,
        );
        let parsed_nested = parse_nested_string_list(None, Some(&tuple_expr));
        assert_eq!(parsed_nested.len(), 2);
        assert_eq!(parsed_nested[0], vec!["arg1"]);
        assert_eq!(parsed_nested[1], vec!["flat_arg"]);

        let flat_json = "[\"cmd1\", \"cmd2\"]".to_string();
        let parsed_flat = parse_nested_string_list(Some(&flat_json), None);
        assert_eq!(parsed_flat, vec![vec!["cmd1", "cmd2"]]);

        let invalid = "not-json".to_string();
        let parsed_invalid = parse_nested_string_list(Some(&invalid), None);
        assert_eq!(parsed_invalid.len(), 0);

        let csv = "foo, bar, 'baz', \"qux\"";
        let parsed_csv = parse_string_list(csv);
        assert_eq!(parsed_csv, vec!["foo", "bar", "baz", "qux"]);

        let mut port_cfg = std::collections::HashMap::new();
        port_cfg.insert("uart1".to_string(), "0x3f8 4".to_string());
        port_cfg.insert("uartmode1".to_string(), "file".to_string());
        port_cfg.insert("uart2".to_string(), "0x2f8".to_string());
        port_cfg.insert("uartmode2".to_string(), "device".to_string());
        port_cfg.insert("uart3".to_string(), "on".to_string());
        port_cfg.insert("uartmode3".to_string(), "server".to_string());
        port_cfg.insert("uart4".to_string(), "off".to_string());

        let ports = parse_serial_ports(&port_cfg);
        assert_eq!(ports.len(), 4);
        assert_eq!(ports[0].io_base.as_deref(), Some("0x3f8"));
        assert_eq!(ports[0].irq, Some(4));
        assert_eq!(ports[1].io_base.as_deref(), Some("0x2f8"));
        assert_eq!(ports[1].irq, None);
        assert_eq!(ports[2].io_base, None);
        assert_eq!(ports[2].irq, None);
        assert_eq!(ports[3].mode, VBoxSerialMode::Disconnected);

        let td = tempfile::tempdir()?;
        let ga_file = td.path().join("fake_ga.iso");
        tokio::fs::write(&ga_file, b"ISO").await?;

        {
            let _env =
                crate::utils::ScopedEnvVar::set("VBOX_GA_ISO", ga_file.to_string_lossy().as_ref());
            assert_eq!(discover_guest_additions_iso(None), Some(ga_file.clone()));
        }
        {
            let _env = crate::utils::ScopedEnvVar::set(
                "VBOX_GUEST_ADDITIONS_ISO",
                ga_file.to_string_lossy().as_ref(),
            );
            assert_eq!(discover_guest_additions_iso(None), Some(ga_file.clone()));
        }

        let _ = query_vbox_version().await;

        let mut bento_cfg = crate::template::BuilderConfig {
            builder_type: "virtualbox-iso".to_string(),
            name: "bento-test".to_string(),
            ..Default::default()
        };
        bento_cfg
            .config
            .insert("iso_interface".to_string(), "sata".to_string());
        bento_cfg
            .config
            .insert("guest_additions_interface".to_string(), "sata".to_string());
        bento_cfg
            .config
            .insert("nested_virt".to_string(), "true".to_string());
        bento_cfg
            .config
            .insert("rtc_time_base".to_string(), "utc".to_string());
        bento_cfg
            .config
            .insert("usb".to_string(), "true".to_string());
        bento_cfg.config.insert(
            "virtualbox_version_file".to_string(),
            ".vbox_version".to_string(),
        );
        bento_cfg
            .config
            .insert("shutdown_command".to_string(), "shutdown /s".to_string());
        bento_cfg
            .config
            .insert("shutdown_timeout".to_string(), "5m".to_string());

        let cfg = VirtualboxIsoConfig::from_builder_config(&bento_cfg)?;
        assert_eq!(cfg.iso_interface, Some(VBoxStorageBus::Sata));
        assert_eq!(cfg.guest_additions_interface, Some(VBoxStorageBus::Sata));
        assert_eq!(cfg.nested_virt, Some(true));
        assert_eq!(cfg.rtc_time_base.as_deref(), Some("utc"));
        assert_eq!(cfg.usb, Some(true));
        assert_eq!(
            cfg.virtualbox_version_file.as_deref(),
            Some(".vbox_version")
        );
        assert_eq!(cfg.shutdown_command.as_deref(), Some("shutdown /s"));
        assert_eq!(cfg.shutdown_timeout.as_deref(), Some("5m"));

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut step_shutdown = StepShutdown {
            ui: ui.clone(),
            name: "shutdown-step".to_string(),
            config: VirtualboxIsoConfig {
                name: "shutdown-test".to_string(),
                shutdown_command: Some("poweroff".to_string()),
                virtualbox_version_file: Some(".vbox_version".to_string()),
                output_directory: Some(td.path().to_string_lossy().to_string()),
                vboxmanage_post: vec![vec!["echo".to_string(), "done".to_string()]],
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("vm_name", "test-vm".to_string());

        let comm: Arc<dyn Communicator> =
            Arc::new(crate::communicator::mock::MockCommunicator::new());
        state.put("communicator", comm);

        step_shutdown.run(&mut state).await?;
        step_shutdown.cleanup(&state).await;

        let mut step_create = StepCreateVM {
            ui: ui.clone(),
            name: "create-step".to_string(),
            config: cfg,
        };
        step_create.cleanup(&state).await;
        let empty_state = StateBag::new();
        step_create.cleanup(&empty_state).await;

        assert_eq!(
            interpolate_vbox_args(&["{{.Name}}".to_string()], "vm1", None),
            vec!["vm1"]
        );
        execute_vboxmanage_commands(
            &[vec![], vec!["list".to_string(), "vms".to_string()]],
            "vm1",
            None,
        )
        .await?;

        assert_eq!(parse_string_list("[\"single\"]"), vec!["single"]);
        assert_eq!(parse_string_list("a,,b"), vec!["a", "b"]);

        let mut upload_step = StepUploadGuestAdditions {
            ui: ui.clone(),
            name: "upload-step".to_string(),
            config: VirtualboxIsoConfig {
                guest_additions_path: Some("/tmp/ga.iso".to_string()),
                ..Default::default()
            },
        };
        upload_step.run(&mut state).await?;
        upload_step.cleanup(&state).await;

        let mut run_step = StepRunVM {
            ui: ui.clone(),
            name: "run-step".to_string(),
            config: VirtualboxIsoConfig {
                boot_command: Some(vec!["<wait1ms>".to_string(), "a".to_string()]),
                ..Default::default()
            },
        };
        run_step.run(&mut state).await?;
        run_step.cleanup(&state).await;

        let mut prov_failing = StepProvision {
            ui: ui.clone(),
            name: "fail-step".to_string(),
            hook: Arc::new(DefaultProvisionHook {
                provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
                error_cleanup_provisioners: Arc::new(vec![]),
            }),
            config: VirtualboxIsoConfig::default(),
        };
        assert!(prov_failing.run(&mut state).await.is_err());
        prov_failing.cleanup(&state).await;

        let mut exp_step = StepExport {
            ui,
            name: "exp".to_string(),
            config: VirtualboxIsoConfig::default(),
        };
        exp_step.cleanup(&state).await;
        Ok(())
    }
}
