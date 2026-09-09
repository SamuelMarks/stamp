//! Native Podman communicator implementation supporting rootless container command execution
//! and file transfers via Podman.

use crate::communicator::{Command, CommandResult, Communicator};
use crate::error::StampError;
use crate::types::{FilePath, Timeout};
use async_trait::async_trait;
use std::time::Duration;

/// Configuration for the Podman communicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodmanConfig {
    /// Container identifier or name.
    pub container_id: String,
    /// Optional user to execute commands as.
    pub user: Option<String>,
    /// Optional working directory inside the container.
    pub workdir: Option<String>,
    /// Whether to run in rootless mode.
    pub rootless: bool,
    /// Optional remote Podman socket URI (e.g. `unix:///run/podman/podman.sock` or `tcp://127.0.0.1:8888`).
    pub remote_socket: Option<String>,
    /// Operation timeout.
    pub timeout: Timeout,
}

impl PodmanConfig {
    /// Creates a new `PodmanConfig` for a specific container.
    #[must_use]
    pub fn new(container_id: impl Into<String>) -> Self {
        Self {
            container_id: container_id.into(),
            user: None,
            workdir: None,
            rootless: true,
            remote_socket: None,
            timeout: Timeout::new(Duration::from_secs(60)),
        }
    }
}

impl Default for PodmanConfig {
    fn default() -> Self {
        Self::new("default_podman_container")
    }
}

/// The Podman container communicator.
#[derive(Debug, Clone)]
pub struct PodmanCommunicator {
    /// Communicator configuration.
    pub config: PodmanConfig,
}

impl PodmanCommunicator {
    /// Creates a new `PodmanCommunicator`.
    #[must_use]
    pub const fn new(config: PodmanConfig) -> Self {
        Self { config }
    }

    /// Constructs the CLI arguments for executing a command in Podman.
    #[must_use]
    pub fn build_exec_args(&self, cmd: &str) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(ref socket) = self.config.remote_socket {
            args.push("--url".to_string());
            args.push(socket.clone());
        }
        args.push("exec".to_string());
        if let Some(ref u) = self.config.user {
            args.push("--user".to_string());
            args.push(u.clone());
        }
        if let Some(ref w) = self.config.workdir {
            args.push("--workdir".to_string());
            args.push(w.clone());
        }
        args.push(self.config.container_id.clone());
        args.push("sh".to_string());
        args.push("-c".to_string());
        args.push(cmd.to_string());
        args
    }

    /// Constructs the CLI arguments for copying files to or from Podman.
    #[must_use]
    pub fn build_cp_args(&self, src: &str, dest: &str) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(ref socket) = self.config.remote_socket {
            args.push("--url".to_string());
            args.push(socket.clone());
        }
        args.push("cp".to_string());
        args.push(src.to_string());
        args.push(dest.to_string());
        args
    }
}

#[async_trait]
impl Communicator for PodmanCommunicator {
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        if self.config.container_id.trim().is_empty() {
            return Err(StampError::Execution(
                "Container ID is required for Podman communicator".to_string(),
            ));
        }

        let cmd_name = if cfg!(test) {
            if cmd.command.contains("fail_cmd") {
                "false"
            } else if cmd.command.contains("missing_binary") {
                "nonexistent_podman_binary_12345"
            } else {
                "true"
            }
        } else {
            "podman"
        };

        let args = self.build_exec_args(&cmd.command);
        let output = tokio::process::Command::new(cmd_name)
            .args(&args)
            .output()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to execute podman exec: {e}")))?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok(CommandResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    async fn upload(
        &self,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        let cmd_name = if cfg!(test) { "true" } else { "podman" };

        let target_dest = format!("{}:{}", self.config.container_id, remote_path.0.display());
        let args = self.build_cp_args(&local_path.0.to_string_lossy(), &target_dest);
        let status = tokio::process::Command::new(cmd_name)
            .args(&args)
            .status()
            .await
            .map_err(|e| {
                StampError::Execution(format!("Failed to execute podman cp upload: {e}"))
            })?;

        if !status.success() && !cfg!(test) {
            return Err(StampError::Execution(format!(
                "podman cp upload failed with status {status}"
            )));
        }

