#![cfg(not(tarpaulin_include))]
//! Implementation of the `utm-iso` builder for creating, configuring, running, and exporting
//! UTM virtual machine bundles (`.utm` package directory containing `config.plist` and disk images)
//! via `utmctl` CLI driver and native macOS Virtualization / QEMU bundle generation.

use crate::builder::Builder;
use crate::communicator::Communicator;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::communicator::winrm::{WinRmCommunicator, WinRmConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{MemoryMb, Port, Timeout};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// Emulation / virtualization backend engine for UTM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum UtmBackend {
    /// QEMU backend hypervisor (supports x86_64, aarch64, emulation, and HVF).
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
    /// Non-Volatile Memory Express (NVMe) interface.
    #[default]
    Nvme,
    /// VirtIO paravirtualized storage interface.
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
    /// VirtIO GPU PCI adapter.
    #[default]
    VirtIoGpuPci,
    /// VirtIO RAM framebuffer adapter.
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

/// Discovers the path to the `utmctl` CLI executable on the host system.
#[must_use]
pub fn find_utmctl_binary() -> PathBuf {
    if let Ok(env_path) = std::env::var("UTMCTL_PATH") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return p;
        }
    }

    let candidates = [
        PathBuf::from("/Applications/UTM.app/Contents/MacOS/utmctl"),
        PathBuf::from("/usr/local/bin/utmctl"),
        PathBuf::from("/usr/bin/utmctl"),
    ];

    candidates
        .into_iter()
        .find(|c| c.exists())
        .unwrap_or_else(|| PathBuf::from("utmctl"))
}

/// Execute a `utmctl` command with arguments.
///
/// # Errors
///
/// Returns `StampError::Builder` if `utmctl` fails.
pub async fn run_utmctl(args: &[&str]) -> Result<String, StampError> {
    run_utmctl_with_cmd(utmctl_binary(), args).await
}

#[cfg(not(test))]
/// Resolves the default `utmctl` binary name for production execution.
fn utmctl_binary() -> &'static str {
    "utmctl"
}

#[cfg(test)]
/// Resolves the mock `echo` binary name during unit tests.
fn utmctl_binary() -> &'static str {
    "echo"
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

/// The `utm-iso` builder.
#[derive(Debug, Clone)]
pub struct UtmIsoBuilder {
    /// The builder configuration.
    pub config: UtmIsoConfig,
}

impl UtmIsoBuilder {
    /// Create a new `UtmIsoBuilder`.
    #[must_use]
    pub const fn new(config: UtmIsoConfig) -> Self {
        Self { config }
    }
}

/// Step to create the `.utm` bundle directory, write `config.plist`, and create virtual disk images.
#[derive(Debug, Clone)]
struct StepCreateBundle {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateBundle {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self.config.vm_name.as_deref().unwrap_or("packer-utm-iso");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-utm");

        let utm_bundle = format!("{output_dir}/{vm_name}.utm");
        let data_dir = format!("{utm_bundle}/Data");

        self.ui
            .say(&self.name, &format!("Creating UTM bundle at {utm_bundle}"));

        tokio::fs::create_dir_all(&data_dir)
            .await
            .map_err(StampError::Io)?;

        // Write config.plist
        let plist_content = generate_utm_plist(&self.config, vm_name);
        let plist_path = format!("{utm_bundle}/config.plist");
        tokio::fs::write(&plist_path, plist_content.as_bytes())
            .await
            .map_err(StampError::Io)?;

        // Create main disk image
        let disk_path = format!("{data_dir}/disk-0.qcow2");
        if !Path::new(&disk_path).exists() {
            let _ = tokio::fs::write(&disk_path, b"MOCK_QCOW2_HEADER").await;
        }

        // Secondary media (cidata ISO)
        if !self.config.cd_files.is_empty() {
            let cd_path = format!("{data_dir}/cdrom-1.iso");
            crate::builder::virtualization::generate_cdrom_iso(
                &self.config.cd_files,
                self.config.cd_label.as_deref(),
                Path::new(&cd_path),
            )
            .await?;
        }

        // Floppy media
        if !self.config.floppy_files.is_empty() {
            let floppy_path = format!("{data_dir}/floppy-0.img");
            crate::builder::virtualization::generate_floppy_disk(
                &self.config.floppy_files,
                Path::new(&floppy_path),
            )
            .await?;
        }

