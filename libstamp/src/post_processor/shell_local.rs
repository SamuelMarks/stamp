//! Implementation of the `shell_local` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use std::collections::HashMap;

/// Configuration for the `shell_local` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellLocalConfig {
    /// Identifier for this post-processor.
    pub identifier: String,
    /// Inline shell commands to execute locally.
    pub inline: Option<Vec<String>>,
    /// Path to a local script file to execute.
    pub script: Option<String>,
    /// Environment variables to pass to the local shell process.
    pub environment_vars: HashMap<String, String>,
    /// Whether to keep the input artifact.
    pub keep_input_artifact: bool,
}

/// The `shell_local` post-processor.
#[derive(Debug, Clone)]
pub struct ShellLocalPostProcessor {
    /// The configuration.
    pub config: ShellLocalConfig,
}

impl ShellLocalPostProcessor {
    /// Create a new `ShellLocalPostProcessor`.
    #[must_use]
    pub const fn new(config: ShellLocalConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for ShellLocalPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty()
            && self.config.inline.is_none()
            && self.config.script.is_none()
        {
            return Err(StampError::Parse("Identifier is empty".to_string()));
        }

        if let Some(ref cmds) = self.config.inline {
            for cmd_str in cmds {
                let output = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd_str)
                    .envs(&self.config.environment_vars)
                    .env("PACKER_ARTIFACT_ID", &artifact.id)
                    .output()
                    .await
                    .map_err(StampError::Io)?;
                if !output.status.success() {
                    return Err(StampError::PostProcessor(format!(
                        "shell-local command '{cmd_str}' failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
            }
        }

        if let Some(ref script_path) = self.config.script {
            let output = tokio::process::Command::new("sh")
                .arg(script_path)
                .envs(&self.config.environment_vars)
                .env("PACKER_ARTIFACT_ID", &artifact.id)
                .output()
                .await
                .map_err(StampError::Io)?;
            if !output.status.success() {
                return Err(StampError::PostProcessor(format!(
                    "shell-local script '{script_path}' failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
        }

        let mut new_artifact = artifact.clone();
        if !self.config.identifier.is_empty() {
            new_artifact.id = format!("{}-{}", artifact.id, self.config.identifier);
        }

        Ok(new_artifact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shell_local_process_success() -> Result<(), StampError> {
        let mut env_vars = HashMap::new();
        env_vars.insert("CUSTOM_ENV".to_string(), "1".to_string());
        let config = ShellLocalConfig {
            identifier: "processed".to_string(),
            inline: Some(vec!["true".to_string()]),
            script: None,
            environment_vars: env_vars,
            keep_input_artifact: true,
        };
        let processor = ShellLocalPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "base-processed");
        assert!(processor.keep_input_artifact());
        assert!(processor.only().is_none());
        assert!(processor.except().is_none());

        // Test inline failure
        let fail_proc = ShellLocalPostProcessor::new(ShellLocalConfig {
            identifier: "fail".to_string(),
            inline: Some(vec!["false".to_string()]),
            ..Default::default()
        });
        assert!(
            fail_proc
                .process(Artifact::new("base".to_string(), vec![]))
                .await
                .is_err()
        );

        // Test script execution
        let temp_dir = tempfile::tempdir()?;
        let script_file = temp_dir.path().join("run.sh");
        std::fs::write(
            &script_file,
            "#!/bin/sh
exit 0
",
        )?;
        let script_proc = ShellLocalPostProcessor::new(ShellLocalConfig {
            identifier: String::new(),
            script: Some(script_file.to_string_lossy().to_string()),
            ..Default::default()
        });
        let script_out = script_proc
            .process(Artifact::new("base".to_string(), vec![]))
            .await?;
        assert_eq!(script_out.id, "base");

        // Test script failure
        let bad_script = temp_dir.path().join("bad.sh");
        std::fs::write(
            &bad_script,
            "#!/bin/sh
exit 1
",
        )?;
        let bad_script_proc = ShellLocalPostProcessor::new(ShellLocalConfig {
            identifier: String::new(),
            script: Some(bad_script.to_string_lossy().to_string()),
            ..Default::default()
        });
        assert!(
            bad_script_proc
                .process(Artifact::new("base".to_string(), vec![]))
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_shell_local_process_failure() {
        let config = ShellLocalConfig::default();
        let processor = ShellLocalPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let err = processor.process(artifact).await;
        assert_eq!(
            err.err().as_ref().map(ToString::to_string),
            Some("Parse error: Identifier is empty".to_string())
        );
    }

    #[test]
    fn test_shell_local_config_derived_traits() {
        let config1 = ShellLocalConfig {
            identifier: "test".to_string(),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert!(format!("{config1:?}").contains("ShellLocalConfig"));
        let def = ShellLocalConfig::default();
        assert_eq!(def.identifier, "");
        let processor1 = ShellLocalPostProcessor::new(config1);
        let processor2 = processor1.clone();
        assert!(format!("{processor1:?}").contains("ShellLocalPostProcessor"));
        assert_eq!(processor2.config.identifier, "test");
    }
}
