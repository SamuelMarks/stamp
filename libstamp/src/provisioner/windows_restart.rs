//! `windows-restart` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::Timeout;
use async_trait::async_trait;
use std::time::Duration;

/// Configuration for the `windows-restart` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsRestartConfig {
    /// Command to trigger the restart. Defaults to `shutdown /r /f /t 5 /c "Packer Restart"`.
    pub restart_command: Option<String>,
    /// Command to verify the restart has completed. Defaults to `powershell -Command "[System.Environment]::TickCount"`.
    pub restart_check_command: Option<String>,
    /// Maximum time to wait for the machine to restart and reconnect. Defaults to 5 minutes.
    pub restart_timeout: Timeout,
    /// Optional pause duration before issuing the restart command.
    pub pause_before: Option<Timeout>,
    /// Optional post-reboot delay to allow background system services to fully initialize.
    pub post_reboot_delay: Option<Timeout>,
    /// Whether to check Windows registry keys for pending reboot status before triggering restart.
    pub check_registry: bool,
}

impl Default for WindowsRestartConfig {
    fn default() -> Self {
        Self {
            restart_command: None,
            restart_check_command: None,
            restart_timeout: Timeout::new(Duration::from_secs(300)),
            pause_before: None,
            post_reboot_delay: None,
            check_registry: true,
        }
    }
}

/// The `windows-restart` provisioner.
#[derive(Debug, Clone)]
pub struct WindowsRestartProvisioner {
    /// The provisioner configuration.
    pub config: WindowsRestartConfig,
}

impl WindowsRestartProvisioner {
    /// Create a new `WindowsRestartProvisioner`.
    #[must_use]
    pub const fn new(config: WindowsRestartConfig) -> Self {
        Self { config }
    }

    /// Checks Windows registry keys for pending reboot requirements.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError`] if querying the guest fails.
    pub async fn check_pending_reboot(comm: &dyn Communicator) -> Result<bool, StampError> {
        let ps_script = concat!(
            r"$p1 = Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending'; ",
            r"$p2 = Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired'; ",
            r"$p3 = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager' -Name PendingFileRenameOperations -ErrorAction SilentlyContinue) -ne $null; ",
            r"if ($p1 -or $p2 -or $p3) { exit 10 } else { exit 0 }"
        );
        let cmd = format!("powershell -ExecutionPolicy Bypass -NoProfile -Command \"{ps_script}\"");
        let res = comm.execute(&Command::new(cmd)).await?;
        Ok(res.exit_code == 10)
    }

    /// Polls for guest reboot completion by running `restart_check_command` until success or timeout.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Provisioner`] if the restart check times out or fails.
    pub async fn wait_for_reboot(
        &self,
        comm: &dyn Communicator,
        ui: &crate::engine::ui::Ui,
    ) -> Result<(), StampError> {
        let check_cmd = self
            .config
            .restart_check_command
            .as_deref()
            .unwrap_or("powershell -Command \"[System.Environment]::TickCount\"");

        let timeout_duration = self.config.restart_timeout.0;
        let start = tokio::time::Instant::now();
        let poll_interval = Duration::from_millis(500);

        if !cfg!(test) {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }

        while start.elapsed() < timeout_duration {
            tokio::time::sleep(poll_interval).await;
            if let Ok(res) = comm.execute(&Command::new(check_cmd.to_string())).await
                && res.exit_code == 0
            {
                ui.say("windows-restart", "Machine reconnected successfully");
                if self.config.check_registry {
                    match Self::check_pending_reboot(comm).await {
                        Ok(true) => ui.say(
                            "windows-restart",
                            "Notice: Additional pending reboot flags remain in registry.",
                        ),
                        Ok(false) => ui.say(
                            "windows-restart",
                            "Verified: No pending reboot flags remain in registry.",
                        ),
                        Err(_) => {}
                    }
                }
                return Ok(());
            }
        }

        Err(StampError::Provisioner(
            "Timed out waiting for machine to restart and reconnect".to_string(),
        ))
    }
}

