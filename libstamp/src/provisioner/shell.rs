//! Implementation of the `shell` provisioner.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::{FilePath, Timeout};
use std::path::PathBuf;

/// Configuration for the `shell` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellConfig {
    /// The inline script or commands to execute.
    pub inline: Option<Vec<String>>,
    /// Path to a script to upload and execute.
    pub script: Option<FilePath>,
    /// List of paths to scripts to upload and execute.
    pub scripts: Option<Vec<FilePath>>,
    /// Environment variables to set before executing the script.
    pub environment_vars: Option<Vec<String>>,
    /// The remote path where scripts will be uploaded. Defaults to `/tmp/script.sh`.
    pub remote_path: Option<String>,
    /// The remote folder where temporary scripts will be uploaded if `remote_path` is not specified.
    pub remote_folder: Option<String>,
    /// The command used to execute the script. E.g. `chmod +x {{ .Path }}; {{ .Vars }} {{ .Path }}`.
    pub execute_command: Option<String>,
    /// Whether to clean up uploaded scripts upon completion or failure. Defaults to true.
    pub clean_up: bool,
    /// List of exit codes considered successful. Defaults to `[0]`.
    pub valid_exit_codes: Option<Vec<i32>>,
    /// Optional pause duration before executing commands.
    pub pause_before: Option<Timeout>,
    /// Optional pause duration after executing commands.
    pub pause_after: Option<Timeout>,
    /// Optional execution timeout for each command or script.
    pub timeout: Option<Timeout>,
    /// Maximum number of retry attempts for failed commands or scripts. Defaults to 0.
    pub max_retries: u32,
    /// Whether the script is an executable binary rather than an interpreted shell script. Defaults to false.
    pub binary: bool,
    /// Whether to use shebang line execution or default shell interpreter. Defaults to true.
    pub use_shebang: bool,
    /// Whether to expect a connection disconnect during execution (e.g. system reboot or network restart).
    pub expect_disconnect: bool,
    /// Optional list of builders this provisioner only applies to.
    pub only: Option<Vec<String>>,
    /// Optional list of builders this provisioner does not apply to.
    pub except: Option<Vec<String>>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            inline: None,
            script: None,
            scripts: None,
            environment_vars: None,
            remote_path: None,
            remote_folder: None,
            execute_command: None,
            clean_up: true,
            valid_exit_codes: None,
            pause_before: None,
            pause_after: None,
            timeout: None,
            max_retries: 0,
            binary: false,
            use_shebang: true,
            expect_disconnect: false,
            only: None,
            except: None,
        }
    }
}

/// The `shell` provisioner.
#[derive(Debug, Clone)]
pub struct ShellProvisioner {
    /// The provisioner configuration.
    pub config: ShellConfig,
}

impl ShellProvisioner {
    /// Create a new `ShellProvisioner`.
    #[must_use]
    pub const fn new(config: ShellConfig) -> Self {
        Self { config }
    }

    /// Creates a new `ShellProvisioner` from a `ProvisionerConfig`.
    #[must_use]
    pub fn from_provisioner_config(config: &crate::template::ProvisionerConfig) -> Self {
        use std::str::FromStr as _;

        let inline = config.config.get("inline").map(|s| {
            if let Ok(vec) = serde_json::from_str::<Vec<String>>(s) {
                vec
            } else {
                vec![s.clone()]
            }
        });
        let script = config
            .config
            .get("script")
            .map(|s| FilePath::new(PathBuf::from(s)));
        let scripts = config.config.get("scripts").map(|s| {
            let list: Vec<String> = serde_json::from_str(s).unwrap_or_else(|_| vec![s.clone()]);
            list.into_iter()
                .map(|p| FilePath::new(PathBuf::from(p)))
                .collect()
        });
        let environment_vars = config.config.get("environment_vars").map(|s| {
            if let Ok(vec) = serde_json::from_str::<Vec<String>>(s) {
                vec
            } else {
                vec![s.clone()]
            }
        });
        let remote_path = config.config.get("remote_path").cloned();
        let remote_folder = config.config.get("remote_folder").cloned();
        let execute_command = config.config.get("execute_command").cloned();
        let clean_up = config.config.get("clean_up").map_or(true, |v| v != "false");
        let valid_exit_codes = config.config.get("valid_exit_codes").map(|s| {
            if let Ok(vec) = serde_json::from_str::<Vec<i32>>(s) {
                vec
            } else if let Ok(c) = s.parse::<i32>() {
                vec![c]
            } else {
                vec![0]
            }
        });
        let pause_before = config
            .config
            .get("pause_before")
            .and_then(|s| Timeout::from_str(s).ok());
        let pause_after = config
            .config
            .get("pause_after")
            .and_then(|s| Timeout::from_str(s).ok());
        let timeout = config
            .config
            .get("timeout")
            .and_then(|s| Timeout::from_str(s).ok());
        let max_retries = config
            .config
            .get("max_retries")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let binary = config.config.get("binary").is_some_and(|v| v == "true");
        let use_shebang = config
            .config
            .get("use_shebang")
            .map_or(true, |v| v != "false");
        let expect_disconnect = config
            .config
            .get("expect_disconnect")
            .is_some_and(|v| v == "true");

        let only = if config.only.is_empty() {
            None
        } else {
            Some(config.only.clone())
        };
        let except = if config.except.is_empty() {
            None
        } else {
            Some(config.except.clone())
        };

        Self::new(ShellConfig {
            inline,
            script,
            scripts,
            environment_vars,
            remote_path,
            remote_folder,
            execute_command,
            clean_up,
            valid_exit_codes,
            pause_before,
            pause_after,
            timeout,
            max_retries,
            binary,
            use_shebang,
            expect_disconnect,
            only,
            except,
        })
    }

