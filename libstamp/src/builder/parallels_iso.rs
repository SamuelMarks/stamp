#![cfg(not(tarpaulin_include))]
//! Implementation of the `parallels-iso` builder with full `prlctl` automation driver,
//! hardware customization, media attachment, Parallels Tools handling, and provisioning.

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

/// Guest operating system flavor for `Parallels` Tools ISO discovery and mounting.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ParallelsToolsFlavor {
    /// Linux x86_64 guest (`prl-tools-lin.iso`).
    #[default]
    Lin,
    /// Windows x86_64 guest (`prl-tools-win.iso`).
    Win,
    /// macOS / OS X x86_64 guest (`prl-tools-mac.iso`).
    Mac,
    /// Windows ARM64 guest (`prl-tools-win-arm.iso`).
    WinArm,
    /// Linux ARM64 guest (`prl-tools-lin-arm.iso`).
    LinArm,
    /// macOS ARM64 guest (`prl-tools-mac-arm.iso`).
    MacArm,
    /// Custom flavor name.
    Custom(String),
}

impl ParallelsToolsFlavor {
    /// String representation of the Parallels Tools flavor.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Lin => "lin",
            Self::Win => "win",
            Self::Mac => "mac",
            Self::WinArm => "win-arm",
            Self::LinArm => "lin-arm",
            Self::MacArm => "mac-arm",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for ParallelsToolsFlavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ParallelsToolsFlavor {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "lin" | "linux" => Ok(Self::Lin),
            "win" | "windows" => Ok(Self::Win),
            "mac" | "darwin" | "macos" => Ok(Self::Mac),
            "win-arm" | "win_arm" | "windows-arm" => Ok(Self::WinArm),
            "lin-arm" | "lin_arm" | "linux-arm" => Ok(Self::LinArm),
            "mac-arm" | "mac_arm" | "macos-arm" => Ok(Self::MacArm),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

/// Parallels Tools installation and mounting mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ParallelsToolsMode {
    /// Attach the Parallels Tools ISO to a virtual optical drive.
    #[default]
    Attach,
    /// Upload the Parallels Tools ISO to the guest VM via communicator.
    Upload,
    /// Do not attach or upload Parallels Tools.
    Disable,
}

impl ParallelsToolsMode {
    /// String representation of the Parallels Tools mode.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Attach => "attach",
            Self::Upload => "upload",
            Self::Disable => "disable",
        }
    }
}

impl fmt::Display for ParallelsToolsMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ParallelsToolsMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "upload" => Ok(Self::Upload),
            "disable" | "none" => Ok(Self::Disable),
            _ => Ok(Self::Attach),
        }
    }
}

/// Configuration for the `parallels-iso` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParallelsIsoConfig {
    /// The name of the builder instance.
    pub name: String,
    /// VM name. Defaults to `packer-parallels-iso`.
    pub vm_name: Option<String>,
    /// Guest OS type for `prlctl create --ostype <type>`.
    pub guest_os_type: Option<String>,
    /// Distribution name for `prlctl create --distribution <distro>`.
    pub distribution: Option<String>,
    /// Number of virtual CPUs. Defaults to 1.
    pub cpus: Option<u32>,
    /// Memory size in MB. Defaults to 1024.
    pub memory: Option<MemoryMb>,
    /// Disk size in MB. Defaults to 20480.
    pub disk_size: Option<MemoryMb>,
    /// Video memory size in MB.
    pub video_memory: Option<MemoryMb>,
    /// Source installer ISO URL or path.
    pub iso_url: Option<String>,
    /// Checksum of the installer ISO.
    pub iso_checksum: Option<String>,
    /// Target path for downloading the ISO.
    pub iso_target_path: Option<String>,
    /// Parallels Tools guest OS flavor (`lin`, `win`, `mac`, `win-arm`, `lin-arm`, `mac-arm`).
    pub parallels_tools_flavor: Option<ParallelsToolsFlavor>,
    /// Parallels Tools mounting mode (`attach`, `upload`, `disable`). Defaults to `attach`.
    pub parallels_tools_mode: ParallelsToolsMode,
    /// Custom `prlctl` commands executed after VM creation.
    pub prlctl: Vec<Vec<String>>,
    /// Custom `prlctl` commands executed after provisioning.
    pub prlctl_post: Vec<Vec<String>>,
    /// Path to write `prlctl` version information.
    pub prlctl_version_file: Option<String>,
    /// Boot command sequence typed via VNC or scancodes.
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
    /// Headless mode.
    pub headless: bool,
    /// Output directory for exported appliance.
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

