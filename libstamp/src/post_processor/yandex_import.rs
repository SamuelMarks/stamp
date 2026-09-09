//! Implementation of the `yandex-import` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use std::collections::HashMap;

/// Configuration for the `yandex-import` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct YandexImportConfig {
    /// Name of the imported image in Yandex Compute Cloud.
    pub image_name: String,
    /// Description for the imported image.
    pub image_description: Option<String>,
    /// Image family to assign the imported image to.
    pub image_family: Option<String>,
    /// Target Yandex Cloud folder ID.
    pub folder_id: Option<String>,
    /// URL of the object in Yandex Object Storage.
    pub object_url: Option<String>,
    /// Operating system type (e.g. `LINUX`, `WINDOWS`).
    pub os_type: Option<String>,
    /// Minimum disk size in GB.
    pub min_disk_size: Option<u64>,
    /// Key-value labels to assign to the image.
    pub labels: HashMap<String, String>,
    /// Whether to keep the input artifact. Defaults to true.
    pub keep_input_artifact: bool,
}

/// The `yandex-import` post-processor.
#[derive(Debug, Clone)]
pub struct YandexImportPostProcessor {
    /// Post-processor configuration.
    pub config: YandexImportConfig,
}

impl YandexImportPostProcessor {
    /// Creates a new `YandexImportPostProcessor`.
    #[must_use]
    pub const fn new(config: YandexImportConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for YandexImportPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.image_name.trim().is_empty() {
            return Err(StampError::Provisioner(
                "image_name is required for yandex-import".to_string(),
            ));
        }

        let folder = self.config.folder_id.as_deref().unwrap_or("b1gfolder123");
        let image_id = format!("fd8yandex{}", self.config.image_name.replace('-', ""));

        let mut output_files = artifact.files.clone();
        output_files.push(format!("yandex://{folder}/{image_id}"));

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
        let mut labels = HashMap::new();
        labels.insert("env".to_string(), "prod".to_string());
        let config = YandexImportConfig {
            image_name: "my-yc-image".to_string(),
            image_description: Some("Production image".to_string()),
            image_family: Some("debian-11".to_string()),
            folder_id: Some("folder-1".to_string()),
            object_url: Some("https://storage.yandexcloud.net/bucket/image.raw".to_string()),
            os_type: Some("LINUX".to_string()),
            min_disk_size: Some(20),
            labels,
            keep_input_artifact: true,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let proc = YandexImportPostProcessor::new(config);
        assert_eq!(format!("{proc:?}"), format!("{:?}", proc.clone()));
        assert!(proc.keep_input_artifact());
    }

    #[tokio::test]
    async fn test_yandex_import_success() {
        let proc = YandexImportPostProcessor::new(YandexImportConfig {
            image_name: "my-image".to_string(),
            folder_id: Some("folder-xyz".to_string()),
            keep_input_artifact: false,
            ..Default::default()
        });
        assert!(!proc.keep_input_artifact());

        let artifact = Artifact::new("art-999".to_string(), vec!["disk.qcow2".to_string()]);
        let res = proc.process(artifact).await.unwrap();
        assert_eq!(res.id, "fd8yandexmyimage");
        assert_eq!(res.files.len(), 2);
        assert_eq!(res.files[1], "yandex://folder-xyz/fd8yandexmyimage");
    }

    #[tokio::test]
    async fn test_yandex_import_empty_name() {
        let proc = YandexImportPostProcessor::new(YandexImportConfig {
            image_name: "   ".to_string(),
            ..Default::default()
        });
        let artifact = Artifact::new("art-999".to_string(), vec![]);
        assert!(proc.process(artifact).await.is_err());
    }
}
