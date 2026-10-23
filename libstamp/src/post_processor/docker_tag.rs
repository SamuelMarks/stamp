//! Implementation of the `docker-tag` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use std::path::PathBuf;

/// Formats a complete Docker image reference from repository, tag, and optional registry.
#[must_use]
pub fn format_image_ref(repository: &str, tag: &str, registry: Option<&str>) -> String {
    let repo_clean = repository.trim_matches('/');
    if let Some(reg) = registry {
        let reg_clean = reg.trim_matches('/');
        format!("{reg_clean}/{repo_clean}:{tag}")
    } else {
        format!("{repo_clean}:{tag}")
    }
}

/// Configuration for the `docker-tag` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockerTagConfig {
    /// The image repository to target.
    pub repository: String,
    /// The image tags to apply.
    pub tags: Vec<String>,
    /// Optional registry hostname to prepend to the repository.
    pub registry: Option<String>,
    /// Optional custom path to the `docker` executable. Defaults to `"docker"`.
    pub docker_binary: Option<PathBuf>,
}

/// The `docker-tag` post-processor.
#[derive(Debug, Clone)]
pub struct DockerTagPostProcessor {
    /// The configuration.
    pub config: DockerTagConfig,
}

impl DockerTagPostProcessor {
    /// Create a new `DockerTagPostProcessor`.
    #[must_use]
    pub const fn new(config: DockerTagConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for DockerTagPostProcessor {
    async fn process(&self, mut artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.repository.is_empty() {
            return Err(StampError::Provisioner("Repository is empty".to_string()));
        }

        let default_bin = if cfg!(test) { "true" } else { "docker" };
        let cmd_name = self.config.docker_binary.as_ref().map_or_else(
            || default_bin.to_string(),
            |p| p.to_string_lossy().to_string(),
        );

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
                .arg("tag")
                .arg(&artifact.id)
                .arg(&target_ref)
                .status()
                .await
                .map_err(|e| {
                    StampError::Provisioner(format!("Failed to execute docker tag: {e}"))
                })?;

            if !status.success() {
                return Err(StampError::Provisioner(format!(
                    "docker tag failed with status: {status}"
                )));
            }

            artifact.files.push(target_ref);
        }

        Ok(artifact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docker_tag_format_image_ref() {
        assert_eq!(format_image_ref("my-app", "v1.0", None), "my-app:v1.0");
        assert_eq!(
            format_image_ref("org/my-app", "latest", Some("registry.example.com")),
            "registry.example.com/org/my-app:latest"
        );
    }

    #[tokio::test]
    async fn test_docker_tag_process_success() -> Result<(), StampError> {
        let config = DockerTagConfig {
            repository: "my-repo".to_string(),
            tags: vec!["v1".to_string(), "v2".to_string()],
            registry: Some("docker.io".to_string()),
            docker_binary: Some(PathBuf::from("true")),
        };
        let processor = DockerTagPostProcessor::new(config);
        let artifact = Artifact::new("sha256:1234".to_string(), vec![]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "sha256:1234");
        assert_eq!(result.files.len(), 2);
        assert_eq!(result.files[0], "docker.io/my-repo:v1");
        assert_eq!(result.files[1], "docker.io/my-repo:v2");
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_tag_process_default_latest() -> Result<(), StampError> {
        let config = DockerTagConfig {
            repository: "ubuntu".to_string(),
            tags: vec![],
            registry: None,
            docker_binary: Some(PathBuf::from("true")),
        };
        let processor = DockerTagPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.files, vec!["ubuntu:latest"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_tag_process_empty_repo() {
        let config = DockerTagConfig::default();
        let processor = DockerTagPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);
        assert!(processor.process(artifact).await.is_err());
    }

    #[tokio::test]
    async fn test_docker_tag_process_missing() {
        let config = DockerTagConfig {
            repository: "my-repo".to_string(),
            tags: vec!["v1".to_string()],
            registry: None,
            docker_binary: Some(PathBuf::from("nonexistent_command_12345")),
        };
        let processor = DockerTagPostProcessor::new(config);
        let artifact = Artifact::new("test_missing".to_string(), vec![]);
        assert!(processor.process(artifact).await.is_err());
    }

    #[tokio::test]
    async fn test_docker_tag_process_bad_exit() {
        let config = DockerTagConfig {
            repository: "my-repo".to_string(),
            tags: vec!["v1".to_string()],
            registry: None,
            docker_binary: Some(PathBuf::from("false")),
        };
        let processor = DockerTagPostProcessor::new(config);
        let artifact = Artifact::new("test_bad_exit".to_string(), vec![]);
        assert!(processor.process(artifact).await.is_err());
    }

    #[test]
    fn test_docker_tag_derived_traits() {
        let config1 = DockerTagConfig {
            repository: "ubuntu".to_string(),
            tags: vec!["latest".to_string()],
            registry: None,
            docker_binary: None,
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert!(format!("{config1:?}").contains("DockerTagConfig"));
        let st1 = DockerTagPostProcessor::new(config1);
        let st2 = st1.clone();
        assert!(format!("{st1:?}").contains("DockerTagPostProcessor"));
        assert_eq!(st1.config, st2.config);
    }
}
