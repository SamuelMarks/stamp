//! Implementation of the `artifactory` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use crate::types::SecretString;
use async_trait::async_trait;

/// Configuration for the `artifactory` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArtifactoryConfig {
    /// Identifier for this post-processor.
    pub identifier: String,
    /// Optional Artifactory repository URL.
    pub artifactory_url: Option<String>,
    /// Optional Artifactory username.
    pub username: Option<String>,
    /// Optional Artifactory password or API key.
    pub password: Option<SecretString>,
}

/// The `artifactory` post-processor.
#[derive(Debug, Clone)]
pub struct ArtifactoryPostProcessor {
    /// The configuration.
    pub config: ArtifactoryConfig,
}

impl ArtifactoryPostProcessor {
    /// Create a new `ArtifactoryPostProcessor`.
    #[must_use]
    pub const fn new(config: ArtifactoryConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for ArtifactoryPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty() {
            return Err(StampError::Parse("Identifier is empty".to_string()));
        }

        if let Some(ref url) = self.config.artifactory_url {
            let client = reqwest::Client::new();
            let mut req = client.put(format!(
                "{}/{}",
                url.trim_end_matches('/'),
                self.config.identifier
            ));
            if let (Some(u), Some(p)) = (&self.config.username, &self.config.password) {
                req = req.basic_auth(u, Some(p.expose_secret()));
            }
            let resp = req.send().await?;
            if !resp.status().is_success() {
                return Err(StampError::PostProcessor(format!(
                    "Artifactory upload failed with status: {}",
                    resp.status()
                )));
            }
        }

        let mut new_artifact = artifact.clone();
        new_artifact.id = format!("{}-{}", artifact.id, self.config.identifier);

        Ok(new_artifact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_artifactory_process_success() -> Result<(), StampError> {
        let config = ArtifactoryConfig {
            identifier: "processed".to_string(),
            ..Default::default()
        };
        let processor = ArtifactoryPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let result = processor.process(artifact.clone()).await?;
        assert_eq!(result.id, "base-processed");

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("PUT", "/processed")
            .with_status(201)
            .create_async()
            .await;

        let http_proc = ArtifactoryPostProcessor::new(ArtifactoryConfig {
            identifier: "processed".to_string(),
            artifactory_url: Some(server.url()),
            username: Some("user".to_string()),
            password: Some(SecretString::from("secret_api_key")),
        });
        let http_res = http_proc.process(artifact.clone()).await?;
        assert_eq!(http_res.id, "base-processed");

        let _m_err = server
            .mock("PUT", "/processed")
            .with_status(500)
            .create_async()
            .await;
        assert!(http_proc.process(artifact).await.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_artifactory_process_failure() {
        let config = ArtifactoryConfig {
            identifier: String::new(),
            ..Default::default()
        };
        let processor = ArtifactoryPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let res = processor.process(artifact).await;
        assert!(res.is_err());
    }

    #[test]
    fn test_artifactory_derived_traits() {
        let config1 = ArtifactoryConfig {
            identifier: "processed".to_string(),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert!(format!("{config1:?}").contains("ArtifactoryConfig"));
        let st1 = ArtifactoryPostProcessor::new(config1);
        let st2 = st1.clone();
        assert!(format!("{st1:?}").contains("ArtifactoryPostProcessor"));
        assert_eq!(st1.config, st2.config);
    }
}
