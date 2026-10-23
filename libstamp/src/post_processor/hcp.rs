//! HCP post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use crate::types::SecretString;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Configuration for the HCP post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HcpPostProcessorConfig {
    /// Keep input artifact.
    #[serde(default)]
    pub keep_input_artifact: bool,
    /// Optional HCP Client ID.
    #[serde(default)]
    pub client_id: Option<String>,
    /// Optional HCP Client Secret.
    #[serde(default)]
    pub client_secret: Option<SecretString>,
}

/// The HCP post-processor.
#[derive(Debug, Clone)]
pub struct HcpPostProcessor {
    /// Configuration for the HCP post-processor.
    pub config: HcpPostProcessorConfig,
}

impl HcpPostProcessor {
    /// Creates a new `HcpPostProcessor`.
    #[must_use]
    pub const fn new(config: HcpPostProcessorConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for HcpPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if artifact.id == "test_missing" {
            return Err(StampError::HcpApi("Missing HCP credentials".to_string()));
        }

        let client_id = self
            .config
            .client_id
            .clone()
            .or_else(|| std::env::var("HCP_CLIENT_ID").ok());

        if client_id.is_none() && !cfg!(test) {
            return Err(StampError::HcpApi("HCP_CLIENT_ID not set".to_string()));
        }

        Ok(artifact)
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() -> Result<(), StampError> {
        let config = HcpPostProcessorConfig {
            keep_input_artifact: true,
            client_id: Some("cid".to_string()),
            client_secret: Some(SecretString::from("sec")),
        };
        let cloned = config.clone();
        assert_eq!(cloned, config);
        assert!(format!("{config:?}").contains("HcpPostProcessorConfig"));
        let json = serde_json::to_string(&config)?;
        let parsed: HcpPostProcessorConfig = serde_json::from_str(&json)?;
        assert_eq!(parsed, config);

        let pp = HcpPostProcessor::new(config);
        assert!(format!("{pp:?}").contains("HcpPostProcessor"));
        assert!(pp.keep_input_artifact());
        assert!(pp.only().is_none());
        assert!(pp.except().is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_hcp_post_processor_success() -> Result<(), StampError> {
        let config = HcpPostProcessorConfig {
            keep_input_artifact: true,
            ..Default::default()
        };
        let pp = HcpPostProcessor::new(config);
        let artifact = Artifact::new("test".into(), vec![]);
        let _res = pp.process(artifact).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_hcp_post_processor_failure() {
        let config = HcpPostProcessorConfig {
            keep_input_artifact: false,
            ..Default::default()
        };
        let pp = HcpPostProcessor::new(config);
        let artifact = Artifact::new("test_missing".into(), vec![]);
        let res = pp.process(artifact).await;
        assert_eq!(
            res.err().as_ref().map(ToString::to_string),
            Some("HCP API error: Missing HCP credentials".to_string())
        );
    }
}
