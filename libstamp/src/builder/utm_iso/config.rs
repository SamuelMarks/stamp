//! Configuration types, backends, architectures, and plist generation for `utm-iso`.

use crate::error::StampError;
use crate::types::{MemoryMb, Port};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// Emulation / virtualization backend engine for UTM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum UtmBackend {
    /// QEMU backend hypervisor (supports `x86_64`, `aarch64`, emulation, and HVF).
    #[default]
    Qemu,
    /// Native Apple Virtualization.framework backend.
    Apple,
}

impl UtmBackend {
    /// String representation of the UTM backend.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Qemu => "qemu",
            Self::Apple => "apple",
        }
    }
}

impl fmt::Display for UtmBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for UtmBackend {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "apple" | "virtualization" => Ok(Self::Apple),
            _ => Ok(Self::Qemu),
        }
    }
}

/// Target CPU architecture for the UTM virtual machine.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum UtmArch {
    /// 64-bit ARM architecture (`aarch64` / `arm64`).
    #[default]
    Aarch64,
    /// 64-bit x86 architecture (`x86_64` / `amd64`).
    X86_64,
    /// Custom architecture.
    Custom(String),
}

impl UtmArch {
    /// String representation of the target architecture.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Aarch64 => "aarch64",
            Self::X86_64 => "x86_64",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for UtmArch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for UtmArch {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "aarch64" | "arm64" => Ok(Self::Aarch64),
            "x86_64" | "amd64" => Ok(Self::X86_64),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// Storage bus controller interface for drives in UTM.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum UtmInterface {
    /// Non-Volatile Memory Express (`NVMe`) interface.
    #[default]
    Nvme,
    /// `VirtIO` paravirtualized storage interface.
    VirtIo,
    /// Serial ATA (SATA) interface.
    Sata,
    /// Integrated Drive Electronics (IDE) interface.
    Ide,
    /// Universal Serial Bus (USB) optical/storage drive.
    Usb,
    /// Custom drive bus interface.
    Custom(String),
}

impl UtmInterface {
    /// String representation of the drive bus interface.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Nvme => "nvme",
            Self::VirtIo => "virtio",
            Self::Sata => "sata",
            Self::Ide => "ide",
            Self::Usb => "usb",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for UtmInterface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for UtmInterface {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "nvme" => Ok(Self::Nvme),
            "virtio" | "virtio-blk" => Ok(Self::VirtIo),
            "sata" => Ok(Self::Sata),
            "ide" => Ok(Self::Ide),
            "usb" => Ok(Self::Usb),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// Display adapter hardware type in UTM.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum UtmDisplayType {
    /// `VirtIO` GPU PCI adapter.
    #[default]
    VirtIoGpuPci,
    /// `VirtIO` RAM framebuffer adapter.
    VirtIoRamfb,
    /// Serial console text terminal only.
    Console,
    /// Headless display without virtual screen.
    None,
    /// Custom display adapter type.
    Custom(String),
}

impl UtmDisplayType {
    /// String representation of the display hardware type.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::VirtIoGpuPci => "virtio-gpu-pci",
            Self::VirtIoRamfb => "virtio-ramfb",
            Self::Console => "console",
            Self::None => "none",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for UtmDisplayType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for UtmDisplayType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "virtio-gpu-pci" | "virtio-gpu" => Ok(Self::VirtIoGpuPci),
            "virtio-ramfb" => Ok(Self::VirtIoRamfb),
            "console" | "serial" => Ok(Self::Console),
            "none" | "headless" => Ok(Self::None),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// Guest Additions mounting and installation mode for UTM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum UtmGuestAdditionsMode {
    /// Attach the Guest Additions / Spice ISO image to a virtual optical drive.
    Attach,
    /// Upload the Guest Additions ISO image to the guest via communicator.
    Upload,
    /// Do not attach or upload Guest Additions.
    #[default]
    Disable,
}

impl UtmGuestAdditionsMode {
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

impl fmt::Display for UtmGuestAdditionsMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for UtmGuestAdditionsMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "attach" => Ok(Self::Attach),
            "upload" => Ok(Self::Upload),
            _ => Ok(Self::Disable),
        }
    }
}

