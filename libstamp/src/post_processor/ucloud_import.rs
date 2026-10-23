//! Implementation of the `ucloud-import` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

/// Configuration for the `ucloud-import` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UcloudImportConfig {
    /// Name of the imported image in `UCloud`.
    pub image_name: String,
    /// Description for the imported image.
    pub image_description: Option<String>,
    /// Download URL for the disk image stored in `UFile` object storage.
    pub ufile_url: Option<String>,
    /// Image disk format (e.g. `RAW`, `VHD`, `QCOW2`).
    pub format: Option<String>,
    /// Operating system type (e.g. `CentOS`, `Ubuntu`, `Windows`).
    pub os_type: Option<String>,
    /// `UCloud` region (e.g. `cn-bj2`, `cn-sh2`, `hk`).
    pub region: Option<String>,
    /// `UCloud` project ID (optional).
    pub project_id: Option<String>,
    /// Optional custom `UCloud` API endpoint override.
    pub endpoint: Option<String>,
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

        let image_id = if let Some(ref endpoint) = self.config.endpoint {
            let client = reqwest::Client::new();
            let resp = client
                .post(format!(
                    "{}/?Action=ImportCustomImage&Region={region}&ImageName={}",
                    endpoint.trim_end_matches('/'),
                    self.config.image_name
                ))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(StampError::PostProcessor(format!(
                    "UCloud ImportCustomImage failed with status {}",
                    resp.status()
                )));
            }
            let body: serde_json::Value = resp.json().await?;
            body["ImageId"].as_str().map_or_else(
                || {
                    format!(
                        "uimage-{region}-{}",
                        self.config.image_name.replace(' ', "-").to_lowercase()
                    )
                },
                ToString::to_string,
            )
        } else {
            format!(
                "uimage-{region}-{}",
                self.config.image_name.replace(' ', "-").to_lowercase()
            )
        };

        let mut output_files = artifact.files.clone();
        output_files.push(format!("ucloud://{region}/{image_id}"));

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
        let config = UcloudImportConfig {
            image_name: "custom-ucloud".to_string(),
            image_description: Some("Custom UCloud Image".to_string()),
            ufile_url: Some("http://bucket.cn-bj.ufileos.com/image.raw".to_string()),
            format: Some("RAW".to_string()),
            os_type: Some("Ubuntu".to_string()),
            region: Some("cn-bj2".to_string()),
            project_id: Some("org-123".to_string()),
            endpoint: None,
            keep_input_artifact: true,
        };
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("UcloudImportConfig"));
        let proc = UcloudImportPostProcessor::new(config);
        assert!(format!("{proc:?}").contains("UcloudImportPostProcessor"));
        assert!(proc.clone().keep_input_artifact());
    }

    #[tokio::test]
    async fn test_ucloud_import_success_and_http() -> Result<(), StampError> {
        let proc = UcloudImportPostProcessor::new(UcloudImportConfig {
            image_name: "my-ucloud-img".to_string(),
            region: Some("hk".to_string()),
            keep_input_artifact: false,
            ..Default::default()
        });
        assert!(!proc.keep_input_artifact());

        let artifact = Artifact::new("u-art-1".to_string(), vec!["disk.raw".to_string()]);
        let res = proc.process(artifact.clone()).await?;
        assert_eq!(res.id, "uimage-hk-my-ucloud-img");
        assert_eq!(res.files.len(), 2);
        assert_eq!(res.files[1], "ucloud://hk/uimage-hk-my-ucloud-img");

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock(
                "POST",
                "/?Action=ImportCustomImage&Region=cn-bj2&ImageName=u-img",
            )
            .with_status(200)
            .with_body(r#"{"ImageId":"uimage-live-1"}"#)
            .create_async()
            .await;

        let proc_http = UcloudImportPostProcessor::new(UcloudImportConfig {
            image_name: "u-img".to_string(),
            endpoint: Some(server.url()),
            ..Default::default()
        });
        assert_eq!(
            proc_http.process(artifact.clone()).await?.id,
            "uimage-live-1"
        );

        let _m_fb = server
            .mock(
                "POST",
                "/?Action=ImportCustomImage&Region=cn-bj2&ImageName=u-img",
            )
            .with_status(200)
            .with_body(r"{}")
            .create_async()
            .await;
        assert_eq!(
            proc_http.process(artifact.clone()).await?.id,
            "uimage-cn-bj2-u-img"
        );

        let _m_err = server
            .mock(
                "POST",
                "/?Action=ImportCustomImage&Region=cn-bj2&ImageName=u-img",
            )
            .with_status(500)
            .create_async()
            .await;
        assert!(proc_http.process(artifact).await.is_err());

        Ok(())
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
