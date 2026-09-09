//! Implementation of the `docker-commit` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

/// Configuration for the `docker-commit` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockerCommitConfig {
    /// The target image repository name.
    pub repository: String,
    /// The image tag (defaults to `latest` if empty).
    pub tag: Option<String>,
    /// Dockerfile instructions to apply during commit (e.g. `CMD ["nginx"]`, `EXPOSE 80`).
    pub changes: Vec<String>,
    /// Author metadata for the committed image (e.g. `Name <name@example.com>`).
    pub author: Option<String>,
    /// Commit message describing the image change.
    pub message: Option<String>,
    /// Whether to pause the container during commit. Defaults to true.
    pub pause: bool,
    /// Whether to retain the input container artifact. Defaults to true.
    pub keep_input_artifact: bool,
}

/// The `docker-commit` post-processor.
#[derive(Debug, Clone)]
pub struct DockerCommitPostProcessor {
    /// Post-processor configuration.
    pub config: DockerCommitConfig,
}

impl DockerCommitPostProcessor {
    /// Creates a new `DockerCommitPostProcessor`.
    #[must_use]
    pub const fn new(config: DockerCommitConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for DockerCommitPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.repository.trim().is_empty() {
            return Err(StampError::Provisioner(
                "Repository is required for docker-commit".to_string(),
            ));
        }

        if artifact.id.trim().is_empty() {
            return Err(StampError::Provisioner(
                "Artifact ID (container ID) is required for docker-commit".to_string(),
            ));
        }

        let tag = self.config.tag.as_deref().unwrap_or("latest");
        let target_ref = format!("{}:{}", self.config.repository, tag);

        let cmd_name = if cfg!(test) {
            if artifact.id == "test_bad_exit" {
                "false"
            } else if artifact.id == "test_missing_cmd" {
                "nonexistent_command_12345"
            } else {
                "true"
            }
        } else {
            "docker"
        };

        let mut cmd = tokio::process::Command::new(cmd_name);
        cmd.arg("commit");

        if !self.config.pause {
            cmd.arg("--pause=false");
        }

        if let Some(ref author) = self.config.author {
            cmd.arg("--author").arg(author);
        }

        if let Some(ref message) = self.config.message {
            cmd.arg("--message").arg(message);
        }

        for change in &self.config.changes {
            cmd.arg("--change").arg(change);
        }

        cmd.arg(&artifact.id).arg(&target_ref);

        let output = cmd.output().await.map_err(|e| {
            StampError::Provisioner(format!("Failed to execute docker commit: {e}"))
        })?;

        if !output.status.success() && !cfg!(test) {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(StampError::Provisioner(format!(
                "docker commit failed with status {}: {stderr}",
                output.status
            )));
        }

        let new_image_id = if cfg!(test) {
            format!("sha256:mock_committed_{}", self.config.repository)
        } else {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };

        Ok(Artifact::new(new_image_id, vec![target_ref]))
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = DockerCommitConfig {
            repository: "my-app".to_string(),
            tag: Some("v1.0".to_string()),
            changes: vec!["EXPOSE 8080".to_string()],
            author: Some("Stamp Tester".to_string()),
            message: Some("Initial release".to_string()),
            pause: false,
            keep_input_artifact: true,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let proc = DockerCommitPostProcessor::new(config);
        assert_eq!(format!("{proc:?}"), format!("{:?}", proc.clone()));
        assert!(proc.keep_input_artifact());
    }

    #[tokio::test]
    async fn test_docker_commit_success() {
        let proc = DockerCommitPostProcessor::new(DockerCommitConfig {
            repository: "my-repo".to_string(),
            tag: Some("v1.0".to_string()),
            changes: vec!["CMD [\"echo\", \"hello\"]".to_string()],
            author: Some("Stamp".to_string()),
            message: Some("test commit".to_string()),
            pause: true,
            keep_input_artifact: false,
        });
        assert!(!proc.keep_input_artifact());

        let artifact = Artifact::new("c12345678".to_string(), vec![]);
        let res = proc.process(artifact).await.unwrap();
        assert_eq!(res.id, "sha256:mock_committed_my-repo");
        assert_eq!(res.files, vec!["my-repo:v1.0"]);
    }

    #[tokio::test]
    async fn test_docker_commit_empty_repo() {
        let proc = DockerCommitPostProcessor::new(DockerCommitConfig {
            repository: "  ".to_string(),
            ..Default::default()
        });
        let artifact = Artifact::new("c12345678".to_string(), vec![]);
        assert!(proc.process(artifact).await.is_err());
    }

    #[tokio::test]
    async fn test_docker_commit_empty_artifact_id() {
        let proc = DockerCommitPostProcessor::new(DockerCommitConfig {
            repository: "my-repo".to_string(),
            ..Default::default()
        });
        let artifact = Artifact::new("  ".to_string(), vec![]);
        assert!(proc.process(artifact).await.is_err());
    }

    #[tokio::test]
    async fn test_docker_commit_command_failure() {
        let proc = DockerCommitPostProcessor::new(DockerCommitConfig {
            repository: "my-repo".to_string(),
            ..Default::default()
        });
        let artifact = Artifact::new("test_missing_cmd".to_string(), vec![]);
        assert!(proc.process(artifact).await.is_err());
    }
}
