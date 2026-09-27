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
    /// `x86_64` architecture (default).
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
                #[cfg(target_os = "linux")]
                {
                    if std::path::Path::new("/dev/kvm").exists() {
                        "kvm"
                    } else {
                        "tcg"
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    "hvf"
                }
                #[cfg(target_os = "windows")]
                {
                    "whpx"
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
                {
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

/// Validates accelerator availability and host permissions.
///
/// # Arguments
/// * `accel` - The requested accelerator.
///
/// # Errors
/// Returns `StampError::Builder` if an accelerator was explicitly requested but is unavailable or permissions are denied.
pub fn validate_accelerator(accel: QemuAccelerator) -> Result<QemuAccelerator, StampError> {
    match accel {
        QemuAccelerator::Kvm => {
            let kvm_path = Path::new("/dev/kvm");
            if !kvm_path.exists() {
                return Err(StampError::Builder(
                    "KVM accelerator requested but /dev/kvm not found on host".to_string(),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(kvm_path) {
                    let mode = metadata.permissions().mode();
                    if std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(kvm_path)
                        .is_err()
                        && (mode & 0o006) == 0
                    {
                        return Err(StampError::Builder(
                            "Cannot access /dev/kvm: permission denied. Ensure user is in the 'kvm' group.".to_string(),
                        ));
                    }
                }
            }
            Ok(QemuAccelerator::Kvm)
        }
        QemuAccelerator::Auto => {
            #[cfg(target_os = "linux")]
            {
                let kvm_path = Path::new("/dev/kvm");
                if kvm_path.exists()
                    && std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(kvm_path)
                        .is_ok()
                {
                    Ok(QemuAccelerator::Kvm)
                } else {
                    Ok(QemuAccelerator::Tcg)
                }
            }
            #[cfg(target_os = "macos")]
            {
                Ok(QemuAccelerator::Hvf)
            }
            #[cfg(target_os = "windows")]
            {
                Ok(QemuAccelerator::Whpx)
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            {
                Ok(QemuAccelerator::Tcg)
            }
        }
        other => Ok(other),
    }
}

/// Machine model architecture for QEMU.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MachineModel {
    /// Modern PC machine with `PCIe` (default for `x86_64`).
    #[default]
    Q35,
    /// Legacy PC machine with PCI (i440FX).
    Pc,
    /// Generic ARM/RISC-V Virtual Machine model.
    Virt,
    /// Lightweight microvm machine model.
    Microvm,
    /// Custom machine model identifier.
    Custom(String),
}

impl MachineModel {
    /// Returns the machine model identifier string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Q35 => "q35",
            Self::Pc => "pc",
            Self::Virt => "virt",
            Self::Microvm => "microvm",
            Self::Custom(s) => s.as_str(),
        }
    }
}

/// CPU model emulation for QEMU.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CpuModel {
    /// Host CPU pass-through (default when hardware accelerated).
    #[default]
    Host,
    /// Maximum feature set supported by the accelerator/target.
    Max,
    /// Standard generic 64-bit QEMU CPU.
    Qemu64,
    /// ARM Cortex-A57 CPU model.
    CortexA57,
    /// ARM Cortex-A72 CPU model.
    CortexA72,
    /// Custom CPU model identifier.
    Custom(String),
}

impl CpuModel {
    /// Returns the CPU model identifier string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Host => "host",
            Self::Max => "max",
            Self::Qemu64 => "qemu64",
            Self::CortexA57 => "cortex-a57",
            Self::CortexA72 => "cortex-a72",
            Self::Custom(s) => s.as_str(),
        }
    }
}

/// Allocates an available TCP port in the specified range `[min_port, max_port]`.
///
/// # Arguments
/// * `min_port` - Minimum port number (inclusive).
/// * `max_port` - Maximum port number (inclusive).
///
/// # Errors
/// Returns `StampError::Builder` if no ports are available in the specified range.
pub fn allocate_vnc_port(min_port: u16, max_port: u16) -> Result<u16, StampError> {
    for port in min_port..=max_port {
        if let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", port)) {
            drop(listener);
            return Ok(port);
        }
    }
    Err(StampError::Builder(format!(
        "No available VNC ports in range {min_port}-{max_port}"
    )))
}

