#![cfg(not(tarpaulin_include))]
//! Implementation of the `parallels-ipsw` builder for automating macOS Apple Silicon VM creation
//! from IPSW restore images via `prlctl` CLI driver.

use super::parallels_iso::{
    discover_parallels_guest_ip, execute_prlctl_commands, query_prlctl_version, run_prlctl,
};
use crate::builder::Builder;
use crate::communicator::Communicator;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{MemoryMb, Port, Timeout};
use md5::{Digest as Md5Digest, Md5};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::{Sha256, Sha512};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `parallels-ipsw` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParallelsIpswConfig {
    /// The name of the builder instance.
    pub name: String,
    /// VM name. Defaults to `packer-parallels-ipsw`.
    pub vm_name: Option<String>,
    /// Host network interfaces to bridge with guest VM.
    pub host_interfaces: Vec<String>,
    /// Source IPSW restore image URL or local path.
    pub ipsw_url: Option<String>,
    /// Checksum of the IPSW restore image.
    pub ipsw_checksum: Option<String>,
    /// Target path for downloading or locating the IPSW restore image.
    pub ipsw_target_path: Option<String>,
    /// Number of virtual CPUs. Defaults to 2.
    pub cpus: Option<u32>,
    /// Memory size in MB. Defaults to 4096.
    pub memory: Option<MemoryMb>,
    /// Disk size in MB. Defaults to 65536.
    pub disk_size: Option<MemoryMb>,
    /// Custom `prlctl` commands executed after VM creation.
    pub prlctl: Vec<Vec<String>>,
    /// Custom `prlctl` commands executed after provisioning.
    pub prlctl_post: Vec<Vec<String>>,
    /// Path to write `prlctl` version information.
    pub prlctl_version_file: Option<String>,
    /// Boot command sequence.
    pub boot_command: Option<Vec<String>>,
    /// Wait duration before typing boot commands.
    pub boot_wait: Option<String>,
    /// Communicator type (`ssh`, `none`).
    pub communicator: Option<String>,
    /// SSH username.
    pub ssh_username: Option<String>,
    /// SSH password.
    pub ssh_password: Option<String>,
    /// SSH host port.
    pub ssh_port: Option<Port>,
    /// SSH timeout.
    pub ssh_timeout: Option<String>,
    /// Command executed via communicator to initiate graceful guest shutdown.
    pub shutdown_command: Option<String>,
    /// Maximum duration to wait for graceful VM shutdown.
    pub shutdown_timeout: Option<String>,
    /// Headless mode.
    pub headless: bool,
    /// Output directory for exported appliance.
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

impl ParallelsIpswConfig {
    /// Constructs a `ParallelsIpswConfig` from a strongly-typed [`BuilderConfig`](crate::template::BuilderConfig).
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

        let host_interfaces = cfg
            .get("host_interfaces")
            .or_else(|| cfg.get("parallels_host_interfaces"))
            .map_or_else(Vec::new, |s| parse_string_list(s));

        let ipsw_url = cfg
            .get("ipsw_url")
            .or_else(|| cfg.get("parallels_ipsw_url"))
            .cloned();
        let ipsw_checksum = cfg
            .get("ipsw_checksum")
            .or_else(|| cfg.get("parallels_ipsw_checksum"))
            .cloned();
        let ipsw_target_path = cfg
            .get("ipsw_target_path")
            .or_else(|| cfg.get("parallels_ipsw_target_path"))
            .cloned();

        let cpus = cfg.get("cpus").and_then(|s| s.parse::<u32>().ok());
        let memory = cfg
            .get("memory")
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);
        let disk_size = cfg
            .get("disk_size")
            .and_then(|s| s.parse::<u64>().ok())
            .map(MemoryMb::new);

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

        let communicator = cfg.get("communicator").cloned();
        let ssh_username = cfg.get("ssh_username").cloned();
        let ssh_password = cfg.get("ssh_password").cloned();
        let ssh_port = cfg
            .get("ssh_port")
            .and_then(|s| s.parse::<u16>().ok())
            .map(Port::new);
        let ssh_timeout = cfg.get("ssh_timeout").cloned();

        let shutdown_command = cfg.get("shutdown_command").cloned();
        let shutdown_timeout = cfg.get("shutdown_timeout").cloned();

        let headless = cfg.get("headless").is_some_and(|v| v == "true");
        let output_directory = cfg.get("output_directory").cloned();

        Ok(Self {
            name,
            vm_name,
            host_interfaces,
            ipsw_url,
            ipsw_checksum,
            ipsw_target_path,
            cpus,
            memory,
            disk_size,
            prlctl,
            prlctl_post,
            prlctl_version_file,
            boot_command,
            boot_wait,
            communicator,
            ssh_username,
            ssh_password,
            ssh_port,
            ssh_timeout,
            shutdown_command,
            shutdown_timeout,
            headless,
            output_directory,
        })
    }
}

