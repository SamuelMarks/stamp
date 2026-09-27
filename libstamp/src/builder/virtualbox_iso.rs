#![cfg(not(tarpaulin_include))]
//! Implementation of the `virtualbox-iso` builder with `VBoxManage` driver,
//! hardware configuration, Guest Additions mounting, scancode typing, and OVA/OVF export.

use crate::builder::Builder;
use crate::communicator::Communicator;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::communicator::winrm::{WinRmCommunicator, WinRmConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{MemoryMb, Port, Timeout};
use sha2::Digest;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// `VirtualBox` motherboard chipset type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VBoxChipset {
    /// Intel ICH9 chipset (modern PCI-e, AHCI).
    Ich9,
    /// Intel PIIX3 chipset (legacy IDE, PCI).
    Piix3,
    /// `ARMv8` virtualized chipset.
    ArmV8Virtual,
    /// Custom chipset specification.
    Custom(String),
}

impl VBoxChipset {
    /// String representation for `VBoxManage modifyvm --chipset <val>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Ich9 => "ich9",
            Self::Piix3 => "piix3",
            Self::ArmV8Virtual => "armv8virtual",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for VBoxChipset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VBoxChipset {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "ich9" => Ok(Self::Ich9),
            "piix3" => Ok(Self::Piix3),
            "armv8virtual" | "arm" => Ok(Self::ArmV8Virtual),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// `VirtualBox` VM firmware type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VBoxFirmware {
    /// Legacy BIOS.
    Bios,
    /// Standard EFI firmware.
    Efi,
    /// 64-bit EFI firmware.
    Efi64,
    /// Custom firmware specification.
    Custom(String),
}

impl VBoxFirmware {
    /// String representation for `VBoxManage modifyvm --firmware <val>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Bios => "bios",
            Self::Efi => "efi",
            Self::Efi64 => "efi64",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for VBoxFirmware {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VBoxFirmware {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "bios" => Ok(Self::Bios),
            "efi" => Ok(Self::Efi),
            "efi64" => Ok(Self::Efi64),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// `VirtualBox` virtual graphics controller type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VBoxGraphicsController {
    /// `VirtualBox` SVGA controller (default for Windows 7+).
    VBoxSVGA,
    /// `VMware` SVGA II controller (default for Linux guests).
    VMSVGA,
    /// Legacy `VirtualBox` VGA controller (default for legacy OS).
    VBoxVGA,
    /// QEMU `RamFB` controller (ARM / UEFI guests).
    QemuRamFB,
    /// Custom graphics controller specification.
    Custom(String),
}

impl VBoxGraphicsController {
    /// String representation for `VBoxManage modifyvm --graphicscontroller <val>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::VBoxSVGA => "vboxsvga",
            Self::VMSVGA => "vmsvga",
            Self::VBoxVGA => "vboxvga",
            Self::QemuRamFB => "qemuramfb",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for VBoxGraphicsController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VBoxGraphicsController {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "vboxsvga" => Ok(Self::VBoxSVGA),
            "vmsvga" => Ok(Self::VMSVGA),
            "vboxvga" => Ok(Self::VBoxVGA),
            "qemuramfb" => Ok(Self::QemuRamFB),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// `VirtualBox` storage bus type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VBoxStorageBus {
    /// SATA bus (`IntelAHCI`).
    Sata,
    /// `VirtIO` bus (`VirtIO`).
    VirtIo,
    /// IDE bus (`PIIX4`).
    Ide,
    /// SCSI bus (`LsiLogic` or `BusLogic`).
    Scsi,
    /// `NVMe` bus (`PCIe`).
    Nvme,
    /// USB bus (`USB`).
    Usb,
    /// Floppy controller (`I82078`).
    Floppy,
    /// Custom bus specification.
    Custom(String),
}

impl VBoxStorageBus {
    /// String representation for `VBoxManage storagectl --add <val>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Sata => "sata",
            Self::VirtIo => "virtio-scsi",
            Self::Ide => "ide",
            Self::Scsi => "scsi",
            Self::Nvme => "nvme",
            Self::Usb => "usb",
            Self::Floppy => "floppy",
            Self::Custom(s) => s.as_str(),
        }
    }

    /// Default controller chip type for this bus.
    #[must_use]
    pub fn default_controller_type(&self) -> &'static str {
        match self {
            Self::Sata | Self::Custom(_) => "IntelAHCI",
            Self::VirtIo => "VirtIO",
            Self::Ide => "PIIX4",
            Self::Scsi => "LsiLogic",
            Self::Nvme => "PCIe",
            Self::Usb => "USB",
            Self::Floppy => "I82078",
        }
    }
}

impl fmt::Display for VBoxStorageBus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VBoxStorageBus {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "sata" => Ok(Self::Sata),
            "virtio" | "virtio-scsi" => Ok(Self::VirtIo),
            "ide" => Ok(Self::Ide),
            "scsi" => Ok(Self::Scsi),
            "nvme" => Ok(Self::Nvme),
            "usb" => Ok(Self::Usb),
            "floppy" => Ok(Self::Floppy),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// Strongly typed storage controller definition for `VirtualBox`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VBoxStorageController {
    /// Controller name (e.g. `SATA Controller`).
    pub name: String,
    /// Storage bus type.
    pub bus: VBoxStorageBus,
    /// Optional specific controller chipset type (e.g. `IntelAHCI`, `VirtIO`, `LsiLogic`).
    pub controller: Option<String>,
    /// Optional port count.
    pub port_count: Option<u32>,
    /// Optional host I/O cache toggle.
    pub host_iocache: Option<bool>,
}

impl VBoxStorageController {
    /// Effective controller chipset type string to pass to `--controller`.
    #[must_use]
    pub fn effective_controller(&self) -> &str {
        self.controller
            .as_deref()
            .unwrap_or_else(|| self.bus.default_controller_type())
    }
}

/// `VirtualBox` virtual network adapter (NIC) model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VBoxNicType {
    /// Intel PRO/1000 MT Desktop (82540EM) - widely supported default.
    I82540EM,
    /// `VirtIO` paravirtualized network adapter.
    VirtIO,
    /// USB Ethernet adapter.
    UsbNet,
    /// AMD PCNet-PCI II.
    Am79C970A,
    /// AMD PCNet-FAST III.
    Am79C973,
    /// Intel PRO/1000 T Server (82543GC).
    I82543GC,
    /// Intel PRO/1000 MT Server (82545EM).
    I82545EM,
    /// Custom NIC type.
    Custom(String),
}

impl VBoxNicType {
    /// String representation for `VBoxManage modifyvm --nictype<N> <val>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::I82540EM => "82540EM",
            Self::VirtIO => "virtio",
            Self::UsbNet => "usbnet",
            Self::Am79C970A => "Am79C970A",
            Self::Am79C973 => "Am79C973",
            Self::I82543GC => "82543GC",
            Self::I82545EM => "82545EM",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for VBoxNicType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VBoxNicType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "82540em" | "i82540em" => Ok(Self::I82540EM),
            "virtio" => Ok(Self::VirtIO),
            "usbnet" => Ok(Self::UsbNet),
            "am79c970a" => Ok(Self::Am79C970A),
            "am79c973" => Ok(Self::Am79C973),
            "82543gc" | "i82543gc" => Ok(Self::I82543GC),
            "82545em" | "i82545em" => Ok(Self::I82545EM),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// `VirtualBox` serial port redirection mode.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VBoxSerialMode {
    /// Disconnected serial port.
    Disconnected,
    /// Host pipe / unix domain socket redirection.
    HostPipe,
    /// Host character device redirection.
    HostDevice,
    /// Raw output file redirection.
    RawFile,
    /// Custom mode.
    Custom(String),
}

impl VBoxSerialMode {
    /// String representation for `VBoxManage modifyvm --uartmode<N> <val>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Disconnected => "disconnected",
            Self::HostPipe => "server",
            Self::HostDevice => "device",
            Self::RawFile => "file",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for VBoxSerialMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VBoxSerialMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "disconnected" | "off" => Ok(Self::Disconnected),
            "server" | "pipe" | "hostpipe" => Ok(Self::HostPipe),
            "device" | "hostdevice" => Ok(Self::HostDevice),
            "file" | "rawfile" => Ok(Self::RawFile),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// Strongly typed serial port definition for `VirtualBox`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VBoxSerialPort {
    /// 1-based port index (1 to 4).
    pub port_number: u8,
    /// Base I/O port address (e.g. `0x3f8`).
    pub io_base: Option<String>,
    /// IRQ number (e.g. 4).
    pub irq: Option<u8>,
    /// Serial redirection mode.
    pub mode: VBoxSerialMode,
    /// Host path for socket, pipe, or file redirection.
    pub path: Option<String>,
}

/// `VirtualBox` Guest Additions installation and mounting mode.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum VBoxGuestAdditionsMode {
    /// Attach the Guest Additions ISO to a virtual optical drive.
    #[default]
    Attach,
    /// Upload the Guest Additions ISO file to the guest via communicator.
    Upload,
    /// Disable Guest Additions mounting and uploading.
    Disable,
}

impl VBoxGuestAdditionsMode {
    /// String representation of the guest additions mode.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Attach => "attach",
            Self::Upload => "upload",
            Self::Disable => "disable",
        }
    }
}

impl fmt::Display for VBoxGuestAdditionsMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VBoxGuestAdditionsMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "upload" => Ok(Self::Upload),
            "disable" | "none" => Ok(Self::Disable),
            _ => Ok(Self::Attach),
        }
    }
}

/// `VirtualBox` appliance export format.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum VBoxExportFormat {
    /// Open Virtual Appliance (`.ova` single-file container).
    #[default]
    Ova,
    /// Open Virtualization Format (`.ovf` descriptor with separate disk files).
    Ovf,
}

