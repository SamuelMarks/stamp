//! Implementation of the `converge` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use std::path::PathBuf;
use std::time::Duration;

/// Configuration for the `converge` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvergeConfig {
    /// The inline script or commands to execute.
    pub inline: Option<Vec<String>>,
    /// Path to a script to upload and execute.
    pub script: Option<FilePath>,
    /// List of paths to scripts to upload and execute.
    pub scripts: Option<Vec<FilePath>>,
    /// Maximum number of retries per command until convergence.
    pub max_retries: u32,
    /// Delay between retries in milliseconds.
    pub retry_delay_ms: u64,
    /// Optional verification command to run after convergence.
    pub verify_command: Option<String>,
}

impl Default for ConvergeConfig {
    fn default() -> Self {
        Self {
            inline: None,
            script: None,
            scripts: None,
            max_retries: 3,
            retry_delay_ms: 1000,
            verify_command: None,
        }
    }
}

/// The `converge` provisioner.
#[derive(Debug, Clone)]
pub struct ConvergeProvisioner {
    /// The provisioner configuration.
    pub config: ConvergeConfig,
}

impl ConvergeProvisioner {
    /// Create a new `ConvergeProvisioner`.
    #[must_use]
    pub const fn new(config: ConvergeConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Provisioner for ConvergeProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        let mut all_commands = Vec::new();

        if let Some(inline) = &self.config.inline {
            all_commands.extend(inline.clone());
        }

        let mut all_scripts = Vec::new();
        if let Some(s) = &self.config.script {
            all_scripts.push(s.clone());
        }
        if let Some(scripts) = &self.config.scripts {
            all_scripts.extend(scripts.iter().cloned());
        }

        for (i, script) in all_scripts.iter().enumerate() {
            let remote_path = format!("/tmp/converge_script_{i}.sh");
            let remote_fp = FilePath::new(PathBuf::from(&remote_path));
            comm.upload(script, &remote_fp).await?;
            let chmod_cmd = format!("chmod +x {remote_path}");
            let _ = comm.execute(&Command::new(chmod_cmd)).await;
            all_commands.push(remote_path);
        }

        if all_commands.is_empty() {
            return Err(StampError::Provisioner(
                "No commands or scripts provided for converge".to_string(),
            ));
        }

        for cmd in all_commands {
            let mut converged = false;
            for attempt in 0..=self.config.max_retries {
                ui.say("converge", &format!("Executing attempt {attempt}: {cmd}"));
                let res = comm.execute(&Command::new(cmd.clone())).await?;
                if res.exit_code == 0 {
                    converged = true;
                    break;
                }
                if attempt < self.config.max_retries {
                    tokio::time::sleep(Duration::from_millis(self.config.retry_delay_ms)).await;
                }
            }

            if !converged {
                return Err(StampError::Provisioner(format!(
                    "Command failed to converge after {} retries: {}",
                    self.config.max_retries, cmd
                )));
            }
        }

        if let Some(verify) = &self.config.verify_command {
            ui.say(
                "converge",
                &format!("Running verification command: {verify}"),
            );
            let res = comm.execute(&Command::new(verify.clone())).await?;
            if res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "Verification command failed with exit code: {}",
                    res.exit_code
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;

    #[tokio::test]
    async fn test_converge_provision_success() -> Result<(), StampError> {
        let config = ConvergeConfig {
            inline: Some(vec!["echo hi".to_string()]),
            verify_command: Some("test 1 -eq 1".to_string()),
            ..Default::default()
        };
        let prov = ConvergeProvisioner::new(config);
        let comm = MockCommunicator::new();
        prov.provision(
            &comm,
            std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_converge_provision_with_scripts() -> Result<(), StampError> {
        let config = ConvergeConfig {
            script: Some(FilePath::new(PathBuf::from("setup.sh"))),
            retry_delay_ms: 10,
            ..Default::default()
        };
        let prov = ConvergeProvisioner::new(config);
        let comm = MockCommunicator::new();
        prov.provision(
            &comm,
            std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_converge_provision_failure_empty() {
        let config = ConvergeConfig::default();
        let prov = ConvergeProvisioner::new(config);
        let comm = MockCommunicator::new();
        let result = prov
            .provision(
                &comm,
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
            )
            .await;
        assert!(matches!(result, Err(StampError::Provisioner(_))));
    }

    #[tokio::test]
    async fn test_converge_provision_failure_retries() {
        let config = ConvergeConfig {
            inline: Some(vec!["fail_provision".to_string()]),
            max_retries: 1,
            retry_delay_ms: 5,
            ..Default::default()
        };
        let prov = ConvergeProvisioner::new(config);
        let comm = MockCommunicator::new();
        let result = prov
            .provision(
                &comm,
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
            )
            .await;
        assert!(matches!(result, Err(StampError::Provisioner(_))));
    }

    #[test]
    fn test_derived_traits() {
        let config1 = ConvergeConfig {
            inline: Some(vec!["echo hi".to_string()]),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = ConvergeProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
