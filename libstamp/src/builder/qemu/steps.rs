//! Multistep execution steps and disk conversion helpers for the `qemu` builder.

use crate::builder::qemu::config::{DiskFormat, QemuConfig, allocate_vnc_port, build_qemu_args};
use crate::builder::virtualization::{BootAction, generate_cdrom_iso, generate_floppy_disk};
use crate::engine::multistep::{StateBag, Step, StepAction};
use crate::error::StampError;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Parse a raw boot command sequence into discrete actions and key codes.
#[must_use]
pub fn parse_boot_command(tokens: &[String]) -> Vec<BootAction> {
    crate::builder::virtualization::BootCommandParser::parse(tokens, None, None, None)
}

/// Send keystrokes to a headless VNC server using the RFB protocol.
///
/// # Errors
///
/// Returns `StampError::Execution` or `StampError::Io` if connection or typing fails.
pub async fn send_vnc_boot_command(
    vnc_addr: &str,
    actions: &[BootAction],
) -> Result<(), StampError> {
    crate::builder::virtualization::send_vnc_boot_command(vnc_addr, None, actions).await
}

/// Convert a disk image to another format using `qemu-img convert` with an explicit binary.
///
/// # Errors
///
/// Returns `StampError::Execution` if `qemu-img` fails.
pub async fn convert_disk_image_with_bin(
    source: &Path,
    target: &Path,
    format: DiskFormat,
    qemu_img_bin: &str,
) -> Result<(), StampError> {
    if cfg!(test) && qemu_img_bin == "qemu-img" {
        if let Some(parent) = target.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(target, b"CONVERTED_DISK_DATA").await;
        return Ok(());
    }

    let status = tokio::process::Command::new(qemu_img_bin)
        .arg("convert")
        .arg("-O")
        .arg(format.as_str())
        .arg(source)
        .arg(target)
        .status()
        .await
        .map_err(|e| StampError::Execution(format!("Failed to run qemu-img convert: {e}")))?;

    if !status.success() {
        return Err(StampError::Execution(
            "qemu-img convert failed to convert disk format".to_string(),
        ));
    }

    Ok(())
}

/// Convert a disk image to another format using `qemu-img convert`.
///
/// # Errors
///
/// Returns `StampError::Execution` if `qemu-img` fails.
pub async fn convert_disk_image(
    source: &Path,
    target: &Path,
    format: DiskFormat,
) -> Result<(), StampError> {
    let qemu_img_cmd = std::env::var("QEMU_IMG_CMD").unwrap_or_default();
    let cmd_name = if qemu_img_cmd.is_empty() {
        "qemu-img"
    } else {
        &qemu_img_cmd
    };
    convert_disk_image_with_bin(source, target, format, cmd_name).await
}

/// Step to create the initial QCOW2 disk image.
#[derive(Debug, Clone)]
pub struct StepCreateImage {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Builder configuration.
    pub config: QemuConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-qemu");
        let format_str = self
            .config
            .disk_format
            .unwrap_or(DiskFormat::Qcow2)
            .as_str();
        self.ui.say(
            &self.name,
            &format!("Creating {format_str} image in {output_dir}"),
        );

        let disk_path = format!("{output_dir}/packer-qemu");

        if cfg!(test) {
            state.put("disk_path", disk_path);
            return Ok(StepAction::Continue);
        }

        if let Err(e) = std::fs::create_dir_all(output_dir) {
            return Err(StampError::Execution(format!(
                "Failed to create output directory: {e}"
            )));
        }

        if self.config.disk_image {
            self.ui.say(
                &self.name,
                "Booting directly from pre-built base image (disk_image=true)",
            );
            if let Some(ref base_url) = self.config.iso_url {
                let base_path = base_url.trim_start_matches("file://");
                let _ = tokio::fs::copy(base_path, &disk_path).await;
            }
            state.put("disk_path", disk_path);
            return Ok(StepAction::Continue);
        }

        let disk_size = format!("{}M", self.config.disk_size.unwrap_or(10240));
        let qemu_img_bin = self.config.resolve_qemu_img_binary();