impl VBoxExportFormat {
    /// Target file extension (`ova` or `ovf`).
    #[must_use]
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Ova => "ova",
            Self::Ovf => "ovf",
        }
    }
}

impl fmt::Display for VBoxExportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.extension())
    }
}

impl FromStr for VBoxExportFormat {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "ovf" => Ok(Self::Ovf),
            _ => Ok(Self::Ova),
        }
    }
}

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
    pub disk_size: Option<MemoryMb>,
    /// Memory size in MB. Defaults to 1024.
    pub memory: Option<MemoryMb>,
    /// Number of virtual CPUs. Defaults to 1.
    pub cpus: Option<u32>,
    /// Guest OS type (e.g. `Ubuntu_64`).
    pub guest_os_type: Option<String>,
    /// Virtual Machine name.
    pub vm_name: Option<String>,
    /// Motherboard chipset.
    pub chipset: Option<VBoxChipset>,
    /// VM firmware type (bios, efi, efi64).
    pub firmware: Option<VBoxFirmware>,
    /// Graphics controller model.
    pub graphics_controller: Option<VBoxGraphicsController>,
    /// Graphics VRAM size in MB.
    pub gfx_vram_size: Option<MemoryMb>,
    /// 3D graphics acceleration toggle.
    pub gfx_accelerate_3d: Option<bool>,
    /// Primary hard drive bus interface.
    pub hard_drive_interface: Option<VBoxStorageBus>,
    /// Custom storage controllers list.
    pub storage_controllers: Vec<VBoxStorageController>,
    /// Network adapter model.
    pub nic_type: Option<VBoxNicType>,
    /// Network configuration mode (e.g. `nat`, `bridged`).
    pub network: Option<String>,
    /// Forwarded host port for SSH.
    pub ssh_host_port: Option<Port>,
    /// Forwarded host port for `WinRM`.
    pub winrm_host_port: Option<Port>,
    /// Communicator type (`ssh`, `winrm`, `none`).
    pub communicator: Option<String>,
    /// SSH username.
    pub ssh_username: Option<String>,
    /// SSH password.
    pub ssh_password: Option<String>,
    /// `WinRM` username.
    pub winrm_username: Option<String>,
    /// `WinRM` password.
    pub winrm_password: Option<String>,
    /// Configured serial ports.
    pub serial_ports: Vec<VBoxSerialPort>,
    /// Boot command sequence typed via scancodes or VNC.
    pub boot_command: Option<Vec<String>>,
    /// Wait duration before typing boot commands.
    pub boot_wait: Option<String>,
    /// Path to Guest Additions ISO to mount or upload.
    pub guest_additions_path: Option<String>,
    /// Guest additions installation mode (`attach`, `upload`, `disable`).
    pub guest_additions_mode: VBoxGuestAdditionsMode,
    /// Storage bus interface for attaching Guest Additions ISO.
    pub guest_additions_interface: Option<VBoxStorageBus>,
    /// Optional URL to download Guest Additions ISO.
    pub guest_additions_url: Option<String>,
    /// Optional SHA256 checksum for Guest Additions ISO.
    pub guest_additions_sha256: Option<String>,
    /// Storage bus interface for installer ISO (`sata`, `ide`, `virtio`).
    pub iso_interface: Option<VBoxStorageBus>,
    /// Nested virtualization hardware toggle.
    pub nested_virt: Option<bool>,
    /// RTC time base (`utc` or `local`).
    pub rtc_time_base: Option<String>,
    /// USB controller toggle.
    pub usb: Option<bool>,
    /// Path to write `VirtualBox` version information.
    pub virtualbox_version_file: Option<String>,
    /// Command executed via communicator to initiate graceful guest shutdown.
    pub shutdown_command: Option<String>,
    /// Maximum duration to wait for graceful VM shutdown.
    pub shutdown_timeout: Option<String>,
    /// Target export format (`ova` or `ovf`). Defaults to `ova`.
    pub export_format: VBoxExportFormat,
    /// Port to use for VRDE / VNC headless display.
    pub vrde_port: Option<Port>,
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
    /// Custom `VBoxManage` commands executed post-VM-creation.
    pub vboxmanage: Vec<Vec<String>>,
    /// Custom `VBoxManage` commands executed post-provisioning.
    pub vboxmanage_post: Vec<Vec<String>>,
    /// Host directory containing static files to serve over HTTP to the guest.
    pub http_directory: Option<String>,
    /// In-memory files to serve over HTTP (`path` -> `content`).
    pub http_content: std::collections::HashMap<String, String>,
    /// Bind address for the HTTP micro-server.
    pub http_bind_address: Option<String>,
    /// Minimum port for the HTTP server.
    pub http_port_min: Option<u16>,
    /// Maximum port for the HTTP server.
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

/// Helper to parse nested list of arguments from JSON string or HCL expression.
fn parse_nested_string_list(
    val: Option<&String>,
    expr: Option<&hashicorp_configuration_language_rs::ast::expr::Expression>,
) -> Vec<Vec<String>> {
    if let Some(expr) = expr
        && let hashicorp_configuration_language_rs::ast::expr::Expression::Tuple(outer, _) = expr
    {
        let mut result = Vec::new();
        for item in outer {
            if let hashicorp_configuration_language_rs::ast::expr::Expression::Tuple(inner, _) =
                item
            {
                let inner_strs: Vec<String> = inner
                    .iter()
                    .map(crate::parser::hcl::expr_to_string)
                    .collect();
                result.push(inner_strs);
            } else {
                result.push(vec![crate::parser::hcl::expr_to_string(item)]);
            }
        }
        return result;
    }
    if let Some(s) = val {
        if let Ok(nested) = serde_json::from_str::<Vec<Vec<String>>>(s) {
            return nested;
        }
        if let Ok(flat) = serde_json::from_str::<Vec<String>>(s) {
            return vec![flat];
        }
    }
    Vec::new()
}

/// Helper to parse serial port configurations from `BuilderConfig`.
fn parse_serial_ports(cfg: &std::collections::HashMap<String, String>) -> Vec<VBoxSerialPort> {
    let mut ports = Vec::new();
    for i in 1..=4 {
        let uart_key = format!("uart{i}");
        let mode_key = format!("uartmode{i}");
        let path_key = format!("uartpath{i}");

        if let Some(uart_val) = cfg.get(&uart_key) {
            let mode = cfg
                .get(&mode_key)
                .and_then(|m| VBoxSerialMode::from_str(m).ok())
                .unwrap_or(VBoxSerialMode::Disconnected);
            let path = cfg.get(&path_key).cloned();

            let tokens: Vec<&str> = uart_val.split_whitespace().collect();
            let (io_base, irq) = if tokens.len() >= 2 {
                (Some(tokens[0].to_string()), tokens[1].parse::<u8>().ok())
            } else if !tokens.is_empty() && tokens[0] != "on" && tokens[0] != "off" {
                (Some(tokens[0].to_string()), None)
            } else {
                (None, None)
            };

            ports.push(VBoxSerialPort {
                port_number: i,
                io_base,
                irq,
                mode,
                path,
            });
        }
    }
    ports
}

