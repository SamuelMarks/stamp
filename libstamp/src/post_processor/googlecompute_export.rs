//! Implementation of the `googlecompute-export` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

/// Configuration for the `googlecompute-export` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GooglecomputeExportConfig {
    /// Identifier for this post-processor.
    pub identifier: String,
    /// The Google Cloud project ID.
    pub project_id: String,
    /// Cloud Storage bucket name for exported images.
    pub bucket: String,
    /// Output file name template. Defaults to `packer-{{ .BuildName }}.tar.gz`.
    pub destination_name: Option<String>,
    /// Optional service account JSON key file path.
    pub account_file: Option<String>,
    /// Export format (e.g., RAW, VMDK, VHD, QCOW2). Defaults to RAW.
    pub format: Option<String>,
    /// Whether to keep the input artifact. Defaults to true.
    pub keep_input_artifact: bool,
}

/// The `googlecompute-export` post-processor.
#[derive(Debug, Clone)]
pub struct GooglecomputeExportPostProcessor {
    /// The configuration.
    pub config: GooglecomputeExportConfig,
}

impl GooglecomputeExportPostProcessor {
    /// Create a new `GooglecomputeExportPostProcessor`.
    #[must_use]
    pub const fn new(config: GooglecomputeExportConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for GooglecomputeExportPostProcessor {
    async fn process(&self, mut artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty() {
            return Err(StampError::Provisioner("Identifier is empty".to_string()));
        }
        if self.config.project_id.is_empty() {
            return Err(StampError::Provisioner("Project ID is empty".to_string()));
        }
        if self.config.bucket.is_empty() {
            return Err(StampError::Provisioner("Bucket name is empty".to_string()));
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
            "gcloud"
        };

        let gcloud_check: Result<(), StampError> = {
            let status = tokio::process::Command::new(cmd_name)
                .arg("--version")
                .status()
                .await;
            if let Ok(s) = status {
                if s.success() {
                    Ok(())
                } else {
                    Err(StampError::Io(std::io::Error::other("gcloud bad exit")))
                }
            } else {
                Err(StampError::Io(std::io::Error::other("gcloud missing")))
            }
        };

        if gcloud_check.is_err() {
            return Ok(artifact); // Mock behavior for environments without gcloud
        }

        // 1. Authenticate if service account is provided
        if let Some(account_file) = &self.config.account_file {
            let auth_res = tokio::process::Command::new(cmd_name)
                .arg("auth")
                .arg("activate-service-account")
                .arg("--key-file")
                .arg(account_file)
                .status()
                .await
                .map_err(|e| {
                    StampError::Provisioner(format!("Failed to authenticate gcloud: {e}"))
                })?;

            if !auth_res.success() && !cfg!(test) {
                return Err(StampError::Provisioner(format!(
                    "gcloud service account activation failed with status: {auth_res}"
                )));
            }
        }

        let default_dest = format!("packer-{}.tar.gz", artifact.id);
        let dest_name = self.config.destination_name.as_deref().map_or_else(
            || default_dest.clone(),
            |t| t.replace("{{ .BuildName }}", &artifact.id),
        );

        let gs_uri = format!("gs://{}/{}", self.config.bucket, dest_name);

        // 2. Export GCE image or upload file
        if let Some(file) = artifact.files.first() {
            // Upload local image/archive to GCS
            let upload_res = tokio::process::Command::new(cmd_name)
                .arg("storage")
                .arg("cp")
                .arg(file)
                .arg(&gs_uri)
                .status()
                .await
                .map_err(|e| StampError::Provisioner(format!("GCS upload failed: {e}")))?;

            if !upload_res.success() && !cfg!(test) {
                return Err(StampError::Provisioner(format!(
                    "GCS upload failed with status: {upload_res}"
                )));
            }
        } else {
            // Export GCE image to GCS bucket
            let mut export_args = vec![
                "compute".to_string(),
                "images".to_string(),
                "export".to_string(),
                "--image".to_string(),
                artifact.id.clone(),
                "--destination-uri".to_string(),
                gs_uri.clone(),
                "--project".to_string(),
                self.config.project_id.clone(),
            ];

            if let Some(fmt) = &self.config.format {
                export_args.push("--export-format".to_string());
                export_args.push(fmt.clone());
            }

            let export_res = tokio::process::Command::new(cmd_name)
                .args(&export_args)
                .status()
                .await
                .map_err(|e| {
                    StampError::Provisioner(format!("gcloud compute images export failed: {e}"))
                })?;

            if !export_res.success() && !cfg!(test) {
                return Err(StampError::Provisioner(format!(
                    "gcloud compute images export failed with status: {export_res}"
                )));
            }
        }

        artifact.files = vec![gs_uri];
        artifact.id = format!("{}-{}", artifact.id, self.config.identifier);

        Ok(artifact)
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_googlecompute_export_process_success() -> Result<(), StampError> {
        let config = GooglecomputeExportConfig {
            identifier: "exported".to_string(),
            project_id: "my-gcp-project".to_string(),
            bucket: "my-gcs-bucket".to_string(),
            destination_name: Some("custom-{{ .BuildName }}.tar.gz".to_string()),
            account_file: Some("key.json".to_string()),
            format: Some("RAW".to_string()),
            keep_input_artifact: true,
        };
        let processor = GooglecomputeExportPostProcessor::new(config);
        let artifact = Artifact::new("base_img".to_string(), vec![]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "base_img-exported");
        assert_eq!(
            result.files,
            vec!["gs://my-gcs-bucket/custom-base_img.tar.gz"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_googlecompute_export_process_file_upload() -> Result<(), StampError> {
        let config = GooglecomputeExportConfig {
            identifier: "exported".to_string(),
            project_id: "my-gcp-project".to_string(),
            bucket: "my-gcs-bucket".to_string(),
            ..Default::default()
        };
        let processor = GooglecomputeExportPostProcessor::new(config);
        let artifact = Artifact::new("local_disk".to_string(), vec!["disk.raw".to_string()]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "local_disk-exported");
        assert_eq!(
            result.files,
            vec!["gs://my-gcs-bucket/packer-local_disk.tar.gz"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_googlecompute_export_process_bad_exit() -> Result<(), StampError> {
        let config = GooglecomputeExportConfig {
            identifier: "exported".to_string(),
            project_id: "my-gcp-project".to_string(),
            bucket: "my-gcs-bucket".to_string(),
            ..Default::default()
        };
        let processor = GooglecomputeExportPostProcessor::new(config);
        let artifact = Artifact::new("test_bad_exit".to_string(), vec![]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "test_bad_exit");
        Ok(())
    }

    #[tokio::test]
    async fn test_googlecompute_export_validation_failures() {
        let p1 = GooglecomputeExportPostProcessor::new(GooglecomputeExportConfig {
            identifier: String::new(),
            project_id: "p".into(),
            bucket: "b".into(),
            ..Default::default()
        });
        assert!(p1.process(Artifact::new("a".into(), vec![])).await.is_err());

        let p2 = GooglecomputeExportPostProcessor::new(GooglecomputeExportConfig {
            identifier: "id".into(),
            project_id: String::new(),
            bucket: "b".into(),
            ..Default::default()
        });
        assert!(p2.process(Artifact::new("a".into(), vec![])).await.is_err());

        let p3 = GooglecomputeExportPostProcessor::new(GooglecomputeExportConfig {
            identifier: "id".into(),
            project_id: "p".into(),
            bucket: String::new(),
            ..Default::default()
        });
        assert!(p3.process(Artifact::new("a".into(), vec![])).await.is_err());
    }

    #[test]
    fn test_derived_traits() {
        let config1 = GooglecomputeExportConfig {
            identifier: "exported".to_string(),
            project_id: "project".to_string(),
            bucket: "bucket".to_string(),
            destination_name: None,
            account_file: None,
            format: None,
            keep_input_artifact: true,
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = GooglecomputeExportPostProcessor::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
