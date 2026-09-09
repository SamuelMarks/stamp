#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `shell-local` provisioner.

use crate::communicator::Communicator;
use crate::error::StampError;
use crate::provisioner::Provisioner;
use std::collections::HashMap;

/// Configuration for the `shell-local` provisioner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellLocalConfig {
    /// The inline script or command to execute.
    pub inline: Option<Vec<String>>,
    /// A single script path to execute locally.
    pub script: Option<String>,
    /// Multiple script paths to execute locally.
    pub scripts: Option<Vec<String>>,
    /// Environment variables to set before executing the script.
    pub environment_vars: Option<Vec<String>>,
    /// Command to use for executing the script.
    pub execute_command: Option<Vec<String>>,
}

/// The `shell-local` provisioner.
#[derive(Debug, Clone)]
pub struct ShellLocalProvisioner {
    /// The provisioner configuration.
    pub config: ShellLocalConfig,
}

impl ShellLocalProvisioner {
    /// Create a new `ShellLocalProvisioner`.
    #[must_use]
    pub const fn new(config: ShellLocalConfig) -> Self {
        Self { config }
    }

    /// Parse environment variables into a `HashMap`.
    fn parse_env_vars(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        if let Some(vars) = &self.config.environment_vars {
            for var in vars {
                if let Some((k, v)) = var.split_once('=') {
                    map.insert(k.to_string(), v.to_string());
                }
            }
        }
        map
    }
}

