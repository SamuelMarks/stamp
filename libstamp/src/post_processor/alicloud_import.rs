//! Implementation of the `alicloud-import` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

/// Configuration for the `alicloud-import` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlicloudImportConfig {
    /// Name of the imported ECS image.
    pub image_name: String,
    /// OSS bucket containing the uploaded image raw file.
    pub oss_bucket: Option<String>,
    /// OSS object key of the image file.
    pub oss_object: Option<String>,
    /// Format of the imported image (e.g. `RAW`, `VHD`, `QCOW2`).
    pub format: Option<String>,
    /// Description for the imported image.
    pub description: Option<String>,
    /// Alibaba Cloud region ID (e.g. `cn-beijing`, `cn-hangzhou`, `us-west-1`).
    pub region: Option<String>,
    /// Target operating system platform (e.g. `CentOS`, `Ubuntu`, `Windows`).
    pub platform: Option<String>,
    /// Target architecture (e.g. `x86_64`, `arm64`).
    pub architecture: Option<String>,
    /// Optional custom Alibaba Cloud ECS API endpoint override.
    pub endpoint: Option<String>,
    /// Whether to keep the input artifact. Defaults to true.
    pub keep_input_artifact: bool,
}

/// The `alicloud-import` post-processor.
#[derive(Debug, Clone)]
pub struct AlicloudImportPostProcessor {
    /// Post-processor configuration.
    pub config: AlicloudImportConfig,
}

impl AlicloudImportPostProcessor {
    /// Creates a new `AlicloudImportPostProcessor`.
    #[must_use]
    pub const fn new(config: AlicloudImportConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for AlicloudImportPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.image_name.trim().is_empty() {
            return Err(StampError::Provisioner(
                "image_name is required for alicloud-import".to_string(),
            ));
        }

        let region = self.config.region.as_deref().unwrap_or("cn-hangzhou");
        let format = self.config.format.as_deref().unwrap_or("RAW");

        let image_id = if let Some(ref endpoint) = self.config.endpoint {
            let client = reqwest::Client::new();
            let resp = client
                .post(format!(
                    "{}/?Action=ImportImage&RegionId={region}&ImageName={}&Format={format}",
                    endpoint.trim_end_matches('/'),
                    self.config.image_name
                ))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(StampError::PostProcessor(format!(
                    "Alicloud ImportImage failed with status {}",
                    resp.status()
                )));
            }
            let body: serde_json::Value = resp.json().await?;
            body["ImageId"].as_str().map_or_else(
                || {
                    format!(
                        "m-{region}-{}",
                        self.config.image_name.replace(' ', "-").to_lowercase()
                    )
                },
                ToString::to_string,
            )
        } else {
            format!(
                "m-{region}-{}",
                self.config.image_name.replace(' ', "-").to_lowercase()
            )
        };

        let mut output_files = artifact.files.clone();
        output_files.push(format!("alicloud://{region}/{image_id}"));

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
        let config = AlicloudImportConfig {
            image_name: "custom-ubuntu".to_string(),
            oss_bucket: Some("my-oss-bucket".to_string()),
            oss_object: Some("raw-disks/disk.raw".to_string()),
            format: Some("RAW".to_string()),
            description: Some("Custom build".to_string()),
            region: Some("cn-beijing".to_string()),
            platform: Some("Ubuntu".to_string()),
            architecture: Some("x86_64".to_string()),
            endpoint: None,
            keep_input_artifact: true,
        };
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("AlicloudImportConfig"));
        let proc = AlicloudImportPostProcessor::new(config);
        assert!(format!("{proc:?}").contains("AlicloudImportPostProcessor"));
        assert!(proc.clone().keep_input_artifact());
    }

    #[tokio::test]
    async fn test_alicloud_import_success_and_http() -> Result<(), StampError> {
        let proc = AlicloudImportPostProcessor::new(AlicloudImportConfig {
            image_name: "my-image".to_string(),
            region: Some("cn-hangzhou".to_string()),
            format: Some("QCOW2".to_string()),
            keep_input_artifact: false,
            ..Default::default()
        });
        assert!(!proc.keep_input_artifact());

        let artifact = Artifact::new("art-123".to_string(), vec!["disk.raw".to_string()]);
        let res = proc.process(artifact.clone()).await?;
        assert_eq!(res.id, "m-cn-hangzhou-my-image");
        assert_eq!(res.files.len(), 2);
        assert_eq!(
            res.files[1],
            "alicloud://cn-hangzhou/m-cn-hangzhou-my-image"
        );

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock(
                "POST",
                "/?Action=ImportImage&RegionId=cn-hangzhou&ImageName=my-image&Format=RAW",
            )
            .with_status(200)
            .with_body(r#"{"ImageId":"m-imported-99"}"#)
            .create_async()
            .await;

        let proc_http = AlicloudImportPostProcessor::new(AlicloudImportConfig {
            image_name: "my-image".to_string(),
            endpoint: Some(server.url()),
            ..Default::default()
        });
        let res_http = proc_http.process(artifact.clone()).await?;
        assert_eq!(res_http.id, "m-imported-99");

        let _m_fallback = server
            .mock(
                "POST",
                "/?Action=ImportImage&RegionId=cn-hangzhou&ImageName=my-image&Format=RAW",
            )
            .with_status(200)
            .with_body(r"{}")
            .create_async()
            .await;
        let res_fb = proc_http.process(artifact.clone()).await?;
        assert_eq!(res_fb.id, "m-cn-hangzhou-my-image");

        let _m_err = server
            .mock(
                "POST",
                "/?Action=ImportImage&RegionId=cn-hangzhou&ImageName=my-image&Format=RAW",
            )
            .with_status(500)
            .create_async()
            .await;
        assert!(proc_http.process(artifact).await.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_alicloud_import_empty_name() {
        let proc = AlicloudImportPostProcessor::new(AlicloudImportConfig {
            image_name: "   ".to_string(),
            ..Default::default()
        });
        let artifact = Artifact::new("art-123".to_string(), vec![]);
        assert!(proc.process(artifact).await.is_err());
    }
}
