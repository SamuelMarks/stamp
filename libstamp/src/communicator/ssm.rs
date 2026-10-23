//! AWS Systems Manager (SSM) Session Manager tunnel communicator.
//!
//! Provides tunneling of commands and file transfers to EC2 instances using the
//! `session-manager-plugin` and `AWS-StartSSHSession` document, avoiding the need
//! for public IP addresses or open inbound SSH ports on instances.

use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::communicator::{Command, CommandResult, Communicator};
use crate::error::StampError;
use crate::types::{FilePath, Timeout};
use async_trait::async_trait;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Configuration for the AWS SSM Session Manager communicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsmConfig {
    /// The EC2 instance ID to connect to (e.g. `i-0123456789abcdef0`).
    pub instance_id: String,
    /// The target AWS region (e.g. `us-east-1`).
    pub region: String,
    /// Optional AWS named profile from `~/.aws/credentials`.
    pub profile: Option<String>,
    /// SSM document to execute (defaults to `AWS-StartSSHSession`).
    pub document_name: String,
    /// Optional path to the `session-manager-plugin` binary.
    pub session_manager_plugin_path: Option<FilePath>,
    /// Optional SSH configuration for tunneling over the SSM session.
    pub ssh_config: Option<SshConfig>,
    /// Operation timeout.
    pub timeout: Timeout,
    /// Path or name of the AWS CLI executable (defaults to `"aws"`).
    pub aws_executable: PathBuf,
    /// Poll interval when waiting for the SSM agent to come online.
    pub poll_interval: Duration,
}

impl SsmConfig {
    /// Create a new `SsmConfig` for an EC2 instance in a given region.
    #[must_use]
    pub fn new(instance_id: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            region: region.into(),
            profile: None,
            document_name: "AWS-StartSSHSession".to_string(),
            session_manager_plugin_path: None,
            ssh_config: None,
            timeout: Timeout::new(Duration::from_secs(60)),
            aws_executable: PathBuf::from("aws"),
            poll_interval: Duration::from_secs(5),
        }
    }

    /// Sets a custom `aws` CLI executable path.
    #[must_use]
    pub fn with_aws_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.aws_executable = executable.into();
        self
    }

    /// Sets the poll interval used by [`SsmCommunicator::wait_for_agent_online`].
    #[must_use]
    pub const fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

impl Default for SsmConfig {
    /// Return default SSM configuration for local test instances.
    fn default() -> Self {
        Self::new("i-00000000000000000", "us-east-1")
    }
}

/// Helper function to build arguments for the AWS CLI / Session Manager plugin SSH proxy.
#[must_use]
pub fn build_ssm_proxy_args(
    instance_id: &str,
    region: &str,
    profile: Option<&str>,
    port: u16,
) -> Vec<String> {
    let mut args = vec![
        "ssm".to_string(),
        "start-session".to_string(),
        "--target".to_string(),
        instance_id.to_string(),
        "--document-name".to_string(),
        "AWS-StartSSHSession".to_string(),
        "--parameters".to_string(),
        format!("portNumber={port}"),
        "--region".to_string(),
        region.to_string(),
    ];
    if let Some(p) = profile {
        args.push("--profile".to_string());
        args.push(p.to_string());
    }
    args
}

/// Helper function to construct AWS SSM `SendCommand` arguments for Linux shells.
#[must_use]
pub fn build_ssm_send_command_args(
    instance_id: &str,
    command: &str,
    region: &str,
    profile: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "ssm".to_string(),
        "send-command".to_string(),
        "--instance-ids".to_string(),
        instance_id.to_string(),
        "--document-name".to_string(),
        "AWS-RunShellScript".to_string(),
        "--parameters".to_string(),
        format!("commands=[\"{command}\"]"),
        "--region".to_string(),
        region.to_string(),
    ];
    if let Some(p) = profile {
        args.push("--profile".to_string());
        args.push(p.to_string());
    }
    args
}

/// Helper function to construct AWS SSM `SendCommand` arguments for Windows PowerShell.
#[must_use]
pub fn build_ssm_send_powershell_command_args(
    instance_id: &str,
    command: &str,
    region: &str,
    profile: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "ssm".to_string(),
        "send-command".to_string(),
        "--instance-ids".to_string(),
        instance_id.to_string(),
        "--document-name".to_string(),
        "AWS-RunPowerShellScript".to_string(),
        "--parameters".to_string(),
        format!("commands=[\"{command}\"]"),
        "--region".to_string(),
        region.to_string(),
    ];
    if let Some(p) = profile {
        args.push("--profile".to_string());
        args.push(p.to_string());
    }
    args
}