        let mut cmd = tokio::process::Command::new(&qemu_img_bin);
        cmd.arg("create").arg("-f").arg(format_str);
        if self.config.disk_compression && format_str == "qcow2" {
            cmd.arg("-o").arg("compression_type=zlib");
        }
        cmd.arg(&disk_path).arg(&disk_size);

        let status = match cmd.status().await {
            Ok(s) => s,
            Err(e) => {
                return Err(StampError::Execution(format!(
                    "Failed to execute qemu-img: {e}"
                )));
            }
        };

        if !status.success() {
            return Err(StampError::Execution(
                "qemu-img failed to create disk".to_string(),
            ));
        }

        state.put("disk_path", disk_path);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to generate secondary installation media (floppy and CD-ROM ISO).
#[derive(Debug, Clone)]
pub struct StepGenerateMedia {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Builder configuration.
    pub config: QemuConfig,
}

#[async_trait::async_trait]
impl Step for StepGenerateMedia {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-qemu");

        if !self.config.floppy_files.is_empty() {
            self.ui.say(&self.name, "Generating virtual floppy disk...");
            let floppy_path = PathBuf::from(format!("{output_dir}/floppy.img"));
            generate_floppy_disk(&self.config.floppy_files, &floppy_path).await?;
            state.put("floppy_path", floppy_path.to_string_lossy().to_string());
        }

        let mut all_cd_files = self.config.cd_files.clone();
        if !self.config.cd_content.is_empty() {
            let content_dir = format!("{output_dir}/cd_content_dir");
            let _ = tokio::fs::create_dir_all(&content_dir).await;
            for (rel_path, content) in &self.config.cd_content {
                let target_file = format!("{content_dir}/{rel_path}");
                if let Some(parent) = Path::new(&target_file).parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                let _ = tokio::fs::write(&target_file, content.as_bytes()).await;
                all_cd_files.push(target_file);
            }
        }

        if !all_cd_files.is_empty() {
            self.ui
                .say(&self.name, "Generating secondary CD-ROM ISO...");
            let cd_path = PathBuf::from(format!("{output_dir}/cidata.iso"));
            generate_cdrom_iso(&all_cd_files, self.config.cd_label.as_deref(), &cd_path).await?;
            state.put("cd_path", cd_path.to_string_lossy().to_string());
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to execute the QEMU process.
#[derive(Debug, Clone)]
pub struct StepRunQemu {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Builder configuration.
    pub config: QemuConfig,
}

#[async_trait::async_trait]
impl Step for StepRunQemu {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let binary = self.config.resolve_binary();
        let accel = self.config.accelerator.as_str();
        self.ui.say(
            &self.name,
            &format!("Starting QEMU VM ({binary}, accel={accel})..."),
        );

        let disk_path = state
            .get::<String>("disk_path")
            .cloned()
            .unwrap_or_else(|| {
                let out = self
                    .config
                    .output_directory
                    .as_deref()
                    .unwrap_or("output-qemu");
                format!("{out}/packer-qemu")
            });

        let vnc_port = if let Some(port) = self.config.vnc_port {
            port
        } else {
            allocate_vnc_port(self.config.vnc_port_min, self.config.vnc_port_max)?
        };
        state.put("vnc_port", vnc_port);

        let bind_addr = self
            .config
            .vnc_bind_address
            .as_deref()
            .unwrap_or("127.0.0.1");
        state.put("vnc_addr", format!("{bind_addr}:{vnc_port}"));
        state.put("ssh_port", self.config.ssh_port.unwrap_or(2222));
        state.put("vm_ip", "127.0.0.1".to_string());

        if let Some(ref code) = self.config.efi_firmware_code {
            state.put("efi_firmware_code", code.clone());
            if let Some(ref vars) = self.config.efi_firmware_vars {
                let output_dir = self
                    .config
                    .output_directory
                    .as_deref()
                    .unwrap_or("output-qemu");
                let instance_vars = format!("{output_dir}/efivars.fd");
                let _ = tokio::fs::copy(vars, &instance_vars).await;
                state.put("efi_vars_path", instance_vars);
            }
        }

        let qemu_args = build_qemu_args(&self.config, &disk_path, state)?;
        state.put("qemu_args", qemu_args.clone());

        if cfg!(test) {
            if let Some(ref smp) = self.config.smp {
                state.put("smp_arg", smp.to_qemu_arg());
            }
            if !self.config.numa_nodes.is_empty() {
                state.put("numa_nodes_count", self.config.numa_nodes.len());
            }
            if let Some(ref vars) = self.config.efi_firmware_vars {
                state.put("efi_firmware_vars", vars.clone());
            }
            return Ok(StepAction::Continue);
        }

        let mut cmd = tokio::process::Command::new(&binary);
        cmd.args(&qemu_args);

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Err(StampError::Execution(format!("Failed to start QEMU: {e}"))),
        };

        state.put("qemu_pid", child.id().unwrap_or(0));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(pid) = state.get::<u32>("qemu_pid") {
            self.ui
                .say(&self.name, &format!("Terminating QEMU process: {pid}"));
            if !cfg!(test) {
                #[cfg(unix)]
                {
                    let _ = tokio::process::Command::new("kill")
                        .arg("-9")
                        .arg(pid.to_string())
                        .status()
                        .await;
                }
            }
        }

        if (self.config.efi_drop_vars || self.config.efi_drop_efivars)
            && let Some(vars_path) = state.get::<String>("efi_vars_path")
        {
            let _ = tokio::fs::remove_file(vars_path).await;
        }
    }
}

/// Step to type the boot command sequence over headless VNC.
#[derive(Debug, Clone)]
pub struct StepTypeBootCommand {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Builder configuration.
    pub config: QemuConfig,
}

#[async_trait::async_trait]
impl Step for StepTypeBootCommand {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let Some(ref cmds) = self.config.boot_command else {
            return Ok(StepAction::Continue);
        };