#[async_trait::async_trait]
impl Provisioner for ShellLocalProvisioner {
    async fn provision(
        &self,
        _comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        let mut has_executed = false;
        let envs = self.parse_env_vars();

        if let Some(inline) = &self.config.inline {
            has_executed = true;
            for cmd in inline {
                // In a real local execution, we'd invoke sh -c or similar
                let mut command = tokio::process::Command::new("sh");
                command.arg("-c").arg(cmd).envs(&envs);
                command.stdout(std::process::Stdio::piped());
                command.stderr(std::process::Stdio::piped());

                let mut child = command.spawn().map_err(StampError::Io)?;
                let stdout = child.stdout.take().ok_or_else(|| {
                    crate::error::StampError::Execution("failed to capture stream".to_string())
                })?;
                let stderr = child.stderr.take().ok_or_else(|| {
                    crate::error::StampError::Execution("failed to capture stream".to_string())
                })?;

                let mut ui_stdout = crate::engine::ui::UiTargetWriter::new(
                    ui.clone(),
                    "shell-local".to_string(),
                    false,
                );
                let mut ui_stderr = crate::engine::ui::UiTargetWriter::new(
                    ui.clone(),
                    "shell-local".to_string(),
                    true,
                );

                let mut out_reader = tokio::io::BufReader::new(stdout);
                let mut err_reader = tokio::io::BufReader::new(stderr);
                let (_, _) = tokio::join!(
                    tokio::io::copy(&mut out_reader, &mut ui_stdout),
                    tokio::io::copy(&mut err_reader, &mut ui_stderr)
                );

                let status = child.wait().await.map_err(StampError::Io)?;

                if !status.success() {
                    return Err(StampError::Parse(format!(
                        "Local command failed with exit code: {:?}",
                        status.code()
                    )));
                }
            }
        }

        let mut all_scripts = Vec::new();
        if let Some(s) = &self.config.script {
            all_scripts.push(s.clone());
        }
        if let Some(scripts) = &self.config.scripts {
            all_scripts.extend(scripts.iter().cloned());
        }

        let default_exec = vec![
            "sh".to_string(),
            "-c".to_string(),
            "{{.Command}}".to_string(),
        ];
        let exec_cmd = self.config.execute_command.clone().unwrap_or(default_exec);

        for script in all_scripts {
            has_executed = true;

            if exec_cmd.is_empty() {
                return Err(StampError::Parse("execute_command is empty".to_string()));
            }

            let mut command = tokio::process::Command::new(&exec_cmd[0]);
            for arg in exec_cmd.iter().skip(1) {
                let replaced_arg = arg.replace("{{.Command}}", &script);
                command.arg(replaced_arg);
            }
            command.envs(&envs);

            command.stdout(std::process::Stdio::piped());
            command.stderr(std::process::Stdio::piped());
            let mut child = command.spawn().map_err(StampError::Io)?;
            let stdout = child.stdout.take().ok_or_else(|| {
                crate::error::StampError::Execution("failed to capture stream".to_string())
            })?;
            let stderr = child.stderr.take().ok_or_else(|| {
                crate::error::StampError::Execution("failed to capture stream".to_string())
            })?;

            let mut ui_stdout = crate::engine::ui::UiTargetWriter::new(
                ui.clone(),
                "shell-local".to_string(),
                false,
            );
            let mut ui_stderr =
                crate::engine::ui::UiTargetWriter::new(ui.clone(), "shell-local".to_string(), true);

            let mut out_reader = tokio::io::BufReader::new(stdout);
            let mut err_reader = tokio::io::BufReader::new(stderr);
            let (_, _) = tokio::join!(
                tokio::io::copy(&mut out_reader, &mut ui_stdout),
                tokio::io::copy(&mut err_reader, &mut ui_stderr)
            );

            let status = child.wait().await.map_err(StampError::Io)?;

            if !status.success() {
                return Err(StampError::Parse(format!(
                    "Local script execution failed with exit code: {:?}",
                    status.code()
                )));
            }
        }

        if !has_executed {
            return Err(StampError::Parse(
                "No commands or scripts provided".to_string(),
            ));
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
    async fn test_shell_local_provision_inline_success() -> Result<(), crate::error::StampError> {
        let config = ShellLocalConfig {
            inline: Some(vec!["exit 0".to_string()]),
            ..Default::default()
        };
        let prov = ShellLocalProvisioner::new(config);
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
    async fn test_shell_local_provision_inline_failure() -> Result<(), crate::error::StampError> {
        let config = ShellLocalConfig {
            inline: Some(vec!["exit 1".to_string()]),
            ..Default::default()
        };
        let prov = ShellLocalProvisioner::new(config);
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
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_shell_local_provision_no_commands() {
        let config = ShellLocalConfig::default();
        let prov = ShellLocalProvisioner::new(config);
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
        assert!(matches!(result, Err(StampError::Parse(_))));
    }

    #[tokio::test]
    async fn test_shell_local_provision_script() -> Result<(), crate::error::StampError> {
        let config = ShellLocalConfig {
            script: Some("echo hello".to_string()),
            ..Default::default()
        };
        let prov = ShellLocalProvisioner::new(config);
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
    async fn test_shell_local_provision_script_failure() -> Result<(), crate::error::StampError> {
        let config = ShellLocalConfig {
            script: Some("exit 1".to_string()),
            ..Default::default()
        };
        let prov = ShellLocalProvisioner::new(config);
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
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_shell_local_provision_scripts() -> Result<(), crate::error::StampError> {
        let config = ShellLocalConfig {
            scripts: Some(vec!["echo script1".to_string(), "echo script2".to_string()]),
            environment_vars: Some(vec!["FOO=bar".to_string(), "INVALID_VAR".to_string()]),
            ..Default::default()
        };
        let prov = ShellLocalProvisioner::new(config);
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
    async fn test_shell_local_empty_exec_command() {
        let config = ShellLocalConfig {
            script: Some("echo 1".to_string()),
            execute_command: Some(vec![]),
            ..Default::default()
        };
        let prov = ShellLocalProvisioner::new(config);
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
        assert!(matches!(result, Err(StampError::Parse(_))));
    }

    #[test]
    fn test_derived_traits() {
        let config1 = ShellLocalConfig {
            inline: Some(vec!["echo hi".to_string()]),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = ShellLocalProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