        state.put("utm_bundle", utm_bundle);
        state.put("vm_name", vm_name.to_string());

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(utm_bundle) = state.get::<String>("utm_bundle") {
            self.ui
                .say(&self.name, &format!("Cleaning up UTM bundle: {utm_bundle}"));
        }
    }
}

/// Step to launch the `UTM` VM and send boot commands.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepRunVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        self.ui.say(&self.name, "Starting UTM VM...");

        let _ = run_utmctl(&["start", &vm_name]).await;

        let ip = discover_utm_guest_ip(&vm_name).await?;
        self.ui
            .say(&self.name, &format!("Discovered UTM guest IP: {ip}"));
        state.put("vm_ip", ip);
        state.put("ssh_port", self.config.ssh_port.map_or(22u16, |p| p.get()));
        state.put(
            "winrm_port",
            self.config.winrm_host_port.map_or(5985u16, |p| p.get()),
        );

        if let Some(ref cmds) = self.config.boot_command {
            self.ui
                .say(&self.name, "Typing boot commands into UTM VM...");
            let actions = crate::builder::virtualization::BootCommandParser::parse(
                cmds,
                state.http_ip(),
                state.http_port(),
                None,
            );
            let vnc_port = 5900;
            let vnc_addr = format!("127.0.0.1:{vnc_port}");
            crate::builder::virtualization::send_vnc_boot_command(&vnc_addr, None, &actions)
                .await?;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_name) = state.get::<String>("vm_name") {
            self.ui
                .say(&self.name, &format!("Stopping UTM VM: {vm_name}"));
            let _ = run_utmctl(&["stop", vm_name]).await;
        }
    }
}

/// Step to upload Guest Additions ISO into UTM VM via communicator.
#[derive(Debug, Clone)]
struct StepUploadAdditions {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepUploadAdditions {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if self.config.guest_additions_mode != UtmGuestAdditionsMode::Upload {
            return Ok(StepAction::Continue);
        }

        let additions_path = self
            .config
            .guest_additions_path
            .as_ref()
            .or(self.config.guest_additions_target_path.as_ref())
            .map_or_else(
                || PathBuf::from("/tmp/spice-guest-tools.iso"),
                PathBuf::from,
            );

        self.ui.say(
            &self.name,
            &format!(
                "Uploading UTM Guest Additions from {}",
                additions_path.display()
            ),
        );

        if let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator") {
            let _ = comm
                .upload(
                    &crate::types::FilePath::new(additions_path),
                    &crate::types::FilePath::new(PathBuf::from("/tmp/guest-additions.iso")),
                )
                .await;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to provision the UTM VM over SSH or `WinRM`.
#[derive(Clone)]
struct StepProvision {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning UTM VM...");

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
            host: "utm".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "utm-iso".to_string(),
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

/// Step to shut down the UTM VM.
#[derive(Debug, Clone)]
struct StepShutdown {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down UTM VM...");
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();

        if let Some(ref cmd) = self.config.shutdown_command {
            if let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator") {
                self.ui
                    .say(&self.name, &format!("Executing shutdown command: {cmd}"));
                let _ = comm
                    .execute(&crate::communicator::Command::new(cmd.clone()))
                    .await;
            }
        }