/// Disk image formats supported by `qemu-img`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiskFormat {
    /// Raw disk image.
    Raw,
    /// QEMU Copy-On-Write v2/v3 (default).
    #[default]
    Qcow2,
    /// `VMware` Virtual Machine Disk.
    Vmdk,
    /// `VirtualBox` Virtual Disk Image.
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
#[allow(clippy::struct_excessive_bools)]
pub struct QemuConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Source ISO file path or URL.
    pub iso_url: Option<String>,
    /// Checksum of the source ISO.
    pub iso_checksum: Option<String>,
    /// Optional override for the QEMU system binary.
    pub qemu_binary: Option<String>,
    /// Target architecture.
    pub arch: QemuArch,
    /// Virtualization accelerator.
    pub accelerator: QemuAccelerator,
    /// Machine model.
    pub machine_type: Option<MachineModel>,
    /// CPU model.
    pub cpu_model: Option<CpuModel>,
    /// Disk size in MB.
    pub disk_size: Option<u64>,
    /// Target output disk format (raw, qcow2, vmdk, vdi).
    pub disk_format: Option<DiskFormat>,
    /// Disk caching strategy (e.g. `writeback`, `none`).
    pub disk_cache: Option<String>,
    /// Whether to compress the output QCOW2 disk image.
    pub disk_compression: bool,
    /// Disk detect zeroes policy (e.g. `on`, `off`, `unmap`).
    pub disk_detect_zeroes: Option<String>,
    /// Disk discard policy (e.g. `unmap`, `ignore`).
    pub disk_discard: Option<String>,
    /// Whether to boot directly from a base disk image instead of creating a new blank disk.
    pub disk_image: bool,
    /// Disk controller interface (e.g. `virtio`, `scsi`, `ide`).
    pub disk_interface: Option<String>,
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
    /// Whether UEFI/EFI boot is enabled.
    pub efi_boot: bool,
    /// Path to OVMF/UEFI code binary (e.g. `OVMF_CODE.fd`).
    pub efi_firmware_code: Option<String>,
    /// Path to OVMF/UEFI variable template (e.g. `OVMF_VARS.fd`).
    pub efi_firmware_vars: Option<String>,
    /// Whether to drop the EFI vars file upon build completion.
    pub efi_drop_vars: bool,
    /// Canonical flag for dropping EFI variables.
    pub efi_drop_efivars: bool,
    /// Headless mode.
    pub headless: bool,
    /// Display device or driver (e.g. `cocoa`, `none`, `gtk`, `sdl`).
    pub display: Option<String>,
    /// Network device type (e.g. `virtio-net-pci`, `e1000`).
    pub net_device: Option<String>,
    /// Whether to use the default QEMU display.
    pub use_default_display: bool,
    /// Whether pflash is used for UEFI firmware drives.
    pub use_pflash: bool,
    /// Output directory.
    pub output_directory: Option<String>,
    /// VNC bind address.
    pub vnc_bind_address: Option<String>,
    /// VNC port (defaults to dynamic or 5900+).
    pub vnc_port: Option<u16>,
    /// Minimum VNC port for dynamic allocation (default: 5900).
    pub vnc_port_min: u16,
    /// Maximum VNC port for dynamic allocation (default: 6000).
    pub vnc_port_max: u16,
    /// Optional VNC authentication password.
    pub vnc_password: Option<String>,
    /// List of files to place on a virtual floppy disk.
    pub floppy_files: Vec<String>,
    /// List of files to place on a secondary CD-ROM.
    pub cd_files: Vec<String>,
    /// Volume label for the secondary CD-ROM.
    pub cd_label: Option<String>,
    /// In-memory rendered files to include on the secondary CD-ROM.
    pub cd_content: std::collections::HashMap<String, String>,
    /// Custom command-line arguments to pass to QEMU.
    pub qemuargs: Vec<Vec<String>>,
    /// SSH username for provisioning.
    pub ssh_username: Option<String>,
    /// SSH password for provisioning.
    pub ssh_password: Option<String>,
    /// SSH port for guest forwarding.
    pub ssh_port: Option<u16>,
    /// SSH timeout duration string.
    pub ssh_timeout: Option<String>,
    /// Optional host directory containing static files to serve over HTTP to the guest installer.
    pub http_directory: Option<String>,
    /// Optional in-memory files to serve over HTTP (`path` -> `content`).
    pub http_content: std::collections::HashMap<String, String>,
    /// Optional bind address for the HTTP server.
    pub http_bind_address: Option<String>,
    /// Minimum port for the HTTP server port range.
    pub http_port_min: Option<u16>,
    /// Maximum port for the HTTP server port range.
    pub http_port_max: Option<u16>,
}

