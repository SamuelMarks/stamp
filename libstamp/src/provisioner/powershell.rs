//! Implementation of the `powershell` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::{FilePath, Timeout};
use std::path::PathBuf;

/// Configuration for the `powershell` provisioner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PowershellConfig {
    /// The inline script or commands to execute.
    pub inline: Option<Vec<String>>,
    /// Path to a script to upload and execute.
    pub script: Option<FilePath>,
    /// List of paths to scripts to upload and execute.
    pub scripts: Option<Vec<FilePath>>,
    /// Environment variables to set before executing the script.
    pub environment_vars: Option<Vec<String>>,
    /// The remote path where scripts will be uploaded. Defaults to `C:\Windows\Temp\script.ps1`.
    pub remote_path: Option<String>,
    /// The command used to execute the script.
    pub execute_command: Option<String>,
    /// Valid exit codes. Defaults to `[0]`.
    pub valid_exit_codes: Option<Vec<i32>>,
    /// Optional user for elevated execution.
    pub elevated_user: Option<String>,
    /// Optional password for elevated user.
    pub elevated_password: Option<String>,
    /// Optional execution timeout for each command or script.
    pub timeout: Option<Timeout>,
    /// Maximum number of retry attempts for failed commands or scripts. Defaults to 0.
    pub max_retries: u32,
    /// Whether to clean up uploaded scripts upon completion or failure. Defaults to true.
    pub clean_up: bool,
}

/// The `powershell` provisioner.
#[derive(Debug, Clone)]
pub struct PowershellProvisioner {
    /// The provisioner configuration.
    pub config: PowershellConfig,
}

impl PowershellProvisioner {
    /// Create a new `PowershellProvisioner`.
    #[must_use]
    pub const fn new(config: PowershellConfig) -> Self {
        Self { config }
    }

    /// Wraps a command for elevated execution via Windows Scheduled Tasks if credentials are configured.
    #[must_use]
    pub fn wrap_elevated_command(&self, cmd: &str) -> String {
        if let Some(user) = &self.config.elevated_user {
            let pass_arg = self
                .config
                .elevated_password
                .as_deref()
                .map_or_else(String::new, |p| format!(" /rp \"{p}\""));
            let task_name = format!("StampElevatedTask_{}", uuid::Uuid::new_v4().simple());
            format!(
                "schtasks /create /tn \"{task_name}\" /tr \"{cmd}\" /sc once /st 00:00 /ru \"{user}\"{pass_arg} /f /rl HIGHEST && \
                 schtasks /run /tn \"{task_name}\" && \
                 schtasks /delete /tn \"{task_name}\" /f"
            )
        } else {
            cmd.to_string()
        }
    }

