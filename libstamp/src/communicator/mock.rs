#![cfg_attr(coverage_nightly, coverage(off))]
//! Mock communicator for testing.

use crate::communicator::{Command, CommandResult, Communicator};
use crate::error::StampError;
use crate::types::FilePath;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A mock communicator that records operations for testing.
#[derive(Debug, Default, Clone)]
pub struct MockCommunicator {
    /// History of executed commands.
    pub executed_commands: Arc<Mutex<Vec<Command>>>,
    /// History of uploaded files as (local, remote).
    pub uploaded_files: Arc<Mutex<Vec<(FilePath, FilePath)>>>,
    /// History of downloaded files as (remote, local).
    pub downloaded_files: Arc<Mutex<Vec<(FilePath, FilePath)>>>,
}

impl MockCommunicator {
    /// Create a new `MockCommunicator`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl Communicator for MockCommunicator {
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        let mut executed = self.executed_commands.lock().await;
        executed.push(cmd.clone());

        Ok(CommandResult {
            exit_code: i32::from(cmd.command.contains("fail")),
            stdout: "mock stdout".to_string(),
            stderr: "mock stderr".to_string(),
        })
    }

    async fn upload(
        &self,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        let mut uploaded = self.uploaded_files.lock().await;
        uploaded.push((local_path.clone(), remote_path.clone()));
        Ok(())
    }

    async fn download(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        let mut downloaded = self.downloaded_files.lock().await;
        downloaded.push((remote_path.clone(), local_path.clone()));
        let _ = tokio::fs::write(local_path.as_path(), b"mock content").await;
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::pedantic,
    clippy::all,
    for_loops_over_fallibles
)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_derived_traits() {
        let comm = MockCommunicator::new();
        assert_eq!(
            format!("{comm:?}"),
            format!("{:?}", MockCommunicator::default())
        );
    }

    #[tokio::test]
    async fn test_mock_communicator() {
        let comm = MockCommunicator::new();
        let cmd = Command::new("ls".to_string());

        let res = comm.execute(&cmd).await;
        assert!(res.is_ok());
        for r in res {
            assert_eq!(r.stdout, "mock stdout");
        }
        let executed = comm.executed_commands.lock().await;
        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].command, "ls");
        drop(executed); // Drop lock before next await

        let local_path = FilePath::new(PathBuf::from("/tmp/local"));
        let remote_path = FilePath::new(PathBuf::from("/tmp/remote"));

        let res_up = comm.upload(&local_path, &remote_path).await;
        assert!(res_up.is_ok());
        let uploaded = comm.uploaded_files.lock().await;
        assert_eq!(uploaded.len(), 1);
        assert_eq!(uploaded[0].0, local_path);
        assert_eq!(uploaded[0].1, remote_path);
        drop(uploaded); // Drop lock before next await

        let res_down = comm.download(&remote_path, &local_path).await;
        assert!(res_down.is_ok());
        let downloaded = comm.downloaded_files.lock().await;
        assert_eq!(downloaded.len(), 1);
        assert_eq!(downloaded[0].0, remote_path);
        assert_eq!(downloaded[0].1, local_path);
    }
}
