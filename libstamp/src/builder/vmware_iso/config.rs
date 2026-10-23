//! Configuration types, drivers, firmware, and adapter enums for `vmware-iso`.

use crate::error::StampError;
use crate::types::{DiskSizeMb, MemoryMb, Port};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

/// Supported automation driver for controlling `VMware` instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VmwareDriver {
    /// Local desktop `VMware` hypervisor controlled via `vmrun` (Workstation/Fusion/Player).
    #[default]
    Vmrun,
    /// Remote `ESXi` or `vCenter` controlled via `govc`.
    Govc,
}

impl VmwareDriver {
    /// String representation of the automation driver.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Vmrun => "vmrun",
            Self::Govc => "govc",
        }
    }
}

impl fmt::Display for VmwareDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VmwareDriver {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "govc" => Ok(Self::Govc),
            _ => Ok(Self::Vmrun),
        }
    }
}

/// Firmware type for `VMware` virtual machines.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VmwareFirmware {
    /// Standard legacy BIOS firmware.
    #[default]
    Bios,
    /// Standard UEFI firmware.
    Efi,
    /// UEFI firmware with Secure Boot enabled.
    EfiSecure,
    /// Custom firmware configuration.
    Custom(String),
}

impl VmwareFirmware {
    /// String representation of the firmware type.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Bios => "bios",
            Self::Efi => "efi",
            Self::EfiSecure => "efi-secure",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for VmwareFirmware {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VmwareFirmware {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "bios" => Ok(Self::Bios),
            "efi" | "uefi" => Ok(Self::Efi),
            "efi-secure" | "efisecure" | "secureboot" => Ok(Self::EfiSecure),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// Disk controller adapter type for `VMware` virtual machines.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VmwareDiskAdapterType {
    /// LSI Logic Parallel SCSI adapter.
    #[default]
    LsiLogic,
    /// `VMware` Paravirtual SCSI (PVSCSI) adapter.
    PvScsi,
    /// `BusLogic` Parallel SCSI adapter.
    BusLogic,
    /// NVM Express (`NVMe`) controller.
    Nvme,
    /// Serial ATA (SATA) AHCI controller.
    Sata,
    /// Integrated Drive Electronics (IDE) controller.
    Ide,
    /// Custom storage controller type.
    Custom(String),
}

impl VmwareDiskAdapterType {
    /// String representation of the storage adapter type.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::LsiLogic => "lsilogic",
            Self::PvScsi => "pvscsi",
            Self::BusLogic => "buslogic",
            Self::Nvme => "nvme",
            Self::Sata => "sata",
            Self::Ide => "ide",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for VmwareDiskAdapterType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VmwareDiskAdapterType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "lsilogic" | "lsilogicsas" | "scsi" => Ok(Self::LsiLogic),
            "pvscsi" => Ok(Self::PvScsi),
            "buslogic" => Ok(Self::BusLogic),
            "nvme" => Ok(Self::Nvme),
            "sata" => Ok(Self::Sata),
            "ide" => Ok(Self::Ide),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// CD-ROM / Optical drive bus adapter type for `VMware` virtual machines.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VmwareCdromAdapterType {
    /// Standard IDE bus optical drive (ide1:0).
    #[default]
    Ide,
    /// SATA bus optical drive (sata0:1).
    Sata,
    /// SCSI bus optical drive (scsi0:1).
    Scsi,
    /// Custom optical drive adapter type.
    Custom(String),
}

impl VmwareCdromAdapterType {
    /// String representation of the CD-ROM adapter type.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Ide => "ide",
            Self::Sata => "sata",
            Self::Scsi => "scsi",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for VmwareCdromAdapterType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VmwareCdromAdapterType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "ide" => Ok(Self::Ide),
            "sata" => Ok(Self::Sata),
            "scsi" => Ok(Self::Scsi),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// Network adapter (NIC) model for `VMware` virtual machines.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VmwareNicType {
    /// VMXNET3 paravirtualized 10Gbps virtual network adapter.
    Vmxnet3,
    /// Intel 82574L Gigabit Ethernet NIC (`e1000e`).
    E1000e,
    /// Intel 82545EM Gigabit Ethernet NIC (`e1000`).
    #[default]
    E1000,
    /// AMD PCnet-PCI II virtual network adapter (`vlance`).
    Vlance,
    /// Custom network interface type.
    Custom(String),
}

impl VmwareNicType {
    /// String representation of the network adapter model.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Vmxnet3 => "vmxnet3",
            Self::E1000e => "e1000e",
            Self::E1000 => "e1000",
            Self::Vlance => "vlance",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for VmwareNicType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VmwareNicType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "vmxnet3" => Ok(Self::Vmxnet3),
            "e1000e" => Ok(Self::E1000e),
            "e1000" => Ok(Self::E1000),
            "vlance" => Ok(Self::Vlance),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// `VMware` Tools mounting and installation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VmwareToolsMode {
    /// Attach the `VMware` Tools ISO image to a virtual optical drive.
    Attach,
    /// Upload the `VMware` Tools ISO image to the guest via communicator.
    Upload,
    /// Do not attach or upload `VMware` Tools.
    #[default]
    Disable,
}

impl VmwareToolsMode {
    /// String representation of the tools mode.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Attach => "attach",
            Self::Upload => "upload",
            Self::Disable => "disable",
        }
    }
}

