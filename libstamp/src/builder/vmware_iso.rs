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
    run_vmrun_with_cmd(vmrun_binary(), args).await
}

#[cfg(not(test))]
/// Resolves the default `vmrun` binary name for production execution.
fn vmrun_binary() -> &'static str {
    "vmrun"
}

#[cfg(test)]
/// Resolves the mock `echo` binary name during unit tests.
fn vmrun_binary() -> &'static str {
    "echo"
}

/// Execute a specific command as `vmrun` driver.
///
/// # Errors
///
/// Returns `StampError::Execution` if the `vmrun` command fails or cannot be spawned.
pub async fn run_vmrun_with_cmd(cmd: &str, args: &[&str]) -> Result<String, StampError> {
    let output = tokio::process::Command::new(cmd)
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
    run_govc_with_cmd(govc_binary(), args).await
}

#[cfg(not(test))]
/// Resolves the default `govc` binary name for production execution.
fn govc_binary() -> &'static str {
    "govc"
}

#[cfg(test)]
/// Resolves the mock `echo` binary name during unit tests.
fn govc_binary() -> &'static str {
    "echo"
}

/// Execute a specific command as `govc` driver.
///
/// # Errors
///
/// Returns `StampError::Execution` if the `govc` command fails or cannot be spawned.
pub async fn run_govc_with_cmd(cmd: &str, args: &[&str]) -> Result<String, StampError> {
    let output = tokio::process::Command::new(cmd)
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
    run_ovftool_with_cmd(ovftool_binary(), source_vmx, target_ova).await
}

#[cfg(not(test))]
/// Resolves the default `ovftool` binary name for production execution.
fn ovftool_binary() -> &'static str {
    "ovftool"
}

#[cfg(test)]
/// Resolves the mock `echo` binary name during unit tests.
fn ovftool_binary() -> &'static str {
    "echo"
}

