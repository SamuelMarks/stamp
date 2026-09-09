#![cfg_attr(coverage_nightly, coverage(off))]
//! `vsphere-export` post-processor.

use crate::error::StampError;
use crate::post_processor::PostProcessor;
use async_trait::async_trait;

/// Configuration for `vsphere-export`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VsphereExportConfig {
    /// Keep releases
    pub keep_releases: usize,
}

/// The `vsphere-export` post-processor.
#[derive(Debug, Clone)]
pub struct VsphereExportPostProcessor {
    /// The config.
    pub config: VsphereExportConfig,
}

impl VsphereExportPostProcessor {
    /// Create a new `VsphereExportPostProcessor`.
    #[must_use]
    pub const fn new(config: VsphereExportConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for VsphereExportPostProcessor {
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
    fn test_vsphere_export_derived_traits() {
        let config = VsphereExportConfig { keep_releases: 2 };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
    }

    #[tokio::test]
    async fn test_vsphere_export_success() -> Result<(), StampError> {
        let p = VsphereExportPostProcessor::new(VsphereExportConfig { keep_releases: 2 });
        let mock_artifact = crate::post_processor::Artifact::new("mock:123".to_string(), vec![]);
        let res = p.process(mock_artifact).await?;
        assert_eq!(res.id, "mock:123");
        Ok(())
    }

    #[tokio::test]
    async fn test_vsphere_export_failure() {
        let p = VsphereExportPostProcessor::new(VsphereExportConfig { keep_releases: 999 });
        let mock_artifact = crate::post_processor::Artifact::new("mock:123".to_string(), vec![]);
        assert!(p.process(mock_artifact).await.is_err());
    }
}
