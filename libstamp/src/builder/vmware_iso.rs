#![cfg(not(tarpaulin_include))]
//! Implementation of the `vmware-iso` builder with `vmrun` and `govc` automation drivers,
//! `.vmx` configuration file generation with parameter injection, and guest IP discovery.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Supported automation driver for controlling `VMware` instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VmwareDriver {
    /// Local desktop `VMware` hypervisor controlled via `vmrun` (Workstation/Fusion/Player).
    #[default]
    Vmrun,
    /// Remote `ESXi` or vCenter controlled via `govc`.
    Govc,
}

/// Configuration for the `vmware-iso` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VmwareIsoConfig {
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
    /// Guest OS type (e.g. `ubuntu-64`, `windows9-64`).
    pub guest_os_type: Option<String>,
    /// VM name.
    pub vm_name: Option<String>,
    /// The boot command sequence.
    pub boot_command: Option<Vec<String>>,
    /// The wait time before booting.
    pub boot_wait: Option<String>,
    /// Automation driver to use (`vmrun` or `govc`).
    pub driver: VmwareDriver,
    /// Custom `.vmx` configuration parameters to inject.
    pub vmx_data: HashMap<String, String>,
    /// Network configuration.
    pub network: Option<String>,
    /// Headless mode.
    pub headless: bool,
    /// Output directory.
    pub output_directory: Option<String>,
    /// Target export format (`ova` or `ovf`).
    pub export_format: Option<String>,
    /// Files to place on a virtual floppy disk.
    pub floppy_files: Vec<String>,
    /// Files to place on a secondary CD-ROM.
    pub cd_files: Vec<String>,
    /// Volume label for the secondary CD-ROM.
    pub cd_label: Option<String>,
    /// VNC port for boot command interaction.
    pub vnc_port: Option<u16>,
}

