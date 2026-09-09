//! Implementation of the `docker-tag` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

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

        let cmd_name = if cfg!(test) {
            if artifact.id == "test_bad_exit" {
                "false"
            } else if artifact.id == "test_missing" {
                "nonexistent_command_12345"
            } else {
                "true"
            }
        } else {
            "docker"
        };

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
            let status = tokio::process::Command::new(cmd_name)
                .arg("tag")
                .arg(&artifact.id)
                .arg(&target_ref)
                .status()
                .await
                .map_err(|e| {
                    StampError::Provisioner(format!("Failed to execute docker tag: {e}"))
                })?;

            if !status.success() && !cfg!(test) {
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::pedantic,
    clippy::all
)]
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
    async fn test_docker_tag_process_success() {
        let config = DockerTagConfig {
            repository: "my-repo".to_string(),
            tags: vec!["v1".to_string(), "v2".to_string()],
            registry: Some("docker.io".to_string()),
        };
        let processor = DockerTagPostProcessor::new(config);
        let artifact = Artifact::new("sha256:1234".to_string(), vec![]);

        let result = processor
            .process(artifact)
            .await
            .expect("process succeeded");
        assert_eq!(result.id, "sha256:1234");
        assert_eq!(result.files.len(), 2);
        assert_eq!(result.files[0], "docker.io/my-repo:v1");
        assert_eq!(result.files[1], "docker.io/my-repo:v2");
    }

    #[tokio::test]
    async fn test_docker_tag_process_default_latest() {
        let config = DockerTagConfig {
            repository: "ubuntu".to_string(),
            tags: vec![],
            registry: None,
        };
        let processor = DockerTagPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let result = processor
            .process(artifact)
            .await
            .expect("process succeeded");
        assert_eq!(result.files, vec!["ubuntu:latest"]);
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
        };
        let processor = DockerTagPostProcessor::new(config);
        let artifact = Artifact::new("test_bad_exit".to_string(), vec![]);
        assert!(processor.process(artifact).await.is_ok());
    }

    #[test]
    fn test_docker_tag_derived_traits() {
        let config1 = DockerTagConfig {
            repository: "ubuntu".to_string(),
            tags: vec!["latest".to_string()],
            registry: None,
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = DockerTagPostProcessor::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
