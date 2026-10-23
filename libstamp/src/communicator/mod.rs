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
pub use crate::types::{CommunicatorKind, FilePath};
use async_trait::async_trait;

/// A command to be executed by a communicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The command string to execute.
    pub command: String,
}

impl Command {
    /// Create a new `Command`.
    ///
    /// # Arguments
    /// * `command` - Shell command string to execute on the target system.
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
    ///
    /// # Arguments
    /// * `cmd` - The [`Command`] to execute.
    ///
    /// # Errors
    /// Returns `StampError` if command execution fails.
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError>;

    /// Upload a file to the remote machine.
    ///
    /// # Arguments
    /// * `local_path` - Path to the source file on the local host.
    /// * `remote_path` - Destination path on the remote guest.
    ///
    /// # Errors
    /// Returns `StampError` if the file transfer fails.
    async fn upload(&self, local_path: &FilePath, remote_path: &FilePath)
    -> Result<(), StampError>;

    /// Download a file from the remote machine.
    ///
    /// # Arguments
    /// * `remote_path` - Path to the source file on the remote guest.
    /// * `local_path` - Destination path on the local host.
    ///
    /// # Errors
    /// Returns `StampError` if the file transfer fails.
    async fn download(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError>;
}
