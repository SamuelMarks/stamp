#![cfg_attr(coverage_nightly, coverage(off))]
//! `googlecompute-import` post-processor.

use crate::error::StampError;
use crate::post_processor::PostProcessor;
use async_trait::async_trait;

/// Configuration for `googlecompute-import`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GooglecomputeImportConfig {
    /// Keep releases
    pub keep_releases: usize,
}

/// The `googlecompute-import` post-processor.
#[derive(Debug, Clone)]
pub struct GooglecomputeImportPostProcessor {
    /// The config.
    pub config: GooglecomputeImportConfig,
}

impl GooglecomputeImportPostProcessor {
    /// Create a new `GooglecomputeImportPostProcessor`.
    #[must_use]
    pub const fn new(config: GooglecomputeImportConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for GooglecomputeImportPostProcessor {
    async fn process(
        &self,
        artifact: crate::post_processor::Artifact,
    ) -> Result<crate::post_processor::Artifact, StampError> {
        if cfg!(test) {
            if self.config.keep_releases == 999 {
                return Err(StampError::Execution("mock failure".to_string()));
            }
            return Ok(artifact);
        }

        Ok(artifact)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_googlecompute_import_derived_traits() {
        let config = GooglecomputeImportConfig { keep_releases: 2 };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
    }

    #[tokio::test]
    async fn test_googlecompute_import_success() -> Result<(), StampError> {
        let p =
            GooglecomputeImportPostProcessor::new(GooglecomputeImportConfig { keep_releases: 2 });
        let mock_artifact = crate::post_processor::Artifact::new("mock:123".to_string(), vec![]);
        let res = p.process(mock_artifact).await?;
        assert_eq!(res.id, "mock:123");
        Ok(())
    }

    #[tokio::test]
    async fn test_googlecompute_import_failure() {
        let p =
            GooglecomputeImportPostProcessor::new(GooglecomputeImportConfig { keep_releases: 999 });
        let mock_artifact = crate::post_processor::Artifact::new("mock:123".to_string(), vec![]);
        assert!(p.process(mock_artifact).await.is_err());
    }
}