/// Configuration for the `utm-iso` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct UtmIsoConfig {
    /// The name of the builder instance.
    pub name: String,
    /// VM name. Defaults to `packer-utm-iso`.
    pub vm_name: Option<String>,
    /// Target CPU architecture (`aarch64`, `x86_64`).
    pub vm_arch: Option<UtmArch>,
    /// Virtualization backend (`qemu` or `apple`).
    pub vm_backend: UtmBackend,
    /// Icon filename or descriptor in UTM.
    pub vm_icon: Option<String>,
    /// Whether to enable hypervisor hardware acceleration (HVF / KVM).
    pub hypervisor: bool,
    /// Whether to boot in UEFI mode.
    pub uefi_boot: bool,
    /// Display hardware adapter type.
    pub display_hardware_type: Option<UtmDisplayType>,
    /// Hard drive storage bus interface (`nvme`, `virtio`, `sata`). Defaults to `nvme`.
    pub hard_drive_interface: UtmInterface,
    /// Installer ISO storage bus interface (`usb`, `sata`, `ide`). Defaults to `usb`.
    pub iso_interface: UtmInterface,
    /// Guest additions storage bus interface. Defaults to `usb`.
    pub guest_additions_interface: UtmInterface,
    /// Guest additions mounting mode (`attach`, `upload`, `disable`).
    pub guest_additions_mode: UtmGuestAdditionsMode,
    /// URL to download Guest Additions ISO.
    pub guest_additions_url: Option<String>,
    /// SHA256 checksum for Guest Additions ISO.
    pub guest_additions_sha256: Option<String>,
    /// Target path for downloading Guest Additions ISO.
    pub guest_additions_target_path: Option<String>,
    /// Explicit local path to Guest Additions ISO.
    pub guest_additions_path: Option<String>,
    /// Whether to disable VNC display server.
    pub disable_vnc: bool,
    /// If true, do not pause after booting.
    pub boot_nopause: bool,
    /// If true, do not pause when configuring display.
    pub display_nopause: bool,
    /// If true, do not pause before export.
    pub export_nopause: bool,
    /// Number of virtual CPUs. Defaults to 2.
    pub cpus: Option<u32>,
    /// Memory size in MB. Defaults to 2048.
    pub memory: Option<MemoryMb>,
    /// Virtual disk size in MB. Defaults to 20480.
    pub disk_size: Option<MemoryMb>,
    /// Source installer ISO URL or path.
    pub iso_url: Option<String>,
    /// Checksum of the installer ISO.
    pub iso_checksum: Option<String>,
    /// Target path for downloading the ISO.
    pub iso_target_path: Option<String>,
    /// Boot command sequence.
    pub boot_command: Option<Vec<String>>,
    /// Wait duration before typing boot commands.
    pub boot_wait: Option<String>,
    /// Files to place on a virtual floppy disk.
    pub floppy_files: Vec<String>,
    /// Files to place on a secondary CD-ROM.
    pub cd_files: Vec<String>,
    /// Volume label for the secondary CD-ROM.
    pub cd_label: Option<String>,
    /// Communicator type (`ssh`, `winrm`, `none`).
    pub communicator: Option<String>,
    /// SSH username.
    pub ssh_username: Option<String>,
    /// SSH password.
    pub ssh_password: Option<String>,
    /// SSH host port.
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
    /// Output directory for exported `.utm` bundle.
    pub output_directory: Option<String>,
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