impl fmt::Display for VmwareToolsMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VmwareToolsMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "attach" => Ok(Self::Attach),
            "upload" => Ok(Self::Upload),
            _ => Ok(Self::Disable),
        }
    }
}

/// Operating system flavor for `VMware` Tools ISO detection and upload.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VmwareToolsFlavor {
    /// Apple macOS / OS X (`darwin.iso`).
    Darwin,
    /// Linux (`linux.iso`).
    #[default]
    Linux,
    /// Microsoft Windows (`windows.iso`).
    Windows,
    /// Custom flavor name.
    Custom(String),
}

impl VmwareToolsFlavor {
    /// String representation of the tools flavor.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Darwin => "darwin",
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for VmwareToolsFlavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VmwareToolsFlavor {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "darwin" | "mac" | "macos" => Ok(Self::Darwin),
            "linux" => Ok(Self::Linux),
            "windows" | "win" => Ok(Self::Windows),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// Configuration for the `vmware-iso` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VmwareIsoConfig {
    /// The name of the builder instance.
    pub name: String,
    /// VM name.
    pub vm_name: Option<String>,
    /// The source ISO path or URL.
    pub iso_url: Option<String>,
    /// The checksum of the ISO.
    pub iso_checksum: Option<String>,
    /// Target path for downloading the ISO.
    pub iso_target_path: Option<String>,
    /// Disk size in MB. Defaults to 10240.
    pub disk_size: Option<DiskSizeMb>,
    /// Memory size in MB. Defaults to 1024.
    pub memory: Option<MemoryMb>,
    /// Number of virtual CPUs. Defaults to 1.
    pub cpus: Option<u32>,
    /// Cores per CPU socket.
    pub cores: Option<u32>,
    /// Guest OS type (e.g. `ubuntu-64`, `windows9-64`).
    pub guest_os_type: Option<String>,
    /// Storage bus controller adapter type (`lsilogic`, `nvme`, `sata`, etc.).
    pub disk_adapter_type: Option<VmwareDiskAdapterType>,
    /// Optical drive controller adapter type (`ide`, `sata`, `scsi`).
    pub cdrom_adapter_type: Option<VmwareCdromAdapterType>,
    /// Network adapter model (`vmxnet3`, `e1000e`, etc.).
    pub network_adapter_type: Option<VmwareNicType>,
    /// Network connection type (`nat`, `bridged`, `hostonly`).
    pub network: Option<String>,
    /// VM firmware (`bios`, `efi`, `efi-secure`).
    pub firmware: Option<VmwareFirmware>,
    /// Virtual hardware version (e.g. `14`, `16`, `19`). Defaults to `14`.
    pub version: Option<String>,
    /// USB and USB xHCI controller toggle.
    pub usb: Option<bool>,
    /// Custom `.vmx` configuration parameters to inject.
    pub vmx_data: HashMap<String, String>,
    /// Whether to strip ethernet interfaces from `.vmx`.
    pub vmx_remove_ethernet_interfaces: bool,
    /// Disable password requirement on VNC display.
    pub vnc_disable_password: bool,
    /// VNC port for boot command interaction.
    pub vnc_port: Option<Port>,
    /// The boot command sequence.
    pub boot_command: Option<Vec<String>>,
    /// The wait time before booting.
    pub boot_wait: Option<String>,
    /// `VMware` Tools installation mode (`attach`, `upload`, `disable`).
    pub tools_mode: VmwareToolsMode,
    /// Explicit source path for `VMware` Tools ISO.
    pub tools_source_path: Option<String>,
    /// Guest OS flavor for `VMware` Tools ISO (`darwin`, `linux`, `windows`).
    pub tools_upload_flavor: Option<VmwareToolsFlavor>,
    /// Destination path inside guest VM for uploaded `VMware` Tools ISO.
    pub tools_upload_path: Option<String>,
    /// Communicator type (`ssh`, `winrm`, `none`).
    pub communicator: Option<String>,
    /// SSH username.
    pub ssh_username: Option<String>,
    /// SSH password.
    pub ssh_password: Option<String>,
    /// SSH forwarded host port.
    pub ssh_port: Option<Port>,
    /// SSH timeout.
    pub ssh_timeout: Option<String>,
    /// `WinRM` username.
    pub winrm_username: Option<String>,
    /// `WinRM` password.
    pub winrm_password: Option<String>,
    /// `WinRM` forwarded host port.
    pub winrm_host_port: Option<Port>,
    /// `WinRM` timeout.
    pub winrm_timeout: Option<String>,
    /// `WinRM` NTLM authentication toggle.
    pub winrm_use_ntlm: Option<bool>,
    /// `WinRM` insecure TLS toggle.
    pub winrm_insecure: Option<bool>,
    /// `WinRM` SSL toggle.
    pub winrm_use_ssl: Option<bool>,
    /// Command executed via communicator to initiate graceful guest shutdown.
    pub shutdown_command: Option<String>,
    /// Maximum duration to wait for graceful VM shutdown.
    pub shutdown_timeout: Option<String>,
    /// Automation driver to use (`vmrun` or `govc`).
    pub driver: VmwareDriver,
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
    /// Optional custom path or command for the `vmrun` executable.
    pub vmrun_binary: Option<String>,
    /// Optional custom path or command for the `govc` executable.
    pub govc_binary: Option<String>,
    /// Optional custom path or command for the `ovftool` executable.
    pub ovftool_binary: Option<String>,
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

/// Helper to parse key-value map from JSON string or HCL expression.
pub fn parse_string_map(
    val: Option<&String>,
    expr: Option<&hashicorp_configuration_language_rs::ast::expr::Expression>,
) -> HashMap<String, String> {
    if let Some(expr) = expr
        && let hashicorp_configuration_language_rs::ast::expr::Expression::Object(entries, _) = expr
    {
        let mut map = HashMap::new();
        for (k, v) in entries {
            let k_str = crate::parser::hcl::expr_to_string(k);
            let v_str = crate::parser::hcl::expr_to_string(v);
            map.insert(k_str, v_str);
        }
        return map;
    }
    if let Some(s) = val
        && let Ok(map) = serde_json::from_str::<HashMap<String, String>>(s)
    {
        return map;
    }
    HashMap::new()
}

impl VmwareIsoConfig {
    /// Constructs a `VmwareIsoConfig` from a strongly-typed [`BuilderConfig`](crate::template::BuilderConfig).
    ///
    /// # Errors
    ///
    /// Returns `StampError::Builder` if parsing fails or invalid types are supplied.
    pub fn from_builder_config(
        builder_config: &crate::template::BuilderConfig,
    ) -> Result<Self, StampError> {
        let name = builder_config.name.clone();
        let cfg = &builder_config.config;
        let exprs = &builder_config.expressions;

        let vm_name = cfg.get("vm_name").cloned();
        let iso_url = cfg.get("iso_url").cloned();
        let iso_checksum = cfg.get("iso_checksum").cloned();
        let iso_target_path = cfg.get("iso_target_path").cloned();

        let disk_size = cfg
            .get("disk_size")
            .and_then(|s| s.parse::<u64>().ok())
            .map(DiskSizeMb::new);
        let memory = cfg
            .get("memory")
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);
        let cpus = cfg.get("cpus").and_then(|s| s.parse::<u32>().ok());
        let cores = cfg
            .get("cores")
            .or_else(|| cfg.get("vmware_cores"))
            .and_then(|s| s.parse::<u32>().ok());

        let guest_os_type = cfg
            .get("guest_os_type")
            .or_else(|| cfg.get("vmware_guest_os_type"))
            .cloned();

        let disk_adapter_type = cfg
            .get("disk_adapter_type")
            .or_else(|| cfg.get("vmware_disk_adapter_type"))
            .and_then(|s| s.parse().ok());
        let cdrom_adapter_type = cfg
            .get("cdrom_adapter_type")
            .or_else(|| cfg.get("vmware_cdrom_adapter_type"))
            .and_then(|s| s.parse().ok());
        let network_adapter_type = cfg
            .get("network_adapter_type")
            .or_else(|| cfg.get("vmware_network_adapter_type"))
            .and_then(|s| s.parse().ok());
        let network = cfg
            .get("network")
            .or_else(|| cfg.get("vmware_network"))
            .cloned();
        let firmware = cfg
            .get("firmware")
            .or_else(|| cfg.get("vmware_firmware"))
            .and_then(|s| s.parse().ok());

        let version = cfg
            .get("version")
            .or_else(|| cfg.get("vmware_version"))
            .cloned();
        let usb = cfg
            .get("usb")
            .or_else(|| cfg.get("vmware_usb"))
            .map(|v| v == "true" || v == "on");

        let vmx_data = parse_string_map(
            cfg.get("vmx_data").or_else(|| cfg.get("vmware_vmx_data")),
            exprs
                .get("vmx_data")
                .or_else(|| exprs.get("vmware_vmx_data")),
        );
        let vmx_remove_ethernet_interfaces = cfg
            .get("vmx_remove_ethernet_interfaces")
            .or_else(|| cfg.get("vmware_vmx_remove_ethernet_interfaces"))
            .is_some_and(|v| v == "true");

        let vnc_disable_password = cfg
            .get("vnc_disable_password")
            .or_else(|| cfg.get("vmware_vnc_disable_password"))
            .is_some_and(|v| v == "true");
        let vnc_port = cfg
            .get("vnc_port")
            .and_then(|s| s.parse::<u16>().ok())
            .map(Port::new);

        let boot_wait = cfg.get("boot_wait").cloned();
        let boot_command = cfg.get("boot_command").map(|s| parse_string_list(s));

        let tools_mode = cfg
            .get("tools_mode")
            .or_else(|| cfg.get("vmware_tools_mode"))
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let tools_source_path = cfg
            .get("tools_source_path")
            .or_else(|| cfg.get("vmware_tools_source_path"))
            .cloned();
        let tools_upload_flavor = cfg
            .get("tools_upload_flavor")
            .or_else(|| cfg.get("vmware_tools_upload_flavor"))
            .and_then(|s| s.parse().ok());
        let tools_upload_path = cfg
            .get("tools_upload_path")
            .or_else(|| cfg.get("vmware_tools_upload_path"))
            .cloned();

        let communicator = cfg.get("communicator").cloned();
        let ssh_username = cfg.get("ssh_username").cloned();
        let ssh_password = cfg.get("ssh_password").cloned();
        let ssh_port = cfg
            .get("ssh_port")
            .and_then(|s| s.parse::<u16>().ok())
            .map(Port::new);
        let ssh_timeout = cfg.get("ssh_timeout").cloned();

        let winrm_username = cfg.get("winrm_username").cloned();
        let winrm_password = cfg.get("winrm_password").cloned();
        let winrm_host_port = cfg
            .get("winrm_host_port")
            .or_else(|| cfg.get("winrm_port"))
            .and_then(|s| s.parse::<u16>().ok())
            .map(Port::new);
        let winrm_timeout = cfg.get("winrm_timeout").cloned();
        let winrm_use_ntlm = cfg.get("winrm_use_ntlm").map(|v| v == "true" || v == "on");
        let winrm_insecure = cfg.get("winrm_insecure").map(|v| v == "true" || v == "on");
        let winrm_use_ssl = cfg.get("winrm_use_ssl").map(|v| v == "true" || v == "on");

        let shutdown_command = cfg.get("shutdown_command").cloned();
        let shutdown_timeout = cfg.get("shutdown_timeout").cloned();

        let driver = cfg
            .get("driver")
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let headless = cfg.get("headless").is_some_and(|v| v == "true");
        let output_directory = cfg.get("output_directory").cloned();
        let export_format = cfg
            .get("export_format")
            .or_else(|| cfg.get("format"))
            .cloned();

        let floppy_files = cfg
            .get("floppy_files")
            .map_or_else(Vec::new, |s| parse_string_list(s));
        let cd_files = cfg
            .get("cd_files")
            .map_or_else(Vec::new, |s| parse_string_list(s));
        let cd_label = cfg.get("cd_label").cloned();

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
        let vmrun_binary = cfg.get("vmrun_binary").cloned();
        let govc_binary = cfg.get("govc_binary").cloned();
        let ovftool_binary = cfg.get("ovftool_binary").cloned();

        Ok(Self {
            name,
            vm_name,
            iso_url,
            iso_checksum,
            iso_target_path,
            disk_size,
            memory,
            cpus,
            cores,
            guest_os_type,
            disk_adapter_type,
            cdrom_adapter_type,
            network_adapter_type,
            network,
            firmware,
            version,
            usb,
            vmx_data,
            vmx_remove_ethernet_interfaces,
            vnc_disable_password,
            vnc_port,
            boot_command,
            boot_wait,
            tools_mode,
            tools_source_path,
            tools_upload_flavor,
            tools_upload_path,
            communicator,
            ssh_username,
            ssh_password,
            ssh_port,
            ssh_timeout,
            winrm_username,
            winrm_password,
            winrm_host_port,
            winrm_timeout,
            winrm_use_ntlm,
            winrm_insecure,
            winrm_use_ssl,
            shutdown_command,
            shutdown_timeout,
            driver,
            headless,
            output_directory,
            export_format,
            floppy_files,
            cd_files,
            cd_label,
            http_directory,
            http_content,
            http_bind_address,
            http_port_min,
            http_port_max,
            vmrun_binary,
            govc_binary,
            ovftool_binary,
        })
    }
}