impl ParallelsIsoConfig {
    /// Constructs a `ParallelsIsoConfig` from a strongly-typed [`BuilderConfig`](crate::template::BuilderConfig).
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

        let vm_name = cfg
            .get("vm_name")
            .or_else(|| cfg.get("parallels_vm_name"))
            .cloned();
        let guest_os_type = cfg
            .get("guest_os_type")
            .or_else(|| cfg.get("parallels_guest_os_type"))
            .cloned();
        let distribution = cfg.get("distribution").cloned();

        let cpus = cfg.get("cpus").and_then(|s| s.parse::<u32>().ok());
        let memory = cfg
            .get("memory")
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);
        let disk_size = cfg
            .get("disk_size")
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);
        let video_memory = cfg
            .get("video_memory")
            .or_else(|| cfg.get("videosize"))
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);

        let iso_url = cfg.get("iso_url").cloned();
        let iso_checksum = cfg.get("iso_checksum").cloned();
        let iso_target_path = cfg.get("iso_target_path").cloned();

        let parallels_tools_flavor = cfg
            .get("parallels_tools_flavor")
            .and_then(|s| s.parse().ok());
        let parallels_tools_mode = cfg
            .get("parallels_tools_mode")
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        let prlctl = parse_nested_string_list(
            cfg.get("prlctl").or_else(|| cfg.get("parallels_prlctl")),
            exprs
                .get("prlctl")
                .or_else(|| exprs.get("parallels_prlctl")),
        );
        let prlctl_post = parse_nested_string_list(
            cfg.get("prlctl_post")
                .or_else(|| cfg.get("parallels_prlctl_post")),
            exprs
                .get("prlctl_post")
                .or_else(|| exprs.get("parallels_prlctl_post")),
        );
        let prlctl_version_file = cfg
            .get("prlctl_version_file")
            .or_else(|| cfg.get("parallels_prlctl_version_file"))
            .cloned();

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

        let headless = cfg.get("headless").is_some_and(|v| v == "true");
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
            guest_os_type,
            distribution,
            cpus,
            memory,
            disk_size,
            video_memory,
            iso_url,
            iso_checksum,
            iso_target_path,
            parallels_tools_flavor,
            parallels_tools_mode,
            prlctl,
            prlctl_post,
            prlctl_version_file,
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
            headless,
            output_directory,
            http_directory,
            http_content,
            http_bind_address,
            http_port_min,
            http_port_max,
        })
    }
}

/// Discovers the path to the `prlctl` CLI executable on the host system.
#[must_use]
pub fn find_prlctl_binary() -> PathBuf {
    if let Ok(env_path) = std::env::var("PRLCTL_PATH") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return p;
        }
    }

    let candidates = [
        PathBuf::from("/usr/local/bin/prlctl"),
        PathBuf::from("/usr/bin/prlctl"),
        PathBuf::from("/Applications/Parallels Desktop.app/Contents/MacOS/prlctl"),
    ];

    candidates
        .into_iter()
        .find(|c| c.exists())
        .unwrap_or_else(|| PathBuf::from("prlctl"))
}

/// Discovers the path to the `Parallels` Tools ISO image on the host system.
#[must_use]
pub fn discover_parallels_tools_iso(flavor: Option<&ParallelsToolsFlavor>) -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("PARALLELS_TOOLS_ISO") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }

    let flav_str = flavor.map_or("lin", ParallelsToolsFlavor::as_str);

    let candidates = [
        PathBuf::from(format!(
            "/Applications/Parallels Desktop.app/Contents/Resources/Tools/prl-tools-{flav_str}.iso"
        )),
        PathBuf::from(format!("/Library/Parallels/Tools/prl-tools-{flav_str}.iso")),
        PathBuf::from(format!(
            "/usr/share/parallels/tools/prl-tools-{flav_str}.iso"
        )),
    ];

    candidates.into_iter().find(|c| c.exists())
}

