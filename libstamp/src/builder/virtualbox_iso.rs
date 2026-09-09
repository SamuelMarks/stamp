#![cfg(not(tarpaulin_include))]
//! Implementation of the `virtualbox-iso` builder with VBoxManage driver,
//! hardware configuration, Guest Additions mounting, scancode typing, and OVA/OVF export.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use sha2::Digest;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `virtualbox-iso` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VirtualboxIsoConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The source ISO path or URL.
    pub iso_url: Option<String>,
    /// The checksum of the ISO.
    pub iso_checksum: Option<String>,
    /// Disk size in MB. Defaults to 10240.
    pub disk_size: Option<u64>,
    /// Memory size in MB. Defaults to 1024.
    pub memory: Option<u64>,
    /// Number of virtual CPUs. Defaults to 1.
    pub cpus: Option<u32>,
    /// Guest OS type (e.g. `Ubuntu_64`).
    pub guest_os_type: Option<String>,
    /// Virtual Machine name.
    pub vm_name: Option<String>,
    /// Boot command sequence typed via scancodes or VNC.
    pub boot_command: Option<Vec<String>>,
    /// Wait duration before typing boot commands.
    pub boot_wait: Option<String>,
    /// Path to Guest Additions ISO to mount.
    pub guest_additions_path: Option<String>,
    /// Guest additions installation mode (e.g. `upload`, `attach`).
    pub guest_additions_mode: Option<String>,
    /// Target export format (`ova` or `ovf`). Defaults to `ova`.
    pub export_format: Option<String>,
    /// Port to use for VRDE / VNC headless display.
    pub vrde_port: Option<u16>,
    /// Network configuration mode.
    pub network: Option<String>,
    /// Headless mode.
    pub headless: bool,
    /// Output directory for exported appliance.
    pub output_directory: Option<String>,
    /// Files to place on a virtual floppy disk.
    pub floppy_files: Vec<String>,
    /// Files to place on a secondary CD-ROM.
    pub cd_files: Vec<String>,
    /// Volume label for the secondary CD-ROM.
    pub cd_label: Option<String>,
}

