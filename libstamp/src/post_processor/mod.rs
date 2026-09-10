#![cfg_attr(coverage_nightly, coverage(off))]
//! Post-processors for transforming build artifacts.

use crate::error::StampError;
use async_trait::async_trait;

/// A build artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// The ID of the artifact.
    pub id: String,
    /// List of files associated with the artifact.
    pub files: Vec<String>,
}

impl Artifact {
    /// Create a new `Artifact`.
    #[must_use]
    pub const fn new(id: String, files: Vec<String>) -> Self {
        Self { id, files }
    }
}

/// The main trait for post-processors.
#[async_trait]
pub trait PostProcessor: Send + Sync {
    /// Process an artifact, returning a potentially transformed artifact.
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError>;

    /// Whether to keep the input artifact files after processing. Defaults to true.
    fn keep_input_artifact(&self) -> bool {
        true
    }
}

/// `alicloud_import` post-processor.
pub mod alicloud_import;
/// `amazon_ami_management` post-processor.
pub mod amazon_ami_management;
/// `amazon_import` post-processor.
pub mod amazon_import;
/// `artifactory` post-processor.
pub mod artifactory;
/// `azure_arm` post-processor.
pub mod azure_arm;
/// `checksum` post-processor.
pub mod checksum;
/// `compress` post-processor.
pub mod compress;
/// `digitalocean_import` post-processor.
pub mod digitalocean_import;
/// `docker_commit` post-processor.
pub mod docker_commit;
/// `docker_import` post-processor.
pub mod docker_import;
/// `docker_push` post-processor.
pub mod docker_push;
/// `docker_save` post-processor.
pub mod docker_save;
/// `docker_tag` post-processor.
pub mod docker_tag;
/// `googlecompute_export` post-processor.
pub mod googlecompute_export;
/// `hcp` post-processor.
pub mod hcp;
/// `manifest` post-processor.
pub mod manifest;
/// `pipeline` execution engine.
pub mod pipeline;
/// `shell_local` post-processor.
pub mod shell_local;
/// `ucloud_import` post-processor.
pub mod ucloud_import;
/// `vagrant` post-processor.
pub mod vagrant;
/// `vagrant_cloud` post-processor.
pub mod vagrant_cloud;
/// `vsphere` post-processor.
pub mod vsphere;
/// `vsphere_template` post-processor.
pub mod vsphere_template;
/// `yandex_import` post-processor.
pub mod yandex_import;

pub use checksum::{ChecksumAlgorithm, ChecksumConfig, ChecksumPostProcessor};
pub use pipeline::{PipelineBranch, PostProcessorPipeline};

use crate::template::PostProcessorConfig;

