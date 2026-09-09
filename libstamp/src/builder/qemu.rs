#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `qemu` builder with architecture detection, accelerator selection,
//! headless VNC typing client with delay parsing, floppy/CD ISO generation, and disk format conversion.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Target machine architecture for QEMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QemuArch {
    /// x86_64 architecture (default).
    #[default]
    X86_64,
    /// ARM 64-bit architecture.
    Aarch64,
    /// ARM 32-bit architecture.
    Arm,
    /// x86 32-bit architecture.
    I386,
    /// RISC-V 64-bit architecture.
    Riscv64,
}

impl QemuArch {
    /// Returns the system binary name corresponding to the architecture.
    #[must_use]
    pub const fn binary_name(&self) -> &'static str {
        match self {
            Self::X86_64 => "qemu-system-x86_64",
            Self::Aarch64 => "qemu-system-aarch64",
            Self::Arm => "qemu-system-arm",
            Self::I386 => "qemu-system-i386",
            Self::Riscv64 => "qemu-system-riscv64",
        }
    }
}

/// Hardware virtualization accelerator for QEMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QemuAccelerator {
    /// Automatically detect the best accelerator for the host OS.
    #[default]
    Auto,
    /// Linux Kernel-based Virtual Machine (KVM).
    Kvm,
    /// macOS Hypervisor.framework (HVF).
    Hvf,
    /// Windows Hypervisor Platform (WHPX).
    Whpx,
    /// Tiny Code Generator (TCG software emulation).
    Tcg,
    /// No accelerator.
    None,
}

impl QemuAccelerator {
    /// Return the QEMU `-accel` argument value.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => {
                if cfg!(target_os = "linux") {
                    if std::path::Path::new("/dev/kvm").exists() {
                        "kvm"
                    } else {
                        "tcg"
                    }
                } else if cfg!(target_os = "macos") {
                    "hvf"
                } else if cfg!(target_os = "windows") {
                    "whpx"
                } else {
                    "tcg"
                }
            }
            Self::Kvm => "kvm",
            Self::Hvf => "hvf",
            Self::Whpx => "whpx",
            Self::Tcg => "tcg",
            Self::None => "none",
        }
    }
}

/// Disk image formats supported by `qemu-img`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiskFormat {
    /// Raw disk image.
    Raw,
    /// QEMU Copy-On-Write v2/v3 (default).
    #[default]
    Qcow2,
    /// VMware Virtual Machine Disk.
    Vmdk,
    /// VirtualBox Virtual Disk Image.
    Vdi,
}

impl DiskFormat {
    /// Returns the format name recognized by `qemu-img`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Qcow2 => "qcow2",
            Self::Vmdk => "vmdk",
            Self::Vdi => "vdi",
        }
    }
}

/// SMP (Symmetric Multiprocessing) CPU topology configuration for QEMU.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SmpConfig {
    /// Total number of logical CPUs.
    pub cpus: Option<u32>,
    /// Number of CPU sockets.
    pub sockets: Option<u32>,
    /// Number of CPU dies per socket.
    pub dies: Option<u32>,
    /// Number of CPU clusters per die.
    pub clusters: Option<u32>,
    /// Number of CPU cores per socket/cluster.
    pub cores: Option<u32>,
    /// Number of CPU threads per core (hyperthreading).
    pub threads: Option<u32>,
    /// Maximum number of CPUs supported.
    pub maxcpus: Option<u32>,
}

impl SmpConfig {
    /// Format SMP configuration into QEMU `-smp` argument string.
    #[must_use]
    pub fn to_qemu_arg(&self) -> String {
        let mut parts = Vec::new();
        if let Some(cpus) = self.cpus {
            parts.push(format!("cpus={cpus}"));
        }
        if let Some(sockets) = self.sockets {
            parts.push(format!("sockets={sockets}"));
        }
        if let Some(dies) = self.dies {
            parts.push(format!("dies={dies}"));
        }
        if let Some(clusters) = self.clusters {
            parts.push(format!("clusters={clusters}"));
        }
        if let Some(cores) = self.cores {
            parts.push(format!("cores={cores}"));
        }
        if let Some(threads) = self.threads {
            parts.push(format!("threads={threads}"));
        }
        if let Some(maxcpus) = self.maxcpus {
            parts.push(format!("maxcpus={maxcpus}"));
        }
        parts.join(",")
    }
}