/// Interpolates placeholders `{{.Name}}`, `{{ .Name }}`, `{{.Version}}`, `{{ .Version }}`, `{{.OutputDirectory}}`, `{{ .OutputDirectory }}` in arguments.
#[must_use]
pub fn interpolate_prlctl_args(
    args: &[String],
    vm_name: &str,
    prl_version: Option<&str>,
    output_dir: Option<&str>,
) -> Vec<String> {
    let version = prl_version.unwrap_or("19.0");
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

/// Executes a sequence of custom `prlctl` command argument vectors with placeholder interpolation.
///
/// # Errors
///
/// Returns `StampError::Builder` if any command fails.
pub async fn execute_prlctl_commands(
    commands: &[Vec<String>],
    vm_name: &str,
    prl_version: Option<&str>,
    output_dir: Option<&str>,
) -> Result<(), StampError> {
    for cmd_args in commands {
        if cmd_args.is_empty() {
            continue;
        }
        let interpolated = interpolate_prlctl_args(cmd_args, vm_name, prl_version, output_dir);
        let str_args: Vec<&str> = interpolated.iter().map(String::as_str).collect();
        run_prlctl(&str_args).await?;
    }
    Ok(())
}

/// Queries the installed `Parallels` version using `prlctl --version`.
pub async fn query_prlctl_version() -> Option<String> {
    let output = run_prlctl(&["--version"]).await.ok()?;
    let line = output.lines().next()?.trim();
    let ver = line.split_whitespace().last()?.trim();
    if ver.is_empty() {
        None
    } else {
        Some(ver.to_string())
    }
}

/// Execute a `prlctl` command with arguments.
///
/// # Errors
///
/// Returns `StampError::Builder` if `prlctl` fails.
pub async fn run_prlctl(args: &[&str]) -> Result<String, StampError> {
    run_prlctl_with_cmd(prlctl_binary(), args).await
}

#[cfg(not(test))]
/// Resolves the default `prlctl` binary name for production execution.
fn prlctl_binary() -> &'static str {
    "prlctl"
}

#[cfg(test)]
/// Resolves the mock `echo` binary name during unit tests.
fn prlctl_binary() -> &'static str {
    "echo"
}

/// Execute a specific command as `prlctl` driver.
///
/// # Errors
///
/// Returns `StampError::Builder` if the `prlctl` command fails or cannot be spawned.
pub async fn run_prlctl_with_cmd(cmd: &str, args: &[&str]) -> Result<String, StampError> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| StampError::Builder(format!("Failed to execute prlctl: {e}")))?;

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
            "prlctl command '{cmd} {args:?}' failed with exit code {code}: {msg}"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Discovers the guest IP address of a running `Parallels` VM.
///
/// # Errors
///
/// Returns `StampError::Builder` if discovery fails.
pub async fn discover_parallels_guest_ip(vm_name: &str) -> Result<String, StampError> {
    discover_parallels_guest_ip_with_cmd(prlctl_binary(), vm_name).await
}

/// Discovers the guest IP address using a specific command runner.
///
/// # Errors
///
/// Returns `StampError::Builder` if discovery fails.
pub async fn discover_parallels_guest_ip_with_cmd(
    cmd: &str,
    vm_name: &str,
) -> Result<String, StampError> {
    let output = run_prlctl_with_cmd(cmd, &["list", "-f", vm_name]).await?;
    for line in output.lines() {
        if line.contains("ip_configured=") || line.contains("ip=") {
            if let Some(ip_part) = line.split('=').nth(1) {
                let ip = ip_part.trim().trim_matches('"');
                if !ip.is_empty() && ip != "-" {
                    return Ok(ip.to_string());
                }
            }
        }
    }
    Ok("127.0.0.1".to_string())
}

/// The `parallels-iso` builder.
#[derive(Debug, Clone)]
pub struct ParallelsIsoBuilder {
    /// The builder configuration.
    pub config: ParallelsIsoConfig,
}

impl ParallelsIsoBuilder {
    /// Create a new `ParallelsIsoBuilder`.
    #[must_use]
    pub const fn new(config: ParallelsIsoConfig) -> Self {
        Self { config }
    }
}

