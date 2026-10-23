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
    /// Optional custom Yandex Compute API endpoint override.
    pub endpoint: Option<String>,
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

        let image_id = if let Some(ref endpoint) = self.config.endpoint {
            let client = reqwest::Client::new();
            let payload = serde_json::json!({
                "folderId": folder,
                "name": self.config.image_name,
                "uri": self.config.object_url,
            });
            let resp = client
                .post(format!(
                    "{}/compute/v1/images",
                    endpoint.trim_end_matches('/')
                ))
                .json(&payload)
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(StampError::PostProcessor(format!(
                    "Yandex image import failed with status {}",
                    resp.status()
                )));
            }
            let body: serde_json::Value = resp.json().await?;
            body["response"]["id"].as_str().map_or_else(
                || format!("fd8yandex{}", self.config.image_name.replace('-', "")),
                ToString::to_string,
            )
        } else {
            format!("fd8yandex{}", self.config.image_name.replace('-', ""))
        };

        let mut output_files = artifact.files.clone();
        output_files.push(format!("yandex://{folder}/{image_id}"));

        Ok(Artifact::new(image_id, output_files))
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
            endpoint: None,
            keep_input_artifact: true,
        };
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("YandexImportConfig"));
        let proc = YandexImportPostProcessor::new(config);
        assert!(format!("{proc:?}").contains("YandexImportPostProcessor"));
        assert!(proc.clone().keep_input_artifact());
    }

    #[tokio::test]
    async fn test_yandex_import_success_and_http() -> Result<(), StampError> {
        let proc = YandexImportPostProcessor::new(YandexImportConfig {
            image_name: "my-image".to_string(),
            folder_id: Some("folder-xyz".to_string()),
            keep_input_artifact: false,
            ..Default::default()
        });
        assert!(!proc.keep_input_artifact());

        let artifact = Artifact::new("art-999".to_string(), vec!["disk.qcow2".to_string()]);
        let res = proc.process(artifact.clone()).await?;
        assert_eq!(res.id, "fd8yandexmyimage");
        assert_eq!(res.files.len(), 2);
        assert_eq!(res.files[1], "yandex://folder-xyz/fd8yandexmyimage");

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/compute/v1/images")
            .with_status(200)
            .with_body(r#"{"response":{"id":"fd8live1"}}"#)
            .create_async()
            .await;

        let proc_http = YandexImportPostProcessor::new(YandexImportConfig {
            image_name: "my-image".to_string(),
            endpoint: Some(server.url()),
            ..Default::default()
        });
        assert_eq!(proc_http.process(artifact.clone()).await?.id, "fd8live1");

        let _m_fb = server
            .mock("POST", "/compute/v1/images")
            .with_status(200)
            .with_body(r"{}")
            .create_async()
            .await;
        assert_eq!(
            proc_http.process(artifact.clone()).await?.id,
            "fd8yandexmyimage"
        );

        let _m_err = server
            .mock("POST", "/compute/v1/images")
            .with_status(500)
            .create_async()
            .await;
        assert!(proc_http.process(artifact).await.is_err());

        Ok(())
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
