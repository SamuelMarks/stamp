#![cfg_attr(coverage_nightly, coverage(off))]
//! Communicator traits and implementations (SSH, `WinRM`, etc.).

pub mod chroot;
pub mod docker;
pub mod grpc_proxy;
pub mod mock;
pub mod none;
pub mod podman;
pub mod ssh;
pub mod ssm;
pub mod winrm;

use crate::error::StampError;
use crate::types::FilePath;
use async_trait::async_trait;

/// A command to be executed by a communicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The command string to execute.
    pub command: String,
}

impl Command {
    /// Create a new `Command`.
    #[must_use]
    pub const fn new(command: String) -> Self {
        Self { command }
    }
}

/// The result of executing a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    /// The exit code of the command.
    pub exit_code: i32,
    /// The standard output of the command.
    pub stdout: String,
    /// The standard error of the command.
    pub stderr: String,
}

/// The main trait for machine communication.
#[async_trait]
pub trait Communicator: Send + Sync {
    /// Execute a command on the remote machine.
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError>;

    /// Upload a file to the remote machine.
    async fn upload(&self, local_path: &FilePath, remote_path: &FilePath)
    -> Result<(), StampError>;

    /// Download a file from the remote machine.
    async fn download(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError>;
}