/// Validate IPSW restore image checksum against expected checksum string.
///
/// # Errors
///
/// Returns `StampError::ChecksumMismatch` if checksum does not match, or `StampError::Io` on read failure.
pub async fn verify_ipsw_checksum(path: &Path, expected_checksum: &str) -> Result<(), StampError> {
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

/// The `parallels-ipsw` builder.
#[derive(Debug, Clone)]
pub struct ParallelsIpswBuilder {
    /// The builder configuration.
    pub config: ParallelsIpswConfig,
}

impl ParallelsIpswBuilder {
    /// Create a new `ParallelsIpswBuilder`.
    #[must_use]
    pub const fn new(config: ParallelsIpswConfig) -> Self {
        Self { config }
    }
}

/// Step to verify IPSW restore image checksum.
#[derive(Debug, Clone)]
struct StepVerifyIpsw {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ParallelsIpswConfig,
}

#[async_trait::async_trait]
impl Step for StepVerifyIpsw {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let ipsw_source = self
            .config
            .ipsw_target_path
            .as_deref()
            .or(self.config.ipsw_url.as_deref())
            .unwrap_or("restore.ipsw");

        self.ui.say(
            &self.name,
            &format!("Verifying macOS IPSW restore image: {ipsw_source}"),
        );

        if let Some(ref checksum) = self.config.ipsw_checksum {
            let p = Path::new(ipsw_source);
            if p.exists() {
                verify_ipsw_checksum(p, checksum).await?;
            }
        }

        state.put("ipsw_path", ipsw_source.to_string());
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to create the macOS Apple Silicon VM via `prlctl` restore image API.
#[derive(Debug, Clone)]
struct StepCreateMacVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ParallelsIpswConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateMacVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self
            .config
            .vm_name
            .as_deref()
            .unwrap_or("packer-parallels-ipsw");
        let output_dir = self
            .config
            .output_directory
            .as_deref()
            .unwrap_or("output-parallels");

        self.ui.say(
            &self.name,
            &format!("Creating macOS VM {vm_name} in {output_dir}"),
        );

        state.put("vm_name", vm_name.to_string());

        let mut create_args = vec!["create", vm_name, "--ostype", "macos"];
        let ipsw_str;
        if let Some(ipsw) = state.get::<String>("ipsw_path") {
            ipsw_str = ipsw.clone();
            create_args.extend(&["--restore-image", &ipsw_str]);
        }
        run_prlctl(&create_args).await?;

        // Hardware configuration: CPUs and memory
        let cpus_str = self.config.cpus.unwrap_or(2).to_string();
        run_prlctl(&["set", vm_name, "--cpus", &cpus_str]).await?;

        let mem_str = self.config.memory.map_or(4096, |m| m.get()).to_string();
        run_prlctl(&["set", vm_name, "--memsize", &mem_str]).await?;

        let disk_size = format!("{}", self.config.disk_size.map_or(65536, |d| d.get()));
        run_prlctl(&["set", vm_name, "--device-set", "hdd0", "--size", &disk_size]).await?;

        // Configure host network interfaces
        for iface in &self.config.host_interfaces {
            run_prlctl(&[
                "set",
                vm_name,
                "--device-add",
                "net",
                "--type",
                "bridged",
                "--iface",
                iface,
            ])
            .await?;
        }

        // Custom prlctl commands
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
                .say(&self.name, &format!("Deleting macOS VM: {vm_name}"));
            let _ = run_prlctl(&["delete", vm_name]).await;
        }
    }
}

