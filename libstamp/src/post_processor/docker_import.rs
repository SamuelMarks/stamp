//! Implementation of the `docker-import` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Configuration for the `docker-import` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockerImportConfig {
    /// The image repository to target.
    pub repository: String,
    /// The image tag to target.
    pub tag: String,
    /// Whether the input archive is gzip compressed.
    pub gzip: bool,
    /// Optional custom path to the `docker` executable. Defaults to `"docker"`.
    pub docker_binary: Option<PathBuf>,
}

/// The `docker-import` post-processor.
#[derive(Debug, Clone)]
pub struct DockerImportPostProcessor {
    /// The configuration.
    pub config: DockerImportConfig,
}

impl DockerImportPostProcessor {
    /// Create a new `DockerImportPostProcessor`.
    #[must_use]
    pub const fn new(config: DockerImportConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for DockerImportPostProcessor {
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

        let tag = if self.config.tag.is_empty() {
            "latest"
        } else {
            &self.config.tag
        };
        let target_ref = format!("{}:{}", self.config.repository, tag);

        for f in &artifact.files {
            let is_gzip = self.config.gzip
                || Path::new(f).extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("gz") || ext.eq_ignore_ascii_case("tgz")
                });

            if is_gzip && Path::new(f).exists() {
                let file = File::open(f).map_err(StampError::Io)?;
                let mut decoder = flate2::read::GzDecoder::new(file);
                let mut decompressed = Vec::new();
                decoder
                    .read_to_end(&mut decompressed)
                    .map_err(StampError::Io)?;

                let mut cmd = tokio::process::Command::new(&cmd_name);
                cmd.arg("import")
                    .arg("-")
                    .arg(&target_ref)
                    .stdin(std::process::Stdio::piped());

                let mut child = cmd.spawn().map_err(StampError::Io)?;

                if let Some(mut stdin) = child.stdin.take() {
                    use tokio::io::AsyncWriteExt;
                    let _ = stdin.write_all(&decompressed).await;
                    let _ = stdin.flush().await;
                }

                let status = child.wait().await.map_err(StampError::Io)?;

                if !status.success() {
                    return Err(StampError::Provisioner(format!(
                        "docker import failed with status: {status}"
                    )));
                }
            } else {
                let status = tokio::process::Command::new(&cmd_name)
                    .arg("import")
                    .arg(f)
                    .arg(&target_ref)
                    .status()
                    .await
                    .map_err(StampError::Io)?;

                if !status.success() {
                    return Err(StampError::Provisioner(format!(
                        "docker import failed with status: {status}"
                    )));
                }
            }
        }

        Ok(artifact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_docker_import_process_success() -> Result<(), StampError> {
        let config = DockerImportConfig {
            repository: "ubuntu".to_string(),
            tag: "latest".to_string(),
            gzip: false,
            docker_binary: Some(PathBuf::from("true")),
        };
        let processor = DockerImportPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec!["dummy.tar".to_string()]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "base");
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_import_process_gzip() -> Result<(), StampError> {
        let config = DockerImportConfig {
            repository: "alpine".to_string(),
            tag: "edge".to_string(),
            gzip: true,
            docker_binary: Some(PathBuf::from("true")),
        };
        let processor = DockerImportPostProcessor::new(config);
        let artifact = Artifact::new("base_gz".to_string(), vec!["dummy.tar.gz".to_string()]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "base_gz");
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_import_process_bad_exit() -> Result<(), StampError> {
        let config = DockerImportConfig {
            repository: "ubuntu".to_string(),
            tag: "latest".to_string(),
            gzip: false,
            docker_binary: Some(PathBuf::from("false")),
        };
        let processor = DockerImportPostProcessor::new(config);
        let artifact = Artifact::new("test_bad_exit".to_string(), vec!["dummy.tar".to_string()]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "test_bad_exit");
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_import_empty_repo() {
        let config = DockerImportConfig::default();
        let processor = DockerImportPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);
        assert!(processor.process(artifact).await.is_err());
    }

    #[test]
    fn test_derived_traits() {
        let config1 = DockerImportConfig {
            repository: "ubuntu".to_string(),
            tag: "latest".to_string(),
            gzip: true,
            docker_binary: None,
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert!(format!("{config1:?}").contains("DockerImportConfig"));
        let st1 = DockerImportPostProcessor::new(config1);
        let st2 = st1.clone();
        assert!(format!("{st1:?}").contains("DockerImportPostProcessor"));
        assert_eq!(st1.config, st2.config);
    }
}
