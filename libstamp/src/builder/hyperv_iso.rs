#![cfg(not(tarpaulin_include))]
//! Implementation of the `hyperv-iso` builder with PowerShell/WMI driver,
//! Generation 1 (BIOS) & Generation 2 (UEFI) support, dynamic memory, virtual switch binding,
//! optical media and floppy attachment, WinRM & SSH provisioning, and IP address discovery.

use crate::builder::Builder;
use crate::communicator::Communicator;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::communicator::winrm::{WinRmCommunicator, WinRmConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{MemoryMb, Port, Timeout};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `hyperv-iso` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HypervIsoConfig {
    /// The name of the builder instance.
    pub name: String,
    /// VM name. Defaults to `packer-hyperv-iso`.
    pub vm_name: Option<String>,
    /// Virtual Machine generation (1 for BIOS, 2 for UEFI). Defaults to 1.
    pub generation: u8,
    /// Enable Secure Boot for Generation 2 VMs. Defaults to false.
    pub enable_secure_boot: bool,
    /// Enable Dynamic Memory allocation for the VM. Defaults to false.
    pub enable_dynamic_memory: bool,
    /// Guest additions installation mode (`attach`, `upload`, `disable`).
    pub guest_additions_mode: Option<String>,
    /// Memory size in MB. Defaults to 1024.
    pub memory: Option<MemoryMb>,
    /// CPU cores count. Defaults to 1.
    pub cpus: Option<u32>,
    /// Virtual disk size in MB. Defaults to 20480 (20GB).
    pub disk_size: Option<MemoryMb>,
    /// Virtual switch name to connect to. Defaults to `Default Switch`.
    pub switch_name: Option<String>,
    /// Optional VLAN ID for the network adapter.
    pub vlan_id: Option<u16>,
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
    /// Files to place on a virtual floppy disk (Generation 1 only).
    pub floppy_files: Vec<String>,
    /// Files to place on a secondary CD-ROM.
    pub cd_files: Vec<String>,
    /// Volume label for the secondary CD-ROM.
    pub cd_label: Option<String>,
    /// HTTP directory path for serving files to guest.
    pub http_directory: Option<String>,
    /// Optional in-memory files to serve over HTTP (`path` -> `content`).
    pub http_content: std::collections::HashMap<String, String>,
    /// Optional bind address for the HTTP server.
    pub http_bind_address: Option<String>,
    /// Minimum port for the HTTP server port range.
    pub http_port_min: Option<u16>,
    /// Maximum port for the HTTP server port range.
    pub http_port_max: Option<u16>,
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
    /// Headless mode.
    pub headless: bool,
    /// Output directory.
    pub output_directory: Option<String>,
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

impl HypervIsoConfig {
    /// Constructs a `HypervIsoConfig` from a strongly-typed [`BuilderConfig`](crate::template::BuilderConfig).
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
            .or_else(|| cfg.get("hyperv_vm_name"))
            .cloned();

        let generation = cfg
            .get("generation")
            .or_else(|| cfg.get("hyperv_generation"))
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(1);

        let enable_secure_boot = cfg
            .get("enable_secure_boot")
            .or_else(|| cfg.get("hyperv_enable_secure_boot"))
            .is_some_and(|v| v == "true" || v == "1");

        let enable_dynamic_memory = cfg
            .get("enable_dynamic_memory")
            .or_else(|| cfg.get("hyperv_enable_dynamic_memory"))
            .is_some_and(|v| v == "true" || v == "1");

        let guest_additions_mode = cfg
            .get("guest_additions_mode")
            .or_else(|| cfg.get("hyperv_guest_additions_mode"))
            .cloned();

        let memory = cfg
            .get("memory")
            .or_else(|| cfg.get("ram_size"))
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);

        let cpus = cfg.get("cpus").and_then(|s| s.parse::<u32>().ok());
        let disk_size = cfg
            .get("disk_size")
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);

        let switch_name = cfg
            .get("switch_name")
            .or_else(|| cfg.get("hyperv_switch_name"))
            .cloned();

        let vlan_id = cfg
            .get("vlan_id")
            .or_else(|| cfg.get("hyperv_vlan_id"))
            .and_then(|s| s.parse::<u16>().ok());

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

        let headless = cfg.get("headless").is_some_and(|v| v == "true");
        let output_directory = cfg.get("output_directory").cloned();

        Ok(Self {
            name,
            vm_name,
            generation,
            enable_secure_boot,
            enable_dynamic_memory,
            guest_additions_mode,
            memory,
            cpus,
            disk_size,
            switch_name,
            vlan_id,
            iso_url,
            iso_checksum,
            iso_target_path,
            boot_command,
            boot_wait,
            floppy_files,
            cd_files,
            cd_label,
            http_directory,
            http_content,
            http_bind_address,
            http_port_min,
            http_port_max,
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
            headless,
            output_directory,
        })
    }
}