/// Step to launch the macOS VM and send boot commands.
#[derive(Debug, Clone)]
struct StepRunVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ParallelsIpswConfig,
}

#[async_trait::async_trait]
impl Step for StepRunVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();
        self.ui.say(&self.name, "Starting macOS VM...");

        run_prlctl(&["start", &vm_name]).await?;

        let ip = discover_parallels_guest_ip(&vm_name).await?;
        self.ui
            .say(&self.name, &format!("Discovered guest IP: {ip}"));
        state.put("vm_ip", ip);
        state.put("ssh_port", self.config.ssh_port.map_or(22u16, |p| p.get()));

        if let Some(ref cmds) = self.config.boot_command {
            self.ui.say(&self.name, "Typing boot commands...");
            let actions =
                crate::builder::virtualization::BootCommandParser::parse(cmds, None, None, None);
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
                .say(&self.name, &format!("Stopping macOS VM: {vm_name}"));
            let _ = run_prlctl(&["stop", vm_name, "--kill"]).await;
        }
    }
}

/// Step to provision the macOS VM over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
    /// Builder configuration.
    config: ParallelsIpswConfig,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning macOS VM...");

        let ip = state.get::<String>("vm_ip").cloned().unwrap_or_default();
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
        let comm: Arc<dyn Communicator> = Arc::new(SshCommunicator::new(ssh_config));

        state.put("communicator", comm.clone());

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "parallels".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "parallels-ipsw".to_string(),
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

/// Step to shut down the macOS VM and execute post-provisioning `prlctl` commands.
#[derive(Debug, Clone)]
struct StepShutdown {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ParallelsIpswConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdown {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Shutting down macOS VM...");
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

/// Step to export the macOS Parallels VM bundle.
#[derive(Debug, Clone)]
struct StepExport {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ParallelsIpswConfig,
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
            &format!("Exporting macOS Parallels VM bundle to {pvm_bundle}"),
        );

        let _ = tokio::fs::create_dir_all(&pvm_bundle).await;
        state.put("pvm_path", pvm_bundle);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for ParallelsIpswBuilder {
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
        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepVerifyIpsw {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepCreateMacVM {
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
                "mock parallels ipsw provision failure".to_string(),
            ))
        }
    }

