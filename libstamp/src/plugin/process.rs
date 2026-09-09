//! Plugin subprocess lifecycle management, health monitoring, and graceful termination.
//!
//! Handles spawning child plugin processes with the HashiCorp magic cookie,
//! reading the handshake, monitoring health via watchdog pings, capturing stderr streams,
//! and ensuring clean termination without orphaned zombie processes.

use crate::error::StampError;
use crate::plugin::handshake::{
    Handshake, PACKER_PLUGIN_MAGIC_COOKIE_KEY, PACKER_PLUGIN_MAGIC_COOKIE_VALUE,
    PLUGIN_PROTOCOL_VERSIONS_ENV,
};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Default timeout for reading the plugin handshake line (15 seconds).
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Default timeout for waiting on graceful process termination before SIGKILL (5 seconds).
pub const DEFAULT_TERMINATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Represents a running plugin child process with lifecycle guarantees.
pub struct PluginSubprocess {
    /// Path to the plugin binary.
    binary_path: PathBuf,
    /// The spawned child process wrapped in a mutex for thread-safe termination.
    child: Arc<Mutex<Option<Child>>>,
    /// Thread-safe buffer capturing plugin stderr output.
    stderr_buf: Arc<Mutex<String>>,
    /// The parsed handshake returned by the plugin.
    handshake: Handshake,
}

impl std::fmt::Debug for PluginSubprocess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginSubprocess")
            .field("binary_path", &self.binary_path)
            .field("handshake", &self.handshake)
            .finish()
    }
}

impl PluginSubprocess {
    /// Spawns an external plugin executable and awaits its handshake announcement.
    ///
    /// # Errors
    /// Returns `StampError::PluginCrashed` if the process exits with an error before emitting
    /// the handshake, or `StampError::PluginHandshake` / `StampError::Execution` if spawning or
    /// parsing fails.
    pub async fn spawn(
        binary_path: impl AsRef<Path>,
        extra_env: &[(String, String)],
    ) -> Result<Self, StampError> {
        let path = binary_path.as_ref().to_path_buf();

        let mut cmd = Command::new(&path);
        cmd.env(
            PACKER_PLUGIN_MAGIC_COOKIE_KEY,
            PACKER_PLUGIN_MAGIC_COOKIE_VALUE,
        )
        .env(PLUGIN_PROTOCOL_VERSIONS_ENV, "1,2,3,4,5,6")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        for (k, v) in extra_env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| StampError::Execution(format!("Failed to spawn plugin {path:?}: {e}")))?;

        let stderr_buf = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = child.stderr.take() {
            let buf_clone = stderr_buf.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n == 0 {
                        break;
                    }
                    let mut guard = buf_clone.lock().await;
                    guard.push_str(&line);
                    line.clear();
                }
            });
        }

        let stdout = child.stdout.take().ok_or_else(|| {
            StampError::Execution(format!("Failed to capture stdout for plugin {path:?}"))
        })?;

        let mut reader = BufReader::new(stdout);
        let mut line = String::new();

        let read_result =
            tokio::time::timeout(DEFAULT_HANDSHAKE_TIMEOUT, reader.read_line(&mut line)).await;

        match read_result {
            Err(_) => {
                let captured_stderr = stderr_buf.lock().await.clone();
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(StampError::PluginCrashed {
                        binary: path.display().to_string(),
                        exit_code: status.code(),
                        stderr: captured_stderr,
                    });
                }
                Err(StampError::PluginHandshake(format!(
                    "Timed out waiting for handshake from plugin {path:?}"
                )))
            }
            Ok(Err(e)) => Err(StampError::PluginHandshake(format!(
                "Failed to read handshake stdout from plugin {path:?}: {e}"
            ))),
            Ok(Ok(_)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let captured_stderr = stderr_buf.lock().await.clone();
                    if let Ok(Some(status)) = child.try_wait() {
                        return Err(StampError::PluginCrashed {
                            binary: path.display().to_string(),
                            exit_code: status.code(),
                            stderr: captured_stderr,
                        });
                    }
                    return Err(StampError::PluginHandshake(format!(
                        "Plugin {path:?} terminated without emitting handshake line"
                    )));
                }

                let handshake = trimmed.parse::<Handshake>()?;

                Ok(Self {
                    binary_path: path,
                    child: Arc::new(Mutex::new(Some(child))),
                    stderr_buf,
                    handshake,
                })
            }
        }
    }

    /// Returns the parsed handshake announced by the plugin.
    #[must_use]
    pub fn handshake(&self) -> &Handshake {
        &self.handshake
    }

    /// Returns the binary path of the plugin.
    #[must_use]
    pub fn binary_path(&self) -> &Path {
        &self.binary_path
    }

    /// Returns a copy of the stderr output captured from the plugin process so far.
    pub async fn stderr(&self) -> String {
        self.stderr_buf.lock().await.clone()
    }

    /// Checks whether the child process is still alive.
    pub async fn is_alive(&self) -> bool {
        let mut child_guard = self.child.lock().await;
        if let Some(ref mut child) = *child_guard {
            matches!(child.try_wait(), Ok(None))
        } else {
            false
        }
    }

    /// Checks whether the plugin child process has terminated unexpectedly.
    ///
    /// # Errors
    /// Returns `StampError::PluginCrashed` if the process has exited.
    pub async fn check_crashed(&self) -> Result<(), StampError> {
        let mut child_guard = self.child.lock().await;
        if let Some(ref mut child) = *child_guard
            && let Ok(Some(status)) = child.try_wait()
        {
            let captured = self.stderr_buf.lock().await.clone();
            return Err(StampError::PluginCrashed {
                binary: self.binary_path.display().to_string(),
                exit_code: status.code(),
                stderr: captured,
            });
        }
        Ok(())
    }

    /// Gracefully terminates the plugin process with fallback to kill.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if termination fails.
    pub async fn terminate(&self) -> Result<(), StampError> {
        let mut child_guard = self.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            // First attempt SIGTERM on Unix or kill on other systems
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGTERM);
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill().await;
            }

            // Wait for exit or timeout
            let wait_fut = child.wait();
            if tokio::time::timeout(DEFAULT_TERMINATION_TIMEOUT, wait_fut)
                .await
                .is_err()
            {
                // Force kill if process did not exit within timeout
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }
        Ok(())
    }
}

