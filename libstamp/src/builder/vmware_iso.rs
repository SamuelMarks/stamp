#![cfg(not(tarpaulin_include))]
//! Implementation of the `vmware-iso` builder with `vmrun` and `govc` automation drivers,
//! `.vmx` configuration file generation with parameter injection, and guest IP discovery.

use crate::builder::Builder;
use crate::communicator::Communicator;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::communicator::winrm::{WinRmCommunicator, WinRmConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{MemoryMb, Port, Timeout};
use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

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
    /// VMware Paravirtual SCSI (PVSCSI) adapter.
    PvScsi,
    /// BusLogic Parallel SCSI adapter.
    BusLogic,
    /// NVM Express (NVMe) controller.
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
    pub disk_size: Option<MemoryMb>,
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
    /// VMware Tools installation mode (`attach`, `upload`, `disable`).
    pub tools_mode: VmwareToolsMode,
    /// Explicit source path for VMware Tools ISO.
    pub tools_source_path: Option<String>,
    /// Guest OS flavor for VMware Tools ISO (`darwin`, `linux`, `windows`).
    pub tools_upload_flavor: Option<VmwareToolsFlavor>,
    /// Destination path inside guest VM for uploaded VMware Tools ISO.
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
    /// WinRM username.
    pub winrm_username: Option<String>,
    /// WinRM password.
    pub winrm_password: Option<String>,
    /// WinRM forwarded host port.
    pub winrm_host_port: Option<Port>,
    /// WinRM timeout.
    pub winrm_timeout: Option<String>,
    /// WinRM NTLM authentication toggle.
    pub winrm_use_ntlm: Option<bool>,
    /// WinRM insecure TLS toggle.
    pub winrm_insecure: Option<bool>,
    /// WinRM SSL toggle.
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

/// Helper to parse key-value map from JSON string or HCL expression.
fn parse_string_map(
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
    if let Some(s) = val {
        if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(s) {
            return map;
        }
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
            .map(MemoryMb::new);
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
        })
    }
}

/// Discovers the path to the `vmrun` CLI executable on the host system.
#[must_use]
pub fn find_vmrun_binary() -> PathBuf {
    if let Ok(env_path) = std::env::var("VMRUN_PATH") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return p;
        }
    }

    let candidates = [
        PathBuf::from("/Applications/VMware Fusion.app/Contents/Library/vmrun"),
        PathBuf::from("/Applications/VMware Fusion.app/Contents/Public/vmrun"),
        PathBuf::from("/Library/Application Support/VMware Fusion/vmrun"),
        PathBuf::from("/usr/bin/vmrun"),
        PathBuf::from("/usr/local/bin/vmrun"),
        PathBuf::from(r"C:\Program Files (x86)\VMware\VMware Workstation\vmrun.exe"),
        PathBuf::from(r"C:\Program Files\VMware\VMware Workstation\vmrun.exe"),
        PathBuf::from(r"C:\Program Files (x86)\VMware\VMware VIX\vmrun.exe"),
    ];

    candidates
        .into_iter()
        .find(|c| c.exists())
        .unwrap_or_else(|| PathBuf::from("vmrun"))
}

/// Discovers the path to the `VMware` Tools ISO image on the host system.
#[must_use]
pub fn find_vmware_tools_iso(flavor: Option<&VmwareToolsFlavor>) -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("VMWARE_TOOLS_ISO") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }

    let flav_str = flavor.map_or("linux", VmwareToolsFlavor::as_str);

    let candidates = [
        PathBuf::from(format!(
            "/Applications/VMware Fusion.app/Contents/Library/isoimages/{flav_str}.iso"
        )),
        PathBuf::from(format!(
            "/Library/Application Support/VMware Fusion/isoimages/{flav_str}.iso"
        )),
        PathBuf::from(format!("/usr/lib/vmware/isoimages/{flav_str}.iso")),
        PathBuf::from(format!("/usr/share/vmware/isoimages/{flav_str}.iso")),
        PathBuf::from(format!(
            r"C:\Program Files (x86)\VMware\VMware Workstation\{flav_str}.iso"
        )),
        PathBuf::from(format!(
            r"C:\Program Files\VMware\VMware Workstation\{flav_str}.iso"
        )),
    ];

    candidates.into_iter().find(|c| c.exists())
}