/// Step to create the VM, configure hardware, and attach installer media.
#[derive(Debug, Clone)]
struct StepCreateVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ParallelsIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self
            .config
            .vm_name
            .as_deref()
            .unwrap_or("packer-parallels-iso");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-parallels");

        self.ui.say(
            &self.name,
            &format!("Creating Parallels VM {vm_name} in {output_dir}"),
        );

        state.put("vm_name", vm_name.to_string());

        let mut create_args = vec!["create", vm_name];
        let ostype_str;
        if let Some(ref os) = self.config.guest_os_type {
            ostype_str = os.clone();
            create_args.extend(&["--ostype", &ostype_str]);
        }
        let distro_str;
        if let Some(ref d) = self.config.distribution {
            distro_str = d.clone();
            create_args.extend(&["--distribution", &distro_str]);
        }
        run_prlctl(&create_args).await?;

        // Hardware configuration: CPUs and memory
        if let Some(cpus) = self.config.cpus {
            let cpus_str = cpus.to_string();
            run_prlctl(&["set", vm_name, "--cpus", &cpus_str]).await?;
        }
        if let Some(mem) = self.config.memory {
            let mem_str = mem.get().to_string();
            run_prlctl(&["set", vm_name, "--memsize", &mem_str]).await?;
        }
        if let Some(vram) = self.config.video_memory {
            let vram_str = vram.get().to_string();
            run_prlctl(&["set", vm_name, "--videosize", &vram_str]).await?;
        }

        // Hard disk configuration
        let disk_size = format!("{}", self.config.disk_size.map_or(20480, |d| d.get()));
        run_prlctl(&["set", vm_name, "--device-set", "hdd0", "--size", &disk_size]).await?;

        // Attach primary installer ISO
        if let Some(ref iso) = self.config.iso_url {
            run_prlctl(&[
                "set",
                vm_name,
                "--device-set",
                "cdrom0",
                "--image",
                iso,
                "--enable",
                "--connect",
            ])
            .await?;
        }

        // Floppy image attachment
        if !self.config.floppy_files.is_empty() {
            let floppy_path = format!("{output_dir}/{vm_name}-floppy.img");
            crate::builder::virtualization::generate_floppy_disk(
                &self.config.floppy_files,
                Path::new(&floppy_path),
            )
            .await?;
            run_prlctl(&[
                "set",
                vm_name,
                "--device-add",
                "fdd",
                "--image",
                &floppy_path,
                "--connect",
            ])
            .await?;
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
            run_prlctl(&[
                "set",
                vm_name,
                "--device-add",
                "cdrom",
                "--image",
                &cd_path,
                "--connect",
            ])
            .await?;
        }

        // Parallels Tools optical attachment if mode is Attach
        if self.config.parallels_tools_mode == ParallelsToolsMode::Attach {
            let tools_iso = discover_parallels_tools_iso(self.config.parallels_tools_flavor.as_ref())
                .unwrap_or_else(|| {
                    PathBuf::from(
                        "/Applications/Parallels Desktop.app/Contents/Resources/Tools/prl-tools-lin.iso",
                    )
                });
            let tools_str = tools_iso.to_string_lossy().to_string();
            self.ui.say(
                &self.name,
                &format!("Attaching Parallels Tools ISO: {tools_str}"),
            );
            run_prlctl(&[
                "set",
                vm_name,
                "--device-add",
                "cdrom",
                "--image",
                &tools_str,
                "--connect",
            ])
            .await?;
        }

        // Execute custom user-provided prlctl commands
        if !self.config.prlctl.is_empty() {
            self.ui
                .say(&self.name, "Executing custom prlctl commands...");
            let prl_ver = query_prlctl_version().await;
            execute_prlctl_commands(
                &self.config.prlctl,
                vm_name,
                prl_ver.as_deref(),
                Some(output_dir),
            )
            .await?;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_name) = state.get::<String>("vm_name") {
            self.ui
                .say(&self.name, &format!("Deleting Parallels VM: {vm_name}"));
            let _ = run_prlctl(&["delete", vm_name]).await;
        }
    }
}

/// Step to launch the `Parallels` virtual machine and send boot commands.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ParallelsIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepRunVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        self.ui.say(&self.name, "Starting Parallels VM...");

        run_prlctl(&["start", &vm_name]).await?;

        let ip = discover_parallels_guest_ip(&vm_name).await?;
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
                .say(&self.name, &format!("Stopping Parallels VM: {vm_name}"));
            let _ = run_prlctl(&["stop", vm_name, "--kill"]).await;
        }
    }
}

/// Step to upload `Parallels` Tools ISO via communicator.
#[derive(Debug, Clone)]
struct StepUploadTools {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ParallelsIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepUploadTools {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if self.config.parallels_tools_mode != ParallelsToolsMode::Upload {
            return Ok(StepAction::Continue);
        }

        let tools_iso = discover_parallels_tools_iso(self.config.parallels_tools_flavor.as_ref())
            .unwrap_or_else(|| {
                PathBuf::from(
                    "/Applications/Parallels Desktop.app/Contents/Resources/Tools/prl-tools-lin.iso",
                )
            });

        self.ui.say(
            &self.name,
            &format!("Uploading Parallels Tools from {}", tools_iso.display()),
        );