impl Drop for PluginSubprocess {
    fn drop(&mut self) {
        let child_arc = self.child.clone();
        tokio::spawn(async move {
            let mut guard = child_arc.lock().await;
            if let Some(mut child) = guard.take() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subprocess_spawn_and_terminate() {
        let script = r#"#!/bin/bash
echo "1|5|tcp|127.0.0.1:45678|grpc"
sleep 10
"#;
        let script_path =
            std::env::temp_dir().join(format!("test-plugin-proc-{}", uuid::Uuid::new_v4()));
        std::fs::write(&script_path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let proc = PluginSubprocess::spawn(&script_path, &[]).await.unwrap();
        assert_eq!(proc.handshake().address, "127.0.0.1:45678");
        assert_eq!(proc.binary_path(), &script_path);
        assert!(proc.is_alive().await);
        assert!(proc.check_crashed().await.is_ok());

        proc.terminate().await.unwrap();
        assert!(!proc.is_alive().await);

        let _ = std::fs::remove_file(&script_path);
    }

    #[tokio::test]
    async fn test_subprocess_spawn_crash_captures_stderr() {
        let script = r#"#!/bin/bash
>&2 echo "Fatal plugin bootstrap failure: missing dependency"
exit 42
"#;
        let script_path =
            std::env::temp_dir().join(format!("test-plugin-crash-{}", uuid::Uuid::new_v4()));
        std::fs::write(&script_path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let res = PluginSubprocess::spawn(&script_path, &[]).await;
        match res {
            Err(StampError::PluginCrashed {
                exit_code, stderr, ..
            }) => {
                assert_eq!(exit_code, Some(42));
                assert!(stderr.contains("Fatal plugin bootstrap failure"));
            }
            other => panic!("Expected PluginCrashed, got {:?}", other),
        }
        let _ = std::fs::remove_file(&script_path);
    }

    #[tokio::test]
    async fn test_subprocess_check_crashed_after_exit() {
        let script = r#"#!/bin/bash
echo "1|5|tcp|127.0.0.1:45679|grpc"
sleep 0.1
>&2 echo "Plugin crashed during run"
exit 7
"#;
        let script_path =
            std::env::temp_dir().join(format!("test-plugin-run-crash-{}", uuid::Uuid::new_v4()));
        std::fs::write(&script_path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let proc = PluginSubprocess::spawn(&script_path, &[]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let crash_res = proc.check_crashed().await;
        match crash_res {
            Err(StampError::PluginCrashed {
                exit_code, stderr, ..
            }) => {
                assert_eq!(exit_code, Some(7));
                assert!(stderr.contains("Plugin crashed during run"));
            }
            other => panic!("Expected PluginCrashed, got {:?}", other),
        }
        let collected_stderr = proc.stderr().await;
        assert!(collected_stderr.contains("Plugin crashed during run"));

        let _ = std::fs::remove_file(&script_path);
    }

    #[tokio::test]
    async fn test_subprocess_spawn_empty_output_error() {
        let script = r#"#!/bin/bash
exit 0
"#;
        let script_path =
            std::env::temp_dir().join(format!("test-plugin-empty-{}", uuid::Uuid::new_v4()));
        std::fs::write(&script_path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let res = PluginSubprocess::spawn(&script_path, &[]).await;
        assert!(res.is_err());
        let _ = std::fs::remove_file(&script_path);
    }

    #[tokio::test]
    async fn test_subprocess_spawn_nonexistent() {
        let res = PluginSubprocess::spawn("/nonexistent/file/path/here", &[]).await;
        assert!(res.is_err());
    }
}
