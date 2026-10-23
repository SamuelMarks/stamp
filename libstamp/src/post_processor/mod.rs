//! Post-processors for transforming build artifacts.

use crate::error::StampError;
use async_trait::async_trait;

/// Re-exported unified build artifact type for post-processing pipelines.
pub type Artifact = crate::artifact::BuildArtifact;

/// The main trait for post-processors.
#[async_trait]
pub trait PostProcessor: Send + Sync {
    /// Process an artifact, returning a potentially transformed artifact.
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError>;

    /// Whether to keep the input artifact files after processing. Defaults to true.
    fn keep_input_artifact(&self) -> bool {
        true
    }

    /// Target source filters for `only`.
    fn only(&self) -> Option<&[String]> {
        None
    }

    /// Target source filters for `except`.
    fn except(&self) -> Option<&[String]> {
        None
    }
}

/// `alicloud_import` post-processor.
pub mod alicloud_import;
/// `artifactory` post-processor.
pub mod artifactory;
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
/// `vsphere_export` post-processor.
pub mod vsphere_export;
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
        "artifactory" => Ok(Box::new(artifactory::ArtifactoryPostProcessor::new(
            artifactory::ArtifactoryConfig::default(),
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
        "vsphere-export" => Ok(Box::new(vsphere_export::VsphereExportPostProcessor::new(
            vsphere_export::VsphereExportConfig::default(),
        ))),
        "vsphere-template" => Ok(Box::new(
            vsphere_template::VsphereTemplatePostProcessor::new(
                vsphere_template::VsphereTemplateConfig::default(),
            ),
        )),
        "vagrant" | "utm-vagrant" => Ok(Box::new(
            vagrant::VagrantPostProcessor::from_post_processor_config(config),
        )),
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

/// Out-of-process Go plugin post-processor RPC client.
pub mod go_plugin;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_post_processor_known() {
        let types = vec![
            "alicloud-import",
            "artifactory",
            "checksum",
            "compress",
            "digitalocean-import",
            "docker-commit",
            "docker-import",
            "docker-push",
            "docker-save",
            "docker-tag",
            "hcp",
            "manifest",
            "shell-local",
            "ucloud-import",
            "vsphere",
            "vsphere-export",
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