/// Execute a `vmrun` command with arguments.
///
/// # Errors
///
/// Returns `StampError::Builder` if `vmrun` fails.
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
/// Returns `StampError::Builder` if the `vmrun` command fails or cannot be spawned.
pub async fn run_vmrun_with_cmd(cmd: &str, args: &[&str]) -> Result<String, StampError> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| StampError::Builder(format!("Failed to execute vmrun: {e}")))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let msg = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(StampError::Builder(format!(
            "vmrun command '{cmd} {args:?}' failed with exit code {code}: {msg}"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Execute a `govc` command with arguments.
///
/// # Errors
///
/// Returns `StampError::Builder` if `govc` fails.
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
/// Returns `StampError::Builder` if the `govc` command fails or cannot be spawned.
pub async fn run_govc_with_cmd(cmd: &str, args: &[&str]) -> Result<String, StampError> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| StampError::Builder(format!("Failed to execute govc: {e}")))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(StampError::Builder(format!(
            "govc command '{cmd} {:?}' failed with exit code {code}: {}",
            args,
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Execute `ovftool` to export or convert a VMX virtual machine to OVA or OVF format.
///
/// # Errors
///
/// Returns `StampError::Builder` or `StampError::Io` if `ovftool` fails.
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
/// Returns `StampError::Builder` or `StampError::Io` if `ovftool` fails.
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
        .map_err(|e| StampError::Builder(format!("Failed to execute ovftool: {e}")))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(StampError::Builder(format!(
            "ovftool failed with exit code {code}: {}",
            stderr.trim()
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
    generate_vmx_content_full(
        vm_name,
        guest_os,
        mem_size,
        cpus,
        None,
        iso_path,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        None,
        false,
        custom_data,
    )
}

/// Generate comprehensive `.vmx` configuration file text supporting all Bento/Packer options.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn generate_vmx_content_full<S: std::hash::BuildHasher>(
    vm_name: &str,
    guest_os: &str,
    mem_size: u64,
    cpus: u32,
    cores: Option<u32>,
    iso_path: Option<&str>,
    disk_adapter: Option<&VmwareDiskAdapterType>,
    cdrom_adapter: Option<&VmwareCdromAdapterType>,
    nic_type: Option<&VmwareNicType>,
    network: Option<&str>,
    firmware: Option<&VmwareFirmware>,
    version: Option<&str>,
    usb: Option<bool>,
    remove_ethernet: bool,
    vnc_port: Option<u16>,
    vnc_disable_password: bool,
    custom_data: &HashMap<String, String, S>,
) -> String {
    let hw_ver = version.unwrap_or("14");
    let mut vmx = format!(
        r#".encoding = "UTF-8"
config.version = "8"
virtualHW.version = "{hw_ver}"
displayName = "{vm_name}"
guestOS = "{guest_os}"
memsize = "{mem_size}"
numvcpus = "{cpus}"
"#
    );

    if let Some(c) = cores {
        let _ = writeln!(vmx, "cpuid.coresPerSocket = \"{c}\"");
    }

    // Firmware options
    match firmware {
        Some(VmwareFirmware::Efi) => {
            let _ = writeln!(vmx, "firmware = \"efi\"");
        }
        Some(VmwareFirmware::EfiSecure) => {
            let _ = writeln!(vmx, "firmware = \"efi\"");
            let _ = writeln!(vmx, "uefi.secureBoot.enabled = \"TRUE\"");
        }
        Some(VmwareFirmware::Bios) => {
            let _ = writeln!(vmx, "firmware = \"bios\"");
        }
        Some(VmwareFirmware::Custom(s)) => {
            let _ = writeln!(vmx, "firmware = \"{s}\"");
        }
        None => {}
    }

    // Disk controller and disk attachment
    match disk_adapter.unwrap_or(&VmwareDiskAdapterType::LsiLogic) {
        VmwareDiskAdapterType::Nvme => {
            let _ = writeln!(
                vmx,
                "nvme0.present = \"TRUE\"\nnvme0:0.present = \"TRUE\"\nnvme0:0.fileName = \"disk.vmdk\""
            );
        }
        VmwareDiskAdapterType::Sata => {
            let _ = writeln!(
                vmx,
                "sata0.present = \"TRUE\"\nsata0:0.present = \"TRUE\"\nsata0:0.fileName = \"disk.vmdk\""
            );
        }
        VmwareDiskAdapterType::Ide => {
            let _ = writeln!(
                vmx,
                "ide0:0.present = \"TRUE\"\nide0:0.fileName = \"disk.vmdk\""
            );
        }
        scsi_type => {
            let dev_str = scsi_type.as_str();
            let _ = writeln!(
                vmx,
                "scsi0.present = \"TRUE\"\nscsi0.virtualDev = \"{dev_str}\"\nscsi0:0.present = \"TRUE\"\nscsi0:0.fileName = \"disk.vmdk\""
            );
        }
    }

    // Optical drive / installer ISO
    if let Some(iso) = iso_path {
        match cdrom_adapter.unwrap_or(&VmwareCdromAdapterType::Ide) {
            VmwareCdromAdapterType::Sata => {
                let _ = writeln!(
                    vmx,
                    "sata0:1.present = \"TRUE\"\nsata0:1.deviceType = \"cdrom-image\"\nsata0:1.fileName = \"{iso}\""
                );
            }
            VmwareCdromAdapterType::Scsi => {
                let _ = writeln!(
                    vmx,
                    "scsi0:1.present = \"TRUE\"\nscsi0:1.deviceType = \"cdrom-image\"\nscsi0:1.fileName = \"{iso}\""
                );
            }
            _ => {
                let _ = writeln!(
                    vmx,
                    "ide1:0.present = \"TRUE\"\nide1:0.deviceType = \"cdrom-image\"\nide1:0.fileName = \"{iso}\""
                );
            }
        }
    }

    // Network configuration
    if !remove_ethernet {
        let net_type = network.unwrap_or("nat");
        let nic_str = nic_type.unwrap_or(&VmwareNicType::E1000).as_str();
        let _ = writeln!(
            vmx,
            "ethernet0.present = \"TRUE\"\nethernet0.connectionType = \"{net_type}\"\nethernet0.virtualDev = \"{nic_str}\"\nethernet0.addressType = \"generated\""
        );
    }

    // USB configuration
    if usb == Some(true) {
        let _ = writeln!(vmx, "usb.present = \"TRUE\"\nusb_xhci.present = \"TRUE\"");
    }

    // VNC headless display configuration
    if let Some(port) = vnc_port {
        let _ = writeln!(
            vmx,
            "RemoteDisplay.vnc.enabled = \"TRUE\"\nRemoteDisplay.vnc.port = \"{port}\""
        );
        if vnc_disable_password {
            let _ = writeln!(vmx, "RemoteDisplay.vnc.password = \"\"");
        }
    }

    // Common VMware infrastructure properties
    let _ = writeln!(
        vmx,
        "pciBridge0.present = \"TRUE\"\ntools.upgrade.policy = \"manual\"\npowerType.powerOff = \"soft\"\npowerType.reset = \"soft\"\npowerType.suspend = \"soft\""
    );

    // Injected custom vmx_data properties
    let mut sorted_keys: Vec<_> = custom_data.keys().collect();
    sorted_keys.sort();
    for k in sorted_keys {
        if let Some(v) = custom_data.get(k) {
            let _ = writeln!(vmx, "{k} = \"{v}\"");
        }
    }

    vmx
}

/// Discover the guest IP address of a running `VMware` VM.
///
/// Attempts discovery via `vmrun getGuestIPAddress` first, then DHCP lease files.
///
/// # Errors
///
/// Returns `StampError::Builder` if IP address cannot be determined.
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
/// Returns `StampError::Builder` if IP address cannot be determined.
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
/// Returns `StampError::Builder` if IP address cannot be determined.
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
                Path::new(r"C:\ProgramData\VMware\vmnetdhcp.leases"),
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
            .map_err(|e| StampError::Builder(format!("Failed to create output dir: {e}")))?;

        let vmx_path = PathBuf::from(output_dir).join(format!("{vm_name}.vmx"));
        state.put("vmx_path", vmx_path.to_string_lossy().to_string());

        let guest_os = self.config.guest_os_type.as_deref().unwrap_or("other-64");
        let memory = self.config.memory.map_or(1024, |m| m.get());
        let cpus = self.config.cpus.unwrap_or(1);
        let iso_url = self.config.iso_url.as_deref();

        let mut vmx_data = self.config.vmx_data.clone();

        // Floppy image attachment
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

        // Secondary CD-ROM (cidata) attachment
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

        // VMware Tools attachment if mode is Attach
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

        // Create initial mock disk.vmdk descriptor if not present
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
            let vnc_port = self.config.vnc_port.map_or(5900, |p| p.get());
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

/// Step to upload `VMware` Tools ISO via communicator.
#[derive(Debug, Clone)]
struct StepUploadTools {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepUploadTools {
    #[cfg_attr(coverage_nightly, coverage(off))]
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
struct StepProvision {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
    /// Builder configuration.
    config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning VMware VM...");

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
    /// Builder configuration.
    config: VmwareIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down VMware VM...");
        let vmx_path = state.get::<String>("vmx_path").cloned().unwrap_or_default();

        if let Some(ref cmd) = self.config.shutdown_command {
            if let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator") {
                self.ui
                    .say(&self.name, &format!("Executing shutdown command: {cmd}"));
                let _ = comm
                    .execute(&crate::communicator::Command::new(cmd.clone()))
                    .await;
            }
        }

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
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                hook,
                config: self.config.clone(),
            }),
            Box::new(StepUploadTools {
                ui: ui.clone(),
                name: self.name(),
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
            let config = VmwareIsoConfig {
                name: "test-builder".to_string(),
                vm_name: Some("test-vm".to_string()),
                iso_url: Some("/tmp/test.iso".to_string()),
                memory: Some(MemoryMb::new(2048)),
                cpus: Some(2),
                cores: Some(2),
                guest_os_type: Some("ubuntu-64".to_string()),
                disk_adapter_type: Some(VmwareDiskAdapterType::Nvme),
                cdrom_adapter_type: Some(VmwareCdromAdapterType::Sata),
                network_adapter_type: Some(VmwareNicType::Vmxnet3),
                network: Some("nat".to_string()),
                firmware: Some(VmwareFirmware::EfiSecure),
                version: Some("16".to_string()),
                usb: Some(true),
                vnc_port: Some(Port::new(5901)),
                vnc_disable_password: true,
                tools_mode: VmwareToolsMode::Upload,
                tools_upload_flavor: Some(VmwareToolsFlavor::Linux),
                output_directory: Some(td.path().to_string_lossy().to_string()),
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

            let res = builder
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await;
            assert!(res.is_ok());
            for art in res {
                assert!(art.id().contains("test-vm.vmx"));
            }

            // Run with communicator: winrm and tools_mode: attach
            let winrm_cfg = VmwareIsoConfig {
                name: "winrm-builder".to_string(),
                communicator: Some("winrm".to_string()),
                winrm_host_port: Some(Port::new(5985)),
                tools_mode: VmwareToolsMode::Attach,
                output_directory: Some(td.path().to_string_lossy().to_string()),
                ..Default::default()
            };
            let b_winrm = VmwareIsoBuilder::new(winrm_cfg);
            let res_winrm = b_winrm
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await;
            assert!(res_winrm.is_ok());

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

        assert!(vmx.contains(r#"displayName = "my-vm""#));
        assert!(vmx.contains(r#"memsize = "4096""#));
        assert!(vmx.contains(r#"numvcpus = "4""#));
        assert!(vmx.contains(r#"ide1:0.fileName = "/tmp/ubuntu.iso""#));
        assert!(vmx.contains(r#"isolation.tools.copy.disable = "TRUE""#));

        // Test without iso_path
        let vmx_no_iso = generate_vmx_content("vm2", "other", 1024, 1, None, &custom);
        assert!(!vmx_no_iso.contains("ide1:0.fileName"));
    }

    #[test]
    fn test_vmx_generation_full_options() {
        let mut custom = HashMap::new();
        custom.insert("svga.autodetect".to_string(), "TRUE".to_string());

        // Test NVMe + SATA CD-ROM + EFI Secure
        let vmx1 = generate_vmx_content_full(
            "vm1",
            "windows9-64",
            8192,
            8,
            Some(4),
            Some("/path/win.iso"),
            Some(&VmwareDiskAdapterType::Nvme),
            Some(&VmwareCdromAdapterType::Sata),
            Some(&VmwareNicType::Vmxnet3),
            Some("bridged"),
            Some(&VmwareFirmware::EfiSecure),
            Some("19"),
            Some(true),
            false,
            Some(5905),
            true,
            &custom,
        );
        assert!(vmx1.contains(r#"virtualHW.version = "19""#));
        assert!(vmx1.contains(r#"cpuid.coresPerSocket = "4""#));
        assert!(vmx1.contains(r#"firmware = "efi""#));
        assert!(vmx1.contains(r#"uefi.secureBoot.enabled = "TRUE""#));
        assert!(vmx1.contains(r#"nvme0.present = "TRUE""#));
        assert!(vmx1.contains(r#"sata0:1.present = "TRUE""#));
        assert!(vmx1.contains(r#"ethernet0.virtualDev = "vmxnet3""#));
        assert!(vmx1.contains(r#"ethernet0.connectionType = "bridged""#));
        assert!(vmx1.contains(r#"usb.present = "TRUE""#));
        assert!(vmx1.contains(r#"RemoteDisplay.vnc.port = "5905""#));
        assert!(vmx1.contains(r#"RemoteDisplay.vnc.password = """#));

        // Test SATA disk + SCSI CD-ROM + BIOS firmware
        let vmx2 = generate_vmx_content_full(
            "vm2",
            "other",
            2048,
            2,
            None,
            Some("/path/other.iso"),
            Some(&VmwareDiskAdapterType::Sata),
            Some(&VmwareCdromAdapterType::Scsi),
            Some(&VmwareNicType::E1000e),
            Some("hostonly"),
            Some(&VmwareFirmware::Bios),
            None,
            None,
            false,
            None,
            false,
            &custom,
        );
        assert!(vmx2.contains(r#"sata0.present = "TRUE""#));
        assert!(vmx2.contains(r#"scsi0:1.present = "TRUE""#));
        assert!(vmx2.contains(r#"firmware = "bios""#));

        // Test IDE disk + Custom CD-ROM + EFI firmware + remove_ethernet
        let vmx3 = generate_vmx_content_full(
            "vm3",
            "other",
            1024,
            1,
            None,
            Some("/path/other.iso"),
            Some(&VmwareDiskAdapterType::Ide),
            Some(&VmwareCdromAdapterType::Custom("custom".to_string())),
            Some(&VmwareNicType::Vlance),
            None,
            Some(&VmwareFirmware::Efi),
            None,
            None,
            true,
            None,
            false,
            &custom,
        );
        assert!(vmx3.contains(r#"ide0:0.present = "TRUE""#));
        assert!(vmx3.contains(r#"firmware = "efi""#));
        assert!(!vmx3.contains("ethernet0.present"));

        // Test BusLogic and PvScsi and Custom disk adapters
        let vmx4 = generate_vmx_content_full(
            "vm4",
            "other",
            1024,
            1,
            None,
            None,
            Some(&VmwareDiskAdapterType::BusLogic),
            None,
            None,
            None,
            Some(&VmwareFirmware::Custom("custom_fw".to_string())),
            None,
            None,
            false,
            None,
            false,
            &custom,
        );
        assert!(vmx4.contains(r#"scsi0.virtualDev = "buslogic""#));
        assert!(vmx4.contains(r#"firmware = "custom_fw""#));

        let vmx5 = generate_vmx_content_full(
            "vm5",
            "other",
            1024,
            1,
            None,
            None,
            Some(&VmwareDiskAdapterType::PvScsi),
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            None,
            false,
            &custom,
        );
        assert!(vmx5.contains(r#"scsi0.virtualDev = "pvscsi""#));

        let vmx6 = generate_vmx_content_full(
            "vm6",
            "other",
            1024,
            1,
            None,
            None,
            Some(&VmwareDiskAdapterType::Custom("my_scsi".to_string())),
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            None,
            false,
            &custom,
        );
        assert!(vmx6.contains(r#"scsi0.virtualDev = "my_scsi""#));
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
        assert_eq!(VmwareDriver::Vmrun.as_str(), "vmrun");
        assert_eq!(VmwareDriver::Govc.as_str(), "govc");
        assert_eq!(format!("{}", VmwareDriver::Vmrun), "vmrun");
        assert_eq!(VmwareDriver::from_str("govc").unwrap(), VmwareDriver::Govc);
        assert_eq!(
            VmwareDriver::from_str("vmrun").unwrap(),
            VmwareDriver::Vmrun
        );
        assert_eq!(
            VmwareDriver::from_str("other").unwrap(),
            VmwareDriver::Vmrun
        );

        // VmwareFirmware
        assert_eq!(VmwareFirmware::Bios.as_str(), "bios");
        assert_eq!(VmwareFirmware::Efi.as_str(), "efi");
        assert_eq!(VmwareFirmware::EfiSecure.as_str(), "efi-secure");
        assert_eq!(
            VmwareFirmware::Custom("my_fw".to_string()).as_str(),
            "my_fw"
        );
        assert_eq!(format!("{}", VmwareFirmware::Bios), "bios");
        assert_eq!(
            VmwareFirmware::from_str("bios").unwrap(),
            VmwareFirmware::Bios
        );
        assert_eq!(
            VmwareFirmware::from_str("efi").unwrap(),
            VmwareFirmware::Efi
        );
        assert_eq!(
            VmwareFirmware::from_str("uefi").unwrap(),
            VmwareFirmware::Efi
        );
        assert_eq!(
            VmwareFirmware::from_str("efi-secure").unwrap(),
            VmwareFirmware::EfiSecure
        );
        assert_eq!(
            VmwareFirmware::from_str("custom").unwrap(),
            VmwareFirmware::Custom("custom".to_string())
        );

        // VmwareDiskAdapterType
        assert_eq!(VmwareDiskAdapterType::LsiLogic.as_str(), "lsilogic");
        assert_eq!(VmwareDiskAdapterType::PvScsi.as_str(), "pvscsi");
        assert_eq!(VmwareDiskAdapterType::BusLogic.as_str(), "buslogic");
        assert_eq!(VmwareDiskAdapterType::Nvme.as_str(), "nvme");
        assert_eq!(VmwareDiskAdapterType::Sata.as_str(), "sata");
        assert_eq!(VmwareDiskAdapterType::Ide.as_str(), "ide");
        assert_eq!(
            VmwareDiskAdapterType::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", VmwareDiskAdapterType::LsiLogic), "lsilogic");
        assert_eq!(
            VmwareDiskAdapterType::from_str("lsilogic").unwrap(),
            VmwareDiskAdapterType::LsiLogic
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("pvscsi").unwrap(),
            VmwareDiskAdapterType::PvScsi
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("buslogic").unwrap(),
            VmwareDiskAdapterType::BusLogic
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("nvme").unwrap(),
            VmwareDiskAdapterType::Nvme
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("sata").unwrap(),
            VmwareDiskAdapterType::Sata
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("ide").unwrap(),
            VmwareDiskAdapterType::Ide
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("other").unwrap(),
            VmwareDiskAdapterType::Custom("other".to_string())
        );

        // VmwareCdromAdapterType
        assert_eq!(VmwareCdromAdapterType::Ide.as_str(), "ide");
        assert_eq!(VmwareCdromAdapterType::Sata.as_str(), "sata");
        assert_eq!(VmwareCdromAdapterType::Scsi.as_str(), "scsi");
        assert_eq!(
            VmwareCdromAdapterType::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", VmwareCdromAdapterType::Ide), "ide");
        assert_eq!(
            VmwareCdromAdapterType::from_str("ide").unwrap(),
            VmwareCdromAdapterType::Ide
        );
        assert_eq!(
            VmwareCdromAdapterType::from_str("sata").unwrap(),
            VmwareCdromAdapterType::Sata
        );
        assert_eq!(
            VmwareCdromAdapterType::from_str("scsi").unwrap(),
            VmwareCdromAdapterType::Scsi
        );
        assert_eq!(
            VmwareCdromAdapterType::from_str("other").unwrap(),
            VmwareCdromAdapterType::Custom("other".to_string())
        );

        // VmwareNicType
        assert_eq!(VmwareNicType::Vmxnet3.as_str(), "vmxnet3");
        assert_eq!(VmwareNicType::E1000e.as_str(), "e1000e");
        assert_eq!(VmwareNicType::E1000.as_str(), "e1000");
        assert_eq!(VmwareNicType::Vlance.as_str(), "vlance");
        assert_eq!(
            VmwareNicType::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", VmwareNicType::E1000), "e1000");
        assert_eq!(
            VmwareNicType::from_str("vmxnet3").unwrap(),
            VmwareNicType::Vmxnet3
        );
        assert_eq!(
            VmwareNicType::from_str("e1000e").unwrap(),
            VmwareNicType::E1000e
        );
        assert_eq!(
            VmwareNicType::from_str("e1000").unwrap(),
            VmwareNicType::E1000
        );
        assert_eq!(
            VmwareNicType::from_str("vlance").unwrap(),
            VmwareNicType::Vlance
        );
        assert_eq!(
            VmwareNicType::from_str("other").unwrap(),
            VmwareNicType::Custom("other".to_string())
        );

        // VmwareToolsMode
        assert_eq!(VmwareToolsMode::Attach.as_str(), "attach");
        assert_eq!(VmwareToolsMode::Upload.as_str(), "upload");
        assert_eq!(VmwareToolsMode::Disable.as_str(), "disable");
        assert_eq!(format!("{}", VmwareToolsMode::Disable), "disable");
        assert_eq!(
            VmwareToolsMode::from_str("attach").unwrap(),
            VmwareToolsMode::Attach
        );
        assert_eq!(
            VmwareToolsMode::from_str("upload").unwrap(),
            VmwareToolsMode::Upload
        );
        assert_eq!(
            VmwareToolsMode::from_str("disable").unwrap(),
            VmwareToolsMode::Disable
        );

        // VmwareToolsFlavor
        assert_eq!(VmwareToolsFlavor::Darwin.as_str(), "darwin");
        assert_eq!(VmwareToolsFlavor::Linux.as_str(), "linux");
        assert_eq!(VmwareToolsFlavor::Windows.as_str(), "windows");
        assert_eq!(
            VmwareToolsFlavor::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", VmwareToolsFlavor::Linux), "linux");
        assert_eq!(
            VmwareToolsFlavor::from_str("darwin").unwrap(),
            VmwareToolsFlavor::Darwin
        );
        assert_eq!(
            VmwareToolsFlavor::from_str("linux").unwrap(),
            VmwareToolsFlavor::Linux
        );
        assert_eq!(
            VmwareToolsFlavor::from_str("windows").unwrap(),
            VmwareToolsFlavor::Windows
        );
        assert_eq!(
            VmwareToolsFlavor::from_str("other").unwrap(),
            VmwareToolsFlavor::Custom("other".to_string())
        );
    }

    #[tokio::test]
    async fn test_vmware_from_builder_config() {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "vmware-iso".to_string(),
            name: "bento-vmware".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("vm_name".to_string(), "my-bento-vmware".to_string());
        builder_cfg
            .config
            .insert("iso_url".to_string(), "/tmp/iso.iso".to_string());
        builder_cfg
            .config
            .insert("iso_checksum".to_string(), "sha256:123".to_string());
        builder_cfg
            .config
            .insert("memory".to_string(), "4096".to_string());
        builder_cfg
            .config
            .insert("cpus".to_string(), "4".to_string());
        builder_cfg
            .config
            .insert("vmware_cores".to_string(), "2".to_string());
        builder_cfg
            .config
            .insert("disk_size".to_string(), "50000".to_string());
        builder_cfg
            .config
            .insert("vmware_disk_adapter_type".to_string(), "nvme".to_string());
        builder_cfg
            .config
            .insert("vmware_cdrom_adapter_type".to_string(), "sata".to_string());
        builder_cfg.config.insert(
            "vmware_network_adapter_type".to_string(),
            "vmxnet3".to_string(),
        );
        builder_cfg
            .config
            .insert("vmware_network".to_string(), "nat".to_string());
        builder_cfg
            .config
            .insert("vmware_firmware".to_string(), "efi-secure".to_string());
        builder_cfg
            .config
            .insert("vmware_version".to_string(), "19".to_string());
        builder_cfg
            .config
            .insert("vmware_usb".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("vmware_tools_mode".to_string(), "upload".to_string());
        builder_cfg.config.insert(
            "vmware_tools_upload_flavor".to_string(),
            "linux".to_string(),
        );
        builder_cfg.config.insert(
            "vmware_tools_upload_path".to_string(),
            "/tmp/tools.iso".to_string(),
        );
        builder_cfg
            .config
            .insert("ssh_username".to_string(), "vagrant".to_string());
        builder_cfg
            .config
            .insert("ssh_password".to_string(), "vagrant".to_string());
        builder_cfg
            .config
            .insert("ssh_port".to_string(), "2222".to_string());
        builder_cfg
            .config
            .insert("headless".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("shutdown_command".to_string(), "poweroff".to_string());
        builder_cfg
            .config
            .insert("export_format".to_string(), "ova".to_string());
        builder_cfg.config.insert(
            "vmware_vmx_data".to_string(),
            r#"{"svga.autodetect": "TRUE"}"#.to_string(),
        );

        let cfg = VmwareIsoConfig::from_builder_config(&builder_cfg).unwrap();
        assert_eq!(cfg.name, "bento-vmware");
        assert_eq!(cfg.vm_name.as_deref(), Some("my-bento-vmware"));
        assert_eq!(cfg.memory, Some(MemoryMb::new(4096)));
        assert_eq!(cfg.cpus, Some(4));
        assert_eq!(cfg.cores, Some(2));
        assert_eq!(cfg.disk_size, Some(MemoryMb::new(50000)));
        assert_eq!(cfg.disk_adapter_type, Some(VmwareDiskAdapterType::Nvme));
        assert_eq!(cfg.cdrom_adapter_type, Some(VmwareCdromAdapterType::Sata));
        assert_eq!(cfg.network_adapter_type, Some(VmwareNicType::Vmxnet3));
        assert_eq!(cfg.network.as_deref(), Some("nat"));
        assert_eq!(cfg.firmware, Some(VmwareFirmware::EfiSecure));
        assert_eq!(cfg.version.as_deref(), Some("19"));
        assert_eq!(cfg.usb, Some(true));
        assert_eq!(cfg.tools_mode, VmwareToolsMode::Upload);
        assert_eq!(cfg.tools_upload_flavor, Some(VmwareToolsFlavor::Linux));
        assert_eq!(cfg.tools_upload_path.as_deref(), Some("/tmp/tools.iso"));
        assert_eq!(cfg.ssh_port, Some(Port::new(2222)));
        assert_eq!(cfg.shutdown_command.as_deref(), Some("poweroff"));
        assert_eq!(cfg.export_format.as_deref(), Some("ova"));
        assert!(cfg.headless);
        assert_eq!(
            cfg.vmx_data.get("svga.autodetect"),
            Some(&"TRUE".to_string())
        );

        // Test with HCL Object expression for vmx_data
        use hashicorp_configuration_language_rs::ast::expr::Expression;
        use hashicorp_configuration_language_rs::span::Span;
        let dummy = Span::default();
        let obj_expr = Expression::Object(
            vec![(
                Expression::String("custom.key".to_string(), dummy.clone()),
                Expression::String("custom.val".to_string(), dummy.clone()),
            )],
            dummy,
        );
        let mut builder_hcl = crate::template::BuilderConfig {
            builder_type: "vmware-iso".to_string(),
            name: "hcl-vbox".to_string(),
            ..Default::default()
        };
        builder_hcl
            .expressions
            .insert("vmx_data".to_string(), obj_expr);
        let cfg_hcl = VmwareIsoConfig::from_builder_config(&builder_hcl).unwrap();
        assert_eq!(
            cfg_hcl.vmx_data.get("custom.key"),
            Some(&"custom.val".to_string())
        );
    }

    #[tokio::test]
    async fn test_vmware_find_paths_and_helpers() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let fake_vmrun = td.path().join("vmrun");
            let _ = tokio::fs::write(&fake_vmrun, b"#!/bin/sh\n").await;
            unsafe {
                std::env::set_var("VMRUN_PATH", fake_vmrun.to_string_lossy().to_string());
            }
            assert_eq!(find_vmrun_binary(), fake_vmrun);
            unsafe {
                std::env::remove_var("VMRUN_PATH");
            }

            let fake_tools = td.path().join("linux.iso");
            let _ = tokio::fs::write(&fake_tools, b"ISO").await;
            unsafe {
                std::env::set_var("VMWARE_TOOLS_ISO", fake_tools.to_string_lossy().to_string());
            }
            assert_eq!(find_vmware_tools_iso(None), Some(fake_tools.clone()));
            unsafe {
                std::env::remove_var("VMWARE_TOOLS_ISO");
            }

            // Test parse_string_list with json and csv
            assert_eq!(
                parse_string_list(r#"["val1", "val2"]"#),
                vec!["val1", "val2"]
            );
            assert_eq!(parse_string_list("a,,b"), vec!["a", "b"]);

            // Test parse_string_map with JSON string and invalid string
            let json_map_str = r#"{"json_k": "json_v"}"#.to_string();
            let parsed_map = parse_string_map(Some(&json_map_str), None);
            assert_eq!(parsed_map.get("json_k"), Some(&"json_v".to_string()));
            let inv_map_str = "not-json".to_string();
            assert!(parse_string_map(Some(&inv_map_str), None).is_empty());

            // Test find_vmrun_binary and find_vmware_tools_iso fallbacks
            let _ = find_vmrun_binary();
            let _ = find_vmware_tools_iso(Some(&VmwareToolsFlavor::Windows));

            // Test run_vmrun_with_cmd stdout error branch
            let stdout_err =
                run_vmrun_with_cmd("sh", &["-c", "echo 'stdout vmrun failure'; exit 1"]).await;
            assert!(stdout_err.is_err());

            // Test StepCreateVM cleanup
            let ui = Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            ));
            let mut step_create = StepCreateVM {
                ui: ui.clone(),
                name: "create-test".to_string(),
                config: VmwareIsoConfig::default(),
            };
            let mut state = StateBag::new();
            state.put("vmx_path", "/path/to/vm.vmx".to_string());
            step_create.cleanup(&state).await;

            // Test StepUploadTools
            let mut step_upload = StepUploadTools {
                ui: ui.clone(),
                name: "upload-test".to_string(),
                config: VmwareIsoConfig {
                    tools_mode: VmwareToolsMode::Upload,
                    tools_source_path: Some(fake_tools.to_string_lossy().to_string()),
                    ..Default::default()
                },
            };
            let comm: Arc<dyn Communicator> =
                Arc::new(crate::communicator::mock::MockCommunicator::new());
            state.put("communicator", comm);
            assert!(step_upload.run(&mut state).await.is_ok());
            step_upload.cleanup(&state).await;

            // Test StepShutdown with shutdown_command
            let mut step_shutdown = StepShutdown {
                ui: ui.clone(),
                name: "shutdown-test".to_string(),
                config: VmwareIsoConfig {
                    shutdown_command: Some("poweroff".to_string()),
                    ..Default::default()
                },
            };
            assert!(step_shutdown.run(&mut state).await.is_ok());
            step_shutdown.cleanup(&state).await;

            // Test StepProvision failure
            let mut step_prov_fail = StepProvision {
                ui,
                name: "prov-fail".to_string(),
                hook: Arc::new(DefaultProvisionHook {
                    provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
                    error_cleanup_provisioners: Arc::new(vec![]),
                }),
                config: VmwareIsoConfig::default(),
            };
            assert!(step_prov_fail.run(&mut state).await.is_err());
            step_prov_fail.cleanup(&state).await;
        }
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
                vnc_port: Some(Port::new(5910)),
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
            let _ = tokio::fs::write(
                &script,
                b"#!/bin/sh
echo 10.0.0.42
",
            )
            .await;
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
            let _ = tokio::fs::write(
                &lease_file,
                b"lease 192.168.99.123 {
  starts 12345;
}
",
            )
            .await;
            let ip_lease =
                discover_guest_ip_internal(&vmx, VmwareDriver::Vmrun, &[&lease_file]).await;
            assert!(ip_lease.is_ok());
            for ip in ip_lease {
                assert_eq!(ip, "192.168.99.123");
            }

            // Vmrun driver without matching lease (fallback to 127.0.0.1)
            let empty_lease = td.path().join("empty.leases");
            let _ = tokio::fs::write(
                &empty_lease,
                b"no lease lines here
",
            )
            .await;
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
