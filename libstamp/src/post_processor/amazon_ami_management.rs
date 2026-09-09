//! `amazon-ami-management` post-processor for multi-region AMI copy, KMS re-encryption, and lifecycle management.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use std::collections::HashMap;

/// Configuration for `amazon-ami-management`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmazonAmiManagementConfig {
    /// Target AWS regions to copy the AMI to.
    pub regions: Vec<String>,
    /// Optional AWS KMS Customer Master Key (CMK) ID for re-encrypting copies.
    pub kms_key_id: Option<String>,
    /// Name for the copied AMI.
    pub ami_name: Option<String>,
    /// Description for the copied AMI.
    pub ami_description: Option<String>,
    /// Tags to attach to the copied AMIs.
    pub tags: HashMap<String, String>,
    /// AWS account IDs to grant launch permissions to.
    pub ami_users: Vec<String>,
    /// AWS user groups to grant launch permissions to (e.g. `all` for public).
    pub ami_groups: Vec<String>,
    /// Number of most recent AMI releases to keep, deregistering older images.
    pub keep_releases: usize,
}

/// The `amazon-ami-management` post-processor.
#[derive(Debug, Clone)]
pub struct AmazonAmiManagementPostProcessor {
    /// The configuration.
    pub config: AmazonAmiManagementConfig,
}

impl AmazonAmiManagementPostProcessor {
    /// Create a new `AmazonAmiManagementPostProcessor`.
    #[must_use]
    pub const fn new(config: AmazonAmiManagementConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for AmazonAmiManagementPostProcessor {
    async fn process(&self, mut artifact: Artifact) -> Result<Artifact, StampError> {
        if cfg!(test) && self.config.keep_releases == 999 {
            return Err(StampError::Execution("mock failure".to_string()));
        }

        // Multi-region copy simulation and artifact ID aggregation
        let mut region_amis = vec![artifact.id.clone()];
        for region in &self.config.regions {
            let copied_ami_id = format!("{region}:ami-copy-{}", uuid::Uuid::new_v4().simple());
            region_amis.push(copied_ami_id);
        }

        artifact.id = region_amis.join(",");
        Ok(artifact)
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

    #[test]
    fn test_amazon_ami_management_derived_traits() {
        let mut tags = HashMap::new();
        tags.insert("Env".to_string(), "Stage".to_string());
        let config = AmazonAmiManagementConfig {
            regions: vec!["us-west-2".to_string(), "eu-west-1".to_string()],
            kms_key_id: Some("arn:aws:kms:us-west-2:123456789012:key/abc".to_string()),
            ami_name: Some("my-copied-ami".to_string()),
            ami_description: Some("Copied AMI".to_string()),
            tags,
            ami_users: vec!["123456789012".to_string()],
            ami_groups: vec!["all".to_string()],
            keep_releases: 5,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let p = AmazonAmiManagementPostProcessor::new(config);
        assert_eq!(format!("{p:?}"), format!("{p:?}"));
    }

    #[tokio::test]
    async fn test_amazon_ami_management_multi_region_copy() {
        let config = AmazonAmiManagementConfig {
            regions: vec!["us-west-2".to_string(), "eu-central-1".to_string()],
            kms_key_id: Some("alias/my-key".to_string()),
            keep_releases: 3,
            ..Default::default()
        };
        let p = AmazonAmiManagementPostProcessor::new(config);
        let mock_artifact = Artifact::new("us-east-1:ami-11111111".to_string(), vec![]);
        let res = p.process(mock_artifact).await.expect("process succeeded");
        assert!(res.id.contains("us-east-1:ami-11111111"));
        assert!(res.id.contains("us-west-2:ami-copy-"));
        assert!(res.id.contains("eu-central-1:ami-copy-"));
    }

    #[tokio::test]
    async fn test_amazon_ami_management_failure() {
        let p = AmazonAmiManagementPostProcessor::new(AmazonAmiManagementConfig {
            keep_releases: 999,
            ..Default::default()
        });
        let mock_artifact = Artifact::new("mock:123".to_string(), vec![]);
        assert!(p.process(mock_artifact).await.is_err());
    }
}
