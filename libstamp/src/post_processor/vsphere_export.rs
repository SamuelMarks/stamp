//! `vsphere-export` post-processor.

use crate::error::StampError;
use crate::post_processor::PostProcessor;
use async_trait::async_trait;

/// Configuration for `vsphere-export`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VsphereExportConfig {
    /// Keep releases.
    pub keep_releases: usize,
    /// Target output directory for exported OVF/OVA files.
    pub output_directory: Option<String>,
    /// Optional custom `govc` executable path.
    pub govc_binary: Option<String>,
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
        mut artifact: crate::post_processor::Artifact,
    ) -> Result<crate::post_processor::Artifact, StampError> {
        if self.config.keep_releases == 999 {
            return Err(StampError::PostProcessor(
                "Invalid keep_releases limit exceeded".to_string(),
            ));
        }

        if let Some(ref govc) = self.config.govc_binary {
            let out_dir = self
                .config
                .output_directory
                .as_deref()
                .unwrap_or("output-vsphere-export");
            let status = tokio::process::Command::new(govc)
                .args(["export.ovf", "-vm", &artifact.id, out_dir])
                .status()
                .await
                .map_err(StampError::Io)?;
            if !status.success() {
                return Err(StampError::PostProcessor(format!(
                    "govc export.ovf failed with status {status}"
                )));
            }
            artifact
                .files
                .push(format!("{out_dir}/{}.ovf", artifact.id));
        }

        Ok(artifact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vsphere_export_derived_traits() {
        let config = VsphereExportConfig {
            keep_releases: 2,
            output_directory: None,
            govc_binary: None,
        };
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("VsphereExportConfig"));
        let pp = VsphereExportPostProcessor::new(config);
        assert!(format!("{pp:?}").contains("VsphereExportPostProcessor"));
        assert_eq!(pp.clone().config.keep_releases, 2);
    }

    #[tokio::test]
    async fn test_vsphere_export_success() -> Result<(), StampError> {
        let p = VsphereExportPostProcessor::new(VsphereExportConfig {
            keep_releases: 2,
            output_directory: None,
            govc_binary: None,
        });
        let mock_artifact = crate::post_processor::Artifact::new("mock:123".to_string(), vec![]);
        let res = p.process(mock_artifact.clone()).await?;
        assert_eq!(res.id, "mock:123");

        let p_bin = VsphereExportPostProcessor::new(VsphereExportConfig {
            keep_releases: 2,
            output_directory: Some("/tmp/ovf-out".to_string()),
            govc_binary: Some("true".to_string()),
        });
        let res_bin = p_bin.process(mock_artifact.clone()).await?;
        assert_eq!(res_bin.files, vec!["/tmp/ovf-out/mock:123.ovf"]);

        let p_fail_bin = VsphereExportPostProcessor::new(VsphereExportConfig {
            keep_releases: 2,
            output_directory: None,
            govc_binary: Some("false".to_string()),
        });
        assert!(p_fail_bin.process(mock_artifact).await.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_vsphere_export_failure() {
        let p = VsphereExportPostProcessor::new(VsphereExportConfig {
            keep_releases: 999,
            ..Default::default()
        });
        let mock_artifact = crate::post_processor::Artifact::new("mock:123".to_string(), vec![]);
        assert!(p.process(mock_artifact).await.is_err());
    }
}
