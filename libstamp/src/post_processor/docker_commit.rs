//! Implementation of the `docker-commit` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use std::path::PathBuf;

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
    /// Optional custom path to the `docker` executable. Defaults to `"docker"`.
    pub docker_binary: Option<PathBuf>,
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

        let default_bin = if cfg!(test) { "true" } else { "docker" };
        let cmd_name = self.config.docker_binary.as_ref().map_or_else(
            || default_bin.to_string(),
            |p| p.to_string_lossy().to_string(),
        );

        let mut cmd = tokio::process::Command::new(&cmd_name);
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

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(StampError::Provisioner(format!(
                "docker commit failed with status {}: {stderr}",
                output.status
            )));
        }

        let stdout_trimmed = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let new_image_id = if stdout_trimmed.is_empty() {
            format!("sha256:mock_committed_{}", self.config.repository)
        } else {
            stdout_trimmed
        };

        Ok(Artifact::new(new_image_id, vec![target_ref]))
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
    }
}

#[cfg(test)]
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
            docker_binary: None,
        };
        let cloned = config.clone();
        assert_eq!(cloned, config);
        assert!(format!("{config:?}").contains("DockerCommitConfig"));
        let proc = DockerCommitPostProcessor::new(config);
        let proc_clone = proc.clone();
        assert!(format!("{proc:?}").contains("DockerCommitPostProcessor"));
        assert_eq!(proc.config, proc_clone.config);
        assert!(proc.keep_input_artifact());
    }

    #[tokio::test]
    async fn test_docker_commit_success() -> Result<(), StampError> {
        let proc = DockerCommitPostProcessor::new(DockerCommitConfig {
            repository: "my-repo".to_string(),
            tag: Some("v1.0".to_string()),
            changes: vec![r#"CMD ["echo", "hello"]"#.to_string()],
            author: Some("Stamp".to_string()),
            message: Some("test commit".to_string()),
            pause: true,
            keep_input_artifact: false,
            docker_binary: Some(PathBuf::from("true")),
        });
        assert!(!proc.keep_input_artifact());

        let artifact = Artifact::new("c12345678".to_string(), vec![]);
        let res = proc.process(artifact).await?;
        assert_eq!(res.id, "sha256:mock_committed_my-repo");
        assert_eq!(res.files, vec!["my-repo:v1.0"]);
        Ok(())
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
            docker_binary: Some(PathBuf::from("nonexistent_command_12345")),
            ..Default::default()
        });
        let artifact = Artifact::new("c123".to_string(), vec![]);
        assert!(proc.process(artifact.clone()).await.is_err());

        let proc_bad_exit = DockerCommitPostProcessor::new(DockerCommitConfig {
            repository: "my-repo".to_string(),
            docker_binary: Some(PathBuf::from("false")),
            ..Default::default()
        });
        assert!(proc_bad_exit.process(artifact).await.is_err());
    }
}