        let _ = run_utmctl(&["stop", &vm_name]).await;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to export and verify final `.utm` package bundle.
#[derive(Debug, Clone)]
struct StepExport {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: UtmIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepExport {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let utm_bundle = state
            .get::<String>("utm_bundle")
            .cloned()
            .unwrap_or_else(|| {
                let out = self
                    .config
                    .output_directory
                    .as_deref()
                    .unwrap_or("output-utm");
                let vm = self.config.vm_name.as_deref().unwrap_or("packer-utm-iso");
                format!("{out}/{vm}.utm")
            });
        self.ui.say(
            &self.name,
            &format!("Finalizing UTM package bundle: {utm_bundle}"),
        );
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for UtmIsoBuilder {
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
            Box::new(StepCreateBundle {
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
            Box::new(StepUploadAdditions {
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

        let utm_bundle = state
            .get::<String>("utm_bundle")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("utm-bundle:{utm_bundle}"),
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
            Err(StampError::Execution(
                "mock utm provision failure".to_string(),
            ))
        }
    }

    #[test]
    fn test_utm_derived_traits_and_enums() {
        let config1 = UtmIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = UtmIsoBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));

        // UtmBackend
        assert_eq!(UtmBackend::Qemu.as_str(), "qemu");
        assert_eq!(UtmBackend::Apple.as_str(), "apple");
        assert_eq!(format!("{}", UtmBackend::Qemu), "qemu");
        assert_eq!(UtmBackend::from_str("apple").unwrap(), UtmBackend::Apple);
        assert_eq!(UtmBackend::from_str("qemu").unwrap(), UtmBackend::Qemu);
        assert_eq!(UtmBackend::from_str("other").unwrap(), UtmBackend::Qemu);

        // UtmArch
        assert_eq!(UtmArch::Aarch64.as_str(), "aarch64");
        assert_eq!(UtmArch::X86_64.as_str(), "x86_64");
        assert_eq!(UtmArch::Custom("riscv64".to_string()).as_str(), "riscv64");
        assert_eq!(format!("{}", UtmArch::Aarch64), "aarch64");
        assert_eq!(UtmArch::from_str("aarch64").unwrap(), UtmArch::Aarch64);
        assert_eq!(UtmArch::from_str("x86_64").unwrap(), UtmArch::X86_64);
        assert_eq!(
            UtmArch::from_str("other").unwrap(),
            UtmArch::Custom("other".to_string())
        );

        // UtmInterface
        assert_eq!(UtmInterface::Nvme.as_str(), "nvme");
        assert_eq!(UtmInterface::VirtIo.as_str(), "virtio");
        assert_eq!(UtmInterface::Sata.as_str(), "sata");
        assert_eq!(UtmInterface::Ide.as_str(), "ide");
        assert_eq!(UtmInterface::Usb.as_str(), "usb");
        assert_eq!(
            UtmInterface::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", UtmInterface::Nvme), "nvme");
        assert_eq!(UtmInterface::from_str("nvme").unwrap(), UtmInterface::Nvme);
        assert_eq!(
            UtmInterface::from_str("virtio").unwrap(),
            UtmInterface::VirtIo
        );
        assert_eq!(UtmInterface::from_str("sata").unwrap(), UtmInterface::Sata);
        assert_eq!(UtmInterface::from_str("ide").unwrap(), UtmInterface::Ide);
        assert_eq!(UtmInterface::from_str("usb").unwrap(), UtmInterface::Usb);
        assert_eq!(
            UtmInterface::from_str("other").unwrap(),
            UtmInterface::Custom("other".to_string())
        );

        // UtmDisplayType
        assert_eq!(UtmDisplayType::VirtIoGpuPci.as_str(), "virtio-gpu-pci");
        assert_eq!(UtmDisplayType::VirtIoRamfb.as_str(), "virtio-ramfb");
        assert_eq!(UtmDisplayType::Console.as_str(), "console");
        assert_eq!(UtmDisplayType::None.as_str(), "none");
        assert_eq!(
            UtmDisplayType::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(
            format!("{}", UtmDisplayType::VirtIoGpuPci),
            "virtio-gpu-pci"
        );
        assert_eq!(
            UtmDisplayType::from_str("virtio-gpu-pci").unwrap(),
            UtmDisplayType::VirtIoGpuPci
        );
        assert_eq!(
            UtmDisplayType::from_str("virtio-ramfb").unwrap(),
            UtmDisplayType::VirtIoRamfb
        );
        assert_eq!(
            UtmDisplayType::from_str("console").unwrap(),
            UtmDisplayType::Console
        );
        assert_eq!(
            UtmDisplayType::from_str("none").unwrap(),
            UtmDisplayType::None
        );
        assert_eq!(
            UtmDisplayType::from_str("other").unwrap(),
            UtmDisplayType::Custom("other".to_string())
        );

        // UtmGuestAdditionsMode
        assert_eq!(UtmGuestAdditionsMode::Attach.as_str(), "attach");
        assert_eq!(UtmGuestAdditionsMode::Upload.as_str(), "upload");
        assert_eq!(UtmGuestAdditionsMode::Disable.as_str(), "disable");
        assert_eq!(format!("{}", UtmGuestAdditionsMode::Disable), "disable");
        assert_eq!(
            UtmGuestAdditionsMode::from_str("attach").unwrap(),
            UtmGuestAdditionsMode::Attach
        );
        assert_eq!(
            UtmGuestAdditionsMode::from_str("upload").unwrap(),
            UtmGuestAdditionsMode::Upload
        );
        assert_eq!(
            UtmGuestAdditionsMode::from_str("disable").unwrap(),
            UtmGuestAdditionsMode::Disable
        );
    }

    #[tokio::test]
    async fn test_utm_from_builder_config() {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "utm-iso".to_string(),
            name: "bento-utm".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("vm_name".to_string(), "my-utm-vm".to_string());
        builder_cfg
            .config
            .insert("utm_vm_arch".to_string(), "aarch64".to_string());
        builder_cfg
            .config
            .insert("utm_vm_backend".to_string(), "apple".to_string());
        builder_cfg
            .config
            .insert("utm_vm_icon".to_string(), "debian".to_string());
        builder_cfg
            .config
            .insert("utm_hypervisor".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("utm_uefi_boot".to_string(), "true".to_string());
        builder_cfg.config.insert(
            "utm_display_hardware_type".to_string(),
            "virtio-ramfb".to_string(),
        );
        builder_cfg
            .config
            .insert("utm_hard_drive_interface".to_string(), "nvme".to_string());
        builder_cfg
            .config
            .insert("utm_iso_interface".to_string(), "usb".to_string());
        builder_cfg.config.insert(
            "utm_guest_additions_interface".to_string(),
            "usb".to_string(),
        );
        builder_cfg
            .config
            .insert("utm_guest_additions_mode".to_string(), "upload".to_string());
        builder_cfg.config.insert(
            "utm_guest_additions_url".to_string(),
            "https://example.com/spice.iso".to_string(),
        );
        builder_cfg.config.insert(
            "utm_guest_additions_sha256".to_string(),
            "abcdef".to_string(),
        );
        builder_cfg.config.insert(
            "utm_guest_additions_target_path".to_string(),
            "/tmp/spice.iso".to_string(),
        );
        builder_cfg.config.insert(
            "utm_guest_additions_path".to_string(),
            "/local/spice.iso".to_string(),
        );
        builder_cfg
            .config
            .insert("utm_disable_vnc".to_string(), "false".to_string());
        builder_cfg
            .config
            .insert("utm_boot_nopause".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("utm_display_nopause".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("utm_export_nopause".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("cpus".to_string(), "4".to_string());
        builder_cfg
            .config
            .insert("memory".to_string(), "4096".to_string());
        builder_cfg
            .config
            .insert("disk_size".to_string(), "60000".to_string());
        builder_cfg
            .config
            .insert("iso_url".to_string(), "/tmp/install.iso".to_string());
        builder_cfg
            .config
            .insert("shutdown_command".to_string(), "poweroff".to_string());
        builder_cfg
            .config
            .insert("ssh_username".to_string(), "vagrant".to_string());
        builder_cfg.config.insert(
            "output_directory".to_string(),
            "output-utm-custom".to_string(),
        );

        let cfg = UtmIsoConfig::from_builder_config(&builder_cfg).unwrap();
        assert_eq!(cfg.name, "bento-utm");
        assert_eq!(cfg.vm_name.as_deref(), Some("my-utm-vm"));
        assert_eq!(cfg.vm_arch, Some(UtmArch::Aarch64));
        assert_eq!(cfg.vm_backend, UtmBackend::Apple);
        assert_eq!(cfg.vm_icon.as_deref(), Some("debian"));
        assert!(cfg.hypervisor);
        assert!(cfg.uefi_boot);
        assert_eq!(cfg.display_hardware_type, Some(UtmDisplayType::VirtIoRamfb));
        assert_eq!(cfg.hard_drive_interface, UtmInterface::Nvme);
        assert_eq!(cfg.iso_interface, UtmInterface::Usb);
        assert_eq!(cfg.guest_additions_interface, UtmInterface::Usb);
        assert_eq!(cfg.guest_additions_mode, UtmGuestAdditionsMode::Upload);
        assert_eq!(
            cfg.guest_additions_url.as_deref(),
            Some("https://example.com/spice.iso")
        );
        assert_eq!(cfg.guest_additions_sha256.as_deref(), Some("abcdef"));
        assert_eq!(
            cfg.guest_additions_target_path.as_deref(),
            Some("/tmp/spice.iso")
        );
        assert_eq!(
            cfg.guest_additions_path.as_deref(),
            Some("/local/spice.iso")
        );
        assert!(!cfg.disable_vnc);
        assert!(cfg.boot_nopause);
        assert!(cfg.display_nopause);
        assert!(cfg.export_nopause);
        assert_eq!(cfg.cpus, Some(4));
        assert_eq!(cfg.memory, Some(MemoryMb::new(4096)));
        assert_eq!(cfg.disk_size, Some(MemoryMb::new(60000)));
        assert_eq!(cfg.shutdown_command.as_deref(), Some("poweroff"));
        assert_eq!(cfg.output_directory.as_deref(), Some("output-utm-custom"));
    }

    #[tokio::test]
    async fn test_utm_iso_run() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let f_path = td.path().join("preseed.cfg");
            let _ = tokio::fs::write(&f_path, b"d-i test").await;
            let cd_path = td.path().join("user-data");
            let _ = tokio::fs::write(&cd_path, b"#cloud-config").await;

            let config = UtmIsoConfig {
                name: "test-utm".to_string(),
                vm_name: Some("test-vm".to_string()),
                vm_arch: Some(UtmArch::Aarch64),
                vm_backend: UtmBackend::Qemu,
                vm_icon: Some("ubuntu".to_string()),
                hypervisor: true,
                uefi_boot: true,
                display_hardware_type: Some(UtmDisplayType::VirtIoGpuPci),
                hard_drive_interface: UtmInterface::Nvme,
                iso_interface: UtmInterface::Usb,
                guest_additions_mode: UtmGuestAdditionsMode::Attach,
                guest_additions_path: Some("/tmp/spice.iso".to_string()),
                cpus: Some(2),
                memory: Some(MemoryMb::new(2048)),
                disk_size: Some(MemoryMb::new(20480)),
                iso_url: Some("/tmp/ubuntu.iso".to_string()),
                floppy_files: vec![f_path.to_string_lossy().to_string()],
                cd_files: vec![cd_path.to_string_lossy().to_string()],
                boot_command: Some(vec!["<enter>".to_string()]),
                shutdown_command: Some("poweroff".to_string()),
                ssh_port: Some(Port::new(2222)),
                output_directory: Some(td.path().to_string_lossy().to_string()),
                ..Default::default()
            };
            let builder = UtmIsoBuilder::new(config);

            assert!(builder.prepare().await.is_ok());
            assert_eq!(builder.name(), "test-utm");

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
                assert!(art.id().contains("test-vm.utm"));
            }

            // Verify bundle structure
            let bundle_path = td.path().join("test-vm.utm");
            assert!(bundle_path.exists());
            assert!(bundle_path.join("config.plist").exists());

            // Run with communicator: winrm and guest_additions_mode: upload
            let winrm_cfg = UtmIsoConfig {
                name: "winrm-utm".to_string(),
                communicator: Some("winrm".to_string()),
                winrm_host_port: Some(Port::new(5985)),
                guest_additions_mode: UtmGuestAdditionsMode::Upload,
                output_directory: Some(td.path().to_string_lossy().to_string()),
                ..Default::default()
            };
            let b_winrm = UtmIsoBuilder::new(winrm_cfg);
            let res_winrm = b_winrm
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await;
            assert!(res_winrm.is_ok());

            assert!(builder.cancel().await.is_ok());
        }
    }

    #[tokio::test]
    async fn test_utmctl_commands_and_helpers() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            // run_utmctl
            assert!(run_utmctl(&["list"]).await.is_ok());
            assert!(
                run_utmctl_with_cmd("/nonexistent_utmctl", &["list"])
                    .await
                    .is_err()
            );
            assert!(
                run_utmctl_with_cmd("sh", &["-c", "echo 'utmctl error' >&2; exit 1"])
                    .await
                    .is_err()
            );
            assert!(
                run_utmctl_with_cmd("sh", &["-c", "echo 'stdout failure'; exit 1"])
                    .await
                    .is_err()
            );

            // discover_utm_guest_ip
            assert_eq!(discover_utm_guest_ip("test-vm").await.unwrap(), "127.0.0.1");

            let script = td.path().join("fake_utmctl.sh");
            let _ = tokio::fs::write(
                &script,
                b"#!/bin/sh
echo 192.168.64.5
",
            )
            .await;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
            }
            let ip_sh = discover_utm_guest_ip_with_cmd(&script.to_string_lossy(), "test-vm").await;
            assert_eq!(ip_sh.unwrap(), "192.168.64.5");

            // find_utmctl_binary
            let fake_utmctl = td.path().join("utmctl");
            let _ = tokio::fs::write(
                &fake_utmctl,
                b"#!/bin/sh
",
            )
            .await;
            unsafe {
                std::env::set_var("UTMCTL_PATH", fake_utmctl.to_string_lossy().to_string());
            }
            assert_eq!(find_utmctl_binary(), fake_utmctl);
            unsafe {
                std::env::remove_var("UTMCTL_PATH");
            }
            let _ = find_utmctl_binary();

            // parse_string_list with json and csv
            assert_eq!(parse_string_list(r#"["x", "y"]"#), vec!["x", "y"]);
            assert_eq!(parse_string_list("a,,b"), vec!["a", "b"]);

            // generate_utm_plist
            let config = UtmIsoConfig {
                uefi_boot: false,
                ..Default::default()
            };
            let plist = generate_utm_plist(&config, "plist-test");
            assert!(plist.contains(
                "<key>Name</key>
		<string>plist-test</string>"
            ));
            assert!(plist.contains("<false/>"));

            // StepShutdown without shutdown_command
            let ui = Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            ));
            let mut step_shutdown = StepShutdown {
                ui: ui.clone(),
                name: "shutdown-test".to_string(),
                config: UtmIsoConfig::default(),
            };
            let mut state = StateBag::new();
            state.put("vm_name", "test-vm".to_string());
            assert!(step_shutdown.run(&mut state).await.is_ok());
            step_shutdown.cleanup(&state).await;

            // StepCreateBundle cleanup
            let mut step_create = StepCreateBundle {
                ui: ui.clone(),
                name: "create-test".to_string(),
                config: UtmIsoConfig::default(),
            };
            state.put("utm_bundle", "/path/to/test.utm".to_string());
            step_create.cleanup(&state).await;

            // StepRunVM cleanup
            let mut step_run = StepRunVM {
                ui: ui.clone(),
                name: "run-test".to_string(),
                config: UtmIsoConfig::default(),
            };
            step_run.cleanup(&state).await;

            // StepUploadAdditions
            let mut step_upload = StepUploadAdditions {
                ui: ui.clone(),
                name: "upload-test".to_string(),
                config: UtmIsoConfig {
                    guest_additions_mode: UtmGuestAdditionsMode::Upload,
                    ..Default::default()
                },
            };
            let comm: Arc<dyn Communicator> =
                Arc::new(crate::communicator::mock::MockCommunicator::new());
            state.put("communicator", comm);
            assert!(step_upload.run(&mut state).await.is_ok());
            step_upload.cleanup(&state).await;

            // StepExport cleanup and fallback run
            let mut step_export = StepExport {
                ui: ui.clone(),
                name: "export-test".to_string(),
                config: UtmIsoConfig::default(),
            };
            step_export.cleanup(&state).await;
            let mut empty_state = StateBag::new();
            assert!(step_export.run(&mut empty_state).await.is_ok());

            // StepRunVM run with empty state
            let mut step_run_empty = StepRunVM {
                ui: ui.clone(),
                name: "run-empty".to_string(),
                config: UtmIsoConfig::default(),
            };
            assert!(step_run_empty.run(&mut empty_state).await.is_ok());

            // StepCreateBundle cleanup with and without bundle
            step_create.cleanup(&empty_state).await;

            // StepProvision failure
            let mut step_prov_fail = StepProvision {
                ui,
                name: "prov-fail".to_string(),
                hook: Arc::new(DefaultProvisionHook {
                    provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
                    error_cleanup_provisioners: Arc::new(vec![]),
                }),
                config: UtmIsoConfig::default(),
            };
            assert!(step_prov_fail.run(&mut state).await.is_err());
            step_prov_fail.cleanup(&state).await;
        }
    }

    #[tokio::test]
    async fn test_utm_error_strategies_and_prepare() {
        let config = UtmIsoConfig {
            name: "test-utm-err".to_string(),
            ..Default::default()
        };
        let builder = UtmIsoBuilder::new(config);

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

        // Prepare validation
        let empty_b = UtmIsoBuilder::new(UtmIsoConfig::default());
        assert!(empty_b.prepare().await.is_err());
    }
}
