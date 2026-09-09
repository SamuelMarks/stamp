#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `artifactory` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

/// Configuration for the `artifactory` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArtifactoryConfig {
    /// Identifier for this post-processor.
    pub identifier: String,
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
    #[cfg(not(tarpaulin_include))]
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty() {
            return Err(StampError::Parse("Identifier is empty".to_string()));
        }

        // Pass through or mutate artifact
        let mut new_artifact = artifact.clone();
        new_artifact.id = format!("{}-{}", artifact.id, self.config.identifier);

        Ok(new_artifact)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::pedantic,
    clippy::all
)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_artifactory_process_success() {
        let config = ArtifactoryConfig {
            identifier: "processed".to_string(),
        };
        let processor = ArtifactoryPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let result = processor
            .process(artifact)
            .await
            .expect("process succeeded");
        assert_eq!(result.id, "base-processed");
    }

    #[tokio::test]
    async fn test_artifactory_process_failure() {
        let config = ArtifactoryConfig {
            identifier: String::new(),
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
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = ArtifactoryPostProcessor::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
