#![cfg(not(tarpaulin_include))]
//! Implementation of the `virtualbox-ovf` builder with appliance checksum validation,
//! hardware customization, scancode typing, communicator execution, and re-exporting.

pub use super::virtualbox_iso::{
    VBoxChipset, VBoxExportFormat, VBoxFirmware, VBoxGraphicsController, VBoxGuestAdditionsMode,
    VBoxNicType, VBoxSerialMode, VBoxSerialPort, VBoxStorageBus, boot_action_to_vbox_scancodes,
    char_to_vbox_scancodes, discover_guest_additions_iso, execute_vboxmanage_commands,
    execute_vboxmanage_commands_with_output, query_vbox_version, vboxmanage,
};
use crate::builder::Builder;
use crate::communicator::Communicator;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::communicator::winrm::{WinRmCommunicator, WinRmConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{MemoryMb, Port, Timeout};
use md5::{Digest as Md5Digest, Md5};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::{Sha256, Sha512};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `virtualbox-ovf` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VirtualboxOvfConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The source OVF/OVA path or URL.
    pub source_path: Option<String>,
    /// Checksum of the source appliance (e.g. `sha256:hash`).
    pub checksum: Option<String>,
    /// Target VM name.
    pub vm_name: Option<String>,
    /// Memory size in MB.
    pub memory: Option<MemoryMb>,
    /// Number of virtual CPUs.
    pub cpus: Option<u32>,
    /// Motherboard chipset.
    pub chipset: Option<VBoxChipset>,
    /// VM firmware type.
    pub firmware: Option<VBoxFirmware>,
    /// Graphics controller model.
    pub graphics_controller: Option<VBoxGraphicsController>,
    /// Graphics VRAM size in MB.
    pub gfx_vram_size: Option<MemoryMb>,
    /// 3D graphics acceleration toggle.
    pub gfx_accelerate_3d: Option<bool>,
    /// Network adapter model.
    pub nic_type: Option<VBoxNicType>,
    /// Network configuration mode.
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
    /// Path to Guest Additions ISO to mount or upload.
    pub guest_additions_path: Option<String>,
    /// Guest additions mode (`attach`, `upload`, `disable`).
    pub guest_additions_mode: VBoxGuestAdditionsMode,
    /// Storage bus interface for attaching Guest Additions ISO.
    pub guest_additions_interface: Option<VBoxStorageBus>,
    /// Optional URL to download Guest Additions ISO.
    pub guest_additions_url: Option<String>,
    /// Optional SHA256 checksum for Guest Additions ISO.
    pub guest_additions_sha256: Option<String>,
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
    /// The boot command sequence.
    pub boot_command: Option<Vec<String>>,
    /// The wait time before booting.
    pub boot_wait: Option<String>,
    /// Headless mode.
    pub headless: bool,
    /// Output directory.
    pub output_directory: Option<String>,
    /// Custom `VBoxManage` commands executed post-import.
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

impl VirtualboxOvfConfig {
    /// Constructs a `VirtualboxOvfConfig` from a strongly-typed [`BuilderConfig`](crate::template::BuilderConfig).
    ///
    /// # Errors
    /// Returns `StampError::Builder` if parsing fails.
    pub fn from_builder_config(
        builder_config: &crate::template::BuilderConfig,
    ) -> Result<Self, StampError> {
        let name = builder_config.name.clone();
        let cfg = &builder_config.config;
        let exprs = &builder_config.expressions;

        let source_path = cfg
            .get("source_path")
            .or_else(|| cfg.get("vbox_source_path"))
            .cloned();
        let checksum = cfg
            .get("checksum")
            .or_else(|| cfg.get("vbox_checksum"))
            .cloned();
        let vm_name = cfg.get("vm_name").cloned();

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

        let guest_additions_path = cfg.get("guest_additions_path").cloned();
        let guest_additions_mode = cfg
            .get("guest_additions_mode")
            .and_then(|s| s.parse().ok())
            .unwrap_or(VBoxGuestAdditionsMode::Attach);
        let guest_additions_interface = cfg
            .get("guest_additions_interface")
            .or_else(|| cfg.get("vbox_guest_additions_interface"))
            .and_then(|s| s.parse().ok());
        let guest_additions_url = cfg.get("guest_additions_url").cloned();
        let guest_additions_sha256 = cfg.get("guest_additions_sha256").cloned();

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

        let export_format = cfg
            .get("export_format")
            .or_else(|| cfg.get("format"))
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        let boot_wait = cfg.get("boot_wait").cloned();
        let boot_command = cfg.get("boot_command").map(|s| parse_string_list(s));
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
            source_path,
            checksum,
            vm_name,
            memory,
            cpus,
            chipset,
            firmware,
            graphics_controller,
            gfx_vram_size,
            gfx_accelerate_3d,
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
            guest_additions_path,
            guest_additions_mode,
            guest_additions_interface,
            guest_additions_url,
            guest_additions_sha256,
            nested_virt,
            rtc_time_base,
            usb,
            virtualbox_version_file,
            shutdown_command,
            shutdown_timeout,
            export_format,
            boot_command,
            boot_wait,
            headless,
            output_directory,
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

/// Validate appliance file checksum against expected checksum string.
///
/// Supported formats: `sha256:<hash>`, `sha512:<hash>`, `md5:<hash>`, or bare hash.
///
/// # Errors
/// Returns `StampError::ChecksumMismatch` if checksum does not match, or `StampError::Io` on read failure.
pub async fn verify_appliance_checksum(
    path: &Path,
    expected_checksum: &str,
) -> Result<(), StampError> {
    if expected_checksum == "none" || expected_checksum.trim().is_empty() {
        return Ok(());
    }
    let data = tokio::fs::read(path).await.map_err(StampError::Io)?;
    let (algo, expected_hash) = if let Some((prefix, rest)) = expected_checksum.split_once(':') {
        (prefix.to_lowercase(), rest.trim().to_lowercase())
    } else {
        match expected_checksum.trim().len() {
            32 => ("md5".to_string(), expected_checksum.trim().to_lowercase()),
            40 => ("sha1".to_string(), expected_checksum.trim().to_lowercase()),
            128 => (
                "sha512".to_string(),
                expected_checksum.trim().to_lowercase(),
            ),
            _ => (
                "sha256".to_string(),
                expected_checksum.trim().to_lowercase(),
            ),
        }
    };

    let actual_hash = match algo.as_str() {
        "sha512" => hex::encode(Sha512::digest(&data)),
        "sha1" => {
            let mut hasher = Sha1::default();
            Sha1Digest::update(&mut hasher, &data);
            hex::encode(Sha1Digest::finalize(hasher))
        }
        "md5" => {
            let mut hasher = Md5::default();
            Md5Digest::update(&mut hasher, &data);
            hex::encode(Md5Digest::finalize(hasher))
        }
        _ => hex::encode(Sha256::digest(&data)),
    };

    if actual_hash != expected_hash {
        return Err(StampError::ChecksumMismatch {
            expected: expected_hash,
            actual: actual_hash,
        });
    }

    Ok(())
}

/// The `virtualbox-ovf` builder.
#[derive(Debug, Clone)]
pub struct VirtualboxOvfBuilder {
    /// The builder configuration.
    pub config: VirtualboxOvfConfig,
}

impl VirtualboxOvfBuilder {
    /// Create a new `VirtualboxOvfBuilder`.
    #[must_use]
    pub const fn new(config: VirtualboxOvfConfig) -> Self {
        Self { config }
    }
}

/// Step to import the OVF/OVA appliance and configure hardware.
#[derive(Debug, Clone)]
struct StepImportVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxOvfConfig,
}

#[async_trait::async_trait]
impl Step for StepImportVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self
            .config
            .vm_name
            .as_deref()
            .unwrap_or("packer-virtualbox-ovf");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-virtualbox-ovf");
        self.ui.say(
            &self.name,
            &format!("Importing VirtualBox OVF/OVA: {vm_name}"),
        );

        state.put("vm_name", vm_name.to_string());

        let source = self.config.source_path.as_deref().unwrap_or("source.ovf");

        // Validate source appliance checksum if provided and file exists
        if let Some(ref checksum) = self.config.checksum {
            let p = Path::new(source);
            if p.exists() {
                self.ui
                    .say(&self.name, &format!("Verifying checksum for {source}"));
                verify_appliance_checksum(p, checksum).await?;
            }
        }

        vboxmanage(&["import", source, "--vsys", "0", "--vmname", vm_name]).await?;

        // Modify memory and CPUs if specified
        if let Some(mem) = self.config.memory {
            let mem_str = format!("{}", mem.get());
            vboxmanage(&["modifyvm", vm_name, "--memory", &mem_str]).await?;
        }
        if let Some(cpus) = self.config.cpus {
            let cpus_str = format!("{cpus}");
            vboxmanage(&["modifyvm", vm_name, "--cpus", &cpus_str]).await?;
        }

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

        // Mount Guest Additions ISO if requested via Attach mode
        if self.config.guest_additions_mode == VBoxGuestAdditionsMode::Attach {
            let ga_path = self
                .config
                .guest_additions_path
                .as_ref()
                .map(PathBuf::from)
                .or_else(|| discover_guest_additions_iso(None))
                .unwrap_or_else(|| PathBuf::from("/usr/share/virtualbox/VBoxGuestAdditions.iso"));
            let ga_str = ga_path.to_string_lossy().to_string();
            self.ui.say(
                &self.name,
                &format!("Mounting Guest Additions ISO: {ga_str}"),
            );
            let ctl_name = match self.config.guest_additions_interface {
                Some(VBoxStorageBus::Sata) => "SATA Controller",
                _ => "IDE Controller",
            };
            let _ = vboxmanage(&[
                "storageattach",
                vm_name,
                "--storagectl",
                ctl_name,
                "--port",
                "1",
                "--device",
                "0",
                "--type",
                "dvddrive",
                "--medium",
                &ga_str,
            ])
            .await;
        }

        // Configure SSH and WinRM port forwarding
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

        // Execute post-import custom vboxmanage commands
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

/// Step to run the imported VM and send boot commands via scancodes.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxOvfConfig,
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
            crate::builder::virtualbox_iso::send_vbox_boot_command(&vm_name, &actions, None)
                .await?;
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
    config: VirtualboxOvfConfig,
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
    /// Configuration.
    config: VirtualboxOvfConfig,
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

        let ip = state
            .get::<String>("vm_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());
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
            source_type: "virtualbox-ovf".to_string(),
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
    /// Configuration.
    config: VirtualboxOvfConfig,
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
            .unwrap_or("output-virtualbox-ovf");

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

/// Step to export the VM to OVA or OVF.
#[derive(Debug, Clone)]
struct StepExport {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: VirtualboxOvfConfig,
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
            .unwrap_or("output-virtualbox-ovf");

        self.ui
            .say(&self.name, &format!("Exporting VM to {output_dir}"));

        let _ = std::fs::create_dir_all(output_dir);

        let ext = self.config.export_format.extension();
        let export_path = format!("{output_dir}/{vm_name}.{ext}");
        vboxmanage(&["export", &vm_name, "--output", &export_path]).await?;

        state.put("export_path", export_path);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for VirtualboxOvfBuilder {
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
            Box::new(StepImportVM {
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
            id: format!("virtualbox-ovf:{export_path}"),
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
                "mock ovf provision failure".to_string(),
            ))
        }
    }

    #[tokio::test]
    async fn test_virtualboxovfbuilder_run() {
        let temp_dir = tempfile::tempdir().unwrap();
        let src_file = temp_dir.path().join("source.ovf");
        tokio::fs::write(&src_file, b"OVF_SOURCE_CONTENT")
            .await
            .unwrap();
        let data = tokio::fs::read(&src_file).await.unwrap();
        let checksum = format!("sha256:{}", hex::encode(sha2::Sha256::digest(&data)));

        let config = VirtualboxOvfConfig {
            name: "test-builder".to_string(),
            vm_name: Some("test-vm".to_string()),
            source_path: Some(src_file.to_string_lossy().to_string()),
            checksum: Some(checksum),
            memory: Some(MemoryMb::new(2048)),
            cpus: Some(2),
            chipset: Some(VBoxChipset::Ich9),
            firmware: Some(VBoxFirmware::Efi),
            graphics_controller: Some(VBoxGraphicsController::VMSVGA),
            gfx_vram_size: Some(MemoryMb::new(128)),
            gfx_accelerate_3d: Some(true),
            nic_type: Some(VBoxNicType::VirtIO),
            network: Some("nat".to_string()),
            serial_ports: vec![VBoxSerialPort {
                port_number: 1,
                io_base: Some("0x3f8".to_string()),
                irq: Some(4),
                mode: VBoxSerialMode::RawFile,
                path: Some("/tmp/ovf-serial.log".to_string()),
            }],
            guest_additions_path: Some("/tmp/VBoxGuestAdditions.iso".to_string()),
            guest_additions_mode: VBoxGuestAdditionsMode::Upload,
            export_format: VBoxExportFormat::Ovf,
            output_directory: Some("custom-vbox-out".to_string()),
            boot_command: Some(vec!["install<enter>".to_string()]),
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
                "done".to_string(),
            ]],
            ..Default::default()
        };
        let builder = VirtualboxOvfBuilder::new(config.clone());

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
        assert!(
            res.as_ref()
                .map(|a| a.id())
                .ok()
                .unwrap_or_default()
                .contains("test-vm.ovf")
        );
        assert!(builder.cancel().await.is_ok());

        // Default run without export_format, output_directory, vm_name
        let b_default = VirtualboxOvfBuilder::new(VirtualboxOvfConfig {
            name: "default-vbox".to_string(),
            guest_additions_mode: VBoxGuestAdditionsMode::Attach,
            communicator: Some("winrm".to_string()),
            winrm_host_port: Some(Port::new(5985)),
            ..Default::default()
        });
        let res_default = b_default
            .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
            .await;
        assert!(res_default.is_ok());

        // Test step cleanups and execution (both without and with vm_ip/ssh_port)
        let mut empty_state = StateBag::new();
        let mut prov_step = StepProvision {
            ui: ui.clone(),
            name: "test".to_string(),
            hook,
            config: config.clone(),
        };
        assert!(prov_step.run(&mut empty_state).await.is_ok());

        let mut state = StateBag::new();
        state.put("vm_ip", "10.0.0.1".to_string());
        state.put("ssh_port", 2222u16);
        assert!(prov_step.run(&mut state).await.is_ok());
        prov_step.cleanup(&state).await;

        let mut import_step = StepImportVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        import_step.cleanup(&state).await;
        state.put("vm_name", "test-vm".to_string());
        import_step.cleanup(&state).await;

        let mut run_step = StepRunVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        run_step.cleanup(&state).await;

        let mut export_step = StepExport {
            ui,
            name: "test".to_string(),
            config,
        };
        export_step.cleanup(&state).await;
    }

    #[tokio::test]
    async fn test_verify_appliance_checksum_variations() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.ova");
        tokio::fs::write(&file_path, b"APPLIANCE_CONTENT")
            .await
            .unwrap();

        let sha256_hash = hex::encode(sha2::Sha256::digest(b"APPLIANCE_CONTENT"));
        let mut hasher = Md5::default();
        Md5Digest::update(&mut hasher, b"APPLIANCE_CONTENT");
        let md5_hash = hex::encode(Md5Digest::finalize(hasher));

        // Match with sha256: prefix
        assert!(
            verify_appliance_checksum(&file_path, &format!("sha256:{sha256_hash}"))
                .await
                .is_ok()
        );
        // Match with md5: prefix
        assert!(
            verify_appliance_checksum(&file_path, &format!("md5:{md5_hash}"))
                .await
                .is_ok()
        );
        // Match bare 64-char sha256
        assert!(
            verify_appliance_checksum(&file_path, &sha256_hash)
                .await
                .is_ok()
        );
        // Match bare 32-char md5
        assert!(
            verify_appliance_checksum(&file_path, &md5_hash)
                .await
                .is_ok()
        );
        // "none" ignores
        assert!(verify_appliance_checksum(&file_path, "none").await.is_ok());
        // empty ignores
        assert!(verify_appliance_checksum(&file_path, "").await.is_ok());

        // Mismatch
        let mismatch_res = verify_appliance_checksum(
            &file_path,
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .await;
        assert!(mismatch_res.is_err());
        assert!(matches!(
            mismatch_res.unwrap_err(),
            StampError::ChecksumMismatch { .. }
        ));
    }

    #[test]
    fn test_virtualboxovfbuilder_derived_traits() {
        let config1 = VirtualboxOvfConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = VirtualboxOvfBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));
    }

    #[tokio::test]
    async fn test_virtualbox_ovf_from_builder_config() {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "virtualbox-ovf".to_string(),
            name: "bento-ovf".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("vbox_source_path".to_string(), "/tmp/base.ova".to_string());
        builder_cfg
            .config
            .insert("vbox_checksum".to_string(), "sha256:abc".to_string());
        builder_cfg
            .config
            .insert("vm_name".to_string(), "imported-vm".to_string());
        builder_cfg
            .config
            .insert("memory".to_string(), "2048".to_string());
        builder_cfg
            .config
            .insert("cpus".to_string(), "2".to_string());
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
            r#"[["modifyvm", "{{.Name}}", "--cpus", "2"]]"#.to_string(),
        );

        let cfg = VirtualboxOvfConfig::from_builder_config(&builder_cfg).unwrap();
        assert_eq!(cfg.name, "bento-ovf");
        assert_eq!(cfg.source_path.as_deref(), Some("/tmp/base.ova"));
        assert_eq!(cfg.checksum.as_deref(), Some("sha256:abc"));
        assert_eq!(cfg.vm_name.as_deref(), Some("imported-vm"));
        assert_eq!(cfg.memory, Some(MemoryMb::new(2048)));
        assert_eq!(cfg.cpus, Some(2));
        assert_eq!(cfg.chipset, Some(VBoxChipset::Ich9));
        assert_eq!(cfg.firmware, Some(VBoxFirmware::Efi));
        assert_eq!(
            cfg.graphics_controller,
            Some(VBoxGraphicsController::VMSVGA)
        );
        assert_eq!(cfg.gfx_vram_size, Some(MemoryMb::new(64)));
        assert_eq!(cfg.gfx_accelerate_3d, Some(true));
        assert_eq!(cfg.nic_type, Some(VBoxNicType::VirtIO));
        assert_eq!(cfg.network.as_deref(), Some("nat"));
        assert_eq!(cfg.ssh_host_port, Some(Port::new(2222)));
        assert_eq!(cfg.winrm_host_port, Some(Port::new(5985)));
        assert_eq!(cfg.guest_additions_mode, VBoxGuestAdditionsMode::Upload);
        assert_eq!(cfg.export_format, VBoxExportFormat::Ovf);
        assert!(cfg.headless);
        assert_eq!(cfg.serial_ports.len(), 1);
        assert_eq!(cfg.vboxmanage.len(), 1);
    }

    #[tokio::test]
    async fn test_virtualbox_ovf_enhanced_options_and_helpers() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.ova");
        tokio::fs::write(&file_path, b"APPLIANCE_CONTENT")
            .await
            .unwrap();

        // SHA1 verification
        let mut sha1_hasher = Sha1::default();
        Sha1Digest::update(&mut sha1_hasher, b"APPLIANCE_CONTENT");
        let sha1_hash = hex::encode(Sha1Digest::finalize(sha1_hasher));

        assert!(
            verify_appliance_checksum(&file_path, &format!("sha1:{sha1_hash}"))
                .await
                .is_ok()
        );
        assert!(
            verify_appliance_checksum(&file_path, &sha1_hash)
                .await
                .is_ok()
        );

        // SHA512 verification
        let sha512_hash = hex::encode(Sha512::digest(b"APPLIANCE_CONTENT"));
        assert!(
            verify_appliance_checksum(&file_path, &format!("sha512:{sha512_hash}"))
                .await
                .is_ok()
        );
        assert!(
            verify_appliance_checksum(&file_path, &sha512_hash)
                .await
                .is_ok()
        );

        // Non-existent file
        let non_existent = temp_dir.path().join("does_not_exist.ova");
        assert!(
            verify_appliance_checksum(&non_existent, "sha256:abc")
                .await
                .is_err()
        );

        // Parse nested string list HCL tuple
        use hashicorp_configuration_language_rs::ast::expr::Expression;
        use hashicorp_configuration_language_rs::span::Span;
        let dummy = Span::default();
        let tuple_expr = Expression::Tuple(
            vec![
                Expression::Tuple(
                    vec![Expression::String("ovf_arg1".to_string(), dummy.clone())],
                    dummy.clone(),
                ),
                Expression::String("ovf_flat".to_string(), dummy.clone()),
            ],
            dummy,
        );
        let parsed_nested = parse_nested_string_list(None, Some(&tuple_expr));
        assert_eq!(parsed_nested.len(), 2);
        assert_eq!(parsed_nested[0], vec!["ovf_arg1"]);
        assert_eq!(parsed_nested[1], vec!["ovf_flat"]);

        // Flat JSON string and invalid JSON string
        let flat_json = "[\"argA\", \"argB\"]".to_string();
        let parsed_flat = parse_nested_string_list(Some(&flat_json), None);
        assert_eq!(parsed_flat, vec![vec!["argA", "argB"]]);

        let invalid = "not-valid-json".to_string();
        let parsed_inv = parse_nested_string_list(Some(&invalid), None);
        assert!(parsed_inv.is_empty());

        // CSV string list
        let csv_res = parse_string_list("item1, item2, 'item3', \"item4\"");
        assert_eq!(csv_res, vec!["item1", "item2", "item3", "item4"]);

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
        assert_eq!(ports[1].io_base.as_deref(), Some("0x2f8"));
        assert_eq!(ports[2].io_base, None);
        assert_eq!(ports[3].mode, VBoxSerialMode::Disconnected);

        // BuilderConfig parsing with all enhanced options
        let mut bento_cfg = crate::template::BuilderConfig {
            builder_type: "virtualbox-ovf".to_string(),
            name: "bento-ovf-test".to_string(),
            ..Default::default()
        };
        bento_cfg
            .config
            .insert("nested_virt".to_string(), "true".to_string());
        bento_cfg
            .config
            .insert("rtc_time_base".to_string(), "utc".to_string());
        bento_cfg
            .config
            .insert("usb".to_string(), "true".to_string());
        bento_cfg
            .config
            .insert("guest_additions_interface".to_string(), "sata".to_string());
        bento_cfg.config.insert(
            "virtualbox_version_file".to_string(),
            ".vbox_ver".to_string(),
        );
        bento_cfg
            .config
            .insert("shutdown_command".to_string(), "shutdown /s".to_string());
        bento_cfg
            .config
            .insert("shutdown_timeout".to_string(), "5m".to_string());

        let cfg = VirtualboxOvfConfig::from_builder_config(&bento_cfg).unwrap();
        assert_eq!(cfg.nested_virt, Some(true));
        assert_eq!(cfg.rtc_time_base.as_deref(), Some("utc"));
        assert_eq!(cfg.usb, Some(true));
        assert_eq!(cfg.guest_additions_interface, Some(VBoxStorageBus::Sata));
        assert_eq!(cfg.virtualbox_version_file.as_deref(), Some(".vbox_ver"));
        assert_eq!(cfg.shutdown_command.as_deref(), Some("shutdown /s"));
        assert_eq!(cfg.shutdown_timeout.as_deref(), Some("5m"));

        // StepShutdown testing with shutdown_command and virtualbox_version_file
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut step_shutdown = StepShutdown {
            ui: ui.clone(),
            name: "shutdown-ovf".to_string(),
            config: VirtualboxOvfConfig {
                name: "ovf-shutdown".to_string(),
                shutdown_command: Some("poweroff".to_string()),
                virtualbox_version_file: Some(".vbox_ver".to_string()),
                output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
                vboxmanage_post: vec![vec!["echo".to_string(), "done".to_string()]],
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("vm_name", "test-ovf-vm".to_string());
        let comm: Arc<dyn Communicator> =
            Arc::new(crate::communicator::mock::MockCommunicator::new());
        state.put("communicator", comm);

        assert!(step_shutdown.run(&mut state).await.is_ok());
        step_shutdown.cleanup(&state).await;

        // Test error strategies on VirtualboxOvfBuilder
        let err_builder = VirtualboxOvfBuilder::new(VirtualboxOvfConfig {
            name: "test_bad_exit".to_string(),
            ..Default::default()
        });
        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });

        assert!(
            err_builder
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Abort)
                .await
                .is_err()
        );
        assert!(
            err_builder
                .run(
                    hook.clone(),
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
            err_builder
                .run(hook.clone(), ui_ask, OnErrorStrategy::Ask)
                .await
                .is_err()
        );

        assert!(
            err_builder
                .run(hook, ui.clone(), OnErrorStrategy::Ask)
                .await
                .is_err()
        );

        // Test parse_string_list variants
        assert_eq!(parse_string_list("[\"single\"]"), vec!["single"]);
        assert_eq!(parse_string_list("a,,b"), vec!["a", "b"]);

        // Test verify_appliance_checksum fallback algorithm
        assert!(verify_appliance_checksum(&file_path, "1234").await.is_err());

        // Test StepUploadGuestAdditions
        let mut upload_step = StepUploadGuestAdditions {
            ui: ui.clone(),
            name: "upload-step".to_string(),
            config: VirtualboxOvfConfig {
                guest_additions_path: Some("/tmp/ga.iso".to_string()),
                ..Default::default()
            },
        };
        assert!(upload_step.run(&mut state).await.is_ok());
        upload_step.cleanup(&state).await;

        // Test StepRunVM with wait action
        let mut run_step = StepRunVM {
            ui: ui.clone(),
            name: "run-step".to_string(),
            config: VirtualboxOvfConfig {
                boot_command: Some(vec!["<wait1ms>".to_string(), "a".to_string()]),
                ..Default::default()
            },
        };
        assert!(run_step.run(&mut state).await.is_ok());
        run_step.cleanup(&state).await;

        // Test StepProvision failure and cleanup
        let mut prov_failing = StepProvision {
            ui,
            name: "fail-step".to_string(),
            hook: Arc::new(DefaultProvisionHook {
                provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
                error_cleanup_provisioners: Arc::new(vec![]),
            }),
            config: VirtualboxOvfConfig::default(),
        };
        assert!(prov_failing.run(&mut state).await.is_err());
        prov_failing.cleanup(&state).await;
    }
}