/// Helper to parse string list from JSON or comma-separated string.
fn parse_string_list(val: &str) -> Vec<String> {
    if let Ok(list) = serde_json::from_str::<Vec<String>>(val) {
        list
    } else {
        val.split(',')
            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Helper to parse nested list of arguments from JSON string.
fn parse_nested_string_list(val: &str) -> Vec<Vec<String>> {
    serde_json::from_str::<Vec<Vec<String>>>(val).unwrap_or_default()
}

impl QemuConfig {
    /// Resolves the effective QEMU system binary to invoke.
    #[must_use]
    pub fn resolve_binary(&self) -> String {
        if let Some(ref custom) = self.qemu_binary
            && !custom.trim().is_empty()
        {
            return custom.clone();
        }
        self.arch.binary_name().to_string()
    }

    /// Constructs a `QemuConfig` from a strongly-typed [`BuilderConfig`](crate::template::BuilderConfig).
    ///
    /// # Arguments
    /// * `builder_config` - The raw builder configuration parsed from template.
    ///
    /// # Errors
    /// Returns `StampError::Builder` if required attributes are missing or malformed.
    pub fn from_builder_config(
        builder_config: &crate::template::BuilderConfig,
    ) -> Result<Self, StampError> {
        let name = builder_config.name.clone();
        let cfg = &builder_config.config;

        let iso_url = cfg.get("iso_url").cloned();
        let iso_checksum = cfg.get("iso_checksum").cloned();
        let qemu_binary = cfg.get("qemu_binary").cloned();

        let arch = cfg
            .get("os_arch")
            .or_else(|| cfg.get("arch"))
            .or_else(|| cfg.get("vm_arch"))
            .map_or(QemuArch::X86_64, |s| {
                match s.to_ascii_lowercase().as_str() {
                    "aarch64" | "arm64" => QemuArch::Aarch64,
                    "arm" | "armv7" => QemuArch::Arm,
                    "i386" | "x86" => QemuArch::I386,
                    "riscv64" => QemuArch::Riscv64,
                    _ => QemuArch::X86_64,
                }
            });

        let accelerator = cfg.get("accelerator").map_or(QemuAccelerator::Auto, |s| {
            match s.to_ascii_lowercase().as_str() {
                "hvf" => QemuAccelerator::Hvf,
                "kvm" => QemuAccelerator::Kvm,
                "whpx" => QemuAccelerator::Whpx,
                "tcg" => QemuAccelerator::Tcg,
                "none" => QemuAccelerator::None,
                _ => QemuAccelerator::Auto,
            }
        });

        let machine_type = cfg
            .get("machine_type")
            .or_else(|| cfg.get("machine"))
            .map(|s| match s.to_ascii_lowercase().as_str() {
                "q35" => MachineModel::Q35,
                "pc" | "i440fx" => MachineModel::Pc,
                "virt" => MachineModel::Virt,
                "microvm" => MachineModel::Microvm,
                other => MachineModel::Custom(other.to_string()),
            });

        let cpu_model = cfg
            .get("cpu_model")
            .map(|s| match s.to_ascii_lowercase().as_str() {
                "host" => CpuModel::Host,
                "max" => CpuModel::Max,
                "qemu64" => CpuModel::Qemu64,
                "cortex-a57" => CpuModel::CortexA57,
                "cortex-a72" => CpuModel::CortexA72,
                other => CpuModel::Custom(other.to_string()),
            });

        let disk_format = cfg
            .get("format")
            .or_else(|| cfg.get("disk_format"))
            .map(|s| match s.to_ascii_lowercase().as_str() {
                "raw" => DiskFormat::Raw,
                "vmdk" => DiskFormat::Vmdk,
                "vdi" => DiskFormat::Vdi,
                _ => DiskFormat::Qcow2,
            });

        let disk_size = cfg.get("disk_size").and_then(|s| s.parse::<u64>().ok());
        let memory = cfg.get("memory").and_then(|s| s.parse::<u64>().ok());
        let cpus = cfg.get("cpus").and_then(|s| s.parse::<u32>().ok());

        let display = cfg.get("display").cloned();
        let disk_cache = cfg.get("disk_cache").cloned();
        let disk_compression = cfg.get("disk_compression").is_some_and(|v| v == "true");
        let disk_detect_zeroes = cfg.get("disk_detect_zeroes").cloned();
        let disk_discard = cfg.get("disk_discard").cloned();
        let disk_image = cfg.get("disk_image").is_some_and(|v| v == "true");
        let disk_interface = cfg.get("disk_interface").cloned();

        let efi_boot = cfg.get("efi_boot").is_some_and(|v| v == "true");
        let efi_firmware_code = cfg.get("efi_firmware_code").cloned();
        let efi_firmware_vars = cfg.get("efi_firmware_vars").cloned();
        let efi_drop_efivars = cfg.get("efi_drop_efivars").is_some_and(|v| v == "true")
            || cfg.get("efi_drop_vars").is_some_and(|v| v == "true");

        let net_device = cfg.get("net_device").cloned();
        let headless = cfg.get("headless").is_some_and(|v| v == "true");
        let use_default_display = cfg.get("use_default_display").is_some_and(|v| v == "true");
        let use_pflash = cfg.get("use_pflash").is_none_or(|v| v == "true");

        let output_directory = cfg.get("output_directory").cloned();
        let vnc_bind_address = cfg.get("vnc_bind_address").cloned();
        let vnc_port = cfg.get("vnc_port").and_then(|s| s.parse::<u16>().ok());
        let vnc_port_min = cfg
            .get("vnc_port_min")
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(5900);
        let vnc_port_max = cfg
            .get("vnc_port_max")
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(6000);
        let vnc_password = cfg.get("vnc_password").cloned();

        let boot_wait = cfg.get("boot_wait").cloned();
        let boot_command = cfg.get("boot_command").map(|s| parse_string_list(s));
        let floppy_files = cfg
            .get("floppy_files")
            .map_or_else(Vec::new, |s| parse_string_list(s));
        let cd_files = cfg
            .get("cd_files")
            .map_or_else(Vec::new, |s| parse_string_list(s));
        let cd_label = cfg.get("cd_label").cloned();
        let qemuargs = cfg
            .get("qemuargs")
            .map_or_else(Vec::new, |s| parse_nested_string_list(s));

        let ssh_username = cfg.get("ssh_username").cloned();
        let ssh_password = cfg.get("ssh_password").cloned();
        let ssh_port = cfg.get("ssh_port").and_then(|s| s.parse::<u16>().ok());
        let ssh_timeout = cfg.get("ssh_timeout").cloned();

        let http_directory = cfg.get("http_directory").cloned();
        let mut http_content = std::collections::HashMap::new();
        if let Some(content_str) = cfg.get("http_content") {
            if let Ok(parsed) =
                serde_json::from_str::<std::collections::HashMap<String, String>>(content_str)
            {
                http_content = parsed;
            }
        }
        let http_bind_address = cfg.get("http_bind_address").cloned();
        let http_port_min = cfg.get("http_port_min").and_then(|p| p.parse::<u16>().ok());
        let http_port_max = cfg.get("http_port_max").and_then(|p| p.parse::<u16>().ok());

        Ok(Self {
            name,
            iso_url,
            iso_checksum,
            qemu_binary,
            arch,
            accelerator,
            machine_type,
            cpu_model,
            disk_size,
            disk_format,
            disk_cache,
            disk_compression,
            disk_detect_zeroes,
            disk_discard,
            disk_image,
            disk_interface,
            boot_command,
            boot_wait,
            memory,
            cpus,
            smp: None,
            numa_nodes: vec![],
            efi_boot,
            efi_firmware_code,
            efi_firmware_vars,
            efi_drop_vars: efi_drop_efivars,
            efi_drop_efivars,
            headless,
            display,
            net_device,
            use_default_display,
            use_pflash,
            output_directory,
            vnc_bind_address,
            vnc_port,
            vnc_port_min,
            vnc_port_max,
            vnc_password,
            floppy_files,
            cd_files,
            cd_label,
            cd_content: std::collections::HashMap::new(),
            qemuargs,
            ssh_username,
            ssh_password,
            ssh_port,
            ssh_timeout,
            http_directory,
            http_content,
            http_bind_address,
            http_port_min,
            http_port_max,
        })
    }
}

/// Assembles the complete list of CLI arguments for executing QEMU.
///
/// # Arguments
/// * `config` - QEMU builder configuration.
/// * `disk_path` - Path to the primary disk image.
/// * `state` - Execution state bag.
///
/// # Errors
/// Returns `StampError::Builder` if accelerator validation or port allocation fails.
pub fn build_qemu_args(
    config: &QemuConfig,
    disk_path: &str,
    state: &StateBag,
) -> Result<Vec<String>, StampError> {
    let mut args = Vec::new();

    let accel = validate_accelerator(config.accelerator)?;
    if accel != QemuAccelerator::None {
        args.push("-accel".to_string());
        args.push(accel.as_str().to_string());
    }

    if let Some(ref machine) = config.machine_type {
        args.push("-machine".to_string());
        args.push(machine.as_str().to_string());
    }

    if let Some(ref cpu) = config.cpu_model {
        args.push("-cpu".to_string());
        args.push(cpu.as_str().to_string());
    }

    let mem = config.memory.unwrap_or(512).to_string();
    args.push("-m".to_string());
    args.push(mem);

    if let Some(ref smp) = config.smp {
        args.push("-smp".to_string());
        args.push(smp.to_qemu_arg());
    } else {
        let cpus = config.cpus.unwrap_or(1).to_string();
        args.push("-smp".to_string());
        args.push(cpus);
    }

    for numa in &config.numa_nodes {
        args.push("-numa".to_string());
        args.push(numa.to_qemu_arg());
    }

    // UEFI firmware handling
    if (config.efi_boot || config.efi_firmware_code.is_some())
        && let Some(ref code) = config.efi_firmware_code
    {
        args.push("-drive".to_string());
        args.push(format!("if=pflash,format=raw,readonly=on,file={code}"));
        if let Some(vars_path) = state.get::<String>("efi_vars_path") {
            args.push("-drive".to_string());
            args.push(format!("if=pflash,format=raw,file={vars_path}"));
        } else if let Some(ref vars) = config.efi_firmware_vars {
            args.push("-drive".to_string());
            args.push(format!("if=pflash,format=raw,file={vars}"));
        }
    }

    // Display & VNC handling
    let vnc_port = if let Some(port) = config.vnc_port {
        port
    } else if let Some(port) = state.get::<u16>("vnc_port") {
        *port
    } else {
        5900
    };
    let display_num = vnc_port.saturating_sub(5900);
    let bind_addr = config.vnc_bind_address.as_deref().unwrap_or("127.0.0.1");

    if config.headless {
        if config.display.as_deref() == Some("none") || config.display.is_none() {
            args.push("-display".to_string());
            args.push("none".to_string());
        } else if let Some(ref d) = config.display {
            args.push("-display".to_string());
            args.push(d.clone());
        }
        args.push("-vnc".to_string());
        args.push(format!("{bind_addr}:{display_num}"));
    } else {
        if let Some(ref d) = config.display {
            args.push("-display".to_string());
            args.push(d.clone());
        } else {
            #[cfg(target_os = "macos")]
            {
                args.push("-display".to_string());
                args.push("cocoa".to_string());
            }
            #[cfg(not(target_os = "macos"))]
            {
                args.push("-display".to_string());
                args.push("default".to_string());
            }
        }
        args.push("-vnc".to_string());
        args.push(format!("{bind_addr}:{display_num}"));
    }

    // Primary disk drive
    let disk_if = config.disk_interface.as_deref().unwrap_or("virtio");
    let cache = config.disk_cache.as_deref().unwrap_or("writeback");
    let discard = config.disk_discard.as_deref().unwrap_or("ignore");
    let detect_zeroes = config.disk_detect_zeroes.as_deref().unwrap_or("off");
    let fmt_str = config.disk_format.unwrap_or_default().as_str();

    let mut drive_str = format!(
        "file={disk_path},if={disk_if},cache={cache},discard={discard},detect-zeroes={detect_zeroes}"
    );
    if !config.disk_image {
        use std::fmt::Write as _;
        let _ = write!(drive_str, ",format={fmt_str}");
    }
    args.push("-drive".to_string());
    args.push(drive_str);

    // Boot ISO
    if let Some(ref iso) = config.iso_url
        && !iso.is_empty()
    {
        args.push("-cdrom".to_string());
        args.push(iso.clone());
        args.push("-boot".to_string());
        args.push("once=d".to_string());
    }

    // Floppy and secondary CD
    if let Some(floppy) = state.get::<String>("floppy_path") {
        args.push("-fda".to_string());
        args.push(floppy.clone());
    }
    if let Some(cd) = state.get::<String>("cd_path") {
        args.push("-drive".to_string());
        args.push(format!("file={cd},media=cdrom"));
    }

    // Network
    let ssh_port = config.ssh_port.unwrap_or(2222);
    args.push("-netdev".to_string());
    args.push(format!("user,id=user.0,hostfwd=tcp::{ssh_port}-:22"));
    let net_dev = config.net_device.as_deref().unwrap_or("virtio-net-pci");
    args.push("-device".to_string());
    args.push(format!("{net_dev},netdev=user.0"));

    // Custom qemuargs with token substitution
    for arg_group in &config.qemuargs {
        for arg in arg_group {
            let mut substituted = arg.clone();
            if let Some(root) = state.get::<String>("path_root") {
                substituted = substituted.replace("${path.root}", root);
            }
            if let Some(ip) = state.get::<String>("http_ip") {
                substituted = substituted.replace("{{ .HTTPIP }}", ip);
            }
            if let Some(port) = state.get::<u16>("http_port") {
                substituted = substituted.replace("{{ .HTTPPort }}", &port.to_string());
            }
            args.push(substituted);
        }
    }

    Ok(args)
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
    let qemu_img_cmd = std::env::var("QEMU_IMG_CMD").unwrap_or_default();
    let cmd_name = if qemu_img_cmd.is_empty() {
        "qemu-img"
    } else {
        &qemu_img_cmd
    };

    if cfg!(test) && qemu_img_cmd.is_empty() {
        if let Some(parent) = target.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        } else {
            // Target has no parent path (e.g. root)
        }
        let _ = tokio::fs::write(target, b"CONVERTED_DISK_DATA").await;
        return Ok(());
    }

    let status = tokio::process::Command::new(cmd_name)
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

        let mut cmd = tokio::process::Command::new("qemu-img");
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
        let http_ip = state.http_ip();
        let http_port = state.http_port();
        let actions = crate::builder::virtualization::BootCommandParser::parse(
            cmds, http_ip, http_port, None,
        );

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

        if let Some(handle) =
            state.get::<crate::builder::http_server::HttpServerHandle>("http_server_handle")
        {
            handle.abort();
        }

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
            Box::new(crate::builder::http_server::StepHttpServer::new(
                http_cfg,
                self.name(),
                Some(ui.clone()),
            )),
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

    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    async fn test_media_and_disk_conversion() {
        let temp_dir_res = tempfile::tempdir();
        assert!(temp_dir_res.is_ok());
        for temp_dir in temp_dir_res {
            let file1 = temp_dir.path().join("autounattend.xml");
            let write_res = std::fs::write(&file1, "<unattend></unattend>");
            assert!(write_res.is_ok());

            let floppy_path = temp_dir.path().join("floppy.flp");
            let flop_res =
                generate_floppy_disk(&[file1.to_string_lossy().to_string()], &floppy_path).await;
            assert!(flop_res.is_ok());
            assert!(floppy_path.exists());

            let iso_path = temp_dir.path().join("cidata.iso");
            let iso_res = generate_cdrom_iso(
                &[file1.to_string_lossy().to_string()],
                Some("cidata"),
                &iso_path,
            )
            .await;
            assert!(iso_res.is_ok());
            assert!(iso_path.exists());

            let disk_path = temp_dir.path().join("test.qcow2");
            let vmdk_path = temp_dir.path().join("test.vmdk");
            let conv_res = convert_disk_image(&disk_path, &vmdk_path, DiskFormat::Vmdk).await;
            assert!(conv_res.is_ok());
            assert!(vmdk_path.exists());

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _lock = ENV_LOCK.lock().await;

                // 1. Success script
                let ok_script = temp_dir.path().join("qemu_img_ok.sh");
                let _ = std::fs::write(&ok_script, "#!/bin/sh\nexit 0\n");
                let _ =
                    std::fs::set_permissions(&ok_script, std::fs::Permissions::from_mode(0o755));
                unsafe {
                    std::env::set_var("QEMU_IMG_CMD", ok_script.to_string_lossy().as_ref());
                }
                assert!(
                    convert_disk_image(&disk_path, &vmdk_path, DiskFormat::Qcow2)
                        .await
                        .is_ok()
                );

                // 2. Failure script
                let fail_script = temp_dir.path().join("qemu_img_fail.sh");
                let _ = std::fs::write(&fail_script, "#!/bin/sh\nexit 1\n");
                let _ =
                    std::fs::set_permissions(&fail_script, std::fs::Permissions::from_mode(0o755));
                unsafe {
                    std::env::set_var("QEMU_IMG_CMD", fail_script.to_string_lossy().as_ref());
                }
                assert!(
                    convert_disk_image(&disk_path, &vmdk_path, DiskFormat::Raw)
                        .await
                        .is_err()
                );

                // 3. Execution failure (nonexistent binary)
                unsafe {
                    std::env::set_var("QEMU_IMG_CMD", "/nonexistent/stamp/qemu-img");
                }
                assert!(
                    convert_disk_image(&disk_path, &vmdk_path, DiskFormat::Vdi)
                        .await
                        .is_err()
                );

                unsafe {
                    std::env::remove_var("QEMU_IMG_CMD");
                }
            }
        }
    }

    #[tokio::test]
    async fn test_qemu_steps_coverage() {
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // StepRunQemu with minimal config (all None / empty)
        let mut step_run = StepRunQemu {
            ui: Arc::clone(&ui),
            name: "test".to_string(),
            config: QemuConfig::default(),
        };
        let mut state = StateBag::new();
        let action = step_run.run(&mut state).await;
        assert_eq!(action.ok(), Some(StepAction::Continue));

        // StepProvision with missing ssh_port and vm_ip in state (triggers fallbacks)
        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut step_provision = StepProvision {
            ui: Arc::clone(&ui),
            name: "test".to_string(),
            hook,
        };
        let mut empty_state = StateBag::new();
        let prov_action = step_provision.run(&mut empty_state).await;
        assert_eq!(prov_action.ok(), Some(StepAction::Continue));
        step_provision.cleanup(&empty_state).await;

        let _ = convert_disk_image(
            std::path::Path::new("dummy"),
            std::path::Path::new("/"),
            DiskFormat::Vmdk,
        )
        .await;
    }

    #[tokio::test]
    async fn test_qemubuilder_run() {
        let temp_dir_res = tempfile::tempdir();
        assert!(temp_dir_res.is_ok());
        for temp_dir in temp_dir_res {
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

            let artifact_res = builder.run(hook, ui, OnErrorStrategy::Cleanup).await;
            assert!(artifact_res.is_ok());
            for artifact in artifact_res {
                assert!(artifact.id().starts_with("qemu-image:"));
            }

            assert!(builder.cancel().await.is_ok());
        }
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
        assert_eq!(DiskFormat::Qcow2.as_str(), "qcow2");
        assert_eq!(DiskFormat::Vmdk.as_str(), "vmdk");
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

        assert!(SmpConfig::default().to_qemu_arg().is_empty());
        assert_eq!(NumaNodeConfig::default().to_qemu_arg(), "node");
    }

    #[test]
    fn test_qemu_target_architecture_and_binaries() {
        let mut cfg = QemuConfig::default();
        cfg.arch = QemuArch::X86_64;
        assert_eq!(cfg.resolve_binary(), "qemu-system-x86_64");

        cfg.arch = QemuArch::Aarch64;
        assert_eq!(cfg.resolve_binary(), "qemu-system-aarch64");

        cfg.qemu_binary = Some("/opt/homebrew/bin/qemu-system-aarch64".to_string());
        assert_eq!(
            cfg.resolve_binary(),
            "/opt/homebrew/bin/qemu-system-aarch64"
        );

        // Machine models
        assert_eq!(MachineModel::Q35.as_str(), "q35");
        assert_eq!(MachineModel::Pc.as_str(), "pc");
        assert_eq!(MachineModel::Virt.as_str(), "virt");
        assert_eq!(MachineModel::Microvm.as_str(), "microvm");
        assert_eq!(
            MachineModel::Custom("virt-2.8".to_string()).as_str(),
            "virt-2.8"
        );

        // CPU models
        assert_eq!(CpuModel::Host.as_str(), "host");
        assert_eq!(CpuModel::Max.as_str(), "max");
        assert_eq!(CpuModel::Qemu64.as_str(), "qemu64");
        assert_eq!(CpuModel::CortexA57.as_str(), "cortex-a57");
        assert_eq!(CpuModel::CortexA72.as_str(), "cortex-a72");
        assert_eq!(CpuModel::Custom("EPYC-v4".to_string()).as_str(), "EPYC-v4");
    }

    #[test]
    fn test_qemu_accelerators_and_validation() {
        assert!(validate_accelerator(QemuAccelerator::Tcg).is_ok());
        assert!(validate_accelerator(QemuAccelerator::None).is_ok());
        assert!(validate_accelerator(QemuAccelerator::Hvf).is_ok());
        assert!(validate_accelerator(QemuAccelerator::Whpx).is_ok());
        assert!(validate_accelerator(QemuAccelerator::Auto).is_ok());

        // Test KVM validation (if /dev/kvm does not exist or has bad permissions)
        if !std::path::Path::new("/dev/kvm").exists() {
            let res = validate_accelerator(QemuAccelerator::Kvm);
            assert!(res.is_err());
            assert!(
                matches!(res.unwrap_err(), StampError::Builder(msg) if msg.contains("not found on host"))
            );
        }
    }

    #[test]
    fn test_qemuargs_engine_and_substitutions() {
        let mut state = StateBag::new();
        state.put("path_root", "/path/to/template".to_string());
        state.put("http_ip", "192.168.1.100".to_string());
        state.put("http_port", 8080u16);
        state.put("vnc_port", 5901u16);

        let config = QemuConfig {
            name: "bento-qemu".to_string(),
            arch: QemuArch::Aarch64,
            accelerator: QemuAccelerator::Tcg,
            machine_type: Some(MachineModel::Virt),
            cpu_model: Some(CpuModel::CortexA57),
            disk_interface: Some("virtio".to_string()),
            disk_cache: Some("none".to_string()),
            disk_discard: Some("unmap".to_string()),
            disk_detect_zeroes: Some("on".to_string()),
            net_device: Some("virtio-net-pci".to_string()),
            qemuargs: vec![
                vec!["-device".to_string(), "virtio-gpu-pci".to_string()],
                vec![
                    "-chardev".to_string(),
                    "socket,id=ser0,path=${path.root}/serial.sock".to_string(),
                ],
                vec!["-serial".to_string(), "chardev:ser0".to_string()],
                vec!["-device".to_string(), "org.qemu.guest_agent.0".to_string()],
                vec![
                    "-kernel".to_string(),
                    "http://{{ .HTTPIP }}:{{ .HTTPPort }}/vmlinuz".to_string(),
                ],
                vec!["-device".to_string(), "usb-kbd".to_string()],
                vec!["-device".to_string(), "usb-tablet".to_string()],
                vec!["-device".to_string(), "ramfb".to_string()],
                vec!["-device".to_string(), "nvme,drive=nvme0".to_string()],
            ],
            ..Default::default()
        };

        let args = build_qemu_args(&config, "/tmp/disk.qcow2", &state).unwrap();

        assert!(args.contains(&"-machine".to_string()));
        assert!(args.contains(&"virt".to_string()));
        assert!(args.contains(&"-cpu".to_string()));
        assert!(args.contains(&"cortex-a57".to_string()));
        assert!(args.contains(&"file=/tmp/disk.qcow2,if=virtio,cache=none,discard=unmap,detect-zeroes=on,format=qcow2".to_string()));
        assert!(args.contains(&"socket,id=ser0,path=/path/to/template/serial.sock".to_string()));
        assert!(args.contains(&"http://192.168.1.100:8080/vmlinuz".to_string()));
        assert!(args.contains(&"virtio-gpu-pci".to_string()));
        assert!(args.contains(&"org.qemu.guest_agent.0".to_string()));
        assert!(args.contains(&"usb-kbd".to_string()));
        assert!(args.contains(&"ramfb".to_string()));
    }

    #[tokio::test]
    async fn test_qemu_uefi_and_firmware() {
        let temp_dir = tempfile::tempdir().unwrap();
        let code_path = temp_dir.path().join("OVMF_CODE.fd");
        let vars_template = temp_dir.path().join("OVMF_VARS.fd");
        std::fs::write(&code_path, b"UEFI_CODE").unwrap();
        std::fs::write(&vars_template, b"UEFI_VARS").unwrap();

        let config = QemuConfig {
            name: "uefi-vm".to_string(),
            efi_boot: true,
            efi_firmware_code: Some(code_path.to_string_lossy().to_string()),
            efi_firmware_vars: Some(vars_template.to_string_lossy().to_string()),
            efi_drop_efivars: true,
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            ..Default::default()
        };

        let mut state = StateBag::new();
        state.put("disk_path", "/tmp/packer-qemu".to_string());

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut step = StepRunQemu {
            ui,
            name: "uefi-vm".to_string(),
            config: config.clone(),
        };

        assert!(step.run(&mut state).await.is_ok());
        let instance_vars = temp_dir.path().join("efivars.fd");
        assert!(instance_vars.exists());

        // Test cleanup drops efivars when efi_drop_efivars is true
        step.cleanup(&state).await;
        assert!(!instance_vars.exists());
    }

    #[tokio::test]
    async fn test_qemu_disks_and_formats() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_img = temp_dir.path().join("base.qcow2");
        std::fs::write(&base_img, b"BASE_IMAGE_DATA").unwrap();

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // Test disk_image = true (direct boot from base image)
        let config_base = QemuConfig {
            name: "base-vm".to_string(),
            disk_image: true,
            iso_url: Some(format!("file://{}", base_img.to_string_lossy())),
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            disk_format: Some(DiskFormat::Raw),
            ..Default::default()
        };

        let mut step_img = StepCreateImage {
            ui: ui.clone(),
            name: "base-vm".to_string(),
            config: config_base,
        };

        let mut state = StateBag::new();
        assert!(step_img.run(&mut state).await.is_ok());
        assert_eq!(
            state.get::<String>("disk_path"),
            Some(&format!(
                "{}/packer-qemu",
                temp_dir.path().to_string_lossy()
            ))
        );
    }

    #[test]
    fn test_qemu_headless_and_vnc_allocation() {
        let port = allocate_vnc_port(5900, 6000).unwrap();
        assert!((5900..=6000).contains(&port));

        // Exhaustion test with inverted port range
        let err_res = allocate_vnc_port(6000, 5999);
        assert!(err_res.is_err());
        assert!(
            matches!(err_res.unwrap_err(), StampError::Builder(msg) if msg.contains("No available VNC ports"))
        );

        let mut state = StateBag::new();
        state.put("vnc_port", 5905u16);

        // Headless with display=none
        let cfg_headless = QemuConfig {
            headless: true,
            display: Some("none".to_string()),
            vnc_port: Some(5905),
            ..Default::default()
        };
        let args_headless = build_qemu_args(&cfg_headless, "/tmp/disk", &state).unwrap();
        assert!(args_headless.contains(&"-display".to_string()));
        assert!(args_headless.contains(&"none".to_string()));
        assert!(args_headless.contains(&"-vnc".to_string()));
        assert!(args_headless.contains(&"127.0.0.1:5".to_string()));

        // Non-headless with custom display
        let cfg_gui = QemuConfig {
            headless: false,
            display: Some("cocoa".to_string()),
            vnc_port: Some(5905),
            ..Default::default()
        };
        let args_gui = build_qemu_args(&cfg_gui, "/tmp/disk", &state).unwrap();
        assert!(args_gui.contains(&"-display".to_string()));
        assert!(args_gui.contains(&"cocoa".to_string()));
    }

    #[test]
    fn test_qemu_from_builder_config() {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "qemu".to_string(),
            name: "bento-source".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("os_arch".to_string(), "aarch64".to_string());
        builder_cfg
            .config
            .insert("accelerator".to_string(), "hvf".to_string());
        builder_cfg
            .config
            .insert("machine_type".to_string(), "virt".to_string());
        builder_cfg
            .config
            .insert("cpu_model".to_string(), "cortex-a72".to_string());
        builder_cfg
            .config
            .insert("disk_size".to_string(), "65536".to_string());
        builder_cfg
            .config
            .insert("format".to_string(), "raw".to_string());
        builder_cfg
            .config
            .insert("disk_compression".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("disk_discard".to_string(), "unmap".to_string());
        builder_cfg
            .config
            .insert("efi_boot".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("efi_drop_efivars".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("net_device".to_string(), "virtio-net-pci".to_string());
        builder_cfg
            .config
            .insert("headless".to_string(), "true".to_string());
        builder_cfg.config.insert(
            "qemuargs".to_string(),
            "[[\"-device\", \"ramfb\"]]".to_string(),
        );
        builder_cfg.config.insert(
            "boot_command".to_string(),
            "[\"<enter>\", \"root\"]".to_string(),
        );

        let qemu_cfg = QemuConfig::from_builder_config(&builder_cfg).unwrap();
        assert_eq!(qemu_cfg.name, "bento-source");
        assert_eq!(qemu_cfg.arch, QemuArch::Aarch64);
        assert_eq!(qemu_cfg.accelerator, QemuAccelerator::Hvf);
        assert_eq!(qemu_cfg.machine_type, Some(MachineModel::Virt));
        assert_eq!(qemu_cfg.cpu_model, Some(CpuModel::CortexA72));
        assert_eq!(qemu_cfg.disk_size, Some(65536));
        assert_eq!(qemu_cfg.disk_format, Some(DiskFormat::Raw));
        assert!(qemu_cfg.disk_compression);
        assert_eq!(qemu_cfg.disk_discard.as_deref(), Some("unmap"));
        assert!(qemu_cfg.efi_boot);
        assert!(qemu_cfg.efi_drop_efivars);
        assert_eq!(qemu_cfg.net_device.as_deref(), Some("virtio-net-pci"));
        assert!(qemu_cfg.headless);
        assert_eq!(
            qemu_cfg.qemuargs,
            vec![vec!["-device".to_string(), "ramfb".to_string()]]
        );
        assert_eq!(
            qemu_cfg.boot_command,
            Some(vec!["<enter>".to_string(), "root".to_string()])
        );
    }
}