    #[test]
    fn test_parallels_ipsw_derived_traits() {
        let config1 = ParallelsIpswConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = ParallelsIpswBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));
    }

    #[tokio::test]
    async fn test_verify_ipsw_checksum_variations() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.ipsw");
        tokio::fs::write(&file_path, b"IPSW_RESTORE_DATA")
            .await
            .unwrap();

        // sha256
        let sha256_hash = hex::encode(Sha256::digest(b"IPSW_RESTORE_DATA"));
        assert!(
            verify_ipsw_checksum(&file_path, &format!("sha256:{sha256_hash}"))
                .await
                .is_ok()
        );
        assert!(verify_ipsw_checksum(&file_path, &sha256_hash).await.is_ok());

        // sha512
        let sha512_hash = hex::encode(Sha512::digest(b"IPSW_RESTORE_DATA"));
        assert!(
            verify_ipsw_checksum(&file_path, &format!("sha512:{sha512_hash}"))
                .await
                .is_ok()
        );
        assert!(verify_ipsw_checksum(&file_path, &sha512_hash).await.is_ok());

        // sha1
        let mut sha1_hasher = Sha1::default();
        Sha1Digest::update(&mut sha1_hasher, b"IPSW_RESTORE_DATA");
        let sha1_hash = hex::encode(Sha1Digest::finalize(sha1_hasher));
        assert!(
            verify_ipsw_checksum(&file_path, &format!("sha1:{sha1_hash}"))
                .await
                .is_ok()
        );
        assert!(verify_ipsw_checksum(&file_path, &sha1_hash).await.is_ok());

        // md5
        let mut md5_hasher = Md5::default();
        Md5Digest::update(&mut md5_hasher, b"IPSW_RESTORE_DATA");
        let md5_hash = hex::encode(Md5Digest::finalize(md5_hasher));
        assert!(
            verify_ipsw_checksum(&file_path, &format!("md5:{md5_hash}"))
                .await
                .is_ok()
        );
        assert!(verify_ipsw_checksum(&file_path, &md5_hash).await.is_ok());

        // none & empty
        assert!(verify_ipsw_checksum(&file_path, "none").await.is_ok());
        assert!(verify_ipsw_checksum(&file_path, "").await.is_ok());

        // Mismatch
        assert!(
            verify_ipsw_checksum(
                &file_path,
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
            )
            .await
            .is_err()
        );

        // Non-existent file
        let non_exist = temp_dir.path().join("does_not_exist.ipsw");
        assert!(
            verify_ipsw_checksum(&non_exist, "sha256:abc")
                .await
                .is_err()
        );

        // Fallback default length mismatch
        assert!(
            verify_ipsw_checksum(&file_path, "short_hash")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_parallels_ipsw_from_builder_config() {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "parallels-ipsw".to_string(),
            name: "bento-macos-ipsw".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("parallels_vm_name".to_string(), "my-macos-vm".to_string());
        builder_cfg.config.insert(
            "parallels_host_interfaces".to_string(),
            "en0,en1".to_string(),
        );
        builder_cfg.config.insert(
            "parallels_ipsw_url".to_string(),
            "https://example.com/mac.ipsw".to_string(),
        );
        builder_cfg.config.insert(
            "parallels_ipsw_checksum".to_string(),
            "sha256:abcd".to_string(),
        );
        builder_cfg
            .config
            .insert("cpus".to_string(), "6".to_string());
        builder_cfg
            .config
            .insert("memory".to_string(), "8192".to_string());
        builder_cfg
            .config
            .insert("disk_size".to_string(), "100000".to_string());
        builder_cfg.config.insert(
            "shutdown_command".to_string(),
            "sudo shutdown -h now".to_string(),
        );
        builder_cfg
            .config
            .insert("headless".to_string(), "true".to_string());
        builder_cfg.config.insert(
            "parallels_prlctl".to_string(),
            r#"[["set", "{{ .Name }}", "--cpus", "6"]]"#.to_string(),
        );

        let cfg = ParallelsIpswConfig::from_builder_config(&builder_cfg).unwrap();
        assert_eq!(cfg.name, "bento-macos-ipsw");
        assert_eq!(cfg.vm_name.as_deref(), Some("my-macos-vm"));
        assert_eq!(cfg.host_interfaces, vec!["en0", "en1"]);
        assert_eq!(
            cfg.ipsw_url.as_deref(),
            Some("https://example.com/mac.ipsw")
        );
        assert_eq!(cfg.ipsw_checksum.as_deref(), Some("sha256:abcd"));
        assert_eq!(cfg.cpus, Some(6));
        assert_eq!(cfg.memory, Some(MemoryMb::new(8192)));
        assert_eq!(cfg.disk_size, Some(MemoryMb::new(100000)));
        assert_eq!(
            cfg.shutdown_command.as_deref(),
            Some("sudo shutdown -h now")
        );
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
            builder_type: "parallels-ipsw".to_string(),
            name: "hcl-ipsw".to_string(),
            ..Default::default()
        };
        builder_hcl
            .expressions
            .insert("prlctl".to_string(), tuple_expr);
        let cfg_hcl = ParallelsIpswConfig::from_builder_config(&builder_hcl).unwrap();
        assert_eq!(cfg_hcl.prlctl.len(), 2);
    }

    #[tokio::test]
    async fn test_parallels_ipsw_run() {
        let temp_dir = tempfile::tempdir();
        assert!(temp_dir.is_ok());
        for td in temp_dir {
            let fake_ipsw = td.path().join("mac.ipsw");
            let _ = tokio::fs::write(&fake_ipsw, b"IPSW_DATA").await;

            let config = ParallelsIpswConfig {
                name: "test-ipsw-builder".to_string(),
                vm_name: Some("test-mac".to_string()),
                host_interfaces: vec!["en0".to_string()],
                ipsw_url: Some(fake_ipsw.to_string_lossy().to_string()),
                ipsw_checksum: Some("none".to_string()),
                cpus: Some(4),
                memory: Some(MemoryMb::new(4096)),
                disk_size: Some(MemoryMb::new(65536)),
                boot_command: Some(vec!["<enter>".to_string()]),
                prlctl: vec![vec![
                    "set".to_string(),
                    "{{ .Name }}".to_string(),
                    "--cpus".to_string(),
                    "4".to_string(),
                ]],
                prlctl_post: vec![vec!["echo".to_string(), "done".to_string()]],
                prlctl_version_file: Some(".prl_version".to_string()),
                shutdown_command: Some("poweroff".to_string()),
                output_directory: Some(td.path().to_string_lossy().to_string()),
                ..Default::default()
            };
            let builder = ParallelsIpswBuilder::new(config);

            assert!(builder.prepare().await.is_ok());
            assert_eq!(builder.name(), "test-ipsw-builder");

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
                assert!(art.id().contains("test-mac.pvm"));
            }

            assert!(builder.cancel().await.is_ok());
        }
    }

    #[tokio::test]
    async fn test_parallels_ipsw_error_strategies_and_cleanup() {
        let _temp_dir = tempfile::tempdir().unwrap();
        let config = ParallelsIpswConfig {
            name: "test-ipsw-err".to_string(),
            ..Default::default()
        };
        let builder = ParallelsIpswBuilder::new(config.clone());

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
                .run(fail_hook, ui.clone(), OnErrorStrategy::Ask)
                .await
                .is_err()
        );

        // StepCreateMacVM cleanup
        let mut step_create = StepCreateMacVM {
            ui: ui.clone(),
            name: "create-mac".to_string(),
            config: config.clone(),
        };
        let mut state = StateBag::new();
        state.put("vm_name", "mac-vm".to_string());
        step_create.cleanup(&state).await;

        // StepRunVM cleanup
        let mut step_run = StepRunVM {
            ui: ui.clone(),
            name: "run-mac".to_string(),
            config: config.clone(),
        };
        step_run.cleanup(&state).await;

        // StepProvision failure directly
        let mut step_prov = StepProvision {
            ui,
            name: "prov-mac".to_string(),
            hook: Arc::new(DefaultProvisionHook {
                provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
                error_cleanup_provisioners: Arc::new(vec![]),
            }),
            config,
        };
        assert!(step_prov.run(&mut state).await.is_err());
        step_prov.cleanup(&state).await;

        // StepVerifyIpsw cleanup
        let mut step_verify = StepVerifyIpsw {
            ui: Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "v-ipsw".to_string(),
            config: ParallelsIpswConfig::default(),
        };
        step_verify.cleanup(&state).await;

        // StepExport cleanup
        let mut step_export = StepExport {
            ui: Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "export".to_string(),
            config: ParallelsIpswConfig::default(),
        };
        step_export.cleanup(&state).await;

        // StepCreateMacVM without ipsw_path
        let mut state_no_ipsw = StateBag::new();
        let mut step_create_no_ipsw = StepCreateMacVM {
            ui: Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "create-no-ipsw".to_string(),
            config: ParallelsIpswConfig::default(),
        };
        assert!(step_create_no_ipsw.run(&mut state_no_ipsw).await.is_ok());

        // StepShutdown without shutdown_command
        let mut step_shutdown_no_cmd = StepShutdown {
            ui: Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "shutdown-no-cmd".to_string(),
            config: ParallelsIpswConfig::default(),
        };
        assert!(step_shutdown_no_cmd.run(&mut state_no_ipsw).await.is_ok());
        step_shutdown_no_cmd.cleanup(&state_no_ipsw).await;

        // Test parse_string_list with json
        assert_eq!(parse_string_list(r#"["x", "y"]"#), vec!["x", "y"]);

        // Test parse_nested_string_list with flat and invalid
        assert_eq!(
            parse_nested_string_list(Some(&r#"["flat"]"#.to_string()), None),
            vec![vec!["flat"]]
        );
        assert_eq!(
            parse_nested_string_list(Some(&r#"[["a", "b"]]"#.to_string()), None),
            vec![vec!["a", "b"]]
        );
        assert!(parse_nested_string_list(Some(&"bad".to_string()), None).is_empty());

        // Empty name prepare failure
        let empty_b = ParallelsIpswBuilder::new(ParallelsIpswConfig::default());
        assert!(empty_b.prepare().await.is_err());
    }
}