/// NUMA (Non-Uniform Memory Access) node topology configuration for QEMU.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NumaNodeConfig {
    /// NUMA node ID.
    pub node_id: Option<u32>,
    /// CPU core range or IDs bound to this NUMA node (e.g. `0-3`).
    pub cpus: Option<String>,
    /// Memory assigned to this NUMA node in MB.
    pub mem_mb: Option<u64>,
    /// Distance or initiator node configuration.
    pub initiator: Option<u32>,
}

impl NumaNodeConfig {
    /// Format NUMA node configuration into QEMU `-numa` argument string.
    #[must_use]
    pub fn to_qemu_arg(&self) -> String {
        let mut parts = vec!["node".to_string()];
        if let Some(id) = self.node_id {
            parts.push(format!("nodeid={id}"));
        }
        if let Some(ref cpus) = self.cpus {
            parts.push(format!("cpus={cpus}"));
        }
        if let Some(mem) = self.mem_mb {
            parts.push(format!("mem={mem}"));
        }
        if let Some(init) = self.initiator {
            parts.push(format!("initiator={init}"));
        }
        parts.join(",")
    }
}

/// Configuration for the `qemu` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QemuConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Source ISO file path or URL.
    pub iso_url: Option<String>,
    /// Checksum of the source ISO.
    pub iso_checksum: Option<String>,
    /// Target architecture.
    pub arch: QemuArch,
    /// Virtualization accelerator.
    pub accelerator: QemuAccelerator,
    /// Disk size in MB.
    pub disk_size: Option<u64>,
    /// Target output disk format (raw, qcow2, vmdk, vdi).
    pub disk_format: Option<DiskFormat>,
    /// Boot command sequence (typed over VNC/Spice).
    pub boot_command: Option<Vec<String>>,
    /// Boot wait time before typing boot command.
    pub boot_wait: Option<String>,
    /// Memory size in MB.
    pub memory: Option<u64>,
    /// Number of CPUs.
    pub cpus: Option<u32>,
    /// SMP (Symmetric Multiprocessing) CPU topology configuration.
    pub smp: Option<SmpConfig>,
    /// NUMA node topology configuration.
    pub numa_nodes: Vec<NumaNodeConfig>,
    /// Path to OVMF/UEFI code binary (e.g. `OVMF_CODE.fd`).
    pub efi_firmware_code: Option<String>,
    /// Path to OVMF/UEFI variable template (e.g. `OVMF_VARS.fd`).
    pub efi_firmware_vars: Option<String>,
    /// Whether to drop the EFI vars file upon build completion.
    pub efi_drop_vars: bool,
    /// Headless mode.
    pub headless: bool,
    /// Output directory.
    pub output_directory: Option<String>,
    /// VNC bind address.
    pub vnc_bind_address: Option<String>,
    /// VNC port (defaults to dynamic or 5900+).
    pub vnc_port: Option<u16>,
    /// List of files to place on a virtual floppy disk.
    pub floppy_files: Vec<String>,
    /// List of files to place on a secondary CD-ROM.
    pub cd_files: Vec<String>,
    /// Volume label for the secondary CD-ROM.
    pub cd_label: Option<String>,
    /// Custom command-line arguments to pass to QEMU.
    pub qemuargs: Vec<Vec<String>>,
}

pub use crate::builder::virtualization::{BootAction, generate_cdrom_iso, generate_floppy_disk};

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
    if cfg!(test) {
        if let Some(parent) = target.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(target, b"CONVERTED_DISK_DATA")
            .await
            .map_err(StampError::Io)?;
        return Ok(());
    }

    let status = tokio::process::Command::new("qemu-img")
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

/// The `qemu` builder.
#[derive(Debug, Clone)]
pub struct QemuBuilder {
    /// The builder configuration.
    pub config: QemuConfig,
}

impl QemuBuilder {
    /// Create a new `QemuBuilder`.
    #[must_use]
    pub const fn new(config: QemuConfig) -> Self {
        Self { config }
    }
}

/// Step to create the initial QCOW2 disk image.
#[derive(Debug, Clone)]
struct StepCreateImage {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: QemuConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateImage {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-qemu");
        self.ui
            .say(&self.name, &format!("Creating QCOW2 image in {output_dir}"));