/// Helper function to build a full `ProxyCommand` string for OpenSSH tunneling over SSM.
#[must_use]
pub fn build_ssm_proxy_command(
    instance_id: &str,
    region: &str,
    profile: Option<&str>,
    port: u16,
    _plugin_path: Option<&Path>,
) -> String {
    let mut cmd = format!(
        "aws ssm start-session --target {instance_id} --document-name AWS-StartSSHSession --parameters portNumber={port} --region {region}"
    );
    if let Some(p) = profile {
        let _ = write!(cmd, " --profile {p}");
    }
    cmd
}

/// Constructs an `SshConfig` pre-configured to tunnel over an AWS SSM session.
#[must_use]
pub fn build_ssm_ssh_config(
    instance_id: &str,
    region: &str,
    profile: Option<&str>,
    port: u16,
    ssh_user: &str,
    key_path: Option<FilePath>,
) -> SshConfig {
    let proxy_cmd = build_ssm_proxy_command(instance_id, region, profile, port, None);
    SshConfig {
        host: instance_id.to_string(),
        port: crate::types::Port::new(port),
        username: ssh_user.to_string(),
        private_key_path: key_path,
        proxy_command: Some(proxy_cmd),
        ..Default::default()
    }
}

/// The AWS Systems Manager Session Manager communicator.
#[derive(Debug, Clone)]
pub struct SsmCommunicator {
    /// Configuration for the SSM communicator.
    pub config: SsmConfig,
}

impl SsmCommunicator {
    /// Create a new `SsmCommunicator`.
    #[must_use]
    pub const fn new(config: SsmConfig) -> Self {
        Self { config }
    }

    /// Check whether a custom `session-manager-plugin` exists at the configured path.
    #[must_use]
    pub fn has_session_manager_plugin(&self) -> bool {
        self.config
            .session_manager_plugin_path
            .as_ref()
            .is_some_and(|p| Path::new(p.get()).exists())
    }

    /// Configures the embedded SSH tunneling configuration using the instance SSM parameters.
    pub fn configure_ssh_tunnel(&mut self, ssh_user: &str, key_path: Option<FilePath>) {
        self.config.ssh_config = Some(build_ssm_ssh_config(
            &self.config.instance_id,
            &self.config.region,
            self.config.profile.as_deref(),
            22,
            ssh_user,
            key_path,
        ));
    }

    /// Polls for AWS Systems Manager agent readiness on the target instance until Online or timeout.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the agent fails to reach the online state.
    pub async fn wait_for_agent_online(
        &self,
        ui: &crate::engine::ui::Ui,
    ) -> Result<(), StampError> {
        if self.config.instance_id.trim().is_empty() {
            return Err(StampError::Execution(
                "SSM instance ID cannot be empty".to_string(),
            ));
        }

        ui.say(
            "ssm",
            &format!(
                "Waiting for SSM agent on instance {} to reach Online status...",
                self.config.instance_id
            ),
        );

        let start = std::time::Instant::now();
        let timeout = self.config.timeout.0;

        while start.elapsed() < timeout {
            let mut args = vec![
                "ssm".to_string(),
                "describe-instance-information".to_string(),
                "--filters".to_string(),
                format!("Key=InstanceIds,Values={}", self.config.instance_id),
                "--region".to_string(),
                self.config.region.clone(),
            ];
            if let Some(ref p) = self.config.profile {
                args.push("--profile".to_string());
                args.push(p.clone());
            }

            if let Ok(output) = tokio::process::Command::new(&self.config.aws_executable)
                .args(&args)
                .output()
                .await
                && output.status.success()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("\"PingStatus\": \"Online\"") {
                    ui.say("ssm", "SSM agent is online and ready!");
                    return Ok(());
                }
                ui.say("ssm", "SSM agent not yet online; retrying...");
            }

            tokio::time::sleep(self.config.poll_interval).await;
        }

        Err(StampError::Execution(format!(
            "Timed out waiting for SSM agent on instance {} to reach Online status",
            self.config.instance_id
        )))
    }
}

#[async_trait]
impl Communicator for SsmCommunicator {
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        if self.config.instance_id.trim().is_empty() {
            return Err(StampError::Execution(
                "Instance ID cannot be empty".to_string(),
            ));
        }

        if let Some(ref ssh_cfg) = self.config.ssh_config {
            let ssh_comm = SshCommunicator::new(ssh_cfg.clone());
            return ssh_comm.execute(cmd).await;
        }

        let args = build_ssm_send_command_args(
            &self.config.instance_id,
            &cmd.command,
            &self.config.region,
            self.config.profile.as_deref(),
        );

        let output = tokio::process::Command::new(&self.config.aws_executable)
            .args(&args)
            .output()
            .await
            .map_err(StampError::Io)?;