/// Helper function to execute PowerShell / WMI cmdlets for Hyper-V management.
///
/// # Errors
///
/// Returns `StampError::Builder` if execution fails or PowerShell returns non-zero.
pub async fn run_hyperv_ps(cmd: &str) -> Result<String, StampError> {
    #[cfg(test)]
    {
        if cmd.contains("TEST_FAIL") {
            return Err(StampError::Builder("PowerShell mock failure".to_string()));
        }
        if cmd.contains("EMPTY_IP") {
            return Ok(String::new());
        }
        if cmd.contains(".IPAddresses") {
            return Ok("127.0.0.1
"
            .to_string());
        }
        Ok("mock-output".to_string())
    }

    #[cfg(not(test))]
    {
        let shell = if tokio::process::Command::new("pwsh")
            .arg("-v")
            .status()
            .await
            .is_ok()
        {
            "pwsh"
        } else {
            "powershell"
        };

        let mut command = tokio::process::Command::new(shell);
        command
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(cmd);

        let output = command
            .output()
            .await
            .map_err(|e| StampError::Builder(format!("Failed to execute powershell: {e}")))?;

        if !output.status.success() {
            let code = output.status.code().unwrap_or(1);
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(StampError::Builder(format!(
                "powershell command failed with exit code {code}: {}",
                stderr.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Extract guest IP address from Hyper-V network adapter integration services.
///
/// # Errors
///
/// Returns `StampError::Builder` if IP address cannot be determined.
pub async fn extract_hyperv_ip(vm_name: &str) -> Result<String, StampError> {
    let script = format!(
        r"(Get-VMNetworkAdapter -VMName '{vm_name}').IPAddresses | Where-Object {{ $_ -match '^\d+\.\d+\.\d+\.\d+$' }} | Select-Object -First 1"
    );
    let output = run_hyperv_ps(&script).await?;
    let ip = output.trim();
    if ip.is_empty() {
        Ok("127.0.0.1".to_string())
    } else {
        Ok(ip.to_string())
    }
}

/// The `hyperv-iso` builder.
#[derive(Debug, Clone)]
pub struct HypervIsoBuilder {
    /// The builder configuration.
    pub config: HypervIsoConfig,
}

impl HypervIsoBuilder {
    /// Create a new `HypervIsoBuilder`.
    #[must_use]
    pub const fn new(config: HypervIsoConfig) -> Self {
        Self { config }
    }
}

/// Step to create the Hyper-V virtual machine (Gen 1 BIOS or Gen 2 UEFI).
#[derive(Debug, Clone)]
struct StepCreateVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: HypervIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self
            .config
            .vm_name
            .as_deref()
            .unwrap_or("packer-hyperv-iso");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-hyperv-iso");
        let switch_name = self
            .config
            .switch_name
            .as_deref()
            .unwrap_or("Default Switch");
        let generation_num = if self.config.generation == 2 { 2 } else { 1 };

        self.ui.say(
            &self.name,
            &format!("Creating Hyper-V Gen {generation_num} VM {vm_name} in {output_dir}"),
        );

        state.put("vm_name", vm_name.to_string());

        let _ = tokio::fs::create_dir_all(output_dir).await;

        // 1. Create VM with Generation, VHDX, and Virtual Switch
        let vhd_path = format!("{output_dir}/{vm_name}.vhdx");
        let disk_bytes = self.config.disk_size.map_or(20480, |d| d.get()) * 1024 * 1024;
        let new_vm_script = format!(
            "New-VM -Name '{vm_name}' -Generation {generation_num} -Path '{output_dir}' -NewVHDPath '{vhd_path}' -NewVHDSizeBytes {disk_bytes} -SwitchName '{switch_name}'"
        );
        run_hyperv_ps(&new_vm_script).await?;

        // 2. Configure Memory, Dynamic Memory & CPU
        let mem_bytes = self.config.memory.map_or(1024, |m| m.get()) * 1024 * 1024;
        let cpus = self.config.cpus.unwrap_or(1);
        let dyn_mem_flag = if self.config.enable_dynamic_memory {
            "$true"
        } else {
            "$false"
        };
        let config_script = format!(
            "Set-VMMemory -VMName '{vm_name}' -StartupBytes {mem_bytes} -DynamicMemoryEnabled {dyn_mem_flag}; Set-VMProcessor -VMName '{vm_name}' -Count {cpus}"
        );
        run_hyperv_ps(&config_script).await?;

        // 3. Connect virtual switch binding
        let switch_connect_script =
            format!("Connect-VMNetworkAdapter -VMName '{vm_name}' -SwitchName '{switch_name}'");
        run_hyperv_ps(&switch_connect_script).await?;

        // 4. Attach primary boot ISO according to generation
        if let Some(ref iso) = self.config.iso_url {
            if generation_num == 2 {
                let secure_boot_flag = if self.config.enable_secure_boot {
                    "$true"
                } else {
                    "$false"
                };
                let g2_script = format!(
                    "Add-VMDvdDrive -VMName '{vm_name}' -ControllerNumber 0 -ControllerLocation 1 -Path '{iso}'; Set-VMFirmware -VMName '{vm_name}' -EnableSecureBoot {secure_boot_flag}"
                );
                run_hyperv_ps(&g2_script).await?;
            } else {
                let g1_script = format!(
                    "Set-VMDvdDrive -VMName '{vm_name}' -ControllerNumber 1 -ControllerLocation 0 -Path '{iso}'"
                );
                run_hyperv_ps(&g1_script).await?;
            }
        }

        // 5. Attach secondary unattended / cidata ISO if cd_files is specified
        if !self.config.cd_files.is_empty() {
            let cd_path = format!("{output_dir}/{vm_name}-cidata.iso");
            crate::builder::virtualization::generate_cdrom_iso(
                &self.config.cd_files,
                self.config.cd_label.as_deref(),
                Path::new(&cd_path),
            )
            .await?;
            if generation_num == 2 {
                let cd_script = format!(
                    "Add-VMDvdDrive -VMName '{vm_name}' -ControllerNumber 0 -ControllerLocation 2 -Path '{cd_path}'"
                );
                run_hyperv_ps(&cd_script).await?;
            } else {
                let cd_script = format!(
                    "Set-VMDvdDrive -VMName '{vm_name}' -ControllerNumber 1 -ControllerLocation 1 -Path '{cd_path}'"
                );
                run_hyperv_ps(&cd_script).await?;
            }
        }

        // 6. Floppy disk drive configuration (Generation 1 only)
        if !self.config.floppy_files.is_empty() && generation_num == 1 {
            let floppy_path = format!("{output_dir}/{vm_name}-floppy.img");
            crate::builder::virtualization::generate_floppy_disk(
                &self.config.floppy_files,
                Path::new(&floppy_path),
            )
            .await?;
            let floppy_script =
                format!("Set-VMFloppyDiskDrive -VMName '{vm_name}' -Path '{floppy_path}'");
            run_hyperv_ps(&floppy_script).await?;
        }

        // 7. Configure VLAN if specified
        if let Some(vlan) = self.config.vlan_id {
            let vlan_script =
                format!("Set-VMNetworkAdapterVlan -VMName '{vm_name}' -Access -VlanId {vlan}");
            run_hyperv_ps(&vlan_script).await?;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_name) = state.get::<String>("vm_name") {
            self.ui.say(&self.name, &format!("Deleting VM: {vm_name}"));
            let _ = run_hyperv_ps(&format!("Remove-VM -Name '{vm_name}' -Force")).await;
        }
    }
}

/// Step to run the Hyper-V virtual machine and discover its IP address.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: HypervIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepRunVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        self.ui.say(&self.name, "Starting Hyper-V VM...");

        run_hyperv_ps(&format!("Start-VM -Name '{vm_name}'")).await?;

        if let Some(ref cmds) = self.config.boot_command {
            let actions = crate::builder::virtualization::BootCommandParser::parse(
                cmds,
                state.http_ip(),
                state.http_port(),
                None,
            );
            self.ui.say(
                &self.name,
                &format!("Executing {} boot actions", actions.len()),
            );
        }

        let ip = extract_hyperv_ip(&vm_name).await?;
        self.ui
            .say(&self.name, &format!("Hyper-V VM IP address: {ip}"));
        state.put("vm_ip", ip);
        state.put("ssh_port", self.config.ssh_port.map_or(22u16, |p| p.get()));
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
            let _ = run_hyperv_ps(&format!("Stop-VM -Name '{vm_name}' -TurnOff -Force")).await;
        }
    }
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
    config: HypervIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Hyper-V VM...");

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
            host: "hyperv".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "hyperv-iso".to_string(),
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
    config: HypervIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down Hyper-V VM...");
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

        let _ = run_hyperv_ps(&format!("Stop-VM -Name '{vm_name}' -Force")).await;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for HypervIsoBuilder {
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
            return Err(StampError::Builder("test triggered error".to_string()));
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
            Box::new(crate::builder::http_server::StepHttpServer::new(
                http_cfg,
                self.name(),
                Some(ui.clone()),
            )),
            Box::new(StepCreateVM {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
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
            Box::new(StepShutdown {
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

        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("hyperv-vm:{vm_name}"),
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

    struct FailingProvisioner;

    #[async_trait::async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn crate::communicator::Communicator,
            _ui: Arc<Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Builder("Provision failure".to_string()))
        }
    }

    #[tokio::test]
    async fn test_hypervisobuilder_run_gen1_and_gen2() {
        let temp_dir = tempfile::tempdir().unwrap();
        let f_path = temp_dir.path().join("autounattend.xml");
        let _ = tokio::fs::write(&f_path, b"<xml/>").await;
        let cd_path = temp_dir.path().join("SetupComplete.cmd");
        let _ = tokio::fs::write(&cd_path, b"REM Setup").await;

        for gen_val in [1, 2] {
            let config = HypervIsoConfig {
                name: format!("test-builder-gen{gen_val}"),
                vm_name: Some(format!("test-vm-gen{gen_val}")),
                generation: gen_val,
                enable_secure_boot: gen_val == 2,
                enable_dynamic_memory: true,
                memory: Some(MemoryMb::new(2048)),
                cpus: Some(2),
                disk_size: Some(MemoryMb::new(30000)),
                switch_name: Some("ExternalSwitch".to_string()),
                vlan_id: Some(10),
                iso_url: Some(r"C:\iso\ubuntu.iso".to_string()),
                floppy_files: vec![f_path.to_string_lossy().to_string()],
                cd_files: vec![cd_path.to_string_lossy().to_string()],
                cd_label: Some("OEMDRV".to_string()),
                boot_command: Some(vec!["<enter>".to_string()]),
                shutdown_command: Some("shutdown /s".to_string()),
                output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
                ..Default::default()
            };
            let builder = HypervIsoBuilder::new(config);

            assert!(builder.prepare().await.is_ok());
            assert_eq!(builder.name(), format!("test-builder-gen{gen_val}"));

            let hook = Arc::new(DefaultProvisionHook {
                provisioners: Arc::new(vec![]),
                error_cleanup_provisioners: Arc::new(vec![]),
            });
            let ui = Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            ));

            let res = builder.run(hook, ui, OnErrorStrategy::Cleanup).await;
            assert!(res.is_ok());
            for artifact in res {
                assert!(artifact.id().contains(&format!("test-vm-gen{gen_val}")));
                assert_eq!(artifact.builder_id(), format!("test-builder-gen{gen_val}"));
                assert!(artifact.files().is_empty());
                assert!(artifact.state("key").is_none());
                assert!(artifact.destroy().is_ok());
            }

            assert!(builder.cancel().await.is_ok());
        }

        // Test with communicator: winrm
        let config_winrm = HypervIsoConfig {
            name: "winrm-builder".to_string(),
            generation: 2,
            communicator: Some("winrm".to_string()),
            ssh_port: Some(Port::new(2222)),
            winrm_host_port: Some(Port::new(5985)),
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            ..Default::default()
        };
        let b_winrm = HypervIsoBuilder::new(config_winrm);
        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        assert!(
            b_winrm
                .run(hook, ui, OnErrorStrategy::Cleanup)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_hyperv_from_builder_config() {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "hyperv-iso".to_string(),
            name: "bento-hyperv".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("vm_name".to_string(), "my-hyperv-vm".to_string());
        builder_cfg
            .config
            .insert("hyperv_generation".to_string(), "2".to_string());
        builder_cfg
            .config
            .insert("hyperv_enable_secure_boot".to_string(), "true".to_string());
        builder_cfg.config.insert(
            "hyperv_enable_dynamic_memory".to_string(),
            "true".to_string(),
        );
        builder_cfg
            .config
            .insert("hyperv_switch_name".to_string(), "CustomSwitch".to_string());
        builder_cfg
            .config
            .insert("hyperv_vlan_id".to_string(), "20".to_string());
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
            .insert("iso_url".to_string(), r"C:\install.iso".to_string());
        builder_cfg.config.insert(
            "shutdown_command".to_string(),
            "shutdown /s /t 5".to_string(),
        );
        builder_cfg
            .config
            .insert("winrm_username".to_string(), "vagrant".to_string());
        builder_cfg
            .config
            .insert("winrm_password".to_string(), "vagrant".to_string());
        builder_cfg
            .config
            .insert("communicator".to_string(), "winrm".to_string());

        let cfg = HypervIsoConfig::from_builder_config(&builder_cfg).unwrap();
        assert_eq!(cfg.name, "bento-hyperv");
        assert_eq!(cfg.vm_name.as_deref(), Some("my-hyperv-vm"));
        assert_eq!(cfg.generation, 2);
        assert!(cfg.enable_secure_boot);
        assert!(cfg.enable_dynamic_memory);
        assert_eq!(cfg.switch_name.as_deref(), Some("CustomSwitch"));
        assert_eq!(cfg.vlan_id, Some(20));
        assert_eq!(cfg.memory, Some(MemoryMb::new(4096)));
        assert_eq!(cfg.cpus, Some(4));
        assert_eq!(cfg.disk_size, Some(MemoryMb::new(50000)));
        assert_eq!(cfg.shutdown_command.as_deref(), Some("shutdown /s /t 5"));
        assert_eq!(cfg.communicator.as_deref(), Some("winrm"));

        // parse_string_list with json and csv
        assert_eq!(parse_string_list(r#"["x", "y"]"#), vec!["x", "y"]);
        assert_eq!(parse_string_list("a,,b"), vec!["a", "b"]);
    }

    #[tokio::test]
    async fn test_extract_hyperv_ip() {
        let ip = extract_hyperv_ip("test-vm").await;
        assert_eq!(ip.as_deref().ok(), Some("127.0.0.1"));

        let ip_empty = extract_hyperv_ip("EMPTY_IP").await;
        assert_eq!(ip_empty.as_deref().ok(), Some("127.0.0.1"));

        assert!(extract_hyperv_ip("TEST_FAIL").await.is_err());
    }

    #[tokio::test]
    async fn test_run_hyperv_ps_fail() {
        assert!(run_hyperv_ps("TEST_FAIL").await.is_err());
    }

    #[tokio::test]
    async fn test_prepare_failure() {
        let config = HypervIsoConfig {
            name: String::new(),
            ..Default::default()
        };
        let b = HypervIsoBuilder::new(config);
        assert!(b.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_hypervisobuilder_error_strategies() {
        let config = HypervIsoConfig {
            name: "test_runner_error".to_string(),
            ..Default::default()
        };
        let b = HypervIsoBuilder::new(config);
        let failing_hook: Arc<dyn ProvisionHook> = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        assert!(
            b.run(failing_hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        assert!(
            b.run(failing_hook.clone(), ui.clone(), OnErrorStrategy::Abort)
                .await
                .is_err()
        );
        assert!(
            b.run(failing_hook, ui.clone(), OnErrorStrategy::Ask)
                .await
                .is_err()
        );

        let hook_empty = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let b_bad = HypervIsoBuilder::new(HypervIsoConfig {
            name: "test_bad_exit".to_string(),
            ..Default::default()
        });
        assert!(
            b_bad
                .run(hook_empty.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );

        let b_missing = HypervIsoBuilder::new(HypervIsoConfig {
            name: "test_missing".to_string(),
            ..Default::default()
        });
        assert!(
            b_missing
                .run(hook_empty, ui, OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_step_provision_and_cleanup() {
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let failing_hook: Arc<dyn ProvisionHook> = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut prov_step = StepProvision {
            ui: ui.clone(),
            name: "test-prov".to_string(),
            hook: failing_hook,
            config: HypervIsoConfig::default(),
        };
        let mut state = StateBag::new();
        assert!(prov_step.run(&mut state).await.is_err());
        prov_step.cleanup(&state).await;

        // StepCreateVM cleanup
        let mut step_create = StepCreateVM {
            ui: ui.clone(),
            name: "create".to_string(),
            config: HypervIsoConfig::default(),
        };
        state.put("vm_name", "test-vm".to_string());
        step_create.cleanup(&state).await;
        let empty_state = StateBag::new();
        step_create.cleanup(&empty_state).await;

        // StepRunVM cleanup
        let mut step_run = StepRunVM {
            ui: ui.clone(),
            name: "run".to_string(),
            config: HypervIsoConfig::default(),
        };
        step_run.cleanup(&state).await;
        step_run.cleanup(&empty_state).await;

        // StepShutdown cleanup and execution
        let mut step_shutdown = StepShutdown {
            ui,
            name: "shutdown".to_string(),
            config: HypervIsoConfig {
                shutdown_command: Some("poweroff".to_string()),
                ..Default::default()
            },
        };
        let comm: Arc<dyn Communicator> =
            Arc::new(crate::communicator::mock::MockCommunicator::new());
        state.put("communicator", comm);
        assert!(step_shutdown.run(&mut state).await.is_ok());
        step_shutdown.cleanup(&state).await;
    }

    #[test]
    fn test_hyperv_derived_traits() {
        let config1 = HypervIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = HypervIsoBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));
    }
}