impl UtmIsoConfig {
    /// Constructs a `UtmIsoConfig` from a strongly-typed [`BuilderConfig`](crate::template::BuilderConfig).
    ///
    /// # Errors
    ///
    /// Returns `StampError::Builder` if parsing fails or invalid types are supplied.
    pub fn from_builder_config(
        builder_config: &crate::template::BuilderConfig,
    ) -> Result<Self, StampError> {
        let name = builder_config.name.clone();
        let cfg = &builder_config.config;

        let vm_name = cfg
            .get("vm_name")
            .or_else(|| cfg.get("utm_vm_name"))
            .cloned();

        let vm_arch = cfg
            .get("vm_arch")
            .or_else(|| cfg.get("utm_vm_arch"))
            .or_else(|| cfg.get("os_arch"))
            .and_then(|s| s.parse().ok());

        let vm_backend = cfg
            .get("vm_backend")
            .or_else(|| cfg.get("utm_vm_backend"))
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        let vm_icon = cfg
            .get("vm_icon")
            .or_else(|| cfg.get("utm_vm_icon"))
            .cloned();

        let hypervisor = cfg
            .get("hypervisor")
            .or_else(|| cfg.get("utm_hypervisor"))
            .is_none_or(|v| v == "true" || v == "1");

        let uefi_boot = cfg
            .get("uefi_boot")
            .or_else(|| cfg.get("utm_uefi_boot"))
            .is_none_or(|v| v == "true" || v == "1");

        let display_hardware_type = cfg
            .get("display_hardware_type")
            .or_else(|| cfg.get("utm_display_hardware_type"))
            .and_then(|s| s.parse().ok());

        let hard_drive_interface = cfg
            .get("hard_drive_interface")
            .or_else(|| cfg.get("utm_hard_drive_interface"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(UtmInterface::Nvme);

        let iso_interface = cfg
            .get("iso_interface")
            .or_else(|| cfg.get("utm_iso_interface"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(UtmInterface::Usb);

        let guest_additions_interface = cfg
            .get("guest_additions_interface")
            .or_else(|| cfg.get("utm_guest_additions_interface"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(UtmInterface::Usb);

        let guest_additions_mode = cfg
            .get("guest_additions_mode")
            .or_else(|| cfg.get("utm_guest_additions_mode"))
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        let guest_additions_url = cfg
            .get("guest_additions_url")
            .or_else(|| cfg.get("utm_guest_additions_url"))
            .cloned();

        let guest_additions_sha256 = cfg
            .get("guest_additions_sha256")
            .or_else(|| cfg.get("utm_guest_additions_sha256"))
            .cloned();

        let guest_additions_target_path = cfg
            .get("guest_additions_target_path")
            .or_else(|| cfg.get("utm_guest_additions_target_path"))
            .cloned();

        let guest_additions_path = cfg
            .get("guest_additions_path")
            .or_else(|| cfg.get("utm_guest_additions_path"))
            .cloned();

        let disable_vnc = cfg
            .get("disable_vnc")
            .or_else(|| cfg.get("utm_disable_vnc"))
            .is_some_and(|v| v == "true" || v == "1");

        let boot_nopause = cfg
            .get("boot_nopause")
            .or_else(|| cfg.get("utm_boot_nopause"))
            .is_none_or(|v| v == "true" || v == "1");

        let display_nopause = cfg
            .get("display_nopause")
            .or_else(|| cfg.get("utm_display_nopause"))
            .is_none_or(|v| v == "true" || v == "1");

        let export_nopause = cfg
            .get("export_nopause")
            .or_else(|| cfg.get("utm_export_nopause"))
            .is_none_or(|v| v == "true" || v == "1");

        let cpus = cfg.get("cpus").and_then(|s| s.parse::<u32>().ok());
        let memory = cfg
            .get("memory")
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);
        let disk_size = cfg
            .get("disk_size")
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);

        let iso_url = cfg.get("iso_url").cloned();
        let iso_checksum = cfg.get("iso_checksum").cloned();
        let iso_target_path = cfg.get("iso_target_path").cloned();

        let boot_wait = cfg.get("boot_wait").cloned();
        let boot_command = cfg.get("boot_command").map(|s| parse_string_list(s));
        let floppy_files = cfg
            .get("floppy_files")
            .map_or_else(Vec::new, |s| parse_string_list(s));
        let cd_files = cfg
            .get("cd_files")
            .map_or_else(Vec::new, |s| parse_string_list(s));
        let cd_label = cfg.get("cd_label").cloned();

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

        let output_directory = cfg.get("output_directory").cloned();

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
            vm_name,
            vm_arch,
            vm_backend,
            vm_icon,
            hypervisor,
            uefi_boot,
            display_hardware_type,
            hard_drive_interface,
            iso_interface,
            guest_additions_interface,
            guest_additions_mode,
            guest_additions_url,
            guest_additions_sha256,
            guest_additions_target_path,
            guest_additions_path,
            disable_vnc,
            boot_nopause,
            display_nopause,
            export_nopause,
            cpus,
            memory,
            disk_size,
            iso_url,
            iso_checksum,
            iso_target_path,
            boot_command,
            boot_wait,
            floppy_files,
            cd_files,
            cd_label,
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
            output_directory,
            http_directory,
            http_content,
            http_bind_address,
            http_port_min,
            http_port_max,
        })
    }
}

/// Discovers candidate paths for the `utmctl` binary.
fn utmctl_candidates() -> [PathBuf; 3] {
    if cfg!(test) && std::env::var("UTMCTL_TEST_EMPTY_CANDIDATES").is_ok() {
        [
            PathBuf::from("/nonexistent/utmctl1"),
            PathBuf::from("/nonexistent/utmctl2"),
            PathBuf::from("/nonexistent/utmctl3"),
        ]
    } else {
        [
            PathBuf::from("/Applications/UTM.app/Contents/MacOS/utmctl"),
            PathBuf::from("/usr/local/bin/utmctl"),
            PathBuf::from("/usr/bin/utmctl"),
        ]
    }
}

/// Discovers the path to the `utmctl` CLI executable on the host system.
#[must_use]
pub fn find_utmctl_binary() -> PathBuf {
    let env_bin = std::env::var("UTMCTL_PATH")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.exists());

    env_bin.unwrap_or_else(|| {
        utmctl_candidates()
            .into_iter()
            .find(|c| c.exists())
            .unwrap_or_else(|| PathBuf::from("utmctl"))
    })
}

/// Execute a `utmctl` command with arguments.
///
/// # Errors
///
/// Returns `StampError::Builder` if `utmctl` fails.
pub async fn run_utmctl(args: &[&str]) -> Result<String, StampError> {
    run_utmctl_with_cmd(utmctl_binary(), args).await
}

/// Resolves the `utmctl` binary name.
#[must_use]
pub fn utmctl_binary() -> &'static str {
    if cfg!(test) { "echo" } else { "utmctl" }
}

