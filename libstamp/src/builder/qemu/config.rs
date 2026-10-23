//! Configuration types, architectures, accelerators, and argument builders for the `qemu` builder.

use crate::engine::multistep::StateBag;
use crate::error::StampError;
use std::path::Path;

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
    validate_accelerator_with_path(accel, Path::new("/dev/kvm"))
}

/// Validates accelerator availability with a specific KVM path.
#[doc(hidden)]
pub fn validate_accelerator_with_path(
    accel: QemuAccelerator,
    kvm_path: &Path,
) -> Result<QemuAccelerator, StampError> {
    match accel {
        QemuAccelerator::Kvm => {
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
    /// Optional override for the `qemu-img` binary.
    pub qemu_img_binary: Option<String>,
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
pub fn parse_string_list(val: &str) -> Vec<String> {
    if let Ok(list) = serde_json::from_str::<Vec<String>>(val) {
        list
    } else {
        val.split(',')
            .map(|s| {
                s.trim()
                    .trim_matches('"')
                    .trim_matches(char::from(39u8))
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Helper to parse nested list of arguments from JSON string.
pub fn parse_nested_string_list(val: &str) -> Vec<Vec<String>> {
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

    /// Resolves the effective `qemu-img` binary to invoke.
    #[must_use]
    pub fn resolve_qemu_img_binary(&self) -> String {
        if let Some(ref custom) = self.qemu_img_binary
            && !custom.trim().is_empty()
        {
            return custom.clone();
        }
        let env_cmd = std::env::var("QEMU_IMG_CMD").unwrap_or_default();
        if !env_cmd.is_empty() {
            return env_cmd;
        }
        "qemu-img".to_string()
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
        let qemu_img_binary = cfg.get("qemu_img_binary").cloned();

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
        if let Some(content_str) = cfg.get("http_content")
            && let Ok(parsed) =
                serde_json::from_str::<std::collections::HashMap<String, String>>(content_str)
        {
            http_content = parsed;
        }
        let http_bind_address = cfg.get("http_bind_address").cloned();
        let http_port_min = cfg.get("http_port_min").and_then(|p| p.parse::<u16>().ok());
        let http_port_max = cfg.get("http_port_max").and_then(|p| p.parse::<u16>().ok());

        Ok(Self {
            name,
            iso_url,
            iso_checksum,
            qemu_binary,
            qemu_img_binary,
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

    if let Some(ref iso) = config.iso_url
        && !iso.is_empty()
    {
        args.push("-cdrom".to_string());
        args.push(iso.clone());
        args.push("-boot".to_string());
        args.push("once=d".to_string());
    }

    if let Some(floppy) = state.get::<String>("floppy_path") {
        args.push("-fda".to_string());
        args.push(floppy.clone());
    }
    if let Some(cd) = state.get::<String>("cd_path") {
        args.push("-drive".to_string());
        args.push(format!("file={cd},media=cdrom"));
    }

    let ssh_port = config.ssh_port.unwrap_or(2222);
    args.push("-netdev".to_string());
    args.push(format!("user,id=user.0,hostfwd=tcp::{ssh_port}-:22"));
    let net_dev = config.net_device.as_deref().unwrap_or("virtio-net-pci");
    args.push("-device".to_string());
    args.push(format!("{net_dev},netdev=user.0"));

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