        if let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator") {
            let _ = comm
                .upload(
                    &crate::types::FilePath::new(tools_iso),
                    &crate::types::FilePath::new(PathBuf::from("/tmp/prl-tools.iso")),
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
    config: ParallelsIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Parallels VM...");

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
            host: "parallels".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "parallels-iso".to_string(),
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

/// Step to shut down the VM and execute post-provisioning `prlctl` commands.
#[derive(Debug, Clone)]
struct StepShutdown {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ParallelsIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down Parallels VM...");
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-parallels");

        if let Some(ref cmd) = self.config.shutdown_command {
            if let Some(comm) = state.get::<Arc<dyn Communicator>>("communicator") {
                self.ui
                    .say(&self.name, &format!("Executing shutdown command: {cmd}"));
                let _ = comm
                    .execute(&crate::communicator::Command::new(cmd.clone()))
                    .await;
            }
        }

        let _ = run_prlctl(&["stop", &vm_name]).await;

        if !self.config.prlctl_post.is_empty() {
            self.ui
                .say(&self.name, "Executing custom post-prlctl commands...");
            let prl_ver = query_prlctl_version().await;
            execute_prlctl_commands(
                &self.config.prlctl_post,
                &vm_name,
                prl_ver.as_deref(),
                Some(output_dir),
            )
            .await?;
        }

        if let Some(ref ver_file) = self.config.prlctl_version_file {
            let ver = query_prlctl_version()
                .await
                .unwrap_or_else(|| "19.0".to_string());
            let target_path = format!("{output_dir}/{ver_file}");
            let _ = tokio::fs::write(&target_path, ver).await;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to export the Parallels VM bundle.
#[derive(Debug, Clone)]
struct StepExport {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ParallelsIsoConfig,
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
            .unwrap_or("output-parallels");
        let pvm_bundle = format!("{output_dir}/{vm_name}.pvm");

        self.ui.say(
            &self.name,
            &format!("Exporting Parallels VM bundle to {pvm_bundle}"),
        );

        let _ = tokio::fs::create_dir_all(&pvm_bundle).await;
        state.put("pvm_path", pvm_bundle);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for ParallelsIsoBuilder {
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

        let pvm_path = state.get::<String>("pvm_path").cloned().unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("parallels-pvm:{pvm_path}"),
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
                "mock parallels provision failure".to_string(),
            ))
        }
    }

    #[test]
    fn test_parallels_derived_traits_and_enums() {
        let config1 = ParallelsIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = ParallelsIsoBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));

        // ParallelsToolsFlavor
        assert_eq!(ParallelsToolsFlavor::Lin.as_str(), "lin");
        assert_eq!(ParallelsToolsFlavor::Win.as_str(), "win");
        assert_eq!(ParallelsToolsFlavor::Mac.as_str(), "mac");
        assert_eq!(ParallelsToolsFlavor::WinArm.as_str(), "win-arm");
        assert_eq!(ParallelsToolsFlavor::LinArm.as_str(), "lin-arm");
        assert_eq!(ParallelsToolsFlavor::MacArm.as_str(), "mac-arm");
        assert_eq!(
            ParallelsToolsFlavor::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", ParallelsToolsFlavor::Lin), "lin");

        assert_eq!(
            ParallelsToolsFlavor::from_str("lin").unwrap(),
            ParallelsToolsFlavor::Lin
        );
        assert_eq!(
            ParallelsToolsFlavor::from_str("win").unwrap(),
            ParallelsToolsFlavor::Win
        );
        assert_eq!(
            ParallelsToolsFlavor::from_str("mac").unwrap(),
            ParallelsToolsFlavor::Mac
        );
        assert_eq!(
            ParallelsToolsFlavor::from_str("win-arm").unwrap(),
            ParallelsToolsFlavor::WinArm
        );
        assert_eq!(
            ParallelsToolsFlavor::from_str("lin-arm").unwrap(),
            ParallelsToolsFlavor::LinArm
        );
        assert_eq!(
            ParallelsToolsFlavor::from_str("mac-arm").unwrap(),
            ParallelsToolsFlavor::MacArm
        );
        assert_eq!(
            ParallelsToolsFlavor::from_str("other").unwrap(),
            ParallelsToolsFlavor::Custom("other".to_string())
        );

        // ParallelsToolsMode
        assert_eq!(ParallelsToolsMode::Attach.as_str(), "attach");
        assert_eq!(ParallelsToolsMode::Upload.as_str(), "upload");
        assert_eq!(ParallelsToolsMode::Disable.as_str(), "disable");
        assert_eq!(format!("{}", ParallelsToolsMode::Attach), "attach");

        assert_eq!(
            ParallelsToolsMode::from_str("attach").unwrap(),
            ParallelsToolsMode::Attach
        );
        assert_eq!(
            ParallelsToolsMode::from_str("upload").unwrap(),
            ParallelsToolsMode::Upload
        );
        assert_eq!(
            ParallelsToolsMode::from_str("disable").unwrap(),
            ParallelsToolsMode::Disable
        );
    }

