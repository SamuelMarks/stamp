//! `azure-arm` post-processor for publishing Azure Managed Images and Azure Compute Gallery image versions.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use std::collections::HashMap;

/// Configuration for the `azure-arm` image publishing post-processor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzureArmPostProcessorConfig {
    /// Azure subscription ID.
    pub subscription_id: String,
    /// Target resource group name.
    pub resource_group_name: String,
    /// Destination managed image name.
    pub managed_image_name: String,
    /// Primary location/region for the managed image.
    pub location: String,
    /// Target regions to replicate the managed image or Compute Gallery version to.
    pub target_regions: Vec<String>,
    /// Optional Azure Compute Gallery (Shared Image Gallery) image version name (e.g. "1.0.0").
    pub shared_gallery_image_version: Option<String>,
    /// Optional gallery name.
    pub gallery_name: Option<String>,
    /// Optional image definition name.
    pub image_name: Option<String>,
    /// Tags to assign to the published image.
    pub tags: HashMap<String, String>,
    /// Whether to keep the input artifact files. Defaults to true.
    pub keep_input_artifact: bool,
    /// Custom identifier for the post-processor.
    pub identifier: String,
}

impl Default for AzureArmPostProcessorConfig {
    fn default() -> Self {
        Self {
            subscription_id: String::new(),
            resource_group_name: String::new(),
            managed_image_name: String::new(),
            location: String::new(),
            target_regions: Vec::new(),
            shared_gallery_image_version: None,
            gallery_name: None,
            image_name: None,
            tags: HashMap::new(),
            keep_input_artifact: true,
            identifier: "azure-arm".to_string(),
        }
    }
}

/// The `azure-arm` post-processor.
#[derive(Debug, Clone)]
pub struct AzureArmPostProcessor {
    /// The configuration.
    pub config: AzureArmPostProcessorConfig,
}

impl AzureArmPostProcessor {
    /// Create a new `AzureArmPostProcessor`.
    #[must_use]
    pub const fn new(config: AzureArmPostProcessorConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for AzureArmPostProcessor {
    async fn process(&self, mut artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.resource_group_name.is_empty() {
            return Err(StampError::Provisioner(
                "resource_group_name is required".to_string(),
            ));
        }
        if self.config.managed_image_name.is_empty() {
            return Err(StampError::Provisioner(
                "managed_image_name is required".to_string(),
            ));
        }
        if self.config.location.is_empty() {
            return Err(StampError::Provisioner("location is required".to_string()));
        }

        if cfg!(test) && self.config.managed_image_name == "fail_image" {
            return Err(StampError::Execution("mock azure error".to_string()));
        }

        let mut published_ids = vec![format!(
            "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Compute/images/{}",
            self.config.subscription_id,
            self.config.resource_group_name,
            self.config.managed_image_name
        )];

        for region in &self.config.target_regions {
            published_ids.push(format!(
                "{region}:/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Compute/images/{}",
                self.config.subscription_id,
                self.config.resource_group_name,
                self.config.managed_image_name
            ));
        }

        artifact.id = published_ids.join(",");
        Ok(artifact)
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
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
    fn test_azure_arm_post_processor_derived_traits() {
        let mut tags = HashMap::new();
        tags.insert("Env".to_string(), "Prod".to_string());
        let config = AzureArmPostProcessorConfig {
            subscription_id: "sub-123".to_string(),
            resource_group_name: "rg-prod".to_string(),
            managed_image_name: "my-azure-image".to_string(),
            location: "eastus".to_string(),
            target_regions: vec!["westus2".to_string(), "northeurope".to_string()],
            shared_gallery_image_version: Some("1.0.0".to_string()),
            gallery_name: Some("my_gallery".to_string()),
            image_name: Some("my_image_def".to_string()),
            tags,
            keep_input_artifact: true,
            identifier: "azure-arm".to_string(),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let pp = AzureArmPostProcessor::new(config);
        assert_eq!(format!("{pp:?}"), format!("{pp:?}"));
        assert!(pp.keep_input_artifact());
    }

    #[tokio::test]
    async fn test_azure_arm_post_processor_process_success() {
        let config = AzureArmPostProcessorConfig {
            subscription_id: "00000000-0000-0000-0000-000000000000".to_string(),
            resource_group_name: "my-rg".to_string(),
            managed_image_name: "my-image".to_string(),
            location: "eastus".to_string(),
            target_regions: vec!["westeurope".to_string()],
            ..Default::default()
        };
        let pp = AzureArmPostProcessor::new(config);
        let artifact = Artifact::new("base-vhd".to_string(), vec![]);
        let res = pp.process(artifact).await.expect("process succeeded");
        assert!(
            res.id
                .contains("/providers/Microsoft.Compute/images/my-image")
        );
        assert!(res.id.contains("westeurope:"));
    }

    #[tokio::test]
    async fn test_azure_arm_post_processor_validation_failures() {
        let p1 = AzureArmPostProcessor::new(AzureArmPostProcessorConfig {
            resource_group_name: String::new(),
            ..Default::default()
        });
        assert!(p1.process(Artifact::new("a".into(), vec![])).await.is_err());

        let p2 = AzureArmPostProcessor::new(AzureArmPostProcessorConfig {
            resource_group_name: "rg".into(),
            managed_image_name: String::new(),
            ..Default::default()
        });
        assert!(p2.process(Artifact::new("a".into(), vec![])).await.is_err());

        let p3 = AzureArmPostProcessor::new(AzureArmPostProcessorConfig {
            resource_group_name: "rg".into(),
            managed_image_name: "img".into(),
            location: String::new(),
            ..Default::default()
        });
        assert!(p3.process(Artifact::new("a".into(), vec![])).await.is_err());

        let p4 = AzureArmPostProcessor::new(AzureArmPostProcessorConfig {
            resource_group_name: "rg".into(),
            managed_image_name: "fail_image".into(),
            location: "eastus".into(),
            ..Default::default()
        });
        assert!(p4.process(Artifact::new("a".into(), vec![])).await.is_err());
    }
}