/// Execute a specific command as `utmctl` driver.
///
/// # Errors
///
/// Returns `StampError::Builder` if the `utmctl` command fails or cannot be spawned.
pub async fn run_utmctl_with_cmd(cmd: &str, args: &[&str]) -> Result<String, StampError> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| StampError::Builder(format!("Failed to execute utmctl: {e}")))?;

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
            "utmctl command '{cmd} {args:?}' failed with exit code {code}: {msg}"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Discovers the guest IP address of a running `UTM` VM via `utmctl ip-address`.
///
/// # Errors
///
/// Returns `StampError::Builder` if discovery fails.
pub async fn discover_utm_guest_ip(vm_name: &str) -> Result<String, StampError> {
    discover_utm_guest_ip_with_cmd(utmctl_binary(), vm_name).await
}

/// Discovers the guest IP address using a specific command runner.
///
/// # Errors
///
/// Returns `StampError::Builder` if discovery fails.
pub async fn discover_utm_guest_ip_with_cmd(
    cmd: &str,
    vm_name: &str,
) -> Result<String, StampError> {
    let output = run_utmctl_with_cmd(cmd, &["ip-address", vm_name]).await?;
    let ip = output.trim();
    if !ip.is_empty() && ip != "unknown" && !ip.starts_with("ip-address") {
        Ok(ip.to_string())
    } else {
        Ok("127.0.0.1".to_string())
    }
}

/// Generate XML property list `config.plist` representing the UTM bundle specification.
#[must_use]
pub fn generate_utm_plist(config: &UtmIsoConfig, vm_name: &str) -> String {
    let arch_str = config.vm_arch.as_ref().map_or("aarch64", UtmArch::as_str);
    let backend_str = config.vm_backend.as_str();
    let mem_mb = config.memory.map_or(2048, |m| m.get());
    let cpus = config.cpus.unwrap_or(2);
    let uefi = if config.uefi_boot { "true" } else { "false" };
    let hdd_if = config.hard_drive_interface.as_str();
    let iso_if = config.iso_interface.as_str();
    let display_str = config
        .display_hardware_type
        .as_ref()
        .map_or("virtio-gpu-pci", UtmDisplayType::as_str);

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Backend</key>
	<string>{backend_str}</string>
	<key>ConfigurationVersion</key>
	<integer>4</integer>
	<key>Information</key>
	<dict>
		<key>Icon</key>
		<string>linux</string>
		<key>Name</key>
		<string>{vm_name}</string>
	</dict>
	<key>System</key>
	<dict>
		<key>Architecture</key>
		<string>{arch_str}</string>
		<key>CPUCount</key>
		<integer>{cpus}</integer>
		<key>Memory</key>
		<integer>{mem_mb}</integer>
		<key>UefiBoot</key>
		<{uefi}/>
	</dict>
	<key>Display</key>
	<dict>
		<key>Hardware</key>
		<string>{display_str}</string>
	</dict>
	<key>Drives</key>
	<array>
		<dict>
			<key>DriveInterface</key>
			<string>{hdd_if}</string>
			<key>ImageName</key>
			<string>disk-0.qcow2</string>
			<key>Removable</key>
			<false/>
		</dict>
		<dict>
			<key>DriveInterface</key>
			<string>{iso_if}</string>
			<key>ImageName</key>
			<string>cdrom-0.iso</string>
			<key>Removable</key>
			<true/>
		</dict>
	</array>
</dict>
</plist>
"#
    )
}
