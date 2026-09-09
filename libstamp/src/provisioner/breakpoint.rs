//! Implementation of the `breakpoint` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// Configuration for the `breakpoint` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpointConfig {
    /// A custom note or instructions to display when the breakpoint is hit.
    pub note: Option<String>,
    /// Disable the breakpoint (useful for non-interactive CI runs).
    pub disable: bool,
    /// Optional command to execute for state cleanup upon abort or completion.
    pub cleanup_command: Option<String>,
    /// Whether to execute state cleanup upon abort. Defaults to true.
    pub clean_up: bool,
}

impl Default for BreakpointConfig {
    fn default() -> Self {
        Self {
            note: None,
            disable: false,
            cleanup_command: None,
            clean_up: true,
        }
    }
}

/// The `breakpoint` provisioner.
#[derive(Debug, Clone)]
pub struct BreakpointProvisioner {
    /// The provisioner configuration.
    pub config: BreakpointConfig,
}

impl BreakpointProvisioner {
    /// Create a new `BreakpointProvisioner`.
    #[must_use]
    pub const fn new(config: BreakpointConfig) -> Self {
        Self { config }
    }

    /// Executes an interactive breakpoint debug session over the provided asynchronous reader and writer.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Provisioner`] if the user chooses to abort or a communication error occurs.
    pub async fn execute_interactive_session<
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    >(
        comm: &dyn Communicator,
        reader: &mut R,
        writer: &mut W,
        ui: &crate::engine::ui::Ui,
        note: &str,
    ) -> Result<(), StampError> {
        Self::execute_interactive_session_with_cleanup(comm, reader, writer, ui, note, None, true)
            .await
    }