    /// Build the command string replacing `{{ .Path }}` and `{{ .Vars }}`.
    #[must_use]
    pub fn build_command(&self, remote_path: &str) -> String {
        let default_cmd =
            "powershell -ExecutionPolicy Bypass -NoProfile -NonInteractive -Command \"$ErrorActionPreference = 'Stop'; & '{{ .Path }}'\""
                .to_string();
        let cmd_template = self.config.execute_command.as_ref().unwrap_or(&default_cmd);

        let env_vars = if let Some(vars) = &self.config.environment_vars {
            vars.iter()
                .map(|v| {
                    if let Some((key, val)) = v.split_once('=') {
                        let escaped = val.replace('"', "`\"");
                        format!("$env:{key}=\"{escaped}\";")
                    } else {
                        String::new()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            String::new()
        };

        let cmd = cmd_template.replace("{{ .Path }}", remote_path);

        let full_cmd = if env_vars.is_empty() {
            cmd.trim().to_string()
        } else {
            format!("{env_vars} {cmd}").trim().to_string()
        };

        self.wrap_elevated_command(&full_cmd)
    }

    /// Executes a command with timeout and retry handling against the provided communicator.
    ///
    /// # Errors
    ///
    /// Returns [`StampError::CommunicatorTimeout`] if execution times out, or [`StampError::Provisioner`]
    /// if the command fails with an invalid exit code.
    pub async fn execute_command_with_retries(
        comm: &dyn Communicator,
        cmd: &str,
        valid_codes: &[i32],
        timeout: Option<Timeout>,
        max_retries: u32,
        ui: &crate::engine::ui::Ui,
    ) -> Result<(), StampError> {
        let mut attempts = 0;
        loop {
            let command = Command::new(cmd.to_string());
            let exec_fut = comm.execute(&command);
            let res = if let Some(to) = timeout {
                if let Ok(r) = tokio::time::timeout(to.0, exec_fut).await {
                    r
                } else {
                    if attempts < max_retries {
                        attempts += 1;
                        ui.say(
                            "powershell",
                            &format!(
                                "Command timed out after {}s, retrying (attempt {attempts}/{max_retries})...",
                                to.0.as_secs()
                            ),
                        );
                        continue;
                    }
                    return Err(StampError::CommunicatorTimeout {
                        target: "powershell".to_string(),
                        timeout_secs: to.0.as_secs(),
                    });
                }
            } else {
                exec_fut.await
            };

            match res {
                Ok(exec_result) => {
                    if !exec_result.stdout.is_empty() {
                        for line in exec_result.stdout.lines() {
                            ui.say("powershell", line);
                        }
                    }
                    if !exec_result.stderr.is_empty() {
                        for line in exec_result.stderr.lines() {
                            ui.error("powershell", line);
                        }
                    }

                    if valid_codes.contains(&exec_result.exit_code) {
                        return Ok(());
                    }
                    if attempts < max_retries {
                        attempts += 1;
                        ui.say(
                            "powershell",
                            &format!(
                                "Command failed with exit code {}, retrying (attempt {attempts}/{max_retries})...",
                                exec_result.exit_code
                            ),
                        );
                        continue;
                    }
                    return Err(StampError::Provisioner(format!(
                        "Command failed with exit code: {}",
                        exec_result.exit_code
                    )));
                }
                Err(err) => {
                    if attempts < max_retries {
                        attempts += 1;
                        ui.say(
                            "powershell",
                            &format!(
                                "Command execution error: {err}, retrying (attempt {attempts}/{max_retries})..."
                            ),
                        );
                        continue;
                    }
                    return Err(err);
                }
            }
        }
    }
}

#[async_trait::async_trait]
#[cfg_attr(coverage_nightly, coverage(off))]
impl Provisioner for PowershellProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        let mut has_executed = false;
        let valid_codes = self
            .config
            .valid_exit_codes
            .clone()
            .unwrap_or_else(|| vec![0]);

        if let Some(inline) = &self.config.inline {
            has_executed = true;
            for cmd in inline {
                let env_vars = if let Some(vars) = &self.config.environment_vars {
                    vars.iter()
                        .map(|v| {
                            if let Some((key, val)) = v.split_once('=') {
                                let escaped = val.replace('"', "`\"");
                                format!("$env:{key}=\"{escaped}\";")
                            } else {
                                String::new()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    String::new()
                };

                let full_script =
                    format!("$ErrorActionPreference = [char]39 + 'Stop' + [char]39; {cmd}");
                let raw_ps = format!(
                    "powershell -ExecutionPolicy Bypass -NoProfile -NonInteractive -Command \"{env_vars} {full_script}\""
                );
                let final_cmd = self.wrap_elevated_command(&raw_ps);

                Self::execute_command_with_retries(
                    comm,
                    &final_cmd,
                    &valid_codes,
                    self.config.timeout,
                    self.config.max_retries,
                    &ui,
                )
                .await?;
            }
        }

        let mut all_scripts = Vec::new();
        if let Some(s) = &self.config.script {
            all_scripts.push(s.clone());
        }
        if let Some(scripts) = &self.config.scripts {
            all_scripts.extend(scripts.iter().cloned());
        }

        for (i, script) in all_scripts.iter().enumerate() {
            has_executed = true;
            let remote_path_str = self
                .config
                .remote_path
                .clone()
                .unwrap_or_else(|| format!("C:\\Windows\\Temp\\script_{i}.ps1"));

            let remote_path = FilePath::new(PathBuf::from(&remote_path_str));

            comm.upload(script, &remote_path).await?;

            let exec_cmd = self.build_command(&remote_path_str);
            let res = Self::execute_command_with_retries(
                comm,
                &exec_cmd,
                &valid_codes,
                self.config.timeout,
                self.config.max_retries,
                &ui,
            )
            .await;

            if self.config.clean_up {
                let rm_cmd = format!("cmd /c del /f /q \"{remote_path_str}\"");
                let _ = comm.execute(&Command::new(rm_cmd)).await;
            }

            res?;
        }

        if !has_executed {
            return Err(StampError::Provisioner(
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
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_powershell_provision_inline_success() -> Result<(), StampError> {
        let config = PowershellConfig {
            inline: Some(vec!["Write-Output hi".to_string()]),
            ..Default::default()
        };
        let prov = PowershellProvisioner::new(config);
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
    async fn test_powershell_provision_inline_with_env() -> Result<(), StampError> {
        let config = PowershellConfig {
            inline: Some(vec!["Write-Output hi".to_string()]),
            environment_vars: Some(vec![
                "FOO=bar".to_string(),
                "SPACE=hello \"world\"".to_string(),
            ]),
            elevated_user: Some("Administrator".to_string()),
            elevated_password: Some("secret123".to_string()),
            ..Default::default()
        };
        let prov = PowershellProvisioner::new(config);
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
    async fn test_powershell_provision_valid_exit_codes() -> Result<(), StampError> {
        let config = PowershellConfig {
            inline: Some(vec!["Write-Output hi".to_string()]),
            valid_exit_codes: Some(vec![0, 1]),
            ..Default::default()
        };
        let prov = PowershellProvisioner::new(config);
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
    async fn test_powershell_provision_failure() -> Result<(), StampError> {
        let config = PowershellConfig {
            inline: Some(vec!["fail_provision".to_string()]),
            ..Default::default()
        };
        let prov = PowershellProvisioner::new(config);
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
        Ok(())
    }

    #[tokio::test]
    async fn test_powershell_provision_no_commands() {
        let config = PowershellConfig::default();
        let prov = PowershellProvisioner::new(config);
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
    async fn test_powershell_provision_script() -> Result<(), StampError> {
        let config = PowershellConfig {
            script: Some(FilePath::new(PathBuf::from("local.ps1"))),
            execute_command: Some(
                "powershell -ExecutionPolicy Bypass -File {{ .Path }}".to_string(),
            ),
            ..Default::default()
        };
        let prov = PowershellProvisioner::new(config);
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
    async fn test_powershell_provision_scripts_and_env() -> Result<(), StampError> {
        let config = PowershellConfig {
            scripts: Some(vec![FilePath::new(PathBuf::from("local.ps1"))]),
            environment_vars: Some(vec!["A=1".to_string()]),
            ..Default::default()
        };
        let prov = PowershellProvisioner::new(config);
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
    async fn test_powershell_provision_script_failure() -> Result<(), StampError> {
        let config = PowershellConfig {
            script: Some(FilePath::new(PathBuf::from("fail.ps1"))),
            execute_command: Some("fail_provision".to_string()),
            ..Default::default()
        };
        let prov = PowershellProvisioner::new(config);
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
        Ok(())
    }

    #[tokio::test]
    async fn test_powershell_provision_retries_and_timeout() -> Result<(), StampError> {
        let config = PowershellConfig {
            inline: Some(vec!["fail_provision".to_string()]),
            max_retries: 1,
            timeout: Some(crate::types::Timeout::new(
                std::time::Duration::from_millis(50),
            )),
            clean_up: true,
            ..Default::default()
        };
        let prov = PowershellProvisioner::new(config);
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
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config1 = PowershellConfig {
            inline: Some(vec!["Write-Output hi".to_string()]),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = PowershellProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
