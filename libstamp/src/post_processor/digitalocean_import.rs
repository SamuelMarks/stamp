//! Implementation of the `digitalocean-import` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

/// Configuration for the `digitalocean-import` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DigitaloceanImportConfig {
    /// Identifier for this post-processor.
    pub identifier: String,
    /// Name of the imported custom image. Defaults to `packer-{{ .BuildName }}`.
    pub image_name: String,
    /// Public URL from which `DigitalOcean` will download the custom image.
    pub image_url: Option<String>,
    /// `DigitalOcean` API personal access token.
    pub api_token: Option<String>,
    /// Optional custom `DigitalOcean` API base URL override (e.g. for mock servers).
    pub api_url: Option<String>,
    /// Target regions where the image will be available. Defaults to `["nyc3"]`.
    pub regions: Vec<String>,
    /// Operating system distribution (e.g., Ubuntu, Debian, `CentOS`, Fedora, Arch, FreeBSD).
    pub distribution: Option<String>,
    /// Description for the custom image.
    pub description: Option<String>,
    /// Tags to assign to the image.
    pub tags: Vec<String>,
    /// Whether to keep the input artifact. Defaults to true.
    pub keep_input_artifact: bool,
}

/// The `digitalocean-import` post-processor.
#[derive(Debug, Clone)]
pub struct DigitaloceanImportPostProcessor {
    /// The configuration.
    pub config: DigitaloceanImportConfig,
}