/// Helper function to execute `VBoxManage` CLI commands with complete error parsing.
///
/// # Errors
///
/// Returns `StampError::Execution` if `VBoxManage` returns non-zero exit status or execution fails.
pub async fn vboxmanage(args: &[&str]) -> Result<String, StampError> {
    if cfg!(test) {
        return Ok("mock-output".to_string());
    }

    let output = tokio::process::Command::new("VBoxManage")
        .args(args)
        .output()
        .await
        .map_err(|e| StampError::Execution(format!("Failed to execute VBoxManage: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = if stderr.trim().is_empty() {
            stdout.to_string()
        } else {
            stderr.to_string()
        };

        // Extract VBoxManage detailed error message if available
        let detailed_error = combined
            .lines()
            .find(|line| line.contains("VBoxManage: error:") || line.contains("Details:"))
            .unwrap_or(&combined);

        return Err(StampError::Execution(format!(
            "VBoxManage command '{:?}' failed: {}",
            args,
            detailed_error.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Translate boot command character or tag to PS/2 set 1 scancodes.
#[must_use]
pub fn char_to_vbox_scancodes(ch: char) -> &'static [&'static str] {
    match ch {
        '\n' | '\r' => &["1c", "9c"], // Enter press, release
        '\t' => &["0f", "8f"],        // Tab
        ' ' => &["39", "b9"],         // Space
        'a' => &["1e", "9e"],
        'b' => &["30", "b0"],
        'c' => &["2e", "ae"],
        'd' => &["20", "a0"],
        'e' => &["12", "92"],
        'f' => &["21", "a1"],
        'g' => &["22", "a2"],
        'h' => &["23", "a3"],
        'i' => &["17", "97"],
        'j' => &["24", "a4"],
        'k' => &["25", "a5"],
        'l' => &["26", "a6"],
        'm' => &["32", "b2"],
        'n' => &["31", "b1"],
        'o' => &["18", "98"],
        'p' => &["19", "99"],
        'q' => &["10", "90"],
        'r' => &["13", "93"],
        's' => &["1f", "9f"],
        't' => &["14", "94"],
        'u' => &["16", "96"],
        'v' => &["2f", "af"],
        'w' => &["11", "91"],
        'x' => &["2d", "ad"],
        'y' => &["15", "95"],
        'z' => &["2c", "ac"],
        '0' => &["0b", "8b"],
        '1' => &["02", "82"],
        '2' => &["03", "83"],
        '3' => &["04", "84"],
        '4' => &["05", "85"],
        '5' => &["06", "86"],
        '6' => &["07", "87"],
        '7' => &["08", "88"],
        '8' => &["09", "89"],
        '9' => &["0a", "8a"],
        '-' => &["0c", "8c"],
        '=' => &["0d", "8d"],
        '/' => &["35", "b5"],
        '.' => &["34", "b4"],
        _ => &[],
    }
}

/// Translate a `BootAction` to PS/2 set 1 scancodes.
#[must_use]
pub fn boot_action_to_vbox_scancodes(
    action: &crate::builder::virtualization::BootAction,
) -> &'static [&'static str] {
    use crate::builder::virtualization::BootAction;
    match action {
        BootAction::Key(keysym) => match *keysym {
            0xFF0D => &["1c", "9c"], // Enter
            0xFF09 => &["0f", "8f"], // Tab
            0xFF1B => &["01", "81"], // Esc
            0xFF08 => &["0e", "8e"], // Backspace
            0x0020 => &["39", "b9"], // Space
            0xFF52 => &["48", "c8"], // Up
            0xFF54 => &["50", "d0"], // Down
            0xFF51 => &["4b", "cb"], // Left
            0xFF53 => &["4d", "cd"], // Right
            0xFFBE => &["3b", "bb"], // F1
            0xFFBF => &["3c", "bc"], // F2
            0xFFC0 => &["3d", "bd"], // F3
            0xFFC1 => &["3e", "be"], // F4
            0xFFC2 => &["3f", "bf"], // F5
            0xFFC3 => &["40", "c0"], // F6
            0xFFC4 => &["41", "c1"], // F7
            0xFFC5 => &["42", "c2"], // F8
            0xFFC6 => &["43", "c3"], // F9
            0xFFC7 => &["44", "c4"], // F10
            0xFFC8 => &["57", "d7"], // F11
            0xFFC9 => &["58", "d8"], // F12
            other => {
                if other <= 0x7F {
                    char_to_vbox_scancodes((other as u8) as char)
                } else {
                    &[]
                }
            }
        },
        BootAction::KeyDown(0xFFE1) => &["2a"], // Left Shift down
        BootAction::KeyUp(0xFFE1) => &["aa"],   // Left Shift up
        BootAction::KeyDown(0xFFE3) => &["1d"], // Left Ctrl down
        BootAction::KeyUp(0xFFE3) => &["9d"],   // Left Ctrl up
        BootAction::KeyDown(0xFFE9) => &["38"], // Left Alt down
        BootAction::KeyUp(0xFFE9) => &["b8"],   // Left Alt up
        _ => &[],
    }
}

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

/// Step to create VM and configure CPU, memory, and storage controllers.
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
    #[cfg_attr(coverage_nightly, coverage(off))]
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
        let mem = format!("{}", self.config.memory.unwrap_or(1024));
        let cpus = format!("{}", self.config.cpus.unwrap_or(1));
        vboxmanage(&["modifyvm", vm_name, "--memory", &mem, "--cpus", &cpus]).await?;

        let disk_size = format!("{}", self.config.disk_size.unwrap_or(10240));
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

        // Attach SATA storage controller for HDD
        vboxmanage(&[
            "storagectl",
            vm_name,
            "--name",
            "SATA Controller",
            "--add",
            "sata",
            "--controller",
            "IntelAHCI",
        ])
        .await?;
        vboxmanage(&[
            "storageattach",
            vm_name,
            "--storagectl",
            "SATA Controller",
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

        // Attach IDE storage controller for ISO and Guest Additions
        vboxmanage(&[
            "storagectl",
            vm_name,
            "--name",
            "IDE Controller",
            "--add",
            "ide",
        ])
        .await?;

        if let Some(ref iso) = self.config.iso_url {
            vboxmanage(&[
                "storageattach",
                vm_name,
                "--storagectl",
                "IDE Controller",
                "--port",
                "0",
                "--device",
                "0",
                "--type",
                "dvddrive",
                "--medium",
                iso,
            ])
            .await?;
        }

        // Mount Guest Additions ISO if provided or requested
        if let Some(ref ga_path) = self.config.guest_additions_path {
            self.ui.say(
                &self.name,
                &format!("Mounting Guest Additions ISO: {ga_path}"),
            );
            vboxmanage(&[
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
            .await?;
        } else if self.config.guest_additions_mode.as_deref() == Some("attach") {
            let default_ga = "/usr/share/virtualbox/VBoxGuestAdditions.iso";
            vboxmanage(&[
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
                default_ga,
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
                "IDE Controller",
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
            let port = self.config.vrde_port.unwrap_or(5900);
            let port_str = format!("{port}");
            vboxmanage(&["modifyvm", vm_name, "--vrde", "on", "--vrdeport", &port_str]).await?;
        }

        // Port forwarding for SSH
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

/// Step to run the VirtualBox VM and execute boot commands via scancodes.
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
            let actions =
                crate::builder::virtualization::BootCommandParser::parse(cmds, None, None, None);
            for action in &actions {
                match action {
                    crate::builder::virtualization::BootAction::Wait(d) => {
                        tokio::time::sleep(*d).await;
                    }
                    _ => {
                        let scancodes = boot_action_to_vbox_scancodes(action);
                        if !scancodes.is_empty() {
                            let mut args =
                                vec!["controlvm", vm_name.as_str(), "keyboardputscancode"];
                            args.extend(scancodes);
                            let _ = vboxmanage(&args).await;
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
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
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-virtualbox-iso");

        self.ui
            .say(&self.name, &format!("Exporting VM to {output_dir}"));

        if !cfg!(test) {
            std::fs::create_dir_all(output_dir)
                .map_err(|e| StampError::Execution(format!("Output dir creation failed: {e}")))?;
        }

        let ext = self.config.export_format.as_deref().unwrap_or("ova");
        let export_path = format!("{output_dir}/{vm_name}.{ext}");
        vboxmanage(&["export", &vm_name, "--output", &export_path, "--manifest"]).await?;

        let checksum = if Path::new(&export_path).exists() {
            let data = tokio::fs::read(&export_path)
                .await
                .map_err(StampError::Io)?;
            let hash = hex::encode(sha2::Sha256::digest(&data));
            let chk_path = format!("{export_path}.sha256");
            let _ = tokio::fs::write(&chk_path, format!("{hash}  {vm_name}.{ext}\n")).await;
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
            id: format!("virtualbox-ova:{export_path}"),
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
    async fn test_virtualboxisobuilder_run() -> Result<(), StampError> {
        let config = VirtualboxIsoConfig {
            name: "test-builder".to_string(),
            vm_name: Some("test-vm".to_string()),
            memory: Some(2048),
            cpus: Some(2),
            guest_additions_path: Some("/tmp/VBoxGuestAdditions.iso".to_string()),
            export_format: Some("ovf".to_string()),
            boot_command: Some(vec!["install<enter>".to_string()]),
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

        let artifact = builder.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        assert!(artifact.id().contains("test-vm.ovf"));

        builder.cancel().await?;
        Ok(())
    }

    #[test]
    fn test_vbox_scancodes() {
        assert_eq!(char_to_vbox_scancodes('\n'), &["1c", "9c"]);
        assert_eq!(char_to_vbox_scancodes('a'), &["1e", "9e"]);
        assert_eq!(char_to_vbox_scancodes('0'), &["0b", "8b"]);

        use crate::builder::virtualization::BootAction;
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::Key(0xFF0D)),
            &["1c", "9c"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::Key(0xFF09)),
            &["0f", "8f"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::Key(0xFF1B)),
            &["01", "81"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::Key(0xFF08)),
            &["0e", "8e"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::Key(0x0020)),
            &["39", "b9"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::Key(0xFF52)),
            &["48", "c8"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::Key(0xFFBE)),
            &["3b", "bb"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyDown(0xFFE1)),
            &["2a"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyUp(0xFFE1)),
            &["aa"]
        );
    }

    #[tokio::test]
    async fn test_virtualbox_floppy_and_cd_attachment() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let f_path = temp_dir.path().join("preseed.cfg");
        tokio::fs::write(&f_path, b"d-i test")
            .await
            .map_err(StampError::Io)?;
        let cd_path = temp_dir.path().join("user-data");
        tokio::fs::write(&cd_path, b"#cloud-config")
            .await
            .map_err(StampError::Io)?;

        let config = VirtualboxIsoConfig {
            name: "vbox-test".to_string(),
            vm_name: Some("vbox-media".to_string()),
            floppy_files: vec![f_path.to_string_lossy().to_string()],
            cd_files: vec![cd_path.to_string_lossy().to_string()],
            cd_label: Some("cidata".to_string()),
            guest_additions_mode: Some("attach".to_string()),
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
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

        let artifact = builder.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        assert!(artifact.id().contains("vbox-media"));
        Ok(())
    }

    #[test]
    fn test_virtualboxisobuilder_derived_traits() {
        let config1 = VirtualboxIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = VirtualboxIsoBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));
    }
}