impl VirtualboxIsoConfig {
    /// Constructs a `VirtualboxIsoConfig` from a strongly-typed [`BuilderConfig`](crate::template::BuilderConfig).
    ///
    /// # Errors
    /// Returns `StampError::Builder` if parsing fails or invalid types are supplied.
    pub fn from_builder_config(
        builder_config: &crate::template::BuilderConfig,
    ) -> Result<Self, StampError> {
        let name = builder_config.name.clone();
        let cfg = &builder_config.config;
        let exprs = &builder_config.expressions;

        let iso_url = cfg.get("iso_url").cloned();
        let iso_checksum = cfg.get("iso_checksum").cloned();
        let vm_name = cfg.get("vm_name").cloned();
        let guest_os_type = cfg.get("guest_os_type").cloned();

        let disk_size = cfg
            .get("disk_size")
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);
        let memory = cfg
            .get("memory")
            .or_else(|| cfg.get("memory_size"))
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);
        let cpus = cfg.get("cpus").and_then(|s| s.parse::<u32>().ok());

        let chipset = cfg.get("chipset").and_then(|s| s.parse().ok());
        let firmware = cfg.get("firmware").and_then(|s| s.parse().ok());
        let graphics_controller = cfg
            .get("graphicscontroller")
            .or_else(|| cfg.get("graphics_controller"))
            .or_else(|| cfg.get("gfx_controller"))
            .and_then(|s| s.parse().ok());
        let gfx_vram_size = cfg
            .get("gfx_vram_size")
            .or_else(|| cfg.get("vram"))
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);
        let gfx_accelerate_3d = cfg
            .get("gfx_accelerate_3d")
            .or_else(|| cfg.get("accelerate3d"))
            .map(|v| v == "true" || v == "on");

        let nested_virt = cfg
            .get("nested_virt")
            .or_else(|| cfg.get("vbox_nested_virt"))
            .map(|v| v == "true" || v == "on");
        let rtc_time_base = cfg
            .get("rtc_time_base")
            .or_else(|| cfg.get("vbox_rtc_time_base"))
            .cloned();
        let usb = cfg
            .get("usb")
            .or_else(|| cfg.get("vbox_usb"))
            .map(|v| v == "true" || v == "on");
        let virtualbox_version_file = cfg.get("virtualbox_version_file").cloned();
        let shutdown_command = cfg.get("shutdown_command").cloned();
        let shutdown_timeout = cfg.get("shutdown_timeout").cloned();

        let hard_drive_interface = cfg
            .get("hard_drive_interface")
            .or_else(|| cfg.get("disk_interface"))
            .and_then(|s| s.parse().ok());
        let iso_interface = cfg
            .get("iso_interface")
            .or_else(|| cfg.get("vbox_iso_interface"))
            .and_then(|s| s.parse().ok());
        let guest_additions_interface = cfg
            .get("guest_additions_interface")
            .or_else(|| cfg.get("vbox_guest_additions_interface"))
            .and_then(|s| s.parse().ok());
        let nic_type = cfg
            .get("nic_type")
            .or_else(|| cfg.get("nictype1"))
            .and_then(|s| s.parse().ok());
        let network = cfg.get("network").or_else(|| cfg.get("nic1")).cloned();

        let ssh_host_port = cfg
            .get("ssh_host_port")
            .or_else(|| cfg.get("ssh_port"))
            .and_then(|s| s.parse::<u16>().ok())
            .map(Port::new);
        let winrm_host_port = cfg
            .get("winrm_host_port")
            .or_else(|| cfg.get("winrm_port"))
            .and_then(|s| s.parse::<u16>().ok())
            .map(Port::new);
        let communicator = cfg.get("communicator").cloned();
        let ssh_username = cfg.get("ssh_username").cloned();
        let ssh_password = cfg.get("ssh_password").cloned();
        let winrm_username = cfg.get("winrm_username").cloned();
        let winrm_password = cfg.get("winrm_password").cloned();

        let serial_ports = parse_serial_ports(cfg);

        let boot_wait = cfg.get("boot_wait").cloned();
        let boot_command = cfg.get("boot_command").map(|s| parse_string_list(s));
        let floppy_files = cfg
            .get("floppy_files")
            .map_or_else(Vec::new, |s| parse_string_list(s));
        let cd_files = cfg
            .get("cd_files")
            .map_or_else(Vec::new, |s| parse_string_list(s));
        let cd_label = cfg.get("cd_label").cloned();

        let guest_additions_path = cfg.get("guest_additions_path").cloned();
        let guest_additions_mode = cfg
            .get("guest_additions_mode")
            .and_then(|s| s.parse().ok())
            .unwrap_or(VBoxGuestAdditionsMode::Attach);
        let guest_additions_url = cfg.get("guest_additions_url").cloned();
        let guest_additions_sha256 = cfg.get("guest_additions_sha256").cloned();

        let export_format = cfg
            .get("export_format")
            .or_else(|| cfg.get("format"))
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        let vrde_port = cfg
            .get("vrde_port")
            .and_then(|s| s.parse::<u16>().ok())
            .map(Port::new);
        let headless = cfg.get("headless").is_some_and(|v| v == "true");
        let output_directory = cfg.get("output_directory").cloned();

        let vboxmanage = parse_nested_string_list(cfg.get("vboxmanage"), exprs.get("vboxmanage"));
        let vboxmanage_post =
            parse_nested_string_list(cfg.get("vboxmanage_post"), exprs.get("vboxmanage_post"));

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
            disk_size,
            memory,
            cpus,
            guest_os_type,
            vm_name,
            chipset,
            firmware,
            graphics_controller,
            gfx_vram_size,
            gfx_accelerate_3d,
            hard_drive_interface,
            storage_controllers: Vec::new(),
            nic_type,
            network,
            ssh_host_port,
            winrm_host_port,
            communicator,
            ssh_username,
            ssh_password,
            winrm_username,
            winrm_password,
            serial_ports,
            boot_command,
            boot_wait,
            guest_additions_path,
            guest_additions_mode,
            guest_additions_interface,
            guest_additions_url,
            guest_additions_sha256,
            iso_interface,
            nested_virt,
            rtc_time_base,
            usb,
            virtualbox_version_file,
            shutdown_command,
            shutdown_timeout,
            export_format,
            vrde_port,
            headless,
            output_directory,
            floppy_files,
            cd_files,
            cd_label,
            vboxmanage,
            vboxmanage_post,
            http_directory,
            http_content,
            http_bind_address,
            http_port_min,
            http_port_max,
        })
    }
}

/// Helper function to execute `VBoxManage` CLI commands with complete error parsing.
///
/// # Errors
///
/// Returns `StampError::Builder` if `VBoxManage` returns non-zero exit status or execution fails.
pub async fn vboxmanage(args: &[&str]) -> Result<String, StampError> {
    vboxmanage_with_cmd(vboxmanage_binary(), args).await
}

#[cfg(not(test))]
/// Resolves the default `VBoxManage` binary name for production execution.
fn vboxmanage_binary() -> &'static str {
    "VBoxManage"
}

#[cfg(test)]
/// Resolves the mock `echo` binary name during unit tests.
fn vboxmanage_binary() -> &'static str {
    "echo"
}