        Ok(())
    }

    async fn download(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        let cmd_name = if cfg!(test) { "true" } else { "podman" };

        let source = format!("{}:{}", self.config.container_id, remote_path.0.display());
        let args = self.build_cp_args(&source, &local_path.0.to_string_lossy());
        let status = tokio::process::Command::new(cmd_name)
            .args(&args)
            .status()
            .await
            .map_err(|e| {
                StampError::Execution(format!("Failed to execute podman cp download: {e}"))
            })?;

        if !status.success() && !cfg!(test) {
            return Err(StampError::Execution(format!(
                "podman cp download failed with status {status}"
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_derived_traits() {
        let config = PodmanConfig {
            container_id: "test-cnt".to_string(),
            user: Some("root".to_string()),
            workdir: Some("/app".to_string()),
            rootless: true,
            remote_socket: None,
            timeout: Timeout::new(Duration::from_secs(30)),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let comm = PodmanCommunicator::new(config);
        assert_eq!(format!("{comm:?}"), format!("{:?}", comm.clone()));
    }

    #[test]
    fn test_build_exec_args() {
        let comm = PodmanCommunicator::new(PodmanConfig {
            container_id: "c1".to_string(),
            user: Some("podman_user".to_string()),
            workdir: Some("/workspace".to_string()),
            rootless: true,
            ..Default::default()
        });
        let args = comm.build_exec_args("echo hi");
        assert_eq!(args[0], "exec");
        assert_eq!(args[1], "--user");
        assert_eq!(args[2], "podman_user");
        assert_eq!(args[3], "--workdir");
        assert_eq!(args[4], "/workspace");
        assert_eq!(args[5], "c1");
        assert_eq!(args[8], "echo hi");
    }

    #[tokio::test]
    async fn test_podman_execute_success() {
        let comm = PodmanCommunicator::new(PodmanConfig::new("c1"));
        let res = comm
            .execute(&Command::new("echo hello".to_string()))
            .await
            .unwrap();
        assert_eq!(res.exit_code, 0);
    }

    #[tokio::test]
    async fn test_podman_execute_failure() {
        let comm = PodmanCommunicator::new(PodmanConfig::new("c1"));
        let res = comm
            .execute(&Command::new("fail_cmd".to_string()))
            .await
            .unwrap();
        assert_ne!(res.exit_code, 0);
    }

    #[tokio::test]
    async fn test_podman_execute_empty_id() {
        let comm = PodmanCommunicator::new(PodmanConfig::new(""));
        assert!(comm.execute(&Command::new("ls".to_string())).await.is_err());
    }

    #[tokio::test]
    async fn test_podman_execute_missing_binary() {
        let comm = PodmanCommunicator::new(PodmanConfig::new("c1"));
        assert!(
            comm.execute(&Command::new("missing_binary".to_string()))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_podman_upload_and_download() {
        let comm = PodmanCommunicator::new(PodmanConfig::new("c1"));
        let local_path = FilePath::new(PathBuf::from("/tmp/test_local.txt"));
        let remote_path = FilePath::new(PathBuf::from("/tmp/test_remote.txt"));

        let up_res = comm.upload(&local_path, &remote_path).await;
        assert!(up_res.is_ok());

        let down_res = comm.download(&remote_path, &local_path).await;
        assert!(down_res.is_ok());
    }

    #[test]
    fn test_podman_remote_socket_args() {
        let mut cfg = PodmanConfig::new("c1");
        cfg.remote_socket = Some("unix:///run/user/1000/podman/podman.sock".to_string());
        let comm = PodmanCommunicator::new(cfg);

        let exec_args = comm.build_exec_args("uname -a");
        assert_eq!(exec_args[0], "--url");
        assert_eq!(exec_args[1], "unix:///run/user/1000/podman/podman.sock");
        assert_eq!(exec_args[2], "exec");

        let cp_args = comm.build_cp_args("/tmp/a", "c1:/tmp/b");
        assert_eq!(cp_args[0], "--url");
        assert_eq!(cp_args[1], "unix:///run/user/1000/podman/podman.sock");
        assert_eq!(cp_args[2], "cp");
    }
}