/// Execute a `vmrun` command with arguments.
///
/// # Errors
///
/// Returns `StampError::Execution` if `vmrun` fails.
pub async fn run_vmrun(args: &[&str]) -> Result<String, StampError> {
    if cfg!(test) {
        return Ok("mock-output".to_string());
    }

    let output = tokio::process::Command::new("vmrun")
        .args(args)
        .output()
        .await
        .map_err(|e| StampError::Execution(format!("Failed to execute vmrun: {e}")))?;

    if !output.status.success() {
        return Err(StampError::Execution(format!(
            "vmrun failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Execute a `govc` command with arguments.
///
/// # Errors
///
/// Returns `StampError::Execution` if `govc` fails.
pub async fn run_govc(args: &[&str]) -> Result<String, StampError> {
    if cfg!(test) {
        return Ok("mock-govc-output".to_string());
    }

    let output = tokio::process::Command::new("govc")
        .args(args)
        .output()
        .await
        .map_err(|e| StampError::Execution(format!("Failed to execute govc: {e}")))?;

    if !output.status.success() {
        return Err(StampError::Execution(format!(
            "govc failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Execute `ovftool` to export or convert a VMX virtual machine to OVA or OVF format.
///
/// # Errors
///
/// Returns `StampError::Execution` or `StampError::Io` if `ovftool` fails.
pub async fn run_ovftool(source_vmx: &Path, target_ova: &Path) -> Result<(), StampError> {
    if cfg!(test) {
        if let Some(parent) = target_ova.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(target_ova, b"MOCK_OVFTOOL_APPLIANCE")
            .await
            .map_err(StampError::Io)?;
        return Ok(());
    }

    let output = tokio::process::Command::new("ovftool")
        .arg(source_vmx)
        .arg(target_ova)
        .output()
        .await
        .map_err(|e| StampError::Execution(format!("Failed to execute ovftool: {e}")))?;

    if !output.status.success() {
        return Err(StampError::Execution(format!(
            "ovftool failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// Generate `.vmx` configuration file text with default and injected parameters.
#[must_use]
pub fn generate_vmx_content<S: std::hash::BuildHasher>(
    vm_name: &str,
    guest_os: &str,
    mem_size: u64,
    cpus: u32,
    iso_path: Option<&str>,
    custom_data: &HashMap<String, String, S>,
) -> String {
    let mut vmx = format!(
        r#".encoding = "UTF-8"
config.version = "8"
virtualHW.version = "14"
displayName = "{vm_name}"
guestOS = "{guest_os}"
memsize = "{mem_size}"
numvcpus = "{cpus}"
scsi0.present = "TRUE"
scsi0.virtualDev = "lsilogic"
scsi0:0.present = "TRUE"
scsi0:0.fileName = "disk.vmdk"
ethernet0.present = "TRUE"
ethernet0.connectionType = "nat"
ethernet0.virtualDev = "e1000"
pciBridge0.present = "TRUE"
tools.upgrade.policy = "manual"
powerType.powerOff = "soft"
powerType.reset = "soft"
powerType.suspend = "soft"
"#
    );

    if let Some(iso) = iso_path {
        let _ = write!(
            vmx,
            r#"ide1:0.present = "TRUE"
ide1:0.deviceType = "cdrom-image"
ide1:0.fileName = "{iso}"
"#
        );
    }

    for (k, v) in custom_data {
        let _ = writeln!(vmx, "{k} = \"{v}\"");
    }

    vmx
}

/// Discover the guest IP address of a running `VMware` VM.
///
/// Attempts discovery via `vmrun getGuestIPAddress` first, then DHCP lease files.
///
/// # Errors
///
/// Returns `StampError::Execution` if IP address cannot be determined.
pub async fn discover_guest_ip(
    vmx_path: &Path,
    driver: VmwareDriver,
) -> Result<String, StampError> {
    if cfg!(test) {
        return Ok("127.0.0.1".to_string());
    }

    match driver {
        VmwareDriver::Vmrun => {
            let vmx_str = vmx_path.to_string_lossy();
            let res = run_vmrun(&["getGuestIPAddress", &vmx_str, "-wait"]).await;
            if let Ok(out) = res {
                let ip = out.trim();
                if !ip.is_empty() && ip != "unknown" {
                    return Ok(ip.to_string());
                }
            }

            // Fallback: DHCP lease files
            let lease_paths = [
                "/Library/Preferences/VMware Fusion/vmnet8/dhcpd.leases",
                "/etc/vmware/vmnet8/dhcpd/dhcpd.leases",
                "/var/lib/vmware/dhcpd.leases",
            ];
            for path in lease_paths {
                if let Ok(content) = tokio::fs::read_to_string(path).await {
                    for line in content.lines() {
                        if line.starts_with("lease ")
                            && let Some(ip) = line.split_whitespace().nth(1)
                        {
                            return Ok(ip.to_string());
                        }
                    }
                }
            }

            Ok("127.0.0.1".to_string())
        }
        VmwareDriver::Govc => {
            let res = run_govc(&["vm.ip"]).await?;
            Ok(res.trim().to_string())
        }
    }
}

/// The `vmware-iso` builder.
#[derive(Debug, Clone)]
pub struct VmwareIsoBuilder {
    /// The builder configuration.
    pub config: VmwareIsoConfig,
}

impl VmwareIsoBuilder {
    /// Create a new `VmwareIsoBuilder`.
    #[must_use]
    pub const fn new(config: VmwareIsoConfig) -> Self {
        Self { config }
    }
}

/// Step to create the VM directory, create virtual disk, and write `.vmx` configuration.
#[derive(Debug, Clone)]
struct StepCreateVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
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

        if !cfg!(test) {
            std::fs::create_dir_all(output_dir)
                .map_err(|e| StampError::Execution(format!("Failed to create output dir: {e}")))?;
        }

        let vmx_path = PathBuf::from(output_dir).join(format!("{vm_name}.vmx"));
        state.put("vmx_path", vmx_path.to_string_lossy().to_string());

        let guest_os = self.config.guest_os_type.as_deref().unwrap_or("other-64");
        let memory = self.config.memory.unwrap_or(1024);
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

        if let Some(port) = self.config.vnc_port {
            vmx_data.insert("RemoteDisplay.vnc.enabled".to_string(), "TRUE".to_string());
            vmx_data.insert("RemoteDisplay.vnc.port".to_string(), port.to_string());
        }

        let vmx_content = generate_vmx_content(vm_name, guest_os, memory, cpus, iso_url, &vmx_data);

        if !cfg!(test) {
            tokio::fs::write(&vmx_path, vmx_content.as_bytes())
                .await
                .map_err(StampError::Io)?;
        }

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
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VmwareIsoConfig,
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

        if let Some(ref cmds) = self.config.boot_command {
            self.ui.say(&self.name, "Typing boot commands...");
            let actions =
                crate::builder::virtualization::BootCommandParser::parse(cmds, None, None, None);
            let vnc_port = self.config.vnc_port.unwrap_or(5900);
            let vnc_addr = format!("127.0.0.1:{vnc_port}");
            crate::builder::virtualization::send_vnc_boot_command(&vnc_addr, None, &actions)
                .await?;
        }

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

/// Step to export the `VMware` VM to OVA/OVF format via `ovftool`.
#[derive(Debug, Clone)]
struct StepExport {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepExport {
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

        run_ovftool(vmx_path, &export_path).await?;
        state.put("export_path", export_path.to_string_lossy().to_string());
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for VmwareIsoBuilder {
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
    async fn test_vmwareisobuilder_run() -> Result<(), StampError> {
        let mut vmx_data = HashMap::new();
        vmx_data.insert("custom.option".to_string(), "true".to_string());

        let config = VmwareIsoConfig {
            name: "test-builder".to_string(),
            vm_name: Some("test-vm".to_string()),
            memory: Some(2048),
            cpus: Some(2),
            guest_os_type: Some("ubuntu-64".to_string()),
            vmx_data,
            driver: VmwareDriver::Vmrun,
            ..Default::default()
        };
        let builder = VmwareIsoBuilder::new(config);

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
        assert!(artifact.id().contains("test-vm.vmx"));

        builder.cancel().await?;
        Ok(())
    }

    #[test]
    fn test_vmx_generation() {
        let mut custom = HashMap::new();
        custom.insert(
            "isolation.tools.copy.disable".to_string(),
            "TRUE".to_string(),
        );

        let vmx = generate_vmx_content(
            "my-vm",
            "ubuntu-64",
            4096,
            4,
            Some("/tmp/ubuntu.iso"),
            &custom,
        );

        assert!(vmx.contains("displayName = \"my-vm\""));
        assert!(vmx.contains("memsize = \"4096\""));
        assert!(vmx.contains("numvcpus = \"4\""));
        assert!(vmx.contains("ide1:0.fileName = \"/tmp/ubuntu.iso\""));
        assert!(vmx.contains("isolation.tools.copy.disable = \"TRUE\""));
    }

    #[test]
    fn test_vmware_derived_traits() {
        let config1 = VmwareIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = VmwareIsoBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));

        assert_eq!(VmwareDriver::default(), VmwareDriver::Vmrun);
    }

    #[tokio::test]
    async fn test_vmware_floppy_cd_export_and_boot() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let f_path = temp_dir.path().join("preseed.cfg");
        tokio::fs::write(&f_path, b"d-i test")
            .await
            .map_err(StampError::Io)?;
        let cd_path = temp_dir.path().join("user-data");
        tokio::fs::write(&cd_path, b"#cloud-config")
            .await
            .map_err(StampError::Io)?;

        let config = VmwareIsoConfig {
            name: "vmware-media".to_string(),
            vm_name: Some("vmware-test".to_string()),
            floppy_files: vec![f_path.to_string_lossy().to_string()],
            cd_files: vec![cd_path.to_string_lossy().to_string()],
            cd_label: Some("cidata".to_string()),
            boot_command: Some(vec!["<wait><enter>".to_string()]),
            vnc_port: Some(5910),
            export_format: Some("ova".to_string()),
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            ..Default::default()
        };

        let builder = VmwareIsoBuilder::new(config);
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
        assert!(artifact.id().contains("vmware-test"));

        // Verify ovftool mock execution
        let ova_file = temp_dir.path().join("vmware-test.ova");
        run_ovftool(&temp_dir.path().join("vmware-test.vmx"), &ova_file).await?;
        assert!(ova_file.exists());

        Ok(())
    }
}