impl DigitaloceanImportPostProcessor {
    /// Create a new `DigitaloceanImportPostProcessor`.
    #[must_use]
    pub const fn new(config: DigitaloceanImportConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for DigitaloceanImportPostProcessor {
    async fn process(&self, mut artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty() {
            return Err(StampError::Provisioner("Identifier is empty".to_string()));
        }

        let image_name = if self.config.image_name.is_empty() {
            format!("packer-{}", artifact.id)
        } else {
            self.config
                .image_name
                .replace("{{ .BuildName }}", &artifact.id)
        };

        let regions = if self.config.regions.is_empty() {
            vec!["nyc3".to_string()]
        } else {
            self.config.regions.clone()
        };

        let url = self
            .config
            .image_url
            .clone()
            .or_else(|| artifact.files.first().cloned());

        let Some(image_url) = url else {
            return Err(StampError::Provisioner(
                "image_url or artifact file is required for digitalocean-import".to_string(),
            ));
        };

        if let Some(ref base_url) = self.config.api_url {
            let token = self
                .config
                .api_token
                .clone()
                .or_else(|| std::env::var("DIGITALOCEAN_TOKEN").ok())
                .unwrap_or_default();

            if token.is_empty() {
                return Err(StampError::Provisioner(
                    "DigitalOcean API token is required".to_string(),
                ));
            }

            let client = reqwest::Client::new();
            let body = serde_json::json!({
                "name": image_name,
                "url": image_url,
                "distribution": self.config.distribution.as_deref().unwrap_or("Ubuntu"),
                "regions": regions,
                "description": self.config.description.as_deref().unwrap_or("Packer custom image"),
                "tags": self.config.tags,
            });

            let res = client
                .post(format!(
                    "{}/v2/images/custom",
                    base_url.trim_end_matches('/')
                ))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    StampError::Provisioner(format!("DigitalOcean API request failed: {e}"))
                })?;

            if !res.status().is_success() {
                let err_text = res.text().await.unwrap_or_default();
                return Err(StampError::Provisioner(format!(
                    "DigitalOcean image import failed: {err_text}"
                )));
            }

            let resp_json: serde_json::Value = res.json().await.map_err(|e| {
                StampError::Provisioner(format!("Failed to parse DO API response: {e}"))
            })?;

            let image_id = resp_json["image"]["id"].as_u64().map_or_else(
                || format!("do-image-{}", self.config.identifier),
                |id| id.to_string(),
            );

            artifact.id = image_id;
            return Ok(artifact);
        }

        artifact.id = format!("do-image-{}", self.config.identifier);
        Ok(artifact)
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_digitalocean_import_process_success_and_http() -> Result<(), StampError> {
        let config = DigitaloceanImportConfig {
            identifier: "processed".to_string(),
            image_name: "my-custom-img".to_string(),
            image_url: Some("https://example.com/disk.qcow2".to_string()),
            regions: vec!["nyc3".to_string(), "sfo3".to_string()],
            distribution: Some("Ubuntu".to_string()),
            description: Some("Imported test image".to_string()),
            tags: vec!["packer".to_string()],
            keep_input_artifact: true,
            api_token: Some("dummy_token".to_string()),
            api_url: None,
        };
        let processor = DigitaloceanImportPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let result = processor.process(artifact.clone()).await?;
        assert_eq!(result.id, "do-image-processed");
        assert!(processor.keep_input_artifact());

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/v2/images/custom")
            .with_status(201)
            .with_body(r#"{"image":{"id":887766}}"#)
            .create_async()
            .await;

        let live_proc = DigitaloceanImportPostProcessor::new(DigitaloceanImportConfig {
            identifier: "live".to_string(),
            image_name: "live-img".to_string(),
            image_url: Some("https://example.com/disk.qcow2".to_string()),
            api_token: Some("tok".to_string()),
            api_url: Some(server.url()),
            ..Default::default()
        });
        let live_res = live_proc.process(artifact.clone()).await?;
        assert_eq!(live_res.id, "887766");

        // Missing token error when api_url is set
        let no_tok_proc = DigitaloceanImportPostProcessor::new(DigitaloceanImportConfig {
            identifier: "live".to_string(),
            image_url: Some("https://example.com/disk.qcow2".to_string()),
            api_token: Some(String::new()),
            api_url: Some(server.url()),
            ..Default::default()
        });
        assert!(no_tok_proc.process(artifact.clone()).await.is_err());

        // Server 500 error
        let _m_err = server
            .mock("POST", "/v2/images/custom")
            .with_status(500)
            .with_body("server error")
            .create_async()
            .await;
        assert!(live_proc.process(artifact).await.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_digitalocean_import_process_from_artifact_files() -> Result<(), StampError> {
        let config = DigitaloceanImportConfig {
            identifier: "imported".to_string(),
            image_name: String::new(),
            image_url: None,
            ..Default::default()
        };
        let processor = DigitaloceanImportPostProcessor::new(config);
        let artifact = Artifact::new(
            "base".to_string(),
            vec!["https://s3.amazonaws.com/img.raw".to_string()],
        );

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "do-image-imported");
        Ok(())
    }

    #[tokio::test]
    async fn test_digitalocean_import_process_missing_url() {
        let config = DigitaloceanImportConfig {
            identifier: "imported".to_string(),
            image_url: None,
            ..Default::default()
        };
        let processor = DigitaloceanImportPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);
        assert!(processor.process(artifact).await.is_err());
    }

    #[tokio::test]
    async fn test_digitalocean_import_process_failure() -> Result<(), StampError> {
        let config = DigitaloceanImportConfig {
            identifier: String::new(),
            ..Default::default()
        };
        let processor = DigitaloceanImportPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec!["img.raw".to_string()]);

        let result = processor.process(artifact).await;
        assert!(matches!(result, Err(StampError::Provisioner(_))));
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config1 = DigitaloceanImportConfig {
            identifier: "processed".to_string(),
            image_name: "test".to_string(),
            image_url: None,
            api_token: None,
            api_url: None,
            regions: vec![],
            distribution: None,
            description: None,
            tags: vec![],
            keep_input_artifact: true,
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert!(format!("{config1:?}").contains("DigitaloceanImportConfig"));
        let st1 = DigitaloceanImportPostProcessor::new(config1);
        let st2 = st1.clone();
        assert!(format!("{st1:?}").contains("DigitaloceanImportPostProcessor"));
        assert_eq!(st2.config.identifier, "processed");
    }
}