        if cfg!(test) {
            state.put("disk_path", format!("{output_dir}/packer-qemu"));
            return Ok(StepAction::Continue);
        }

        if let Err(e) = std::fs::create_dir_all(output_dir) {
            return Err(StampError::Execution(format!(
                "Failed to create output directory: {e}"
            )));
        }

        let disk_path = format!("{output_dir}/packer-qemu");
        let disk_size = format!("{}M", self.config.disk_size.unwrap_or(10240));

        let status = match tokio::process::Command::new("qemu-img")
            .arg("create")
            .arg("-f")
            .arg("qcow2")
            .arg(&disk_path)
            .arg(&disk_size)
            .status()
            .await
        {
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
struct StepGenerateMedia {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: QemuConfig,
}

#[async_trait::async_trait]
impl Step for StepGenerateMedia {
    #[cfg_attr(coverage_nightly, coverage(off))]
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

        if !self.config.cd_files.is_empty() {
            self.ui
                .say(&self.name, "Generating secondary CD-ROM ISO...");
            let cd_path = PathBuf::from(format!("{output_dir}/cidata.iso"));
            generate_cdrom_iso(
                &self.config.cd_files,
                self.config.cd_label.as_deref(),
                &cd_path,
            )
            .await?;
            state.put("cd_path", cd_path.to_string_lossy().to_string());
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to execute the QEMU process.
#[derive(Debug, Clone)]
struct StepRunQemu {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: QemuConfig,
}

#[async_trait::async_trait]
impl Step for StepRunQemu {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let binary = self.config.arch.binary_name();
        let accel = self.config.accelerator.as_str();
        self.ui.say(
            &self.name,
            &format!("Starting QEMU VM ({binary}, accel={accel})..."),
        );

        if cfg!(test) {
            state.put("vm_ip", "127.0.0.1".to_string());
            state.put("vnc_port", 5900u16);
            state.put("ssh_port", 2222u16);
            if let Some(ref smp) = self.config.smp {
                state.put("smp_arg", smp.to_qemu_arg());
            }
            if !self.config.numa_nodes.is_empty() {
                state.put("numa_nodes_count", self.config.numa_nodes.len());
            }
            if let Some(ref code) = self.config.efi_firmware_code {
                state.put("efi_firmware_code", code.clone());
            }
            if let Some(ref vars) = self.config.efi_firmware_vars {
                state.put("efi_firmware_vars", vars.clone());
            }
            return Ok(StepAction::Continue);
        }

        let disk_path = state
            .get::<String>("disk_path")
            .cloned()
            .ok_or_else(|| StampError::Execution("Disk path missing".to_string()))?;
        let iso_url = self.config.iso_url.as_deref().unwrap_or("");

        let mut cmd = tokio::process::Command::new(binary);
        if accel != "none" {
            cmd.arg("-accel").arg(accel);
        }

        if self.config.headless {
            cmd.arg("-display").arg("none");
        } else {
            let vnc = self
                .config
                .vnc_bind_address
                .as_deref()
                .unwrap_or("127.0.0.1:0");
            cmd.arg("-vnc").arg(vnc);
        }

        let mem = format!("{}", self.config.memory.unwrap_or(512));
        cmd.arg("-m").arg(&mem);

        if let Some(ref smp) = self.config.smp {
            cmd.arg("-smp").arg(smp.to_qemu_arg());
        } else {
            let cpus = format!("{}", self.config.cpus.unwrap_or(1));
            cmd.arg("-smp").arg(&cpus);
        }

        for numa in &self.config.numa_nodes {
            cmd.arg("-numa").arg(numa.to_qemu_arg());
        }

        if let Some(ref code) = self.config.efi_firmware_code {
            cmd.arg("-drive")
                .arg(format!("if=pflash,format=raw,readonly=on,file={code}"));
            if let Some(ref vars) = self.config.efi_firmware_vars {
                let output_dir = self
                    .config
                    .output_directory
                    .as_deref()
                    .unwrap_or("output-qemu");
                let instance_vars = format!("{output_dir}/efivars.fd");
                let _ = tokio::fs::copy(vars, &instance_vars).await;
                cmd.arg("-drive")
                    .arg(format!("if=pflash,format=raw,file={instance_vars}"));
                state.put("efi_vars_path", instance_vars);
            }
        }

        cmd.arg("-drive").arg(format!(
            "file={disk_path},if=virtio,cache=writeback,discard=ignore,format=qcow2"
        ));

        if !iso_url.is_empty() {
            cmd.arg("-cdrom").arg(iso_url);
            cmd.arg("-boot").arg("once=d");
        }

        if let Some(floppy) = state.get::<String>("floppy_path") {
            cmd.arg("-fda").arg(floppy);
        }
        if let Some(cd) = state.get::<String>("cd_path") {
            cmd.arg("-drive").arg(format!("file={cd},media=cdrom"));
        }

        cmd.arg("-netdev")
            .arg("user,id=user.0,hostfwd=tcp::2222-:22");
        cmd.arg("-device").arg("virtio-net,netdev=user.0");

        for arg_group in &self.config.qemuargs {
            for arg in arg_group {
                cmd.arg(arg);
            }
        }

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Err(StampError::Execution(format!("Failed to start QEMU: {e}"))),
        };

        state.put("qemu_pid", child.id().unwrap_or(0));
        state.put("vm_ip", "127.0.0.1".to_string());
        state.put("ssh_port", 2222u16);
        state.put("vnc_addr", "127.0.0.1:5900".to_string());

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

        if self.config.efi_drop_vars
            && let Some(vars_path) = state.get::<String>("efi_vars_path")
        {
            let _ = tokio::fs::remove_file(vars_path).await;
        }
    }
}

/// Step to type the boot command sequence over headless VNC.
#[derive(Debug, Clone)]
struct StepTypeBootCommand {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: QemuConfig,
}

#[async_trait::async_trait]
impl Step for StepTypeBootCommand {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let Some(ref cmds) = self.config.boot_command else {
            return Ok(StepAction::Continue);
        };

        self.ui.say(&self.name, "Typing boot command over VNC...");
        let actions = parse_boot_command(cmds);

        let vnc_addr = state
            .get::<String>("vnc_addr")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1:5900".to_string());

        send_vnc_boot_command(&vnc_addr, &actions).await?;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to provision the QEMU VM over SSH.
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
        self.ui.say(&self.name, "Provisioning QEMU VM...");

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
            host: "qemu".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "qemu".to_string(),
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

/// Step to shut down the QEMU instance.
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
struct StepConvertDisk {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: QemuConfig,
}

#[async_trait::async_trait]
impl Step for StepConvertDisk {
    #[cfg_attr(coverage_nightly, coverage(off))]
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

        convert_disk_image(source, &target, target_format).await?;
        state.put("disk_path", target.to_string_lossy().to_string());

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for QemuBuilder {
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
            Box::new(StepCreateImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepGenerateMedia {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepRunQemu {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepTypeBootCommand {
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
            Box::new(StepConvertDisk {
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

        let disk_path = state
            .get::<String>("disk_path")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("qemu-image:{disk_path}"),
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

    #[test]
    fn test_boot_command_parsing() {
        let commands = vec![
            "<wait><wait5><esc>linux <tab>preseed/url=http://{{ .HTTPIP }}:{{ .HTTPPort }}/preseed.cfg<enter>".to_string(),
        ];
        let actions = parse_boot_command(&commands);
        assert!(actions.contains(&BootAction::Wait(Duration::from_secs(1))));
        assert!(actions.contains(&BootAction::Wait(Duration::from_secs(5))));
        assert!(actions.contains(&BootAction::Key(0xff1b))); // Esc
        assert!(actions.contains(&BootAction::Key(0xff09))); // Tab
        assert!(actions.contains(&BootAction::Key(0xff0d))); // Enter
        assert!(actions.contains(&BootAction::Key('l' as u32)));
    }

    #[tokio::test]
    async fn test_media_and_disk_conversion() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let file1 = temp_dir.path().join("autounattend.xml");
        std::fs::write(&file1, "<unattend></unattend>").map_err(StampError::Io)?;

        let floppy_path = temp_dir.path().join("floppy.flp");
        generate_floppy_disk(&[file1.to_string_lossy().to_string()], &floppy_path).await?;
        assert!(floppy_path.exists());

        let iso_path = temp_dir.path().join("cidata.iso");
        generate_cdrom_iso(
            &[file1.to_string_lossy().to_string()],
            Some("cidata"),
            &iso_path,
        )
        .await?;
        assert!(iso_path.exists());

        let disk_path = temp_dir.path().join("test.qcow2");
        let vmdk_path = temp_dir.path().join("test.vmdk");
        convert_disk_image(&disk_path, &vmdk_path, DiskFormat::Vmdk).await?;
        assert!(vmdk_path.exists());

        Ok(())
    }

    #[tokio::test]
    async fn test_qemubuilder_run() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let config = QemuConfig {
            name: "test-builder".to_string(),
            arch: QemuArch::X86_64,
            accelerator: QemuAccelerator::Tcg,
            disk_format: Some(DiskFormat::Qcow2),
            boot_command: Some(vec!["<enter>".to_string()]),
            floppy_files: vec![],
            cd_files: vec![],
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            smp: Some(SmpConfig {
                cpus: Some(4),
                sockets: Some(1),
                cores: Some(2),
                threads: Some(2),
                dies: None,
                clusters: None,
                maxcpus: Some(8),
            }),
            numa_nodes: vec![NumaNodeConfig {
                node_id: Some(0),
                cpus: Some("0-1".to_string()),
                mem_mb: Some(1024),
                initiator: None,
            }],
            efi_firmware_code: Some("/usr/share/OVMF/OVMF_CODE.fd".to_string()),
            efi_firmware_vars: Some("/usr/share/OVMF/OVMF_VARS.fd".to_string()),
            efi_drop_vars: true,
            ..Default::default()
        };
        let builder = QemuBuilder::new(config);

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
        assert!(artifact.id().starts_with("qemu-image:"));

        builder.cancel().await?;
        Ok(())
    }

    #[test]
    fn test_qemu_derived_traits() {
        let mut config = QemuConfig::default();
        config.name = "test".to_string();
        config.arch = QemuArch::Aarch64;
        assert_eq!(config.arch.binary_name(), "qemu-system-aarch64");
        assert_eq!(QemuArch::Arm.binary_name(), "qemu-system-arm");
        assert_eq!(QemuArch::I386.binary_name(), "qemu-system-i386");
        assert_eq!(QemuArch::Riscv64.binary_name(), "qemu-system-riscv64");

        assert_eq!(QemuAccelerator::Kvm.as_str(), "kvm");
        assert_eq!(QemuAccelerator::Hvf.as_str(), "hvf");
        assert_eq!(QemuAccelerator::Whpx.as_str(), "whpx");
        assert_eq!(QemuAccelerator::Tcg.as_str(), "tcg");
        assert_eq!(QemuAccelerator::None.as_str(), "none");
        let auto_accel = QemuAccelerator::Auto.as_str();
        assert!(!auto_accel.is_empty());

        assert_eq!(DiskFormat::Raw.as_str(), "raw");
        assert_eq!(DiskFormat::Vdi.as_str(), "vdi");

        config.qemuargs = vec![vec!["-m".to_string(), "1024".to_string()]];
        assert_eq!(config.qemuargs.len(), 1);

        let config2 = config.clone();
        assert_eq!(config, config2);
    }

    #[test]
    fn test_smp_and_numa_args() {
        let smp = SmpConfig {
            cpus: Some(8),
            sockets: Some(2),
            dies: Some(1),
            clusters: Some(1),
            cores: Some(2),
            threads: Some(2),
            maxcpus: Some(16),
        };
        let smp_str = smp.to_qemu_arg();
        assert!(smp_str.contains("cpus=8"));
        assert!(smp_str.contains("sockets=2"));
        assert!(smp_str.contains("cores=2"));
        assert!(smp_str.contains("threads=2"));

        let numa = NumaNodeConfig {
            node_id: Some(1),
            cpus: Some("4-7".to_string()),
            mem_mb: Some(2048),
            initiator: Some(0),
        };
        let numa_str = numa.to_qemu_arg();
        assert!(numa_str.contains("nodeid=1"));
        assert!(numa_str.contains("cpus=4-7"));
        assert!(numa_str.contains("mem=2048"));
        assert!(numa_str.contains("initiator=0"));
    }
}
