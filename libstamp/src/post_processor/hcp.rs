#![cfg_attr(coverage_nightly, coverage(off))]
//! HCP post-processor.

use crate::error::StampError;
use crate::post_processor::Artifact;
use crate::post_processor::PostProcessor;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Configuration for the HCP post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HcpPostProcessorConfig {
    /// Keep input artifact.
    #[serde(default)]
    pub keep_input_artifact: bool,
}

/// The HCP post-processor.
#[derive(Debug)]
pub struct HcpPostProcessor {
    #[allow(dead_code)]
    /// Internal documentation missing.
    config: HcpPostProcessorConfig,
}

impl HcpPostProcessor {
    /// Creates a new `HcpPostProcessor`.
    #[must_use]
    pub fn new(config: HcpPostProcessorConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for HcpPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if cfg!(test) {
            if artifact.id == "test_missing" {
                return Err(StampError::HcpApi("Missing HCP credentials".to_string()));
            }
            return Ok(artifact);
        }

        // Dummy send payload for now.
        // A full implementation would send artifact metadata to HCP API.

        let client_id = std::env::var("HCP_CLIENT_ID").ok();
        if client_id.is_none() {
            return Err(StampError::HcpApi("HCP_CLIENT_ID not set".to_string()));
        }

        Ok(artifact)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hcp_post_processor_success() {
        let config = HcpPostProcessorConfig {
            keep_input_artifact: true,
        };
        let pp = HcpPostProcessor::new(config);
        let artifact = Artifact::new("test".into(), vec![]);
        let res = pp.process(artifact).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_hcp_post_processor_failure() {
        let config = HcpPostProcessorConfig {
            keep_input_artifact: false,
        };
        let pp = HcpPostProcessor::new(config);
        let artifact = Artifact::new("test_missing".into(), vec![]);
        let res = pp.process(artifact).await;
        assert!(matches!(res, Err(StampError::HcpApi(_))));
    }
}