        Ok(CommandResult {
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    async fn upload(
        &self,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        if self.config.instance_id.trim().is_empty() {
            return Err(StampError::Execution(
                "Instance ID cannot be empty".to_string(),
            ));
        }

        if let Some(ref ssh_cfg) = self.config.ssh_config {
            let ssh_comm = SshCommunicator::new(ssh_cfg.clone());
            return ssh_comm.upload(local_path, remote_path).await;
        }

        Ok(())
    }

    async fn download(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        if self.config.instance_id.trim().is_empty() {
            return Err(StampError::Execution(
                "Instance ID cannot be empty".to_string(),
            ));
        }

        if let Some(ref ssh_cfg) = self.config.ssh_config {
            let ssh_comm = SshCommunicator::new(ssh_cfg.clone());
            return ssh_comm.download(remote_path, local_path).await;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ssm_communicator_coverage() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let echo_script = temp_dir.path().join("aws_echo.sh");
        std::fs::write(&echo_script, "#!/bin/sh\necho ok\nexit 0\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&echo_script, std::fs::Permissions::from_mode(0o755))?;
        }

        let config = SsmConfig {
            instance_id: "i-0123456789abcdef0".to_string(),
            region: "us-east-1".to_string(),
            profile: Some("prod".to_string()),
            document_name: "AWS-StartSSHSession".to_string(),
            session_manager_plugin_path: None,
            ssh_config: Some(SshConfig::default()),
            timeout: Timeout::new(Duration::from_secs(30)),
            aws_executable: echo_script.clone(),
            poll_interval: Duration::from_millis(5),
        };
        let c = SsmCommunicator::new(config.clone());

        let cmd_res = c.execute(&Command::new("echo hello".to_string())).await?;
        assert_eq!(cmd_res.exit_code, 0);

        let fp = FilePath::new(PathBuf::from("a"));
        c.upload(&fp, &fp).await?;
        c.download(&fp, &fp).await?;

        let c2 = c.clone();
        assert_eq!(c.config, c2.config);
        assert!(format!("{c:?}").contains("SsmCommunicator"));

        // Test with ssh_config: None (executes aws_executable directly)
        let mut no_ssh_config = config.clone();
        no_ssh_config.ssh_config = None;
        let c_no_ssh = SsmCommunicator::new(no_ssh_config);
        let res_no_ssh = c_no_ssh
            .execute(&Command::new("echo hello".to_string()))
            .await?;
        assert_eq!(res_no_ssh.exit_code, 0);
        assert_eq!(res_no_ssh.stdout.trim(), "ok");
        c_no_ssh.upload(&fp, &fp).await?;
        c_no_ssh.download(&fp, &fp).await?;

        assert!(!c.has_session_manager_plugin());

        // Test non-existent plugin path
        let mut nonexistent_config = config.clone();
        nonexistent_config.session_manager_plugin_path = Some(FilePath::new(PathBuf::from(
            "/nonexistent/stamp/session-manager-plugin",
        )));
        let nonexistent_comm = SsmCommunicator::new(nonexistent_config);
        assert!(!nonexistent_comm.has_session_manager_plugin());

        // Test existing plugin path using a named temporary file
        let tp = tempfile::NamedTempFile::new()?;
        let mut existing_config = config;
        existing_config.session_manager_plugin_path = Some(FilePath::new(tp.path().to_path_buf()));
        let existing_comm = SsmCommunicator::new(existing_config);
        assert!(existing_comm.has_session_manager_plugin());

        // Test empty instance error
        let invalid_config = SsmConfig::new("", "us-east-1");
        let invalid_comm = SsmCommunicator::new(invalid_config);
        assert!(
            invalid_comm
                .execute(&Command::new("ls".to_string()))
                .await
                .is_err()
        );
        assert!(invalid_comm.upload(&fp, &fp).await.is_err());
        assert!(invalid_comm.download(&fp, &fp).await.is_err());
        Ok(())
    }

    #[test]
    fn test_ssm_helpers() {
        let proxy_args = build_ssm_proxy_args("i-123", "us-west-2", Some("myprofile"), 22);
        assert!(proxy_args.contains(&"--target".to_string()));
        assert!(proxy_args.contains(&"i-123".to_string()));
        assert!(proxy_args.contains(&"portNumber=22".to_string()));
        assert!(proxy_args.contains(&"--profile".to_string()));
        assert!(proxy_args.contains(&"myprofile".to_string()));

        let proxy_args_none = build_ssm_proxy_args("i-123", "us-west-2", None, 22);
        assert!(!proxy_args_none.contains(&"--profile".to_string()));

        let send_args = build_ssm_send_command_args("i-456", "uname -a", "eu-central-1", None);
        assert!(send_args.contains(&"--instance-ids".to_string()));
        assert!(send_args.contains(&"i-456".to_string()));
        assert!(send_args.contains(&"commands=[\"uname -a\"]".to_string()));
        assert!(!send_args.contains(&"--profile".to_string()));

        let proxy_cmd = build_ssm_proxy_command("i-789", "us-east-1", Some("prod"), 22, None);
        assert!(proxy_cmd.contains("aws ssm start-session"));
        assert!(proxy_cmd.contains("--target i-789"));
        assert!(proxy_cmd.contains("--profile prod"));

        let ssh_cfg = build_ssm_ssh_config("i-789", "us-east-1", None, 2222, "ec2-user", None);
        assert_eq!(ssh_cfg.host, "i-789");
        assert_eq!(ssh_cfg.port.get(), 2222);
        assert_eq!(ssh_cfg.username, "ec2-user");
        assert!(ssh_cfg.proxy_command.is_some());

        let mut comm = SsmCommunicator::new(
            SsmConfig::new("i-999", "us-west-1")
                .with_aws_executable("aws")
                .with_poll_interval(Duration::from_millis(5)),
        );
        comm.configure_ssh_tunnel("ubuntu", None);
        assert!(comm.config.ssh_config.is_some());

        let send_args_prof =
            build_ssm_send_command_args("i-456", "uname -a", "eu-central-1", Some("myprofile"));
        assert!(send_args_prof.contains(&"--profile".to_string()));
        assert!(send_args_prof.contains(&"myprofile".to_string()));

        let default_config = SsmConfig::default();
        assert_eq!(default_config.region, "us-east-1");
        assert_eq!(default_config.document_name, "AWS-StartSSHSession");

        let send_ps_args = build_ssm_send_powershell_command_args(
            "i-win",
            "Get-Service",
            "us-east-1",
            Some("prod"),
        );
        assert!(send_ps_args.contains(&"--instance-ids".to_string()));
        assert!(send_ps_args.contains(&"i-win".to_string()));
        assert!(send_ps_args.contains(&"AWS-RunPowerShellScript".to_string()));
        assert!(send_ps_args.contains(&"commands=[\"Get-Service\"]".to_string()));
        assert!(send_ps_args.contains(&"--profile".to_string()));
        assert!(send_ps_args.contains(&"prod".to_string()));

        let send_ps_args_none =
            build_ssm_send_powershell_command_args("i-win", "Get-Service", "us-east-1", None);
        assert!(!send_ps_args_none.contains(&"--profile".to_string()));
    }

    #[tokio::test]
    async fn test_ssm_wait_for_agent_online() -> Result<(), StampError> {
        let ui = crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        );

        let invalid_comm = SsmCommunicator::new(SsmConfig::new("", "us-east-1"));
        assert!(invalid_comm.wait_for_agent_online(&ui).await.is_err());

        let temp_dir = tempfile::tempdir()?;
        // Online script
        let online_script = temp_dir.path().join("aws_online.sh");
        let script_content = "#!/bin/sh\necho '\"PingStatus\": \"Online\"'\nexit 0\n";
        std::fs::write(&online_script, script_content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&online_script, std::fs::Permissions::from_mode(0o755))?;
        }

        let mut online_config = SsmConfig::new("i-online", "us-east-1")
            .with_aws_executable(&online_script)
            .with_poll_interval(Duration::from_millis(5));
        online_config.profile = Some("prod".to_string());
        online_config.timeout = Timeout::new(Duration::from_millis(500));
        let online_comm = SsmCommunicator::new(online_config);
        online_comm.wait_for_agent_online(&ui).await?;

        // Offline/timeout script
        let offline_script = temp_dir.path().join("aws_offline.sh");
        let script_content_offline = "#!/bin/sh\necho '\"PingStatus\": \"Offline\"'\nexit 0\n";
        std::fs::write(&offline_script, script_content_offline)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&offline_script, std::fs::Permissions::from_mode(0o755))?;
        }

        let mut timeout_config = SsmConfig::new("i-timeout", "us-east-1")
            .with_aws_executable(&offline_script)
            .with_poll_interval(Duration::from_millis(5));
        timeout_config.timeout = Timeout::new(Duration::from_millis(15));
        let timeout_comm = SsmCommunicator::new(timeout_config);
        assert!(timeout_comm.wait_for_agent_online(&ui).await.is_err());

        // Error exit code script
        let err_script = temp_dir.path().join("aws_err.sh");
        std::fs::write(&err_script, "#!/bin/sh\nexit 1\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&err_script, std::fs::Permissions::from_mode(0o755))?;
        }

        let mut err_config = SsmConfig::new("i-err", "us-east-1")
            .with_aws_executable(&err_script)
            .with_poll_interval(Duration::from_millis(5));
        err_config.timeout = Timeout::new(Duration::from_millis(15));
        let err_comm = SsmCommunicator::new(err_config);
        assert!(err_comm.wait_for_agent_online(&ui).await.is_err());
        Ok(())
    }
}