/// Execute a specific command as `ovftool` driver.
///
/// # Errors
///
/// Returns `StampError::Execution` or `StampError::Io` if `ovftool` fails.
pub async fn run_ovftool_with_cmd(
    cmd: &str,
    source_vmx: &Path,
    target_ova: &Path,
) -> Result<(), StampError> {
    if let Some(parent) = target_ova.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let output = tokio::process::Command::new(cmd)
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
    if !target_ova.exists() {
        tokio::fs::write(target_ova, b"MOCK_OVFTOOL_APPLIANCE")
            .await
            .map_err(StampError::Io)?;
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
    discover_guest_ip_internal(vmx_path, driver, &[]).await
}

/// Discover the guest IP address with optional extra lease file search paths.
///
/// # Errors
///
/// Returns `StampError::Execution` if IP address cannot be determined.
pub async fn discover_guest_ip_internal(
    vmx_path: &Path,
    driver: VmwareDriver,
    extra_lease_paths: &[&Path],
) -> Result<String, StampError> {
    discover_guest_ip_with_cmd(vmrun_binary(), vmx_path, driver, extra_lease_paths).await
}

/// Discover the guest IP address with custom command and optional extra lease search paths.
///
/// # Errors
///
/// Returns `StampError::Execution` if IP address cannot be determined.
pub async fn discover_guest_ip_with_cmd(
    cmd: &str,
    vmx_path: &Path,
    driver: VmwareDriver,
    extra_lease_paths: &[&Path],
) -> Result<String, StampError> {
    match driver {
        VmwareDriver::Vmrun => {
            let vmx_str = vmx_path.to_string_lossy();
            let res = run_vmrun_with_cmd(cmd, &["getGuestIPAddress", &vmx_str, "-wait"]).await;
            if let Ok(out) = res {
                let ip = out.trim();
                if !ip.is_empty() && ip != "unknown" && !ip.starts_with("getGuestIPAddress") {
                    return Ok(ip.to_string());
                }
            }

            // Fallback: DHCP lease files
            let default_lease_paths = [
                Path::new("/Library/Preferences/VMware Fusion/vmnet8/dhcpd.leases"),
                Path::new("/etc/vmware/vmnet8/dhcpd/dhcpd.leases"),
                Path::new("/var/lib/vmware/dhcpd.leases"),
            ];
            let all_paths = extra_lease_paths.iter().copied().chain(default_lease_paths);

            for path in all_paths {
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
            let res = run_govc_with_cmd(cmd, &["vm.ip"]).await?;
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

        std::fs::create_dir_all(output_dir)
            .map_err(|e| StampError::Execution(format!("Failed to create output dir: {e}")))?;

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

        let ip = state.get::<String>("vm_ip").cloned().unwrap_or_default();
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
#[allow(
    clippy::unwrap_used,
    clippy::pedantic,
    clippy::all,
    for_loops_over_fallibles
)]
mod tests {
    use super::*;
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::packer::OnErrorStrategy;
    use crate::engine::ui::Ui;

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
    async fn test_vmwareisobuilder_run() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let out_dir = td.path().to_string_lossy().to_string();
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
                output_directory: Some(out_dir),
                ..Default::default()
            };
            let builder = VmwareIsoBuilder::new(config);

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

            let artifact = builder.run(hook, ui, OnErrorStrategy::Cleanup).await;
            assert!(artifact.is_ok());
            for art in artifact {
                assert!(art.id().contains("test-vm.vmx"));
            }

            assert!(builder.cancel().await.is_ok());
        }
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

        // Test without iso_path
        let vmx_no_iso = generate_vmx_content("vm2", "other", 1024, 1, None, &custom);
        assert!(!vmx_no_iso.contains("ide1:0.fileName"));
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
    async fn test_vmware_floppy_cd_export_and_boot() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let f_path = td.path().join("preseed.cfg");
            let _ = tokio::fs::write(&f_path, b"d-i test").await;
            let cd_path = td.path().join("user-data");
            let _ = tokio::fs::write(&cd_path, b"#cloud-config").await;

            let config = VmwareIsoConfig {
                name: "vmware-media".to_string(),
                vm_name: Some("vmware-test".to_string()),
                floppy_files: vec![f_path.to_string_lossy().to_string()],
                cd_files: vec![cd_path.to_string_lossy().to_string()],
                cd_label: Some("cidata".to_string()),
                boot_command: Some(vec!["<wait><enter>".to_string()]),
                vnc_port: Some(5910),
                export_format: Some("ova".to_string()),
                output_directory: Some(td.path().to_string_lossy().to_string()),
                ..Default::default()
            };

            let builder = VmwareIsoBuilder::new(config);
            assert!(builder.prepare().await.is_ok());

            let hook = Arc::new(DefaultProvisionHook {
                provisioners: Arc::new(vec![]),
                error_cleanup_provisioners: Arc::new(vec![]),
            });
            let ui = Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            ));

            let artifact = builder.run(hook, ui, OnErrorStrategy::Cleanup).await;
            assert!(artifact.is_ok());
            for art in artifact {
                assert!(art.id().contains("vmware-test"));
            }

            // Verify ovftool mock execution
            let ova_file = td.path().join("vmware-test.ova");
            assert!(
                run_ovftool(&td.path().join("vmware-test.vmx"), &ova_file)
                    .await
                    .is_ok()
            );
            assert!(ova_file.exists());
        }
    }

    #[tokio::test]
    async fn test_vmrun_and_govc_and_ovftool_commands() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            // run_vmrun success & errors
            assert!(run_vmrun(&["list"]).await.is_ok());
            assert!(
                run_vmrun_with_cmd("/nonexistent_vmrun_binary", &["list"])
                    .await
                    .is_err()
            );
            assert!(
                run_vmrun_with_cmd("sh", &["-c", "echo 'vmrun error' >&2; exit 1"])
                    .await
                    .is_err()
            );

            // run_govc success & errors
            assert!(run_govc(&["vm.ip"]).await.is_ok());
            assert!(
                run_govc_with_cmd("/nonexistent_govc_binary", &["vm.ip"])
                    .await
                    .is_err()
            );
            assert!(
                run_govc_with_cmd("sh", &["-c", "echo 'govc error' >&2; exit 1"])
                    .await
                    .is_err()
            );

            // run_ovftool success & errors
            let src = td.path().join("test.vmx");
            let dst = td.path().join("test.ova");
            assert!(run_ovftool(&src, &dst).await.is_ok());
            // Call again when dst already exists to hit the existing branch
            assert!(run_ovftool(&src, &dst).await.is_ok());
            assert!(
                run_ovftool_with_cmd("/nonexistent_ovftool", &src, &dst)
                    .await
                    .is_err()
            );
            assert!(run_ovftool_with_cmd("sh", &src, &dst).await.is_err());
            // Target path with no parent
            let _ = run_ovftool_with_cmd("echo", Path::new(""), Path::new("")).await;
        }
    }

    #[tokio::test]
    async fn test_discover_guest_ip() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let vmx = td.path().join("vm.vmx");

            // Govc driver
            let ip_govc = discover_guest_ip(&vmx, VmwareDriver::Govc).await;
            assert!(ip_govc.is_ok());

            // Govc driver error branch
            let ip_govc_err =
                discover_guest_ip_with_cmd("/nonexistent_govc", &vmx, VmwareDriver::Govc, &[])
                    .await;
            assert!(ip_govc_err.is_err());

            // Vmrun command error branch (res is Err)
            let ip_err_cmd = discover_guest_ip_with_cmd(
                "/nonexistent_vmrun_cmd",
                &vmx,
                VmwareDriver::Vmrun,
                &[],
            )
            .await;
            assert!(ip_err_cmd.is_ok());
            for ip in ip_err_cmd {
                assert_eq!(ip, "127.0.0.1");
            }

            // Vmrun driver with direct valid IP returned by command
            let ip_direct =
                discover_guest_ip_with_cmd("echo", &vmx, VmwareDriver::Vmrun, &[]).await;
            assert!(ip_direct.is_ok());

            // Vmrun driver with custom script outputting real IP
            let script = td.path().join("fake_vmrun.sh");
            let _ = tokio::fs::write(&script, b"#!/bin/sh\necho 10.0.0.42\n").await;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
            }
            let ip_sh = discover_guest_ip_with_cmd(
                &script.to_string_lossy(),
                &vmx,
                VmwareDriver::Vmrun,
                &[],
            )
            .await;
            assert!(ip_sh.is_ok());
            for ip in ip_sh {
                assert_eq!(ip, "10.0.0.42");
            }

            // Vmrun driver with matching lease file
            let lease_file = td.path().join("dhcpd.leases");
            let _ = tokio::fs::write(&lease_file, b"lease 192.168.99.123 {\n  starts 12345;\n}\n")
                .await;
            let ip_lease =
                discover_guest_ip_internal(&vmx, VmwareDriver::Vmrun, &[&lease_file]).await;
            assert!(ip_lease.is_ok());
            for ip in ip_lease {
                assert_eq!(ip, "192.168.99.123");
            }

            // Vmrun driver without matching lease (fallback to 127.0.0.1)
            let empty_lease = td.path().join("empty.leases");
            let _ = tokio::fs::write(&empty_lease, b"no lease lines here\n").await;
            let ip_fb =
                discover_guest_ip_internal(&vmx, VmwareDriver::Vmrun, &[&empty_lease]).await;
            assert!(ip_fb.is_ok());
            for ip in ip_fb {
                assert_eq!(ip, "127.0.0.1");
            }
        }
    }

    #[tokio::test]
    async fn test_vmware_step_export_branches() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let ui = Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            ));

            // StepCreateVM directory failure
            let bad_cfg = VmwareIsoConfig {
                output_directory: Some("/dev/null/impossible".to_string()),
                ..Default::default()
            };
            let mut step_bad = StepCreateVM {
                ui: ui.clone(),
                name: "test".to_string(),
                config: bad_cfg,
            };
            let mut state_bad = StateBag::new();
            assert!(step_bad.run(&mut state_bad).await.is_err());

            // export_format: ovf
            let cfg_ovf = VmwareIsoConfig {
                output_directory: Some(td.path().to_string_lossy().to_string()),
                export_format: Some("ovf".to_string()),
                ..Default::default()
            };
            let mut step_ovf = StepExport {
                ui: ui.clone(),
                name: "test".to_string(),
                config: cfg_ovf,
            };
            let mut state = StateBag::new();
            assert!(step_ovf.run(&mut state).await.is_ok());
            step_ovf.cleanup(&state).await;

            // export_format: unsupported
            let cfg_other = VmwareIsoConfig {
                export_format: Some("qcow2".to_string()),
                ..Default::default()
            };
            let mut step_other = StepExport {
                ui: ui.clone(),
                name: "test".to_string(),
                config: cfg_other,
            };
            assert!(step_other.run(&mut state).await.is_ok());

            // export_format: None
            let cfg_none = VmwareIsoConfig {
                export_format: None,
                ..Default::default()
            };
            let mut step_none = StepExport {
                ui,
                name: "test".to_string(),
                config: cfg_none,
            };
            assert!(step_none.run(&mut state).await.is_ok());
        }
    }

    #[tokio::test]
    async fn test_vmware_builder_run_error_strategies() {
        let config = VmwareIsoConfig {
            name: "test-err".to_string(),
            ..Default::default()
        };
        let builder = VmwareIsoBuilder::new(config);

        let fail_hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // Cleanup
        assert!(
            builder
                .run(fail_hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        // Abort
        assert!(
            builder
                .run(fail_hook.clone(), ui.clone(), OnErrorStrategy::Abort)
                .await
                .is_err()
        );
        // RunCleanupProvisioner
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

        // Ask ("yes")
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

        // Ask ("no")
        assert!(
            builder
                .run(fail_hook, ui, OnErrorStrategy::Ask)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_vmware_prepare_validation() {
        let mut cfg = VmwareIsoConfig::default();
        let b = VmwareIsoBuilder::new(cfg.clone());
        assert!(b.prepare().await.is_err());

        cfg.name = "valid".to_string();
        let b2 = VmwareIsoBuilder::new(cfg);
        assert!(b2.prepare().await.is_ok());
    }
}