    /// Executes an interactive breakpoint debug session with cleanup command on abort.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Provisioner`] if the user chooses to abort or a communication error occurs.
    pub async fn execute_interactive_session_with_cleanup<
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    >(
        comm: &dyn Communicator,
        reader: &mut R,
        writer: &mut W,
        ui: &crate::engine::ui::Ui,
        note: &str,
        cleanup_cmd: Option<&str>,
        clean_up: bool,
    ) -> Result<(), StampError> {
        let banner = format!(
            "\n======================== BREAKPOINT ========================\n{note}\nCommands:\n  c, continue     - Resume the build pipeline\n  i, info         - Display target communicator status and note\n  s, shell        - Launch interactive shell session on guest\n  r, run <cmd>    - Execute a command on the remote guest\n  a, abort        - Abort the build immediately (with cleanup)\n  h, help         - Show this help message\n============================================================\n"
        );
        writer
            .write_all(banner.as_bytes())
            .await
            .map_err(StampError::Io)?;
        writer.flush().await.map_err(StampError::Io)?;
        ui.say("breakpoint", note);

        loop {
            writer
                .write_all(b"breakpoint> ")
                .await
                .map_err(StampError::Io)?;
            writer.flush().await.map_err(StampError::Io)?;

            let mut line = String::new();
            let bytes_read = reader.read_line(&mut line).await.map_err(StampError::Io)?;
            if bytes_read == 0 {
                // EOF encountered
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed == "c" || trimmed == "continue" {
                ui.say("breakpoint", "Resuming build pipeline...");
                break;
            } else if trimmed == "a" || trimmed == "abort" {
                ui.error("breakpoint", "Build aborted by user at breakpoint");
                if clean_up && let Some(cmd) = cleanup_cmd {
                    ui.say("breakpoint", "Executing state cleanup before abort...");
                    let _ = comm.execute(&Command::new(cmd.to_string())).await;
                }
                return Err(StampError::Provisioner(
                    "Build aborted by user at breakpoint".to_string(),
                ));
            } else if trimmed == "h" || trimmed == "help" {
                let help_text = b"Commands:\n  c, continue  - Resume\n  i, info      - Target status\n  s, shell     - Interactive guest shell\n  r <cmd>      - Run command\n  a, abort     - Abort\n";
                writer.write_all(help_text).await.map_err(StampError::Io)?;
                writer.flush().await.map_err(StampError::Io)?;
            } else if trimmed == "i" || trimmed == "info" {
                let info_msg = format!("Target: Communicator session active\nNote: {note}\n");
                writer
                    .write_all(info_msg.as_bytes())
                    .await
                    .map_err(StampError::Io)?;
                writer.flush().await.map_err(StampError::Io)?;
            } else if trimmed == "s" || trimmed == "shell" {
                writer
                    .write_all(
                        b"Entering guest shell session (type 'exit' to return to breakpoint)...\n",
                    )
                    .await
                    .map_err(StampError::Io)?;
                writer.flush().await.map_err(StampError::Io)?;

                loop {
                    writer.write_all(b"guest$ ").await.map_err(StampError::Io)?;
                    writer.flush().await.map_err(StampError::Io)?;

                    let mut guest_line = String::new();
                    let n = reader
                        .read_line(&mut guest_line)
                        .await
                        .map_err(StampError::Io)?;
                    if n == 0 {
                        break;
                    }
                    let guest_trimmed = guest_line.trim();
                    if guest_trimmed.is_empty() {
                        continue;
                    }
                    if guest_trimmed == "exit" || guest_trimmed == "quit" {
                        break;
                    }

                    let res = comm
                        .execute(&Command::new(guest_trimmed.to_string()))
                        .await?;
                    if !res.stdout.is_empty() {
                        writer
                            .write_all(res.stdout.as_bytes())
                            .await
                            .map_err(StampError::Io)?;
                    }
                    if !res.stderr.is_empty() {
                        writer
                            .write_all(res.stderr.as_bytes())
                            .await
                            .map_err(StampError::Io)?;
                    }
                    writer.flush().await.map_err(StampError::Io)?;
                }
            } else if trimmed.starts_with("r ") || trimmed.starts_with("run ") {
                let cmd_to_run = if let Some(stripped) = trimmed.strip_prefix("run ") {
                    stripped
                } else if let Some(stripped) = trimmed.strip_prefix("r ") {
                    stripped
                } else {
                    trimmed
                };

                let res = comm.execute(&Command::new(cmd_to_run.to_string())).await?;
                let out = format!(
                    "Exit code: {}\nStdout:\n{}\nStderr:\n{}\n",
                    res.exit_code, res.stdout, res.stderr
                );
                writer
                    .write_all(out.as_bytes())
                    .await
                    .map_err(StampError::Io)?;
                writer.flush().await.map_err(StampError::Io)?;
            } else {
                let msg = format!(
                    "Unknown command: '{trimmed}'. Type 'help' for options or 'c' to continue.\n"
                );
                writer
                    .write_all(msg.as_bytes())
                    .await
                    .map_err(StampError::Io)?;
                writer.flush().await.map_err(StampError::Io)?;
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl Provisioner for BreakpointProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.disable || ui.machine_readable.is_enabled() {
            ui.say(
                "breakpoint",
                "Skipping breakpoint in disabled or machine-readable CI mode.",
            );
            return Ok(());
        }

        let note = self
            .config
            .note
            .as_deref()
            .unwrap_or("Breakpoint hit. Pausing execution.");

        #[cfg(test)]
        {
            let mut input = std::io::Cursor::new(b"c\n");
            let mut output = Vec::new();
            Self::execute_interactive_session_with_cleanup(
                comm,
                &mut input,
                &mut output,
                &ui,
                note,
                self.config.cleanup_command.as_deref(),
                self.config.clean_up,
            )
            .await
        }

        #[cfg(not(test))]
        {
            let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
            let mut writer = tokio::io::stdout();
            Self::execute_interactive_session_with_cleanup(
                comm,
                &mut reader,
                &mut writer,
                &ui,
                note,
                self.config.cleanup_command.as_deref(),
                self.config.clean_up,
            )
            .await
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;
    use std::io::Cursor;

    #[tokio::test]
    async fn test_breakpoint_interactive_continue() -> Result<(), StampError> {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut input = Cursor::new(b"continue\n");
        let mut output = Vec::new();

        BreakpointProvisioner::execute_interactive_session(
            &comm,
            &mut input,
            &mut output,
            &ui,
            "Testing breakpoint",
        )
        .await?;
        assert!(String::from_utf8_lossy(&output).contains("BREAKPOINT"));
        Ok(())
    }

    #[tokio::test]
    async fn test_breakpoint_interactive_abort() {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut input = Cursor::new(b"abort\n");
        let mut output = Vec::new();

        let res = BreakpointProvisioner::execute_interactive_session(
            &comm,
            &mut input,
            &mut output,
            &ui,
            "Testing breakpoint abort",
        )
        .await;
        assert!(matches!(res, Err(StampError::Provisioner(_))));
    }

    #[tokio::test]
    async fn test_breakpoint_interactive_run_and_help() -> Result<(), StampError> {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut input = Cursor::new(b"help\ninfo\nunknown_cmd\nr uname -a\nc\n");
        let mut output = Vec::new();

        BreakpointProvisioner::execute_interactive_session(
            &comm,
            &mut input,
            &mut output,
            &ui,
            "Testing run commands",
        )
        .await?;
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("Target: Communicator session active"));
        assert!(output_str.contains("Exit code:"));
        Ok(())
    }

    #[tokio::test]
    async fn test_breakpoint_provision_machine_readable() -> Result<(), StampError> {
        let config = BreakpointConfig::default();
        let prov = BreakpointProvisioner::new(config);
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Enabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        prov.provision(&comm, ui).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_breakpoint_provision_trait() -> Result<(), StampError> {
        let config = BreakpointConfig {
            note: Some("Test note".to_string()),
            disable: false,
            ..Default::default()
        };
        let prov = BreakpointProvisioner::new(config);
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        prov.provision(&comm, ui).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_breakpoint_provision_disabled() -> Result<(), StampError> {
        let config = BreakpointConfig {
            disable: true,
            ..Default::default()
        };
        let prov = BreakpointProvisioner::new(config);
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        prov.provision(&comm, ui).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_breakpoint_interactive_shell_session() -> Result<(), StampError> {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut input = Cursor::new(b"s\nls -la\nexit\nc\n");
        let mut output = Vec::new();

        BreakpointProvisioner::execute_interactive_session_with_cleanup(
            &comm,
            &mut input,
            &mut output,
            &ui,
            "Testing shell session",
            None,
            true,
        )
        .await?;
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("guest$"));
        Ok(())
    }

    #[tokio::test]
    async fn test_breakpoint_abort_with_cleanup() {
        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut input = Cursor::new(b"abort\n");
        let mut output = Vec::new();

        let res = BreakpointProvisioner::execute_interactive_session_with_cleanup(
            &comm,
            &mut input,
            &mut output,
            &ui,
            "Testing abort cleanup",
            Some("rm -rf /tmp/test-state"),
            true,
        )
        .await;
        assert!(matches!(res, Err(StampError::Provisioner(_))));
    }

    #[test]
    fn test_derived_traits() {
        let config1 = BreakpointConfig {
            disable: false,
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = BreakpointProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