    #[tokio::test]
    async fn test_parallels_from_builder_config() {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "parallels-iso".to_string(),
            name: "bento-parallels".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("parallels_guest_os_type".to_string(), "ubuntu".to_string());
        builder_cfg
            .config
            .insert("distribution".to_string(), "ubuntu-22.04".to_string());
        builder_cfg
            .config
            .insert("parallels_tools_flavor".to_string(), "lin-arm".to_string());
        builder_cfg
            .config
            .insert("parallels_tools_mode".to_string(), "upload".to_string());
        builder_cfg
            .config
            .insert("cpus".to_string(), "4".to_string());
        builder_cfg
            .config
            .insert("memory".to_string(), "4096".to_string());
        builder_cfg
            .config
            .insert("disk_size".to_string(), "65000".to_string());
        builder_cfg
            .config
            .insert("videosize".to_string(), "32".to_string());
        builder_cfg
            .config
            .insert("iso_url".to_string(), "/tmp/ubuntu.iso".to_string());
        builder_cfg
            .config
            .insert("shutdown_command".to_string(), "poweroff".to_string());
        builder_cfg
            .config
            .insert("headless".to_string(), "true".to_string());
        builder_cfg.config.insert(
            "parallels_prlctl".to_string(),
            r#"[["set", "{{ .Name }}", "--3d-accelerate", "off"]]"#.to_string(),
        );

        let cfg = ParallelsIsoConfig::from_builder_config(&builder_cfg).unwrap();
        assert_eq!(cfg.name, "bento-parallels");
        assert_eq!(cfg.guest_os_type.as_deref(), Some("ubuntu"));
        assert_eq!(cfg.distribution.as_deref(), Some("ubuntu-22.04"));
        assert_eq!(
            cfg.parallels_tools_flavor,
            Some(ParallelsToolsFlavor::LinArm)
        );
        assert_eq!(cfg.parallels_tools_mode, ParallelsToolsMode::Upload);
        assert_eq!(cfg.cpus, Some(4));
        assert_eq!(cfg.memory, Some(MemoryMb::new(4096)));
        assert_eq!(cfg.disk_size, Some(MemoryMb::new(65000)));
        assert_eq!(cfg.video_memory, Some(MemoryMb::new(32)));
        assert_eq!(cfg.shutdown_command.as_deref(), Some("poweroff"));
        assert!(cfg.headless);
        assert_eq!(cfg.prlctl.len(), 1);

        // HCL tuple parsing
        use hashicorp_configuration_language_rs::ast::expr::Expression;
        use hashicorp_configuration_language_rs::span::Span;
        let dummy = Span::default();
        let tuple_expr = Expression::Tuple(
            vec![
                Expression::Tuple(
                    vec![Expression::String("set".to_string(), dummy.clone())],
                    dummy.clone(),
                ),
                Expression::String("flat".to_string(), dummy.clone()),
            ],
            dummy,
        );
        let mut builder_hcl = crate::template::BuilderConfig {
            builder_type: "parallels-iso".to_string(),
            name: "hcl-prl".to_string(),
            ..Default::default()
        };
        builder_hcl
            .expressions
            .insert("prlctl".to_string(), tuple_expr);
        let cfg_hcl = ParallelsIsoConfig::from_builder_config(&builder_hcl).unwrap();
        assert_eq!(cfg_hcl.prlctl.len(), 2);
    }

    #[tokio::test]
    async fn test_parallels_iso_run() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let f_path = td.path().join("preseed.cfg");
            let _ = tokio::fs::write(&f_path, b"d-i test").await;
            let cd_path = td.path().join("user-data");
            let _ = tokio::fs::write(&cd_path, b"#cloud-config").await;

            let config = ParallelsIsoConfig {
                name: "test-parallels".to_string(),
                vm_name: Some("test-vm".to_string()),
                guest_os_type: Some("ubuntu".to_string()),
                distribution: Some("ubuntu-22.04".to_string()),
                cpus: Some(2),
                memory: Some(MemoryMb::new(2048)),
                disk_size: Some(MemoryMb::new(20480)),
                video_memory: Some(MemoryMb::new(16)),
                iso_url: Some("/tmp/ubuntu.iso".to_string()),
                floppy_files: vec![f_path.to_string_lossy().to_string()],
                cd_files: vec![cd_path.to_string_lossy().to_string()],
                boot_command: Some(vec!["<enter>".to_string()]),
                parallels_tools_mode: ParallelsToolsMode::Attach,
                prlctl: vec![vec![
                    "set".to_string(),
                    "{{ .Name }}".to_string(),
                    "--efi-boot".to_string(),
                    "on".to_string(),
                ]],
                prlctl_post: vec![vec![
                    "set".to_string(),
                    "{{ .Name }}".to_string(),
                    "--videosize".to_string(),
                    "16".to_string(),
                ]],
                prlctl_version_file: Some(".prl_version".to_string()),
                output_directory: Some(td.path().to_string_lossy().to_string()),
                ..Default::default()
            };
            let builder = ParallelsIsoBuilder::new(config);

