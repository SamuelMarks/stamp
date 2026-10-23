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
    /// Optional custom responses keyed by substring pattern.
    pub custom_responses: Arc<Mutex<Vec<(String, CommandResult)>>>,
}

impl MockCommunicator {
    /// Create a new `MockCommunicator`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a custom [`CommandResult`] for commands containing `pattern`.
    ///
    /// # Arguments
    /// * `pattern` - Substring to match against `cmd.command`.
    /// * `result` - Result returned when `pattern` matches.
    pub async fn add_response(&self, pattern: impl Into<String>, result: CommandResult) {
        self.custom_responses
            .lock()
            .await
            .push((pattern.into(), result));
    }
}

#[async_trait::async_trait]
impl Communicator for MockCommunicator {
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        let mut executed = self.executed_commands.lock().await;
        executed.push(cmd.clone());
        drop(executed);

        let custom = self.custom_responses.lock().await;
        for (pat, res) in custom.iter() {
            if cmd.command.contains(pat) {
                return Ok(res.clone());
            }
        }
        drop(custom);

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
    async fn test_mock_communicator() -> Result<(), StampError> {
        let comm = MockCommunicator::new();
        let cmd = Command::new("ls".to_string());

        let r = comm.execute(&cmd).await?;
        assert_eq!(r.stdout, "mock stdout");

        comm.add_response(
            "custom_cmd",
            CommandResult {
                exit_code: 0,
                stdout: "custom output".to_string(),
                stderr: String::new(),
            },
        )
        .await;
        let custom_res = comm
            .execute(&Command::new("run custom_cmd".to_string()))
            .await?;
        assert_eq!(custom_res.stdout, "custom output");

        let executed = comm.executed_commands.lock().await;
        assert_eq!(executed.len(), 2);
        assert_eq!(executed[0].command, "ls");
        drop(executed);

        let temp_dir = tempfile::tempdir()?;
        let local_path = FilePath::new(temp_dir.path().join("local"));
        let remote_path = FilePath::new(PathBuf::from("/tmp/remote"));

        comm.upload(&local_path, &remote_path).await?;
        let uploaded = comm.uploaded_files.lock().await;
        assert_eq!(uploaded.len(), 1);
        assert_eq!(uploaded[0].0, local_path);
        assert_eq!(uploaded[0].1, remote_path);
        drop(uploaded);

        comm.download(&remote_path, &local_path).await?;
        let downloaded = comm.downloaded_files.lock().await;
        assert_eq!(downloaded.len(), 1);
        assert_eq!(downloaded[0].0, remote_path);
        assert_eq!(downloaded[0].1, local_path);
        Ok(())
    }
}