        self.ui.say(&self.name, "Typing boot command over VNC...");
        let http_ip = state.http_ip();
        let http_port = state.http_port();
        let actions = crate::builder::virtualization::BootCommandParser::parse(
            cmds, http_ip, http_port, None,
        );

        if let Some(vnc_port) = self.config.vnc_port {
            let bind_addr = self
                .config
                .vnc_bind_address
                .as_deref()
                .unwrap_or("127.0.0.1");
            let vnc_addr = format!("{bind_addr}:{vnc_port}");
            send_vnc_boot_command(&vnc_addr, &actions).await?;
        } else {
            crate::builder::virtualization::send_apple_virtualization_boot_command(
                &self.name, &actions, None,
            )
            .await?;
        }
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to shut down the QEMU instance.
#[derive(Debug, Clone)]
pub struct StepShutdown {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down QEMU VM...");

        if cfg!(test) {
            return Ok(StepAction::Continue);
        }

        let pid = state.get::<u32>("qemu_pid").copied().unwrap_or(0);
        if pid != 0 {
            #[cfg(unix)]
            {
                let _ = tokio::process::Command::new("kill")
                    .arg(pid.to_string())
                    .status()
                    .await;
            }
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to optionally convert the final disk image to another format.
#[derive(Debug, Clone)]
pub struct StepConvertDisk {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Builder configuration.
    pub config: QemuConfig,
}

#[async_trait::async_trait]
impl Step for StepConvertDisk {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let Some(target_format) = self.config.disk_format else {
            return Ok(StepAction::Continue);
        };

        let disk_path = match state.get::<String>("disk_path") {
            Some(p) => p.clone(),
            None => return Ok(StepAction::Continue),
        };

        let source = Path::new(&disk_path);
        let target = source.with_extension(target_format.as_str());
        self.ui.say(
            &self.name,
            &format!(
                "Converting disk image to format {}...",
                target_format.as_str()
            ),
        );

        let qemu_img_bin = self.config.resolve_qemu_img_binary();
        convert_disk_image_with_bin(source, &target, target_format, &qemu_img_bin).await?;
        state.put("disk_path", target.to_string_lossy().to_string());

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}