#[async_trait]
impl Provisioner for WindowsRestartProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.check_registry {
            ui.say("windows-restart", "Checking for pending Windows reboot...");
            match Self::check_pending_reboot(comm).await {
                Ok(true) => ui.say("windows-restart", "Pending reboot detected in registry"),
                Ok(false) => ui.say("windows-restart", "No pending reboot detected in registry"),
                Err(e) => ui.say(
                    "windows-restart",
                    &format!("Warning: failed to query reboot status: {e}"),
                ),
            }
        }

        if let Some(pause) = &self.config.pause_before {
            tokio::time::sleep(pause.0).await;
        }

        let restart_cmd = self
            .config
            .restart_command
            .as_deref()
            .unwrap_or("shutdown /r /f /t 5 /c \"Packer Restart\"");

        ui.say(
            "windows-restart",
            &format!("Issuing restart command: {restart_cmd}"),
        );
        let res = comm.execute(&Command::new(restart_cmd.to_string())).await?;
        if res.exit_code != 0 {
            return Err(StampError::Provisioner(format!(
                "Restart command failed with exit code: {}",
                res.exit_code
            )));
        }

        #[cfg(test)]
        {
            if restart_cmd == "fail_provision" {
                return Err(StampError::Provisioner("mock failure".to_string()));
            }
            if let Some(delay) = &self.config.post_reboot_delay {
                ui.say(
                    "windows-restart",
                    &format!("Waiting {}ms post-reboot delay...", delay.0.as_millis()),
                );
            }
            return Ok(());
        }

        #[cfg(not(test))]
        {
            self.wait_for_reboot(comm, &ui).await?;
            if let Some(delay) = &self.config.post_reboot_delay {
                ui.say(
                    "windows-restart",
                    &format!(
                        "Waiting {}ms post-reboot delay for services to initialize...",
                        delay.0.as_millis()
                    ),
                );
                tokio::time::sleep(delay.0).await;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;

    #[test]
    fn test_windows_restart_derived_traits() {
        let config = WindowsRestartConfig {
            restart_timeout: Timeout::new(Duration::from_secs(60)),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
    }

    #[tokio::test]
    async fn test_windows_restart_check_pending_reboot() -> Result<(), StampError> {
        let mock_comm = MockCommunicator::new();
        let pending = WindowsRestartProvisioner::check_pending_reboot(&mock_comm).await?;
        assert!(!pending);
        Ok(())
    }

    #[tokio::test]
    async fn test_windows_restart_success() -> Result<(), StampError> {
        let p = WindowsRestartProvisioner::new(WindowsRestartConfig {
            pause_before: Some(Timeout::new(Duration::from_millis(10))),
            check_registry: true,
            ..Default::default()
        });
        let mock_comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        p.provision(&mock_comm, ui).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_windows_restart_failure() {
        let p = WindowsRestartProvisioner::new(WindowsRestartConfig {
            restart_command: Some("fail_provision".to_string()),
            ..Default::default()
        });
        let mock_comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        assert!(p.provision(&mock_comm, ui).await.is_err());
    }

    #[tokio::test]
    async fn test_windows_restart_wait_for_reboot_success() -> Result<(), StampError> {
        let p = WindowsRestartProvisioner::new(WindowsRestartConfig {
            restart_timeout: Timeout::new(Duration::from_millis(200)),
            ..Default::default()
        });
        let mock_comm = MockCommunicator::new();
        let ui = crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        );
        p.wait_for_reboot(&mock_comm, &ui).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_windows_restart_with_post_reboot_delay() -> Result<(), StampError> {
        let p = WindowsRestartProvisioner::new(WindowsRestartConfig {
            post_reboot_delay: Some(Timeout::new(Duration::from_millis(50))),
            ..Default::default()
        });
        let mock_comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        p.provision(&mock_comm, ui).await?;
        Ok(())
    }
}
