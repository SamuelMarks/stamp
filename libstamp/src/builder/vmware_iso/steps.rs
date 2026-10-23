//! Multistep execution steps for `vmware-iso`.

use crate::builder::vmware_iso::config::{VmwareDriver, VmwareIsoConfig, VmwareToolsMode};
use crate::builder::vmware_iso::driver::{
    discover_guest_ip_with_cmd, find_vmware_tools_iso, govc_binary, ovftool_binary,
    run_ovftool_with_cmd, run_vmrun_with_cmd, vmrun_binary,
};
use crate::builder::vmware_iso::vmx::generate_vmx_content_full;
use crate::communicator::Communicator;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::communicator::winrm::{WinRmCommunicator, WinRmConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Step to create the VM directory, create virtual disk, and write `.vmx` configuration.
#[derive(Debug, Clone)]
pub struct StepCreateVM {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Builder configuration.
    pub config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateVM {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self
            .config
            .vm_name
            .as_deref()
            .unwrap_or("packer-vmware-iso");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-vmware-iso");

        self.ui.say(
            &self.name,
            &format!("Creating VMware VM {vm_name} in {output_dir}"),
        );

        std::fs::create_dir_all(output_dir)
            .map_err(|e| StampError::Builder(format!("Failed to create output dir: {e}")))?;

        let vmx_path = PathBuf::from(output_dir).join(format!("{vm_name}.vmx"));
        state.put("vmx_path", vmx_path.to_string_lossy().to_string());

        let guest_os = self.config.guest_os_type.as_deref().unwrap_or("other-64");
        let memory = self.config.memory.map_or(1024, |m| m.get());
        let cpus = self.config.cpus.unwrap_or(1);
        let iso_url = self.config.iso_url.as_deref();

        let mut vmx_data = self.config.vmx_data.clone();

        if !self.config.floppy_files.is_empty() {
            let floppy_path = format!("{output_dir}/{vm_name}-floppy.img");
            crate::builder::virtualization::generate_floppy_disk(
                &self.config.floppy_files,
                Path::new(&floppy_path),
            )
            .await?;
            vmx_data.insert("floppy0.present".to_string(), "TRUE".to_string());
            vmx_data.insert("floppy0.fileType".to_string(), "file".to_string());
            vmx_data.insert("floppy0.fileName".to_string(), floppy_path);
        }

        if !self.config.cd_files.is_empty() {
            let cd_path = format!("{output_dir}/{vm_name}-cidata.iso");
            crate::builder::virtualization::generate_cdrom_iso(
                &self.config.cd_files,
                self.config.cd_label.as_deref(),
                Path::new(&cd_path),
            )
            .await?;
            vmx_data.insert("ide1:1.present".to_string(), "TRUE".to_string());
            vmx_data.insert("ide1:1.deviceType".to_string(), "cdrom-image".to_string());
            vmx_data.insert("ide1:1.fileName".to_string(), cd_path);
        }

        if self.config.tools_mode == VmwareToolsMode::Attach {
            let tools_iso = self
                .config
                .tools_source_path
                .as_ref()
                .map(PathBuf::from)
                .or_else(|| find_vmware_tools_iso(self.config.tools_upload_flavor.as_ref()))
                .unwrap_or_else(|| PathBuf::from("/usr/lib/vmware/isoimages/linux.iso"));
            let tools_str = tools_iso.to_string_lossy().to_string();
            self.ui.say(
                &self.name,
                &format!("Attaching VMware Tools ISO: {tools_str}"),
            );
            vmx_data.insert("ide1:1.present".to_string(), "TRUE".to_string());
            vmx_data.insert("ide1:1.deviceType".to_string(), "cdrom-image".to_string());
            vmx_data.insert("ide1:1.fileName".to_string(), tools_str);
        }

        let disk_path = PathBuf::from(output_dir).join("disk.vmdk");
        if !disk_path.exists() {
            let _ = tokio::fs::write(
                &disk_path,
                b"# Disk DescriptorFile
version=1
",
            )
            .await;
        }

        let vmx_content = generate_vmx_content_full(
            vm_name,
            guest_os,
            memory,
            cpus,
            self.config.cores,
            iso_url,
            self.config.disk_adapter_type.as_ref(),
            self.config.cdrom_adapter_type.as_ref(),
            self.config.network_adapter_type.as_ref(),
            self.config.network.as_deref(),
            self.config.firmware.as_ref(),
            self.config.version.as_deref(),
            self.config.usb,
            self.config.vmx_remove_ethernet_interfaces,
            self.config.vnc_port.map(|p| p.get()),
            self.config.vnc_disable_password,
            &vmx_data,
        );

        tokio::fs::write(&vmx_path, vmx_content.as_bytes())
            .await
            .map_err(StampError::Io)?;

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vmx_path) = state.get::<String>("vmx_path") {
            self.ui
                .say(&self.name, &format!("Cleaning up VMware VM: {vmx_path}"));
        }
    }
}