    /// Formats environment variables with POSIX shell escaping.
    #[must_use]
    pub fn format_environment_vars(vars: &[String]) -> String {
        vars.iter()
            .map(|v| {
                if let Some((key, val)) = v.split_once('=') {
                    let escaped = val.replace('\'', "'\\''");
                    format!("{key}='{escaped}'")
                } else {
                    v.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Build the command string replacing `{{ .Path }}` and `{{ .Vars }}`.
    #[must_use]
    pub fn build_command(&self, remote_path: &str) -> String {
        let default_cmd = if self.config.binary {
            "chmod +x {{ .Path }}; {{ .Vars }} {{ .Path }}".to_string()
        } else if !self.config.use_shebang {
            "chmod +x {{ .Path }}; {{ .Vars }} /bin/sh {{ .Path }}".to_string()
        } else {
            "chmod +x {{ .Path }}; {{ .Vars }} {{ .Path }}".to_string()
        };
        let cmd_template = self.config.execute_command.as_ref().unwrap_or(&default_cmd);

        let env_vars = self
            .config
            .environment_vars
            .as_ref()
            .map_or_else(String::new, |vars| Self::format_environment_vars(vars));

        let mut replaced = cmd_template
            .replace("{{ .Path }}", remote_path)
            .replace("{{ .Vars }}", &env_vars);

        // Normalize spacing if vars was empty
        if env_vars.is_empty() {
            replaced = replaced.replace("  ", " ");
        }

        replaced.trim().to_string()
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
        Self::execute_command_advanced(comm, cmd, valid_codes, timeout, max_retries, false, ui)
            .await
    }

    /// Executes a command with timeout, retry, and disconnect handling.
    ///
    /// # Errors
    ///
    /// Returns [`StampError::CommunicatorTimeout`] if execution times out, or [`StampError::Provisioner`]
    /// if the command fails with an invalid exit code.
    pub async fn execute_command_advanced(
        comm: &dyn Communicator,
        cmd: &str,
        valid_codes: &[i32],
        timeout: Option<Timeout>,
        max_retries: u32,
        expect_disconnect: bool,
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
                            "shell",
                            &format!(
                                "Command timed out after {}s, retrying (attempt {attempts}/{max_retries})...",
                                to.0.as_secs()
                            ),
                        );
                        continue;
                    }
                    return Err(StampError::CommunicatorTimeout {
                        target: "shell".to_string(),
                        timeout_secs: to.0.as_secs(),
                    });
                }
            } else {
                exec_fut.await
            };

            match res {
                Ok(exec_result) => {
                    Self::stream_output(ui, &exec_result.stdout, &exec_result.stderr);
                    if valid_codes.contains(&exec_result.exit_code) || expect_disconnect {
                        return Ok(());
                    }
                    if attempts < max_retries {
                        attempts += 1;
                        ui.say(
                            "shell",
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
                    if expect_disconnect {
                        ui.say(
                            "shell",
                            "Connection severed as expected (expect_disconnect = true)",
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        return Ok(());
                    }
                    if attempts < max_retries {
                        attempts += 1;
                        ui.say(
                            "shell",
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

    /// Streams command stdout and stderr to the Stamp UI system.
    fn stream_output(ui: &crate::engine::ui::Ui, stdout: &str, stderr: &str) {
        if !stdout.is_empty() {
            for line in stdout.lines() {
                ui.say("shell", line);
            }
        }
        if !stderr.is_empty() {
            for line in stderr.lines() {
                ui.error("shell", line);
            }
        }
    }
}

#[async_trait::async_trait]
impl Provisioner for ShellProvisioner {
    fn only(&self) -> Option<&[String]> {
        self.config.only.as_deref()
    }

    fn except(&self) -> Option<&[String]> {
        self.config.except.as_deref()
    }

    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if let Some(pause) = &self.config.pause_before {
            tokio::time::sleep(pause.0).await;
        }

        let mut has_executed = false;
        let default_valid_codes = vec![0];
        let valid_codes = self
            .config
            .valid_exit_codes
            .as_deref()
            .unwrap_or(&default_valid_codes);

        if let Some(inline) = &self.config.inline {
            has_executed = true;
            for cmd in inline {
                let env_vars = self
                    .config
                    .environment_vars
                    .as_ref()
                    .map_or_else(String::new, |vars| Self::format_environment_vars(vars));

                let full_cmd = if env_vars.is_empty() {
                    cmd.clone()
                } else {
                    format!("{env_vars} {cmd}")
                };

                Self::execute_command_advanced(
                    comm,
                    &full_cmd,
                    valid_codes,
                    self.config.timeout,
                    self.config.max_retries,
                    self.config.expect_disconnect,
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

        let folder = self.config.remote_folder.as_deref().unwrap_or("/tmp");

        for (i, script) in all_scripts.iter().enumerate() {
            has_executed = true;
            let remote_path_str = self
                .config
                .remote_path
                .clone()
                .unwrap_or_else(|| format!("{folder}/script_{i}.sh"));

            let remote_path = FilePath::new(PathBuf::from(&remote_path_str));

            comm.upload(script, &remote_path).await?;

            let exec_cmd = self.build_command(&remote_path_str);
            let res = Self::execute_command_advanced(
                comm,
                &exec_cmd,
                valid_codes,
                self.config.timeout,
                self.config.max_retries,
                self.config.expect_disconnect,
                &ui,
            )
            .await;

            // Attempt script cleanup if configured
            if self.config.clean_up {
                let rm_cmd = format!("rm -f {remote_path_str}");
                let _ = comm.execute(&Command::new(rm_cmd)).await;
            }

            res?;
        }

        if !has_executed {
            return Err(StampError::Provisioner(
                "No commands or scripts provided".to_string(),
            ));
        }

        if let Some(pause) = &self.config.pause_after {
            tokio::time::sleep(pause.0).await;
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
    async fn test_shell_provision_inline_success() -> Result<(), StampError> {
        let config = ShellConfig {
            inline: Some(vec!["echo hi".to_string()]),
            ..Default::default()
        };
        let prov = ShellProvisioner::new(config);
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
    async fn test_shell_provision_inline_with_env() -> Result<(), StampError> {
        let config = ShellConfig {
            inline: Some(vec!["echo hi".to_string()]),
            environment_vars: Some(vec![
                "FOO=bar baz".to_string(),
                "SINGLE='quotes'".to_string(),
            ]),
            ..Default::default()
        };
        let prov = ShellProvisioner::new(config);
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
    async fn test_shell_provision_failure() -> Result<(), StampError> {
        let config = ShellConfig {
            inline: Some(vec!["fail_provision".to_string()]),
            ..Default::default()
        };
        let prov = ShellProvisioner::new(config);
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
    async fn test_shell_provision_no_commands() {
        let config = ShellConfig::default();
        let prov = ShellProvisioner::new(config);
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
    async fn test_shell_provision_script() -> Result<(), StampError> {
        let config = ShellConfig {
            script: Some(FilePath::new(PathBuf::from("local.sh"))),
            execute_command: Some(
                "chmod +x {{ .Path }}; {{ .Vars }} sudo -E {{ .Path }}".to_string(),
            ),
            remote_folder: Some("/var/tmp".to_string()),
            clean_up: true,
            ..Default::default()
        };
        let prov = ShellProvisioner::new(config);
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
    async fn test_shell_provision_scripts_and_env() -> Result<(), StampError> {
        let config = ShellConfig {
            scripts: Some(vec![FilePath::new(PathBuf::from("local.sh"))]),
            environment_vars: Some(vec!["A=1".to_string()]),
            ..Default::default()
        };
        let prov = ShellProvisioner::new(config);
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
    async fn test_shell_provision_script_failure() -> Result<(), StampError> {
        let config = ShellConfig {
            script: Some(FilePath::new(PathBuf::from("fail.sh"))),
            execute_command: Some("fail_provision".to_string()),
            ..Default::default()
        };
        let prov = ShellProvisioner::new(config);
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
    async fn test_shell_provision_valid_exit_codes_and_binary() -> Result<(), StampError> {
        let config = ShellConfig {
            inline: Some(vec!["fail_provision".to_string()]),
            valid_exit_codes: Some(vec![1]),
            binary: true,
            use_shebang: false,
            ..Default::default()
        };
        let prov = ShellProvisioner::new(config);
        assert_eq!(
            prov.build_command("/bin/foo"),
            "chmod +x /bin/foo; /bin/foo"
        );

        let non_binary = ShellProvisioner::new(ShellConfig {
            use_shebang: false,
            binary: false,
            ..Default::default()
        });
        assert_eq!(
            non_binary.build_command("/bin/bar"),
            "chmod +x /bin/bar; /bin/sh /bin/bar"
        );

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
    async fn test_shell_provision_retries_and_timeout() -> Result<(), StampError> {
        let config = ShellConfig {
            inline: Some(vec!["fail_provision".to_string()]),
            max_retries: 1,
            timeout: Some(crate::types::Timeout::new(
                std::time::Duration::from_millis(50),
            )),
            ..Default::default()
        };
        let prov = ShellProvisioner::new(config);
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

    #[tokio::test]
    async fn test_shell_provision_pause_before() -> Result<(), StampError> {
        let config = ShellConfig {
            inline: Some(vec!["echo delayed".to_string()]),
            pause_before: Some(crate::types::Timeout::new(
                std::time::Duration::from_millis(10),
            )),
            ..Default::default()
        };
        let prov = ShellProvisioner::new(config);
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
    async fn test_shell_expect_disconnect_and_pause_after() -> Result<(), StampError> {
        let config = ShellConfig {
            inline: Some(vec!["fail_provision".to_string()]),
            expect_disconnect: true,
            pause_after: Some(Timeout::new(std::time::Duration::from_millis(5))),
            only: Some(vec!["qemu.vm".to_string()]),
            except: Some(vec!["docker".to_string()]),
            ..Default::default()
        };
        let prov = ShellProvisioner::new(config);
        assert_eq!(prov.only(), Some(&["qemu.vm".to_string()][..]));
        assert_eq!(prov.except(), Some(&["docker".to_string()][..]));

        let comm = MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        assert!(prov.provision(&comm, ui).await.is_ok());
        Ok(())
    }

    #[test]
    fn test_shell_from_provisioner_config() {
        let mut cfg = crate::template::ProvisionerConfig {
            provisioner_type: "shell".to_string(),
            only: vec!["qemu".to_string()],
            except: vec!["vmware".to_string()],
            ..Default::default()
        };
        cfg.config
            .insert("inline".to_string(), "echo 1".to_string());
        cfg.config
            .insert("scripts".to_string(), "[\"s1.sh\"]".to_string());
        cfg.config
            .insert("environment_vars".to_string(), "[\"FOO=bar\"]".to_string());
        cfg.config
            .insert("valid_exit_codes".to_string(), "[0, 143]".to_string());
        cfg.config
            .insert("pause_before".to_string(), "1s".to_string());
        cfg.config
            .insert("pause_after".to_string(), "500ms".to_string());
        cfg.config.insert("timeout".to_string(), "2m".to_string());
        cfg.config
            .insert("expect_disconnect".to_string(), "true".to_string());
        cfg.config.insert("binary".to_string(), "true".to_string());

        let prov = ShellProvisioner::from_provisioner_config(&cfg);
        assert_eq!(prov.config.expect_disconnect, true);
        assert_eq!(prov.config.binary, true);
        assert_eq!(prov.config.valid_exit_codes, Some(vec![0, 143]));
        assert!(prov.config.pause_before.is_some());
        assert!(prov.config.pause_after.is_some());
    }

    #[test]
    fn test_derived_traits() {
        let config1 = ShellConfig {
            inline: Some(vec!["echo hi".to_string()]),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = ShellProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