/// Creates a new post-processor instance from configuration.
///
/// # Errors
///
/// Returns a `StampError` if the post-processor type is unknown.
pub fn create_post_processor(
    config: &PostProcessorConfig,
) -> Result<Box<dyn PostProcessor>, StampError> {
    match config.post_processor_type.as_str() {
        "alicloud-import" => Ok(Box::new(alicloud_import::AlicloudImportPostProcessor::new(
            alicloud_import::AlicloudImportConfig::default(),
        ))),
        "amazon-ami-management" => Ok(Box::new(
            amazon_ami_management::AmazonAmiManagementPostProcessor::new(
                amazon_ami_management::AmazonAmiManagementConfig::default(),
            ),
        )),
        "amazon-import" => Ok(Box::new(amazon_import::AmazonImportPostProcessor::new(
            amazon_import::AmazonImportConfig::default(),
        ))),
        "artifactory" => Ok(Box::new(artifactory::ArtifactoryPostProcessor::new(
            artifactory::ArtifactoryConfig::default(),
        ))),
        "azure-arm" | "azure-image" => Ok(Box::new(azure_arm::AzureArmPostProcessor::new(
            azure_arm::AzureArmPostProcessorConfig::default(),
        ))),
        "checksum" => Ok(Box::new(checksum::ChecksumPostProcessor::new(
            checksum::ChecksumConfig::default(),
        ))),
        "compress" => Ok(Box::new(compress::CompressPostProcessor::new(
            compress::CompressConfig::default(),
        ))),
        "digitalocean-import" => Ok(Box::new(
            digitalocean_import::DigitaloceanImportPostProcessor::new(
                digitalocean_import::DigitaloceanImportConfig::default(),
            ),
        )),
        "docker-commit" => Ok(Box::new(docker_commit::DockerCommitPostProcessor::new(
            docker_commit::DockerCommitConfig::default(),
        ))),
        "docker-import" => Ok(Box::new(docker_import::DockerImportPostProcessor::new(
            docker_import::DockerImportConfig::default(),
        ))),
        "docker-push" => Ok(Box::new(docker_push::DockerPushPostProcessor::new(
            docker_push::DockerPushConfig::default(),
        ))),
        "docker-save" => Ok(Box::new(docker_save::DockerSavePostProcessor::new(
            docker_save::DockerSaveConfig::default(),
        ))),
        "docker-tag" => Ok(Box::new(docker_tag::DockerTagPostProcessor::new(
            docker_tag::DockerTagConfig::default(),
        ))),
        "googlecompute-export" => Ok(Box::new(
            googlecompute_export::GooglecomputeExportPostProcessor::new(
                googlecompute_export::GooglecomputeExportConfig::default(),
            ),
        )),
        "hcp" => Ok(Box::new(hcp::HcpPostProcessor::new(
            hcp::HcpPostProcessorConfig {
                keep_input_artifact: false,
            },
        ))),
        "manifest" => Ok(Box::new(manifest::ManifestPostProcessor::new(
            manifest::ManifestConfig::default(),
        ))),
        "shell-local" => Ok(Box::new(shell_local::ShellLocalPostProcessor::new(
            shell_local::ShellLocalConfig::default(),
        ))),
        "vsphere" => Ok(Box::new(vsphere::VspherePostProcessor::new(
            vsphere::VsphereConfig::default(),
        ))),
        "vsphere-template" => Ok(Box::new(
            vsphere_template::VsphereTemplatePostProcessor::new(
                vsphere_template::VsphereTemplateConfig::default(),
            ),
        )),
        "vagrant" => Ok(Box::new(vagrant::VagrantPostProcessor::new(
            vagrant::VagrantConfig::default(),
        ))),
        "vagrant-cloud" => Ok(Box::new(vagrant_cloud::VagrantCloudPostProcessor::new(
            vagrant_cloud::VagrantCloudConfig::default(),
        ))),
        "ucloud-import" => Ok(Box::new(ucloud_import::UcloudImportPostProcessor::new(
            ucloud_import::UcloudImportConfig::default(),
        ))),
        "yandex-import" => Ok(Box::new(yandex_import::YandexImportPostProcessor::new(
            yandex_import::YandexImportConfig::default(),
        ))),
        "plugin" | "go-plugin" => Ok(Box::new(go_plugin::GoPluginPostProcessor::new(
            go_plugin::GoPluginPostProcessorConfig {
                post_processor_type: config.post_processor_type.clone(),
                plugin_path: config
                    .config
                    .get("plugin_path")
                    .cloned()
                    .unwrap_or_default(),
                endpoint: config.config.get("endpoint").cloned(),
            },
        ))),
        _ => Err(StampError::Parse(format!(
            "Unknown post-processor type: {}",
            config.post_processor_type
        ))),
    }
}

pub mod go_plugin;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_create_post_processor_known() {
        let types = vec![
            "alicloud-import",
            "amazon-ami-management",
            "amazon-import",
            "artifactory",
            "azure-arm",
            "azure-image",
            "checksum",
            "compress",
            "digitalocean-import",
            "docker-commit",
            "docker-import",
            "docker-push",
            "docker-save",
            "docker-tag",
            "googlecompute-export",
            "hcp",
            "manifest",
            "shell-local",
            "ucloud-import",
            "vsphere",
            "vsphere-template",
            "vagrant",
            "vagrant-cloud",
            "yandex-import",
            "plugin",
            "go-plugin",
        ];

        for p_type in types {
            let c = PostProcessorConfig {
                post_processor_type: p_type.to_string(),
                ..Default::default()
            };
            assert!(create_post_processor(&c).is_ok());
        }
    }

    #[test]
    fn test_create_post_processor_unknown() {
        let c = PostProcessorConfig {
            post_processor_type: "unknown".to_string(),
            ..Default::default()
        };
        assert!(create_post_processor(&c).is_err());
    }
}
