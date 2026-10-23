//! Configuration types, chipsets, storage buses, and `VBoxManage` drivers for `virtualbox-iso`.

use crate::error::StampError;
use crate::types::{MemoryMb, Port};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
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
    pub const fn default_controller_type(&self) -> &'static str {
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
    pub const fn as_str(&self) -> &'static str {
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
    pub const fn extension(&self) -> &'static str {
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

/// Helper to parse nested list of arguments from JSON string or HCL expression.
pub fn parse_nested_string_list(
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
pub fn parse_serial_ports<S: std::hash::BuildHasher>(
    cfg: &std::collections::HashMap<String, String, S>,
) -> Vec<VBoxSerialPort> {
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

/// Resolves the `VBoxManage` binary name.
#[must_use]
pub fn vboxmanage_binary() -> &'static str {
    if cfg!(test) { "echo" } else { "VBoxManage" }
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
    query_vbox_version_with_cmd(vboxmanage_binary()).await
}

/// Queries the installed `VirtualBox` version using a specific `VBoxManage` executable.
pub async fn query_vbox_version_with_cmd(cmd: &str) -> Option<String> {
    let output = vboxmanage_with_cmd(cmd, &["--version"]).await.ok()?;
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
        c if u32::from(c) == 10 || u32::from(c) == 13 => &["1c", "9c"],
        c if u32::from(c) == 9 => &["0f", "8f"],
        ' ' => &["39", "b9"],
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
            0xFF0D => &["1c", "9c"],
            0xFF09 => &["0f", "8f"],
            0xFF1B => &["01", "81"],
            0xFF08 => &["0e", "8e"],
            0x0020 => &["39", "b9"],
            0xFF52 => &["48", "c8"],
            0xFF54 => &["50", "d0"],
            0xFF51 => &["4b", "cb"],
            0xFF53 => &["4d", "cd"],
            0xFFBE => &["3b", "bb"],
            0xFFBF => &["3c", "bc"],
            0xFFC0 => &["3d", "bd"],
            0xFFC1 => &["3e", "be"],
            0xFFC2 => &["3f", "bf"],
            0xFFC3 => &["40", "c0"],
            0xFFC4 => &["41", "c1"],
            0xFFC5 => &["42", "c2"],
            0xFFC6 => &["43", "c3"],
            0xFFC7 => &["44", "c4"],
            0xFFC8 => &["57", "d7"],
            0xFFC9 => &["58", "d8"],
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
        BootAction::KeyDown(0xFFE1) => &["2a"],
        BootAction::KeyUp(0xFFE1) => &["aa"],
        BootAction::KeyDown(0xFFE3) => &["1d"],
        BootAction::KeyUp(0xFFE3) => &["9d"],
        BootAction::KeyDown(0xFFE9) => &["38"],
        BootAction::KeyUp(0xFFE9) => &["b8"],
        _ => &[],
    }
}

/// Sends boot command scancodes to a `VirtualBox` VM using `VBoxManage controlvm <name> keyboardputscancode`.
///
/// # Errors
/// Returns `StampError::Execution` or `StampError::Io` if `VBoxManage` execution fails.
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