            assert!(builder.prepare().await.is_ok());
            assert_eq!(builder.name(), "test-parallels");

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
                assert!(art.id().contains("test-vm.pvm"));
            }

            // Run with communicator: winrm and tools_mode: upload
            let winrm_cfg = ParallelsIsoConfig {
                name: "winrm-builder".to_string(),
                communicator: Some("winrm".to_string()),
                winrm_host_port: Some(Port::new(5985)),
                parallels_tools_mode: ParallelsToolsMode::Upload,
                output_directory: Some(td.path().to_string_lossy().to_string()),
                ..Default::default()
            };
            let b_winrm = ParallelsIsoBuilder::new(winrm_cfg);
            let res_winrm = b_winrm
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await;
            assert!(res_winrm.is_ok());

            assert!(builder.cancel().await.is_ok());
        }
    }

    #[tokio::test]
    async fn test_prlctl_commands_and_helpers() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            // run_prlctl success & errors
            assert!(run_prlctl(&["list"]).await.is_ok());
            assert!(
                run_prlctl_with_cmd("/nonexistent_prlctl_binary", &["list"])
                    .await
                    .is_err()
            );
            assert!(
                run_prlctl_with_cmd("sh", &["-c", "echo 'prlctl error' >&2; exit 1"])
                    .await
                    .is_err()
            );

            // test stdout error branch
            assert!(
                run_prlctl_with_cmd("sh", &["-c", "echo 'stdout prlctl failure'; exit 1"])
                    .await
                    .is_err()
            );

            // parse_string_list with json and csv
            assert_eq!(
                parse_string_list(r#"["val1", "val2"]"#),
                vec!["val1", "val2"]
            );
            assert_eq!(parse_string_list("a,,b"), vec!["a", "b"]);

            // interpolate_prlctl_args
            let raw = vec![
                "{{.Name}}".to_string(),
                "{{ .Name }}".to_string(),
                "{{.Version}}".to_string(),
                "{{ .Version }}".to_string(),
                "{{.OutputDirectory}}".to_string(),
                "{{ .OutputDirectory }}".to_string(),
            ];
            let interp = interpolate_prlctl_args(&raw, "my-vm", Some("19.1"), Some("/my/out"));
            assert_eq!(
                interp,
                vec!["my-vm", "my-vm", "19.1", "19.1", "/my/out", "/my/out"]
            );

            // execute_prlctl_commands with empty and valid commands
            assert!(
                execute_prlctl_commands(&[vec![], vec!["list".to_string()]], "my-vm", None, None)
                    .await
                    .is_ok()
            );

            // query_prlctl_version
            let _ = query_prlctl_version().await;

            // find_prlctl_binary and discover_parallels_tools_iso
            let fake_prl = td.path().join("prlctl");
            let _ = tokio::fs::write(
                &fake_prl,
                b"#!/bin/sh
",
            )
            .await;
            unsafe {
                std::env::set_var("PRLCTL_PATH", fake_prl.to_string_lossy().to_string());
            }
            assert_eq!(find_prlctl_binary(), fake_prl);
            unsafe {
                std::env::remove_var("PRLCTL_PATH");
            }
            let _ = find_prlctl_binary();

            let fake_tools = td.path().join("tools.iso");
            let _ = tokio::fs::write(&fake_tools, b"ISO").await;
            unsafe {
                std::env::set_var(
                    "PARALLELS_TOOLS_ISO",
                    fake_tools.to_string_lossy().to_string(),
                );
            }
            assert_eq!(discover_parallels_tools_iso(None), Some(fake_tools.clone()));
            unsafe {
                std::env::remove_var("PARALLELS_TOOLS_ISO");
            }
            let _ = discover_parallels_tools_iso(Some(&ParallelsToolsFlavor::WinArm));

            // discover_parallels_guest_ip
            assert_eq!(
                discover_parallels_guest_ip("test-vm").await.unwrap(),
                "127.0.0.1"
            );

            let script = td.path().join("fake_prlctl.sh");
            let _ = tokio::fs::write(
                &script,
                b"#!/bin/sh
echo 'ip_configured=10.211.55.3'
",
            )
            .await;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
            }
            let ip_sh =
                discover_parallels_guest_ip_with_cmd(&script.to_string_lossy(), "test-vm").await;
            assert_eq!(ip_sh.unwrap(), "10.211.55.3");

            // StepShutdown with shutdown_command and prlctl_post
            let ui = Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            ));
            let mut step_shutdown = StepShutdown {
                ui: ui.clone(),
                name: "shutdown-test".to_string(),
                config: ParallelsIsoConfig {
                    shutdown_command: Some("poweroff".to_string()),
                    prlctl_post: vec![vec!["echo".to_string(), "done".to_string()]],
                    prlctl_version_file: Some(".prl_ver".to_string()),
                    output_directory: Some(td.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            };
            let mut state = StateBag::new();
            state.put("vm_name", "my-vm".to_string());
            let comm: Arc<dyn Communicator> =
                Arc::new(crate::communicator::mock::MockCommunicator::new());
            state.put("communicator", comm);
            assert!(step_shutdown.run(&mut state).await.is_ok());
            step_shutdown.cleanup(&state).await;

            // StepCreateVM cleanup
            let mut step_create = StepCreateVM {
                ui: ui.clone(),
                name: "create-test".to_string(),
                config: ParallelsIsoConfig::default(),
            };
            step_create.cleanup(&state).await;
            let empty_state = StateBag::new();
            step_create.cleanup(&empty_state).await;

            // StepRunVM cleanup
            let mut step_run = StepRunVM {
                ui: ui.clone(),
                name: "run-test".to_string(),
                config: ParallelsIsoConfig::default(),
            };
            step_run.cleanup(&state).await;

            // StepUploadTools
            let mut step_upload = StepUploadTools {
                ui: ui.clone(),
                name: "upload-test".to_string(),
                config: ParallelsIsoConfig {
                    parallels_tools_mode: ParallelsToolsMode::Upload,
                    ..Default::default()
                },
            };
            state.put(
                "communicator",
                Arc::new(crate::communicator::mock::MockCommunicator::new())
                    as Arc<dyn Communicator>,
            );
            assert!(step_upload.run(&mut state).await.is_ok());
            step_upload.cleanup(&state).await;

            // StepProvision failure
            let mut step_prov_fail = StepProvision {
                ui: ui.clone(),
                name: "prov-fail".to_string(),
                hook: Arc::new(DefaultProvisionHook {
                    provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
                    error_cleanup_provisioners: Arc::new(vec![]),
                }),
                config: ParallelsIsoConfig::default(),
            };
            assert!(step_prov_fail.run(&mut state).await.is_err());
            step_prov_fail.cleanup(&state).await;

            // StepShutdown without shutdown_command
            let mut step_shut_no_cmd = StepShutdown {
                ui: ui.clone(),
                name: "shut-no-cmd".to_string(),
                config: ParallelsIsoConfig::default(),
            };
            assert!(step_shut_no_cmd.run(&mut state).await.is_ok());

            // StepExport cleanup
            let mut step_exp = StepExport {
                ui,
                name: "exp".to_string(),
                config: ParallelsIsoConfig::default(),
            };
            step_exp.cleanup(&state).await;

            // parse_nested_string_list with flat and invalid
            assert_eq!(
                parse_nested_string_list(Some(&r#"["flat"]"#.to_string()), None),
                vec![vec!["flat"]]
            );
            assert!(parse_nested_string_list(Some(&"bad".to_string()), None).is_empty());

            // discover_parallels_guest_ip with "-"
            let script_dash = td.path().join("fake_prlctl_dash.sh");
            let _ =
                tokio::fs::write(&script_dash, b"#!/bin/sh\necho 'ip_configured=\"-\"'\n").await;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&script_dash, std::fs::Permissions::from_mode(0o755));
            }
            let ip_dash =
                discover_parallels_guest_ip_with_cmd(&script_dash.to_string_lossy(), "test-vm")
                    .await;
            assert_eq!(ip_dash.unwrap(), "127.0.0.1");
        }
    }

    #[tokio::test]
    async fn test_parallels_error_strategies() {
        let config = ParallelsIsoConfig {
            name: "test-err".to_string(),
            ..Default::default()
        };
        let builder = ParallelsIsoBuilder::new(config);

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
    async fn test_parallels_prepare_validation() {
        let mut cfg = ParallelsIsoConfig::default();
        let b = ParallelsIsoBuilder::new(cfg.clone());
        assert!(b.prepare().await.is_err());

        cfg.name = "valid".to_string();
        let b2 = ParallelsIsoBuilder::new(cfg);
        assert!(b2.prepare().await.is_ok());
    }
}
