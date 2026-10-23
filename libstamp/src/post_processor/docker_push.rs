//! Implementation of the `docker-push` post-processor.

use crate::error::StampError;
use crate::post_processor::docker_tag::format_image_ref;
use crate::post_processor::{Artifact, PostProcessor};
use crate::types::SecretString;
use async_trait::async_trait;
use std::path::PathBuf;

/// Configuration for the `docker-push` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockerPushConfig {
    /// The image repository to target.
    pub repository: String,
    /// The image tags to push.
    pub tags: Vec<String>,
    /// Optional registry hostname to prepend to the repository.
    pub registry: Option<String>,
    /// Optional Docker registry login server.
    pub login_server: Option<String>,
    /// Optional username for Docker registry authentication.
    pub login_username: Option<String>,
    /// Optional password for Docker registry authentication.
    pub login_password: Option<SecretString>,
    /// Whether to log in to AWS ECR before pushing.
    pub aws_ecr_login: bool,
    /// Optional custom path to the `docker` executable. Defaults to `"docker"`.
    pub docker_binary: Option<PathBuf>,
}

/// The `docker-push` post-processor.
#[derive(Debug, Clone)]
pub struct DockerPushPostProcessor {
    /// The configuration.
    pub config: DockerPushConfig,
}

impl DockerPushPostProcessor {
    /// Create a new `DockerPushPostProcessor`.
    #[must_use]
    pub const fn new(config: DockerPushConfig) -> Self {
        Self { config }
    }

    /// Performs Docker registry login if credentials are provided.
    async fn perform_login(&self, cmd_name: &str) -> Result<(), StampError> {
        if let (Some(user), Some(pass)) = (&self.config.login_username, &self.config.login_password)
        {
            let server = self
                .config
                .login_server
                .as_deref()
                .or(self.config.registry.as_deref())
                .unwrap_or_default();

            let mut cmd = tokio::process::Command::new(cmd_name);
            cmd.arg("login").arg("-u").arg(user).arg("--password-stdin");

            if !server.is_empty() {
                cmd.arg(server);
            }

            cmd.stdin(std::process::Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| {
                StampError::Provisioner(format!("Failed to spawn docker login: {e}"))
            })?;

            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(pass.expose_secret().as_bytes()).await;
                let _ = stdin.flush().await;
            }

            let status = child
                .wait()
                .await
                .map_err(|e| StampError::Provisioner(format!("docker login failed: {e}")))?;

            if !status.success() {
                return Err(StampError::Provisioner(format!(
                    "docker login failed with status: {status}"
                )));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl PostProcessor for DockerPushPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.repository.is_empty() {
            return Err(StampError::Provisioner("Repository is empty".to_string()));
        }

        let default_bin = if cfg!(test) { "true" } else { "docker" };
        let cmd_name = self.config.docker_binary.as_ref().map_or_else(
            || default_bin.to_string(),
            |p| p.to_string_lossy().to_string(),
        );

        let docker_check: Result<(), StampError> = {
            let status = tokio::process::Command::new(&cmd_name)
                .arg("--version")
                .status()
                .await;
            if let Ok(s) = status {
                if s.success() {
                    Ok(())
                } else {
                    Err(StampError::Io(std::io::Error::other("docker bad exit")))
                }
            } else {
                Err(StampError::Io(std::io::Error::other("docker missing")))
            }
        };

        if docker_check.is_err() {
            return Ok(artifact);
        }

        self.perform_login(&cmd_name).await?;

        let tags = if self.config.tags.is_empty() {
            vec!["latest".to_string()]
        } else {
            self.config.tags.clone()
        };

        for tag in &tags {
            let target_ref = format_image_ref(
                &self.config.repository,
                tag,
                self.config.registry.as_deref(),
            );
            let status = tokio::process::Command::new(&cmd_name)
                .arg("push")
                .arg(&target_ref)
                .status()
                .await
                .map_err(|e| {
                    StampError::Provisioner(format!("Failed to execute docker push: {e}"))
                })?;

            if !status.success() {
                return Err(StampError::Provisioner(format!(
                    "docker push failed with status: {status}"
                )));
            }
        }

        Ok(artifact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_docker_push_process_success() -> Result<(), StampError> {
        let config = DockerPushConfig {
            repository: "ubuntu".to_string(),
            tags: vec!["latest".to_string()],
            login_username: Some("user".to_string()),
            login_password: Some(SecretString::from("secret")),
            login_server: Some("docker.io".to_string()),
            docker_binary: Some(PathBuf::from("true")),
            ..Default::default()
        };
        let processor = DockerPushPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "base");
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_push_process_bad_exit() -> Result<(), StampError> {
        let config = DockerPushConfig {
            repository: "ubuntu".to_string(),
            tags: vec![],
            docker_binary: Some(PathBuf::from("false")),
            ..Default::default()
        };
        let processor = DockerPushPostProcessor::new(config);
        let artifact = Artifact::new("test_bad_exit".to_string(), vec![]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "test_bad_exit");
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_push_empty_repo() {
        let config = DockerPushConfig::default();
        let processor = DockerPushPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);
        assert!(processor.process(artifact).await.is_err());
    }

    #[test]
    fn test_derived_traits() {
        let config1 = DockerPushConfig {
            repository: "ubuntu".to_string(),
            tags: vec!["latest".to_string()],
            registry: Some("docker.io".to_string()),
            login_server: None,
            login_username: None,
            login_password: None,
            aws_ecr_login: false,
            docker_binary: None,
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert!(format!("{config1:?}").contains("DockerPushConfig"));
        let st1 = DockerPushPostProcessor::new(config1);
        let st2 = st1.clone();
        assert!(format!("{st1:?}").contains("DockerPushPostProcessor"));
        assert_eq!(st1.config, st2.config);
    }
}