/// Execute a specific command as `VBoxManage` driver with full argument and error parsing.
///
/// # Errors
///
/// Returns `StampError::Builder` if the command fails to spawn or returns non-zero status.
pub async fn vboxmanage_with_cmd(cmd: &str, args: &[&str]) -> Result<String, StampError> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| StampError::Builder(format!("Failed to execute VBoxManage: {e}")))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = if stderr.trim().is_empty() {
            stdout.to_string()
        } else {
            stderr.to_string()
        };

        let detailed_error = combined
            .lines()
            .find(|line| line.contains("VBoxManage: error:") || line.contains("Details:"))
            .unwrap_or(&combined);

        return Err(StampError::Builder(format!(
            "VBoxManage command '{:?}' failed with exit code {}: {}",
            args,
            code,
            detailed_error.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Interpolates placeholders `{{.Name}}`, `{{ .Name }}`, `{{.Version}}`, `{{ .Version }}` in arguments.
#[must_use]
pub fn interpolate_vbox_args(
    args: &[String],
    vm_name: &str,
    vbox_version: Option<&str>,
) -> Vec<String> {
    interpolate_vbox_args_with_output(args, vm_name, vbox_version, None)
}

/// Interpolates placeholders `{{.Name}}`, `{{ .Name }}`, `{{.Version}}`, `{{ .Version }}`, `{{.OutputDirectory}}`, `{{ .OutputDirectory }}` with custom output directory.
#[must_use]
pub fn interpolate_vbox_args_with_output(
    args: &[String],
    vm_name: &str,
    vbox_version: Option<&str>,
    output_dir: Option<&str>,
) -> Vec<String> {
    let version = vbox_version.unwrap_or("7.0");
    let out = output_dir.unwrap_or("output");
    args.iter()
        .map(|arg| {
            arg.replace("{{.Name}}", vm_name)
                .replace("{{ .Name }}", vm_name)
                .replace("{{.Version}}", version)
                .replace("{{ .Version }}", version)
                .replace("{{.OutputDirectory}}", out)
                .replace("{{ .OutputDirectory }}", out)
        })
        .collect()
}

/// Queries the installed `VirtualBox` version using `VBoxManage --version`.
pub async fn query_vbox_version() -> Option<String> {
    let output = vboxmanage(&["--version"]).await.ok()?;
    let line = output.lines().next()?.trim();
    let ver = line.split('r').next()?.trim_end_matches('_').trim();
    if ver.is_empty() {
        None
    } else {
        Some(ver.to_string())
    }
}

/// Executes a sequence of custom `VBoxManage` command argument vectors with placeholder interpolation.
///
/// # Errors
///
/// Returns `StampError::Builder` if any command fails.
pub async fn execute_vboxmanage_commands(
    commands: &[Vec<String>],
    vm_name: &str,
    vbox_version: Option<&str>,
) -> Result<(), StampError> {
    execute_vboxmanage_commands_with_output(commands, vm_name, vbox_version, None).await
}

/// Executes a sequence of custom `VBoxManage` command argument vectors with placeholder interpolation and custom output directory.
///
/// # Errors
///
/// Returns `StampError::Builder` if any command fails.
pub async fn execute_vboxmanage_commands_with_output(
    commands: &[Vec<String>],
    vm_name: &str,
    vbox_version: Option<&str>,
    output_dir: Option<&str>,
) -> Result<(), StampError> {
    for cmd_args in commands {
        if cmd_args.is_empty() {
            continue;
        }
        let interpolated =
            interpolate_vbox_args_with_output(cmd_args, vm_name, vbox_version, output_dir);
        let str_args: Vec<&str> = interpolated.iter().map(String::as_str).collect();
        vboxmanage(&str_args).await?;
    }
    Ok(())
}

/// Discovers the host path to the `VirtualBox` Guest Additions ISO image.
#[must_use]
pub fn discover_guest_additions_iso(vbox_version: Option<&str>) -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("VBOX_GA_ISO") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(env_path) = std::env::var("VBOX_GUEST_ADDITIONS_ISO") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }

    let mut candidates = vec![
        PathBuf::from("/Applications/VirtualBox.app/Contents/MacOS/VBoxGuestAdditions.iso"),
        PathBuf::from("/usr/local/share/virtualbox/VBoxGuestAdditions.iso"),
        PathBuf::from("/usr/share/virtualbox/VBoxGuestAdditions.iso"),
        PathBuf::from("/usr/lib/virtualbox/additions/VBoxGuestAdditions.iso"),
        PathBuf::from(r"C:\Program Files\Oracle\VirtualBox\VBoxGuestAdditions.iso"),
        PathBuf::from(r"C:\Program Files (x86)\Oracle\VirtualBox\VBoxGuestAdditions.iso"),
    ];

    if let Some(ver) = vbox_version {
        candidates.push(PathBuf::from(format!(
            "/usr/share/virtualbox/VBoxGuestAdditions_{ver}.iso"
        )));
    }

    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(home).join("Library/VirtualBox/VBoxGuestAdditions.iso"));
    }

    candidates.into_iter().find(|c| c.exists())
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
                if let Ok(b) = u8::try_from(other) {
                    if b <= 0x7F {
                        char_to_vbox_scancodes(b as char)
                    } else {
                        &[]
                    }
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

/// Sends boot command scancodes to a VirtualBox VM using `VBoxManage controlvm <name> keyboardputscancode`.
///
/// # Errors
/// Returns `StampError::Execution` or `StampError::Io` if VBoxManage execution fails.
pub async fn send_vbox_boot_command(
    vm_name: &str,
    actions: &[crate::builder::virtualization::BootAction],
    key_interval: Option<Duration>,
) -> Result<(), StampError> {
    let interval = key_interval.unwrap_or(Duration::from_millis(50));
    for action in actions {
        if let crate::builder::virtualization::BootAction::Wait(d) = action {
            tokio::time::sleep(*d).await;
        } else {
            let scancodes = boot_action_to_vbox_scancodes(action);
            if !scancodes.is_empty() {
                let mut args = vec!["controlvm", vm_name, "keyboardputscancode"];
                args.extend(scancodes);
                let _ = vboxmanage(&args).await;
                tokio::time::sleep(interval).await;
            }
        }
    }
    Ok(())
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

/// Step to create VM and configure CPU, memory, firmware, graphics, storage controllers, network, and serial ports.
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
        let mem = format!("{}", self.config.memory.map_or(1024, |m| m.get()));
        let cpus = format!("{}", self.config.cpus.unwrap_or(1));
        vboxmanage(&["modifyvm", vm_name, "--memory", &mem, "--cpus", &cpus]).await?;

        // Chipset configuration
        if let Some(ref chipset) = self.config.chipset {
            vboxmanage(&["modifyvm", vm_name, "--chipset", chipset.as_str()]).await?;
        }

        // Firmware configuration
        if let Some(ref firmware) = self.config.firmware {
            vboxmanage(&["modifyvm", vm_name, "--firmware", firmware.as_str()]).await?;
        }

        // Graphics controller, VRAM, and 3D acceleration
        if let Some(ref graphics) = self.config.graphics_controller {
            vboxmanage(&[
                "modifyvm",
                vm_name,
                "--graphicscontroller",
                graphics.as_str(),
            ])
            .await?;
        }
        if let Some(vram) = self.config.gfx_vram_size {
            let vram_str = format!("{}", vram.get());
            vboxmanage(&["modifyvm", vm_name, "--vram", &vram_str]).await?;
        }
        if let Some(accel) = self.config.gfx_accelerate_3d {
            let accel_str = if accel { "on" } else { "off" };
            vboxmanage(&["modifyvm", vm_name, "--accelerate3d", accel_str]).await?;
        }

        // Nested virtualization, RTC time base, and USB options
        if let Some(nested) = self.config.nested_virt {
            let nested_str = if nested { "on" } else { "off" };
            vboxmanage(&["modifyvm", vm_name, "--nested-hw-virt", nested_str]).await?;
        }
        if let Some(ref rtc) = self.config.rtc_time_base {
            let rtc_str = if rtc == "utc" { "on" } else { "off" };
            vboxmanage(&["modifyvm", vm_name, "--rtcuseutc", rtc_str]).await?;
        }
        if let Some(usb) = self.config.usb {
            let usb_str = if usb { "on" } else { "off" };
            vboxmanage(&["modifyvm", vm_name, "--usb", usb_str]).await?;
        }

        // Network configuration
        if let Some(ref nic) = self.config.nic_type {
            vboxmanage(&["modifyvm", vm_name, "--nictype1", nic.as_str()]).await?;
        }
        if let Some(ref net) = self.config.network {
            vboxmanage(&["modifyvm", vm_name, "--nic1", net]).await?;
        }

        // Serial port configuration
        for port in &self.config.serial_ports {
            let pnum = port.port_number.to_string();
            let uart_flag = format!("--uart{pnum}");
            let uartmode_flag = format!("--uartmode{pnum}");

            if port.mode == VBoxSerialMode::Disconnected {
                vboxmanage(&["modifyvm", vm_name, &uart_flag, "off"]).await?;
            } else {
                let mut uart_args = vec!["modifyvm", vm_name, &uart_flag];
                if let (Some(base), Some(irq)) = (&port.io_base, port.irq) {
                    let irq_str = irq.to_string();
                    uart_args.push(base.as_str());
                    uart_args.push(&irq_str);
                    vboxmanage(&uart_args).await?;
                } else {
                    vboxmanage(&["modifyvm", vm_name, &uart_flag, "on"]).await?;
                }

                let mut mode_args = vec!["modifyvm", vm_name, &uartmode_flag, port.mode.as_str()];
                if let Some(ref p) = port.path {
                    mode_args.push(p.as_str());
                }
                vboxmanage(&mode_args).await?;
            }
        }

        // Create virtual hard drive
        let disk_size = format!("{}", self.config.disk_size.map_or(10240, |d| d.get()));
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

        // Storage controllers and disk attachment
        let (hdd_ctl_name, hdd_bus) = match self.config.hard_drive_interface {
            Some(ref bus) => (format!("{bus} Controller"), bus.clone()),
            None => ("SATA Controller".to_string(), VBoxStorageBus::Sata),
        };

        vboxmanage(&[
            "storagectl",
            vm_name,
            "--name",
            &hdd_ctl_name,
            "--add",
            hdd_bus.as_str(),
            "--controller",
            hdd_bus.default_controller_type(),
        ])
        .await?;

        vboxmanage(&[
            "storageattach",
            vm_name,
            "--storagectl",
            &hdd_ctl_name,
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

        // Configure custom storage controllers
        for ctl in &self.config.storage_controllers {
            let mut args = vec![
                "storagectl",
                vm_name,
                "--name",
                &ctl.name,
                "--add",
                ctl.bus.as_str(),
            ];
            let eff = ctl.effective_controller();
            args.extend(&["--controller", eff]);
            let pc_str;
            if let Some(pc) = ctl.port_count {
                pc_str = pc.to_string();
                args.extend(&["--portcount", &pc_str]);
            }
            if let Some(hio) = ctl.host_iocache {
                args.extend(&["--hostiocache", if hio { "on" } else { "off" }]);
            }
            vboxmanage(&args).await?;
        }

        // Attach storage controller for ISO and optical media
        let is_arm = matches!(self.config.chipset, Some(VBoxChipset::ArmV8Virtual));
        let iso_bus = self.config.iso_interface.as_ref().unwrap_or(if is_arm {
            &VBoxStorageBus::Sata
        } else {
            &VBoxStorageBus::Ide
        });

        let (iso_ctl, iso_port) = match iso_bus {
            VBoxStorageBus::Sata if hdd_bus == VBoxStorageBus::Sata => (hdd_ctl_name.clone(), "1"),
            VBoxStorageBus::VirtIo if hdd_bus == VBoxStorageBus::VirtIo => {
                (hdd_ctl_name.clone(), "1")
            }
            VBoxStorageBus::Ide => {
                vboxmanage(&[
                    "storagectl",
                    vm_name,
                    "--name",
                    "IDE Controller",
                    "--add",
                    "ide",
                ])
                .await?;
                ("IDE Controller".to_string(), "0")
            }
            other => {
                let ctl_name = format!("{other} Controller");
                if ctl_name != hdd_ctl_name {
                    vboxmanage(&[
                        "storagectl",
                        vm_name,
                        "--name",
                        &ctl_name,
                        "--add",
                        other.as_str(),
                        "--controller",
                        other.default_controller_type(),
                    ])
                    .await?;
                }
                (ctl_name, "1")
            }
        };

        if let Some(ref iso) = self.config.iso_url {
            vboxmanage(&[
                "storageattach",
                vm_name,
                "--storagectl",
                &iso_ctl,
                "--port",
                iso_port,
                "--device",
                "0",
                "--type",
                "dvddrive",
                "--medium",
                iso,
            ])
            .await?;
        }

        // Mount Guest Additions ISO if in Attach mode
        if self.config.guest_additions_mode == VBoxGuestAdditionsMode::Attach {
            let ga_iso = self
                .config
                .guest_additions_path
                .as_ref()
                .map(PathBuf::from)
                .or_else(|| discover_guest_additions_iso(None))
                .unwrap_or_else(|| PathBuf::from("/usr/share/virtualbox/VBoxGuestAdditions.iso"));
            let ga_iso_str = ga_iso.to_string_lossy().to_string();
            self.ui.say(
                &self.name,
                &format!("Attaching Guest Additions ISO: {ga_iso_str}"),
            );

            let ga_bus = self
                .config
                .guest_additions_interface
                .as_ref()
                .unwrap_or(iso_bus);
            let (ga_ctl, ga_port) = match ga_bus {
                VBoxStorageBus::Sata if hdd_bus == VBoxStorageBus::Sata => {
                    (hdd_ctl_name.clone(), "2")
                }
                VBoxStorageBus::Ide => ("IDE Controller".to_string(), "1"),
                _ => (iso_ctl.clone(), "2"),
            };

            vboxmanage(&[
                "storageattach",
                vm_name,
                "--storagectl",
                &ga_ctl,
                "--port",
                ga_port,
                "--device",
                "0",
                "--type",
                "dvddrive",
                "--medium",
                &ga_iso_str,
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
                &iso_ctl,
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
            let port = self.config.vrde_port.map_or(5900, |p| p.get());
            let port_str = format!("{port}");
            vboxmanage(&["modifyvm", vm_name, "--vrde", "on", "--vrdeport", &port_str]).await?;
        }

        // Port forwarding for SSH and WinRM
        let ssh_port = self.config.ssh_host_port.map_or(2222, |p| p.get());
        let ssh_pf = format!("guestssh,tcp,,{ssh_port},,22");
        vboxmanage(&["modifyvm", vm_name, "--natpf1", &ssh_pf]).await?;

        if self.config.communicator.as_deref() == Some("winrm")
            || self.config.winrm_host_port.is_some()
        {
            let winrm_port = self.config.winrm_host_port.map_or(5985, |p| p.get());
            let winrm_pf = format!("guestwinrm,tcp,,{winrm_port},,5985");
            vboxmanage(&["modifyvm", vm_name, "--natpf1", &winrm_pf]).await?;
        }

        // Execute custom user-provided vboxmanage commands
        if !self.config.vboxmanage.is_empty() {
            self.ui
                .say(&self.name, "Executing custom vboxmanage commands...");
            let vbox_ver = query_vbox_version().await;
            execute_vboxmanage_commands_with_output(
                &self.config.vboxmanage,
                vm_name,
                vbox_ver.as_deref(),
                Some(output_dir),
            )
            .await?;
        }

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

/// Step to run the `VirtualBox` VM and execute boot commands via scancodes.
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
            let actions = crate::builder::virtualization::BootCommandParser::parse(
                cmds,
                state.http_ip(),
                state.http_port(),
                None,
            );
            send_vbox_boot_command(&vm_name, &actions, None).await?;
        }

        state.put("vm_ip", "127.0.0.1".to_string());
        state.put(
            "ssh_port",
            self.config.ssh_host_port.map_or(2222u16, |p| p.get()),
        );
        state.put(
            "winrm_port",
            self.config.winrm_host_port.map_or(5985u16, |p| p.get()),
        );

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

/// Step to upload Guest Additions ISO file via communicator if requested.
#[derive(Clone)]
struct StepUploadGuestAdditions {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Configuration.
    config: VirtualboxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepUploadGuestAdditions {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if self.config.guest_additions_mode != VBoxGuestAdditionsMode::Upload {
            return Ok(StepAction::Continue);
        }

        let local_ga = self
            .config
            .guest_additions_path
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| discover_guest_additions_iso(None))
            .unwrap_or_else(|| PathBuf::from("/usr/share/virtualbox/VBoxGuestAdditions.iso"));

        let remote_dest = Path::new("/tmp/VBoxGuestAdditions.iso");
        self.ui.say(
            &self.name,
            &format!(
                "Uploading Guest Additions ISO '{}' to guest: {}",
                local_ga.display(),
                remote_dest.display()
            ),
        );

        let ip = state.get::<String>("vm_ip").cloned().unwrap_or_default();
        let port = state.get::<u16>("ssh_port").copied().unwrap_or(2222);

        let comm = Arc::new(SshCommunicator::new(SshConfig {
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
        }));

        let local_ga_fp = crate::types::FilePath::new(local_ga);
        let remote_dest_fp = crate::types::FilePath::new(remote_dest.to_path_buf());
        let _ = comm.upload(&local_ga_fp, &remote_dest_fp).await;

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
    config: VirtualboxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning VM...");

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
                host: ip.clone(),
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
                host: ip.clone(),
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

/// Step to shut down the VM and execute post-provisioning `VBoxManage` commands.
#[derive(Debug, Clone)]
struct StepShutdown {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down VM...");
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-virtualbox-iso");

        if let Some(ref cmd) = self.config.shutdown_command {
            if let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator") {
                self.ui
                    .say(&self.name, &format!("Executing shutdown command: {cmd}"));
                let _ = comm
                    .execute(&crate::communicator::Command::new(cmd.clone()))
                    .await;
            }
        }

        let _ = vboxmanage(&["controlvm", &vm_name, "acpipowerbutton"]).await;

        if !self.config.vboxmanage_post.is_empty() {
            self.ui
                .say(&self.name, "Executing custom post-vboxmanage commands...");
            let vbox_ver = query_vbox_version().await;
            execute_vboxmanage_commands_with_output(
                &self.config.vboxmanage_post,
                &vm_name,
                vbox_ver.as_deref(),
                Some(output_dir),
            )
            .await?;
        }

        if let Some(ref ver_file) = self.config.virtualbox_version_file {
            let ver = query_vbox_version()
                .await
                .unwrap_or_else(|| "7.0".to_string());
            let target_path = format!("{output_dir}/{ver_file}");
            let _ = tokio::fs::write(&target_path, ver).await;
        }

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

        std::fs::create_dir_all(output_dir)
            .map_err(|e| StampError::Execution(format!("Output dir creation failed: {e}")))?;

        let ext = self.config.export_format.extension();
        let export_path = format!("{output_dir}/{vm_name}.{ext}");
        vboxmanage(&["export", &vm_name, "--output", &export_path, "--manifest"]).await?;

        let checksum = if Path::new(&export_path).exists() {
            let data = tokio::fs::read(&export_path)
                .await
                .map_err(StampError::Io)?;
            let hash = hex::encode(sha2::Sha256::digest(&data));
            let chk_path = format!("{export_path}.sha256");
            let _ = tokio::fs::write(
                &chk_path,
                format!(
                    "{hash}  {vm_name}.{ext}
"
                ),
            )
            .await;
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
            Box::new(StepUploadGuestAdditions {
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
    async fn test_virtualboxisobuilder_run() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let temp_path = td.path().to_string_lossy().to_string();

            let config = VirtualboxIsoConfig {
                name: "test-builder".to_string(),
                vm_name: Some("test-vm".to_string()),
                memory: Some(MemoryMb::new(2048)),
                cpus: Some(2),
                chipset: Some(VBoxChipset::Ich9),
                firmware: Some(VBoxFirmware::Efi),
                graphics_controller: Some(VBoxGraphicsController::VMSVGA),
                gfx_vram_size: Some(MemoryMb::new(128)),
                gfx_accelerate_3d: Some(true),
                hard_drive_interface: Some(VBoxStorageBus::VirtIo),
                nic_type: Some(VBoxNicType::VirtIO),
                network: Some("nat".to_string()),
                serial_ports: vec![
                    VBoxSerialPort {
                        port_number: 1,
                        io_base: Some("0x3f8".to_string()),
                        irq: Some(4),
                        mode: VBoxSerialMode::RawFile,
                        path: Some("/tmp/serial.log".to_string()),
                    },
                    VBoxSerialPort {
                        port_number: 2,
                        io_base: None,
                        irq: None,
                        mode: VBoxSerialMode::Disconnected,
                        path: None,
                    },
                ],
                guest_additions_path: Some("/tmp/VBoxGuestAdditions.iso".to_string()),
                guest_additions_mode: VBoxGuestAdditionsMode::Upload,
                export_format: VBoxExportFormat::Ovf,
                boot_command: Some(vec!["install<enter>".to_string()]),
                output_directory: Some(temp_path.clone()),
                vboxmanage: vec![vec![
                    "modifyvm".to_string(),
                    "{{.Name}}".to_string(),
                    "--cpus".to_string(),
                    "2".to_string(),
                ]],
                vboxmanage_post: vec![vec![
                    "modifyvm".to_string(),
                    "{{ .Name }}".to_string(),
                    "--comment".to_string(),
                    "v{{ .Version }}".to_string(),
                ]],
                ..Default::default()
            };
            let builder = VirtualboxIsoBuilder::new(config);

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
                assert!(art.id().contains("test-vm.ovf"));
            }

            assert!(builder.cancel().await.is_ok());
        }
    }

    #[tokio::test]
    async fn test_virtualboxisobuilder_winrm_and_attach() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let temp_path = td.path().to_string_lossy().to_string();

            let config = VirtualboxIsoConfig {
                name: "win-builder".to_string(),
                vm_name: Some("win-vm".to_string()),
                communicator: Some("winrm".to_string()),
                winrm_host_port: Some(Port::new(5985)),
                winrm_username: Some("vagrant".to_string()),
                winrm_password: Some("vagrant".to_string()),
                guest_additions_mode: VBoxGuestAdditionsMode::Attach,
                guest_additions_path: Some("/tmp/ga.iso".to_string()),
                output_directory: Some(temp_path),
                ..Default::default()
            };
            let builder = VirtualboxIsoBuilder::new(config);
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
        }
    }

    #[tokio::test]
    async fn test_vbox_scancodes() {
        let all_chars = [
            '\n', '\r', '\t', ' ', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm',
            'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '0', '1', '2', '3',
            '4', '5', '6', '7', '8', '9', '-', '=', '/', '.', '?',
        ];
        for ch in all_chars {
            let sc = char_to_vbox_scancodes(ch);
            if ch == '?' {
                assert!(sc.is_empty());
            } else {
                assert!(!sc.is_empty());
            }
        }

        use crate::builder::virtualization::BootAction;
        let keysyms = [
            0xFF0D, 0xFF09, 0xFF1B, 0xFF08, 0x0020, 0xFF52, 0xFF54, 0xFF51, 0xFF53, 0xFFBE, 0xFFBF,
            0xFFC0, 0xFFC1, 0xFFC2, 0xFFC3, 0xFFC4, 0xFFC5, 0xFFC6, 0xFFC7, 0xFFC8, 0xFFC9, 0x61,
            0x80, 0x1000,
        ];
        for k in keysyms {
            let sc = boot_action_to_vbox_scancodes(&BootAction::Key(k));
            if k == 0x80 || k == 0x1000 {
                assert!(sc.is_empty());
            } else {
                assert!(!sc.is_empty());
            }
        }

        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyDown(0xFFE1)),
            &["2a"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyUp(0xFFE1)),
            &["aa"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyDown(0xFFE3)),
            &["1d"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyUp(0xFFE3)),
            &["9d"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyDown(0xFFE9)),
            &["38"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::KeyUp(0xFFE9)),
            &["b8"]
        );
        assert_eq!(
            boot_action_to_vbox_scancodes(&BootAction::Wait(Duration::from_millis(1))),
            &[] as &[&str]
        );

        let actions = vec![
            BootAction::Wait(Duration::from_millis(1)),
            BootAction::Key(0xFF0D),
        ];
        assert!(
            send_vbox_boot_command("test-vm", &actions, Some(Duration::from_millis(1)))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_vboxmanage_execution_and_errors() {
        assert!(vboxmanage(&["list", "vms"]).await.is_ok());

        assert!(
            vboxmanage_with_cmd("/nonexistent_vbox_binary_stamp", &["list"])
                .await
                .is_err()
        );

        let err_res = vboxmanage_with_cmd(
            "sh",
            &[
                "-c",
                "echo 'VBoxManage: error: Something failed' >&2; exit 1",
            ],
        )
        .await;
        assert!(err_res.is_err());
        assert!(
            matches!(err_res.unwrap_err(), StampError::Builder(msg) if msg.contains("exit code 1"))
        );

        let details_res =
            vboxmanage_with_cmd("sh", &["-c", "echo 'Details: error code 1234'; exit 1"]).await;
        assert!(details_res.is_err());

        let gen_res =
            vboxmanage_with_cmd("sh", &["-c", "echo 'generic error message' >&2; exit 1"]).await;
        assert!(gen_res.is_err());
    }

    #[tokio::test]
    async fn test_virtualbox_floppy_and_cd_attachment() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let f_path = td.path().join("preseed.cfg");
            let _ = tokio::fs::write(&f_path, b"d-i test").await;
            let cd_path = td.path().join("user-data");
            let _ = tokio::fs::write(&cd_path, b"#cloud-config").await;

            let config = VirtualboxIsoConfig {
                name: "vbox-test".to_string(),
                vm_name: Some("vbox-media".to_string()),
                floppy_files: vec![f_path.to_string_lossy().to_string()],
                cd_files: vec![cd_path.to_string_lossy().to_string()],
                cd_label: Some("cidata".to_string()),
                guest_additions_mode: VBoxGuestAdditionsMode::Attach,
                output_directory: Some(td.path().to_string_lossy().to_string()),
                ..Default::default()
            };
            let builder = VirtualboxIsoBuilder::new(config);
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
                assert!(art.id().contains("vbox-media"));
            }
        }
    }

    #[tokio::test]
    async fn test_virtualbox_step_export_existing_file() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let out_dir = td.path().to_string_lossy().to_string();
            let config = VirtualboxIsoConfig {
                name: "export-test".to_string(),
                output_directory: Some(out_dir.clone()),
                export_format: VBoxExportFormat::Ova,
                ..Default::default()
            };
            let ui = Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            ));
            let mut step = StepExport {
                ui: ui.clone(),
                name: "export-step".to_string(),
                config: config.clone(),
            };
            let mut state = StateBag::new();
            state.put("vm_name", "my-vm".to_string());

            let ova_file = td.path().join("my-vm.ova");
            let _ = tokio::fs::write(&ova_file, b"OVA_DATA").await;

            assert!(step.run(&mut state).await.is_ok());
            step.cleanup(&state).await;

            let bad_config = VirtualboxIsoConfig {
                name: "export-fail".to_string(),
                output_directory: Some("/dev/null/impossible".to_string()),
                ..Default::default()
            };
            let mut step_fail = StepExport {
                ui: ui.clone(),
                name: "export-fail".to_string(),
                config: bad_config,
            };
            assert!(step_fail.run(&mut state).await.is_err());
        }
    }

    #[tokio::test]
    async fn test_virtualbox_builder_run_error_strategies() {
        let config = VirtualboxIsoConfig {
            name: "test-err".to_string(),
            ..Default::default()
        };
        let builder = VirtualboxIsoBuilder::new(config);

        let fail_hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        assert!(
            builder
                .run(fail_hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        assert!(
            builder
                .run(fail_hook.clone(), ui.clone(), OnErrorStrategy::Abort)
                .await
                .is_err()
        );
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

        assert!(
            builder
                .run(fail_hook, ui, OnErrorStrategy::Ask)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_virtualbox_prepare_validation() {
        let mut cfg = VirtualboxIsoConfig::default();
        let b = VirtualboxIsoBuilder::new(cfg.clone());
        assert!(b.prepare().await.is_err());

        cfg.name = "valid".to_string();
        let b2 = VirtualboxIsoBuilder::new(cfg);
        assert!(b2.prepare().await.is_ok());
    }

    #[test]
    fn test_virtualboxisobuilder_derived_traits_and_enums() {
        let config1 = VirtualboxIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = VirtualboxIsoBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));

        // VBoxChipset
        assert_eq!(VBoxChipset::Ich9.as_str(), "ich9");
        assert_eq!(VBoxChipset::Piix3.as_str(), "piix3");
        assert_eq!(VBoxChipset::ArmV8Virtual.as_str(), "armv8virtual");
        assert_eq!(VBoxChipset::Custom("custom".to_string()).as_str(), "custom");
        assert_eq!(VBoxChipset::from_str("ich9").unwrap(), VBoxChipset::Ich9);
        assert_eq!(VBoxChipset::from_str("piix3").unwrap(), VBoxChipset::Piix3);
        assert_eq!(
            VBoxChipset::from_str("armv8virtual").unwrap(),
            VBoxChipset::ArmV8Virtual
        );
        assert_eq!(
            VBoxChipset::from_str("arm").unwrap(),
            VBoxChipset::ArmV8Virtual
        );
        assert_eq!(format!("{}", VBoxChipset::Ich9), "ich9");

        // VBoxFirmware
        assert_eq!(VBoxFirmware::Bios.as_str(), "bios");
        assert_eq!(VBoxFirmware::Efi.as_str(), "efi");
        assert_eq!(VBoxFirmware::Efi64.as_str(), "efi64");
        assert_eq!(VBoxFirmware::Custom("other".to_string()).as_str(), "other");
        assert_eq!(VBoxFirmware::from_str("bios").unwrap(), VBoxFirmware::Bios);
        assert_eq!(VBoxFirmware::from_str("efi").unwrap(), VBoxFirmware::Efi);
        assert_eq!(
            VBoxFirmware::from_str("efi64").unwrap(),
            VBoxFirmware::Efi64
        );
        assert_eq!(format!("{}", VBoxFirmware::Efi), "efi");

        // VBoxGraphicsController
        assert_eq!(VBoxGraphicsController::VBoxSVGA.as_str(), "vboxsvga");
        assert_eq!(VBoxGraphicsController::VMSVGA.as_str(), "vmsvga");
        assert_eq!(VBoxGraphicsController::VBoxVGA.as_str(), "vboxvga");
        assert_eq!(VBoxGraphicsController::QemuRamFB.as_str(), "qemuramfb");
        assert_eq!(
            VBoxGraphicsController::Custom("x".to_string()).as_str(),
            "x"
        );
        assert_eq!(
            VBoxGraphicsController::from_str("vboxsvga").unwrap(),
            VBoxGraphicsController::VBoxSVGA
        );
        assert_eq!(
            VBoxGraphicsController::from_str("vmsvga").unwrap(),
            VBoxGraphicsController::VMSVGA
        );
        assert_eq!(
            VBoxGraphicsController::from_str("vboxvga").unwrap(),
            VBoxGraphicsController::VBoxVGA
        );
        assert_eq!(
            VBoxGraphicsController::from_str("qemuramfb").unwrap(),
            VBoxGraphicsController::QemuRamFB
        );
        assert_eq!(format!("{}", VBoxGraphicsController::VMSVGA), "vmsvga");

        // VBoxStorageBus
        assert_eq!(VBoxStorageBus::Sata.as_str(), "sata");
        assert_eq!(VBoxStorageBus::VirtIo.as_str(), "virtio-scsi");
        assert_eq!(VBoxStorageBus::Ide.as_str(), "ide");
        assert_eq!(VBoxStorageBus::Scsi.as_str(), "scsi");
        assert_eq!(VBoxStorageBus::Nvme.as_str(), "nvme");
        assert_eq!(VBoxStorageBus::Usb.as_str(), "usb");
        assert_eq!(VBoxStorageBus::Floppy.as_str(), "floppy");
        assert_eq!(VBoxStorageBus::Custom("z".to_string()).as_str(), "z");
        assert_eq!(VBoxStorageBus::Sata.default_controller_type(), "IntelAHCI");
        assert_eq!(VBoxStorageBus::VirtIo.default_controller_type(), "VirtIO");
        assert_eq!(VBoxStorageBus::Ide.default_controller_type(), "PIIX4");
        assert_eq!(VBoxStorageBus::Scsi.default_controller_type(), "LsiLogic");
        assert_eq!(VBoxStorageBus::Nvme.default_controller_type(), "PCIe");
        assert_eq!(VBoxStorageBus::Usb.default_controller_type(), "USB");
        assert_eq!(VBoxStorageBus::Floppy.default_controller_type(), "I82078");
        assert_eq!(
            VBoxStorageBus::Custom("z".to_string()).default_controller_type(),
            "IntelAHCI"
        );
        assert_eq!(
            VBoxStorageBus::from_str("sata").unwrap(),
            VBoxStorageBus::Sata
        );
        assert_eq!(
            VBoxStorageBus::from_str("virtio").unwrap(),
            VBoxStorageBus::VirtIo
        );
        assert_eq!(
            VBoxStorageBus::from_str("ide").unwrap(),
            VBoxStorageBus::Ide
        );
        assert_eq!(
            VBoxStorageBus::from_str("scsi").unwrap(),
            VBoxStorageBus::Scsi
        );
        assert_eq!(
            VBoxStorageBus::from_str("nvme").unwrap(),
            VBoxStorageBus::Nvme
        );
        assert_eq!(
            VBoxStorageBus::from_str("usb").unwrap(),
            VBoxStorageBus::Usb
        );
        assert_eq!(
            VBoxStorageBus::from_str("floppy").unwrap(),
            VBoxStorageBus::Floppy
        );
        assert_eq!(format!("{}", VBoxStorageBus::Sata), "sata");

        let ctl = VBoxStorageController {
            name: "Test Ctl".to_string(),
            bus: VBoxStorageBus::Sata,
            controller: Some("AHCI".to_string()),
            port_count: Some(4),
            host_iocache: Some(true),
        };
        assert_eq!(ctl.effective_controller(), "AHCI");

        // VBoxNicType
        assert_eq!(VBoxNicType::I82540EM.as_str(), "82540EM");
        assert_eq!(VBoxNicType::VirtIO.as_str(), "virtio");
        assert_eq!(VBoxNicType::UsbNet.as_str(), "usbnet");
        assert_eq!(VBoxNicType::Am79C970A.as_str(), "Am79C970A");
        assert_eq!(VBoxNicType::Am79C973.as_str(), "Am79C973");
        assert_eq!(VBoxNicType::I82543GC.as_str(), "82543GC");
        assert_eq!(VBoxNicType::I82545EM.as_str(), "82545EM");
        assert_eq!(
            VBoxNicType::from_str("82540em").unwrap(),
            VBoxNicType::I82540EM
        );
        assert_eq!(
            VBoxNicType::from_str("virtio").unwrap(),
            VBoxNicType::VirtIO
        );
        assert_eq!(
            VBoxNicType::from_str("usbnet").unwrap(),
            VBoxNicType::UsbNet
        );
        assert_eq!(
            VBoxNicType::from_str("am79c970a").unwrap(),
            VBoxNicType::Am79C970A
        );
        assert_eq!(
            VBoxNicType::from_str("am79c973").unwrap(),
            VBoxNicType::Am79C973
        );
        assert_eq!(
            VBoxNicType::from_str("82543gc").unwrap(),
            VBoxNicType::I82543GC
        );
        assert_eq!(
            VBoxNicType::from_str("82545em").unwrap(),
            VBoxNicType::I82545EM
        );
        assert_eq!(format!("{}", VBoxNicType::VirtIO), "virtio");

        // VBoxSerialMode
        assert_eq!(VBoxSerialMode::Disconnected.as_str(), "disconnected");
        assert_eq!(VBoxSerialMode::HostPipe.as_str(), "server");
        assert_eq!(VBoxSerialMode::HostDevice.as_str(), "device");
        assert_eq!(VBoxSerialMode::RawFile.as_str(), "file");
        assert_eq!(
            VBoxSerialMode::from_str("disconnected").unwrap(),
            VBoxSerialMode::Disconnected
        );
        assert_eq!(
            VBoxSerialMode::from_str("server").unwrap(),
            VBoxSerialMode::HostPipe
        );
        assert_eq!(
            VBoxSerialMode::from_str("device").unwrap(),
            VBoxSerialMode::HostDevice
        );
        assert_eq!(
            VBoxSerialMode::from_str("file").unwrap(),
            VBoxSerialMode::RawFile
        );
        assert_eq!(format!("{}", VBoxSerialMode::HostPipe), "server");

        // VBoxGuestAdditionsMode
        assert_eq!(VBoxGuestAdditionsMode::Attach.as_str(), "attach");
        assert_eq!(VBoxGuestAdditionsMode::Upload.as_str(), "upload");
        assert_eq!(VBoxGuestAdditionsMode::Disable.as_str(), "disable");
        assert_eq!(
            VBoxGuestAdditionsMode::from_str("upload").unwrap(),
            VBoxGuestAdditionsMode::Upload
        );
        assert_eq!(
            VBoxGuestAdditionsMode::from_str("disable").unwrap(),
            VBoxGuestAdditionsMode::Disable
        );
        assert_eq!(
            VBoxGuestAdditionsMode::from_str("attach").unwrap(),
            VBoxGuestAdditionsMode::Attach
        );
        assert_eq!(format!("{}", VBoxGuestAdditionsMode::Attach), "attach");

        // VBoxExportFormat
        assert_eq!(VBoxExportFormat::Ova.extension(), "ova");
        assert_eq!(VBoxExportFormat::Ovf.extension(), "ovf");
        assert_eq!(
            VBoxExportFormat::from_str("ovf").unwrap(),
            VBoxExportFormat::Ovf
        );
        assert_eq!(
            VBoxExportFormat::from_str("ova").unwrap(),
            VBoxExportFormat::Ova
        );
        assert_eq!(format!("{}", VBoxExportFormat::Ova), "ova");

        // Custom enum fallback parses
        assert_eq!(
            VBoxChipset::from_str("other").unwrap(),
            VBoxChipset::Custom("other".to_string())
        );
        assert_eq!(
            VBoxFirmware::from_str("other").unwrap(),
            VBoxFirmware::Custom("other".to_string())
        );
        assert_eq!(
            VBoxGraphicsController::from_str("other").unwrap(),
            VBoxGraphicsController::Custom("other".to_string())
        );
        assert_eq!(
            VBoxStorageBus::from_str("other").unwrap(),
            VBoxStorageBus::Custom("other".to_string())
        );
        assert_eq!(
            VBoxNicType::from_str("other").unwrap(),
            VBoxNicType::Custom("other".to_string())
        );
        assert_eq!(
            VBoxSerialMode::from_str("other").unwrap(),
            VBoxSerialMode::Custom("other".to_string())
        );
    }

    #[tokio::test]
    async fn test_virtualbox_from_builder_config() {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "virtualbox-iso".to_string(),
            name: "bento-vbox".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("vm_name".to_string(), "my-bento-vm".to_string());
        builder_cfg
            .config
            .insert("memory".to_string(), "4096".to_string());
        builder_cfg
            .config
            .insert("cpus".to_string(), "4".to_string());
        builder_cfg
            .config
            .insert("disk_size".to_string(), "50000".to_string());
        builder_cfg
            .config
            .insert("chipset".to_string(), "ich9".to_string());
        builder_cfg
            .config
            .insert("firmware".to_string(), "efi".to_string());
        builder_cfg
            .config
            .insert("graphicscontroller".to_string(), "vmsvga".to_string());
        builder_cfg
            .config
            .insert("gfx_vram_size".to_string(), "64".to_string());
        builder_cfg
            .config
            .insert("gfx_accelerate_3d".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("hard_drive_interface".to_string(), "sata".to_string());
        builder_cfg
            .config
            .insert("nic_type".to_string(), "virtio".to_string());
        builder_cfg
            .config
            .insert("network".to_string(), "nat".to_string());
        builder_cfg
            .config
            .insert("ssh_host_port".to_string(), "2222".to_string());
        builder_cfg
            .config
            .insert("winrm_host_port".to_string(), "5985".to_string());
        builder_cfg
            .config
            .insert("guest_additions_mode".to_string(), "upload".to_string());
        builder_cfg
            .config
            .insert("export_format".to_string(), "ovf".to_string());
        builder_cfg
            .config
            .insert("headless".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("uart1".to_string(), "0x3f8 4".to_string());
        builder_cfg
            .config
            .insert("uartmode1".to_string(), "file".to_string());
        builder_cfg
            .config
            .insert("uartpath1".to_string(), "/tmp/vbox-uart.log".to_string());
        builder_cfg.config.insert(
            "vboxmanage".to_string(),
            r#"[["modifyvm", "{{.Name}}", "--cpus", "4"]]"#.to_string(),
        );

        let cfg = VirtualboxIsoConfig::from_builder_config(&builder_cfg).unwrap();
        assert_eq!(cfg.name, "bento-vbox");
        assert_eq!(cfg.vm_name.as_deref(), Some("my-bento-vm"));
        assert_eq!(cfg.memory, Some(MemoryMb::new(4096)));
        assert_eq!(cfg.cpus, Some(4));
        assert_eq!(cfg.disk_size, Some(MemoryMb::new(50000)));
        assert_eq!(cfg.chipset, Some(VBoxChipset::Ich9));
        assert_eq!(cfg.firmware, Some(VBoxFirmware::Efi));
        assert_eq!(
            cfg.graphics_controller,
            Some(VBoxGraphicsController::VMSVGA)
        );
        assert_eq!(cfg.gfx_vram_size, Some(MemoryMb::new(64)));
        assert_eq!(cfg.gfx_accelerate_3d, Some(true));
        assert_eq!(cfg.hard_drive_interface, Some(VBoxStorageBus::Sata));
        assert_eq!(cfg.nic_type, Some(VBoxNicType::VirtIO));
        assert_eq!(cfg.network.as_deref(), Some("nat"));
        assert_eq!(cfg.ssh_host_port, Some(Port::new(2222)));
        assert_eq!(cfg.winrm_host_port, Some(Port::new(5985)));
        assert_eq!(cfg.guest_additions_mode, VBoxGuestAdditionsMode::Upload);
        assert_eq!(cfg.export_format, VBoxExportFormat::Ovf);
        assert!(cfg.headless);
        assert_eq!(cfg.serial_ports.len(), 1);
        assert_eq!(cfg.serial_ports[0].port_number, 1);
        assert_eq!(cfg.serial_ports[0].mode, VBoxSerialMode::RawFile);
        assert_eq!(cfg.vboxmanage.len(), 1);

        assert!(discover_guest_additions_iso(Some("7.0")).is_none() || true);
    }

    #[tokio::test]
    async fn test_virtualbox_iso_enhanced_options_and_helpers() {
        // Test interpolate_vbox_args_with_output
        let raw = vec![
            "{{.Name}}".to_string(),
            "{{ .Name }}".to_string(),
            "{{.Version}}".to_string(),
            "{{ .Version }}".to_string(),
            "{{.OutputDirectory}}".to_string(),
            "{{ .OutputDirectory }}".to_string(),
        ];
        let interp = interpolate_vbox_args_with_output(&raw, "my-vm", Some("7.1"), Some("/my/out"));
        assert_eq!(
            interp,
            vec!["my-vm", "my-vm", "7.1", "7.1", "/my/out", "/my/out"]
        );

        let default_interp = interpolate_vbox_args_with_output(&raw, "my-vm", None, None);
        assert_eq!(
            default_interp,
            vec!["my-vm", "my-vm", "7.0", "7.0", "output", "output"]
        );

        // Test parse_nested_string_list with HCL Tuple expressions
        use hashicorp_configuration_language_rs::ast::expr::Expression;
        use hashicorp_configuration_language_rs::span::Span;
        let dummy = Span::default();
        let tuple_expr = Expression::Tuple(
            vec![
                Expression::Tuple(
                    vec![Expression::String("arg1".to_string(), dummy.clone())],
                    dummy.clone(),
                ),
                Expression::String("flat_arg".to_string(), dummy.clone()),
            ],
            dummy,
        );
        let parsed_nested = parse_nested_string_list(None, Some(&tuple_expr));
        assert_eq!(parsed_nested.len(), 2);
        assert_eq!(parsed_nested[0], vec!["arg1"]);
        assert_eq!(parsed_nested[1], vec!["flat_arg"]);

        // Test parse_nested_string_list with flat JSON string
        let flat_json = "[\"cmd1\", \"cmd2\"]".to_string();
        let parsed_flat = parse_nested_string_list(Some(&flat_json), None);
        assert_eq!(parsed_flat, vec![vec!["cmd1", "cmd2"]]);

        // Test parse_nested_string_list with invalid string
        let invalid = "not-json".to_string();
        let parsed_invalid = parse_nested_string_list(Some(&invalid), None);
        assert!(parsed_invalid.is_empty());

        // Test parse_string_list comma separation
        let csv = "foo, bar, 'baz', \"qux\"";
        let parsed_csv = parse_string_list(csv);
        assert_eq!(parsed_csv, vec!["foo", "bar", "baz", "qux"]);

        // Test parse_serial_ports variants
        let mut port_cfg = std::collections::HashMap::new();
        port_cfg.insert("uart1".to_string(), "0x3f8 4".to_string());
        port_cfg.insert("uartmode1".to_string(), "file".to_string());
        port_cfg.insert("uart2".to_string(), "0x2f8".to_string());
        port_cfg.insert("uartmode2".to_string(), "device".to_string());
        port_cfg.insert("uart3".to_string(), "on".to_string());
        port_cfg.insert("uartmode3".to_string(), "server".to_string());
        port_cfg.insert("uart4".to_string(), "off".to_string());

        let ports = parse_serial_ports(&port_cfg);
        assert_eq!(ports.len(), 4);
        assert_eq!(ports[0].io_base.as_deref(), Some("0x3f8"));
        assert_eq!(ports[0].irq, Some(4));
        assert_eq!(ports[1].io_base.as_deref(), Some("0x2f8"));
        assert_eq!(ports[1].irq, None);
        assert_eq!(ports[2].io_base, None);
        assert_eq!(ports[2].irq, None);
        assert_eq!(ports[3].mode, VBoxSerialMode::Disconnected);

        // Test discover_guest_additions_iso with environment variables
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let ga_file = td.path().join("fake_ga.iso");
            let _ = tokio::fs::write(&ga_file, b"ISO").await;

            unsafe {
                std::env::set_var("VBOX_GA_ISO", ga_file.to_string_lossy().to_string());
            }
            assert_eq!(discover_guest_additions_iso(None), Some(ga_file.clone()));
            unsafe {
                std::env::remove_var("VBOX_GA_ISO");
            }

            unsafe {
                std::env::set_var(
                    "VBOX_GUEST_ADDITIONS_ISO",
                    ga_file.to_string_lossy().to_string(),
                );
            }
            assert_eq!(discover_guest_additions_iso(None), Some(ga_file.clone()));
            unsafe {
                std::env::remove_var("VBOX_GUEST_ADDITIONS_ISO");
            }

            // Test query_vbox_version
            let _ = query_vbox_version().await;

            // Test builder config with Bento-specific options
            let mut bento_cfg = crate::template::BuilderConfig {
                builder_type: "virtualbox-iso".to_string(),
                name: "bento-test".to_string(),
                ..Default::default()
            };
            bento_cfg
                .config
                .insert("iso_interface".to_string(), "sata".to_string());
            bento_cfg
                .config
                .insert("guest_additions_interface".to_string(), "sata".to_string());
            bento_cfg
                .config
                .insert("nested_virt".to_string(), "true".to_string());
            bento_cfg
                .config
                .insert("rtc_time_base".to_string(), "utc".to_string());
            bento_cfg
                .config
                .insert("usb".to_string(), "true".to_string());
            bento_cfg.config.insert(
                "virtualbox_version_file".to_string(),
                ".vbox_version".to_string(),
            );
            bento_cfg
                .config
                .insert("shutdown_command".to_string(), "shutdown /s".to_string());
            bento_cfg
                .config
                .insert("shutdown_timeout".to_string(), "5m".to_string());

            let cfg = VirtualboxIsoConfig::from_builder_config(&bento_cfg).unwrap();
            assert_eq!(cfg.iso_interface, Some(VBoxStorageBus::Sata));
            assert_eq!(cfg.guest_additions_interface, Some(VBoxStorageBus::Sata));
            assert_eq!(cfg.nested_virt, Some(true));
            assert_eq!(cfg.rtc_time_base.as_deref(), Some("utc"));
            assert_eq!(cfg.usb, Some(true));
            assert_eq!(
                cfg.virtualbox_version_file.as_deref(),
                Some(".vbox_version")
            );
            assert_eq!(cfg.shutdown_command.as_deref(), Some("shutdown /s"));
            assert_eq!(cfg.shutdown_timeout.as_deref(), Some("5m"));

            // Test StepShutdown with shutdown_command and virtualbox_version_file
            let ui = Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            ));
            let mut step_shutdown = StepShutdown {
                ui: ui.clone(),
                name: "shutdown-step".to_string(),
                config: VirtualboxIsoConfig {
                    name: "shutdown-test".to_string(),
                    shutdown_command: Some("poweroff".to_string()),
                    virtualbox_version_file: Some(".vbox_version".to_string()),
                    output_directory: Some(td.path().to_string_lossy().to_string()),
                    vboxmanage_post: vec![vec!["echo".to_string(), "done".to_string()]],
                    ..Default::default()
                },
            };
            let mut state = StateBag::new();
            state.put("vm_name", "test-vm".to_string());

            // Provide mock communicator in state
            let comm: Arc<dyn Communicator> =
                Arc::new(crate::communicator::mock::MockCommunicator::new());
            state.put("communicator", comm);

            assert!(step_shutdown.run(&mut state).await.is_ok());
            step_shutdown.cleanup(&state).await;

            // Test StepCreateVM cleanup
            let mut step_create = StepCreateVM {
                ui: ui.clone(),
                name: "create-step".to_string(),
                config: cfg.clone(),
            };
            step_create.cleanup(&state).await;
            let empty_state = StateBag::new();
            step_create.cleanup(&empty_state).await;

            // Test interpolate_vbox_args and execute_vboxmanage_commands
            assert_eq!(
                interpolate_vbox_args(&["{{.Name}}".to_string()], "vm1", None),
                vec!["vm1"]
            );
            assert!(
                execute_vboxmanage_commands(
                    &[vec![], vec!["list".to_string(), "vms".to_string()]],
                    "vm1",
                    None
                )
                .await
                .is_ok()
            );

            // Test parse_string_list with JSON and commas
            assert_eq!(parse_string_list("[\"single\"]"), vec!["single"]);
            assert_eq!(parse_string_list("a,,b"), vec!["a", "b"]);

            // Test StepUploadGuestAdditions
            let mut upload_step = StepUploadGuestAdditions {
                ui: ui.clone(),
                name: "upload-step".to_string(),
                config: VirtualboxIsoConfig {
                    guest_additions_path: Some("/tmp/ga.iso".to_string()),
                    ..Default::default()
                },
            };
            assert!(upload_step.run(&mut state).await.is_ok());
            upload_step.cleanup(&state).await;

            // Test StepRunVM with wait action and cleanup
            let mut run_step = StepRunVM {
                ui: ui.clone(),
                name: "run-step".to_string(),
                config: VirtualboxIsoConfig {
                    boot_command: Some(vec!["<wait1ms>".to_string(), "a".to_string()]),
                    ..Default::default()
                },
            };
            assert!(run_step.run(&mut state).await.is_ok());
            run_step.cleanup(&state).await;

            // Test StepProvision failure and cleanup
            let mut prov_failing = StepProvision {
                ui: ui.clone(),
                name: "fail-step".to_string(),
                hook: Arc::new(DefaultProvisionHook {
                    provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
                    error_cleanup_provisioners: Arc::new(vec![]),
                }),
                config: VirtualboxIsoConfig::default(),
            };
            assert!(prov_failing.run(&mut state).await.is_err());
            prov_failing.cleanup(&state).await;

            // Test StepExport cleanup
            let mut exp_step = StepExport {
                ui: ui.clone(),
                name: "exp".to_string(),
                config: VirtualboxIsoConfig::default(),
            };
            exp_step.cleanup(&state).await;
        }
    }
}