/// Step to launch the `VMware` virtual machine.
#[derive(Debug, Clone)]
pub struct StepRunVM {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Builder configuration.
    pub config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepRunVM {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vmx_path = state.get::<String>("vmx_path").cloned().unwrap_or_default();
        self.ui.say(&self.name, "Starting VMware VM...");

        let vmrun_cmd = self
            .config
            .vmrun_binary
            .as_deref()
            .unwrap_or_else(|| vmrun_binary());
        let mode = if self.config.headless { "nogui" } else { "gui" };
        let _ = run_vmrun_with_cmd(vmrun_cmd, &["start", &vmx_path, mode]).await;

        let driver_cmd = match self.config.driver {
            VmwareDriver::Vmrun => vmrun_cmd,
            VmwareDriver::Govc => self
                .config
                .govc_binary
                .as_deref()
                .unwrap_or_else(|| govc_binary()),
        };
        let ip =
            discover_guest_ip_with_cmd(driver_cmd, Path::new(&vmx_path), self.config.driver, &[])
                .await?;
        self.ui
            .say(&self.name, &format!("Discovered guest IP: {ip}"));
        state.put("vm_ip", ip);
        state.put("ssh_port", self.config.ssh_port.map_or(22u16, |p| p.get()));
        state.put(
            "winrm_port",
            self.config.winrm_host_port.map_or(5985u16, |p| p.get()),
        );

        if let Some(ref cmds) = self.config.boot_command {
            self.ui.say(&self.name, "Typing boot commands...");
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
                    &vmx_path, &actions, None,
                )
                .await?;
            }
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vmx_path) = state.get::<String>("vmx_path") {
            self.ui
                .say(&self.name, &format!("Stopping VMware VM: {vmx_path}"));
            let vmrun_cmd = self
                .config
                .vmrun_binary
                .as_deref()
                .unwrap_or_else(|| vmrun_binary());
            let _ = run_vmrun_with_cmd(vmrun_cmd, &["stop", vmx_path, "hard"]).await;
        }
    }
}

/// Step to upload `VMware` Tools ISO via communicator.
#[derive(Debug, Clone)]
pub struct StepUploadTools {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Builder configuration.
    pub config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepUploadTools {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if self.config.tools_mode != VmwareToolsMode::Upload {
            return Ok(StepAction::Continue);
        }

        let tools_iso = self
            .config
            .tools_source_path
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| find_vmware_tools_iso(self.config.tools_upload_flavor.as_ref()))
            .unwrap_or_else(|| PathBuf::from("/usr/lib/vmware/isoimages/linux.iso"));

        let target_path = self
            .config
            .tools_upload_path
            .as_deref()
            .unwrap_or("/tmp/vmware-tools.iso");

        self.ui.say(
            &self.name,
            &format!(
                "Uploading VMware Tools from {} to guest {}",
                tools_iso.display(),
                target_path
            ),
        );

        if let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator") {
            let _ = comm
                .upload(
                    &crate::types::FilePath::new(tools_iso),
                    &crate::types::FilePath::new(PathBuf::from(target_path)),
                )
                .await;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to provision the VM over SSH or `WinRM`.
#[derive(Clone)]
pub struct StepProvision {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Provisioning hook.
    pub hook: Arc<dyn ProvisionHook>,
    /// Builder configuration.
    pub config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning VMware VM...");

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
            host: "vmware".to_string(),
            user: "root".to_string(),
            packer_run_uuid: uuid::Uuid::new_v4().to_string(),
            source_name: self.name.clone(),
            source_type: "vmware-iso".to_string(),
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
            if let Some(handle) =
                state.get::<crate::builder::http_server::HttpServerHandle>("http_server_handle")
            {
                handle.abort();
            }
            return Err(e);
        }

        if let Some(handle) =
            state.get::<crate::builder::http_server::HttpServerHandle>("http_server_handle")
        {
            handle.abort();
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to shut down the VM.
#[derive(Debug, Clone)]
pub struct StepShutdown {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Builder configuration.
    pub config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down VMware VM...");
        let vmx_path = state.get::<String>("vmx_path").cloned().unwrap_or_default();

        if let Some(ref cmd) = self.config.shutdown_command
            && let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator")
        {
            self.ui
                .say(&self.name, &format!("Executing shutdown command: {cmd}"));
            let _ = comm
                .execute(&crate::communicator::Command::new(cmd.clone()))
                .await;
        }

        let vmrun_cmd = self
            .config
            .vmrun_binary
            .as_deref()
            .unwrap_or_else(|| vmrun_binary());
        let _ = run_vmrun_with_cmd(vmrun_cmd, &["stop", &vmx_path, "soft"]).await;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to export the `VMware` VM to OVA/OVF format via `ovftool`.
#[derive(Debug, Clone)]
pub struct StepExport {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// Builder configuration.
    pub config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepExport {
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
            .unwrap_or("output-vmware-iso");
        let vm_name = self.config.vm_name.as_deref().unwrap_or("vm");
        let export_path = PathBuf::from(output_dir).join(format!("{vm_name}.{format}"));

        self.ui.say(
            &self.name,
            &format!(
                "Exporting VMware VM to {} via ovftool...",
                export_path.display()
            ),
        );

        let ovftool_cmd = self
            .config
            .ovftool_binary
            .as_deref()
            .unwrap_or_else(|| ovftool_binary());
        run_ovftool_with_cmd(ovftool_cmd, vmx_path, &export_path).await?;
        state.put("export_path", export_path.to_string_lossy().to_string());
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}
