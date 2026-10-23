//! The "none" communicator, which does nothing.

use crate::communicator::{Command, CommandResult, Communicator};
use crate::error::StampError;
use crate::types::FilePath;

/// A communicator that performs no operations.
/// Used when a builder doesn't require connecting to the machine.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoneCommunicator;

impl NoneCommunicator {
    /// Create a new `NoneCommunicator`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Communicator for NoneCommunicator {
    async fn execute(&self, _cmd: &Command) -> Result<CommandResult, StampError> {
        Ok(CommandResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    async fn upload(
        &self,
        _local_path: &FilePath,
        _remote_path: &FilePath,
    ) -> Result<(), StampError> {
        Ok(())
    }

    async fn download(
        &self,
        _remote_path: &FilePath,
        _local_path: &FilePath,
    ) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_derived_traits() {
        let comm = NoneCommunicator::new();
        assert_eq!(format!("{comm:?}"), "NoneCommunicator");
        assert_eq!(format!("{NoneCommunicator:?}"), "NoneCommunicator");
        let comm2 = comm;
        assert_eq!(format!("{comm2:?}"), "NoneCommunicator");
    }

    #[tokio::test]
    async fn test_none_communicator_execute() -> Result<(), StampError> {
        let comm = NoneCommunicator::new();
        let cmd = Command::new("echo hello".to_string());
        let r = comm.execute(&cmd).await?;
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "");
        assert_eq!(r.stderr, "");
        Ok(())
    }

    #[tokio::test]
    async fn test_none_communicator_upload() -> Result<(), StampError> {
        let comm = NoneCommunicator::new();
        let path = FilePath::new(PathBuf::from("/tmp/test"));
        comm.upload(&path, &path).await
    }

    #[tokio::test]
    async fn test_none_communicator_download() -> Result<(), StampError> {
        let comm = NoneCommunicator::new();
        let path = FilePath::new(PathBuf::from("/tmp/test"));
        comm.download(&path, &path).await
    }
}
