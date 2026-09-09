//! Implementation of the `ucloud-import` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

/// Configuration for the `ucloud-import` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UcloudImportConfig {
    /// Name of the imported image in UCloud.
    pub image_name: String,
    /// Description for the imported image.
    pub image_description: Option<String>,
    /// Download URL for the disk image stored in UFile object storage.
    pub ufile_url: Option<String>,
    /// Image disk format (e.g. `RAW`, `VHD`, `QCOW2`).
    pub format: Option<String>,
    /// Operating system type (e.g. `CentOS`, `Ubuntu`, `Windows`).
    pub os_type: Option<String>,
    /// UCloud region (e.g. `cn-bj2`, `cn-sh2`, `hk`).
    pub region: Option<String>,
    /// UCloud project ID (optional).
    pub project_id: Option<String>,
    /// Whether to keep the input artifact. Defaults to true.
    pub keep_input_artifact: bool,
}

/// The `ucloud-import` post-processor.
#[derive(Debug, Clone)]
pub struct UcloudImportPostProcessor {
    /// Post-processor configuration.
    pub config: UcloudImportConfig,
}

impl UcloudImportPostProcessor {
    /// Creates a new `UcloudImportPostProcessor`.
    #[must_use]
    pub const fn new(config: UcloudImportConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for UcloudImportPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.image_name.trim().is_empty() {
            return Err(StampError::Provisioner(
                "image_name is required for ucloud-import".to_string(),
            ));
        }

        let region = self.config.region.as_deref().unwrap_or("cn-bj2");
        let image_id = format!(
            "uimage-{}-{}",
            region,
            self.config.image_name.replace(' ', "-").to_lowercase()
        );

        let mut output_files = artifact.files.clone();
        output_files.push(format!("ucloud://{region}/{image_id}"));

        Ok(Artifact::new(image_id, output_files))
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
        let config = UcloudImportConfig {
            image_name: "custom-ucloud".to_string(),
            image_description: Some("Custom UCloud Image".to_string()),
            ufile_url: Some("http://bucket.cn-bj.ufileos.com/image.raw".to_string()),
            format: Some("RAW".to_string()),
            os_type: Some("Ubuntu".to_string()),
            region: Some("cn-bj2".to_string()),
            project_id: Some("org-123".to_string()),
            keep_input_artifact: true,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let proc = UcloudImportPostProcessor::new(config);
        assert_eq!(format!("{proc:?}"), format!("{:?}", proc.clone()));
        assert!(proc.keep_input_artifact());
    }

    #[tokio::test]
    async fn test_ucloud_import_success() {
        let proc = UcloudImportPostProcessor::new(UcloudImportConfig {
            image_name: "my-ucloud-img".to_string(),
            region: Some("hk".to_string()),
            keep_input_artifact: false,
            ..Default::default()
        });
        assert!(!proc.keep_input_artifact());

        let artifact = Artifact::new("u-art-1".to_string(), vec!["disk.raw".to_string()]);
        let res = proc.process(artifact).await.unwrap();
        assert_eq!(res.id, "uimage-hk-my-ucloud-img");
        assert_eq!(res.files.len(), 2);
        assert_eq!(res.files[1], "ucloud://hk/uimage-hk-my-ucloud-img");
    }

    #[tokio::test]
    async fn test_ucloud_import_empty_name() {
        let proc = UcloudImportPostProcessor::new(UcloudImportConfig {
            image_name: "   ".to_string(),
            ..Default::default()
        });
        let artifact = Artifact::new("u-art-1".to_string(), vec![]);
        assert!(proc.process(artifact).await.is_err());
    }
}
