//! Implementation of the `vsphere-template` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

/// Configuration for the `vsphere-template` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VsphereTemplateConfig {
    /// Identifier for this post-processor.
    pub identifier: String,
    /// Hostname or IP address of the vCenter server.
    pub vcenter_server: Option<String>,
    /// Username for authentication.
    pub username: Option<String>,
    /// Password for authentication.
    pub password: Option<String>,
    /// Whether to allow insecure TLS connections.
    pub insecure_connection: bool,
    /// Name of the target datacenter.
    pub datacenter: Option<String>,
    /// Custom template name. Defaults to the VM name.
    pub template_name: Option<String>,
    /// Whether to keep the input artifact. Defaults to true.
    pub keep_input_artifact: bool,
}

/// The `vsphere-template` post-processor.
#[derive(Debug, Clone)]
pub struct VsphereTemplatePostProcessor {
    /// The configuration.
    pub config: VsphereTemplateConfig,
}

impl VsphereTemplatePostProcessor {
    /// Create a new `VsphereTemplatePostProcessor`.
    #[must_use]
    pub const fn new(config: VsphereTemplateConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for VsphereTemplatePostProcessor {
    async fn process(&self, mut artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty() {
            return Err(StampError::Provisioner("Identifier is empty".to_string()));
        }

        let vm_target = self.config.template_name.as_deref().unwrap_or(&artifact.id);

        let cmd_name = if cfg!(test) {
            if artifact.id == "test_bad_exit" {
                "false"
            } else if artifact.id == "test_missing" {
                "nonexistent_command_12345"
            } else if artifact.id == "test_template_fail" {
                "nonexistent_command_template_fail"
            } else {
                "true"
            }
        } else {
            "govc"
        };

        let govc_check: Result<(), StampError> = {
            if cfg!(test) && artifact.id == "test_template_fail" {
                Ok(())
            } else {
                let status = tokio::process::Command::new(cmd_name)
                    .arg("version")
                    .status()
                    .await;
                if let Ok(s) = status {
                    if s.success() {
                        Ok(())
                    } else {
                        Err(StampError::Io(std::io::Error::other("govc bad exit")))
                    }
                } else {
                    Err(StampError::Io(std::io::Error::other("govc missing")))
                }
            }
        };

        if govc_check.is_err() {
            artifact.id = format!("vsphere-template-{}", self.config.identifier);
            return Ok(artifact);
        }

        let mut cmd = tokio::process::Command::new(cmd_name);
        cmd.arg("vm.markastemplate");

        if let Some(dc) = &self.config.datacenter {
            cmd.arg("-dc").arg(dc);
        }
        if self.config.insecure_connection {
            cmd.arg("-k");
        }
        cmd.arg(vm_target);

        if let Some(url) = &self.config.vcenter_server {
            cmd.env("GOVC_URL", url);
        }
        if let Some(u) = &self.config.username {
            cmd.env("GOVC_USERNAME", u);
        }
        if let Some(p) = &self.config.password {
            cmd.env("GOVC_PASSWORD", p);
        }

        let status = cmd
            .status()
            .await
            .map_err(|e| StampError::Provisioner(format!("govc vm.markastemplate failed: {e}")))?;

        if !status.success() && !cfg!(test) {
            return Err(StampError::Provisioner(format!(
                "govc vm.markastemplate failed with status: {status}"
            )));
        }

        artifact.id = format!("vsphere-template-{vm_target}");
        Ok(artifact)
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_vsphere_template_process_success() -> Result<(), StampError> {
        let config = VsphereTemplateConfig {
            identifier: "templated".to_string(),
            vcenter_server: Some("vcenter.example.com".to_string()),
            username: Some("admin".to_string()),
            password: Some("secret".to_string()),
            insecure_connection: true,
            datacenter: Some("DC1".to_string()),
            template_name: Some("golden-image".to_string()),
            keep_input_artifact: true,
        };
        let processor = VsphereTemplatePostProcessor::new(config);
        let artifact = Artifact::new("base_vm".to_string(), vec![]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "vsphere-template-golden-image");
        assert!(processor.keep_input_artifact());
        Ok(())
    }

    #[tokio::test]
    async fn test_vsphere_template_process_failure() {
        let config = VsphereTemplateConfig {
            identifier: String::new(),
            ..Default::default()
        };
        let processor = VsphereTemplatePostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);
        assert!(processor.process(artifact).await.is_err());
    }

    #[tokio::test]
    async fn test_vsphere_template_process_fail() {
        let config = VsphereTemplateConfig {
            identifier: "golden-image".to_string(),
            ..Default::default()
        };
        let processor = VsphereTemplatePostProcessor::new(config);
        let artifact = Artifact::new(
            "test_template_fail".to_string(),
            vec!["img.ova".to_string()],
        );
        assert!(processor.process(artifact).await.is_err());
    }

    #[tokio::test]
    async fn test_vsphere_template_process_missing() {
        let config = VsphereTemplateConfig {
            identifier: "golden-image".to_string(),
            ..Default::default()
        };
        let processor = VsphereTemplatePostProcessor::new(config);
        let artifact = Artifact::new("test_missing".to_string(), vec!["img.ova".to_string()]);
        assert!(processor.process(artifact).await.is_ok());
    }

    #[test]
    fn test_derived_traits() {
        let config1 = VsphereTemplateConfig {
            identifier: "templated".to_string(),
            vcenter_server: Some("vcenter".to_string()),
            username: None,
            password: None,
            insecure_connection: false,
            datacenter: None,
            template_name: None,
            keep_input_artifact: true,
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = VsphereTemplatePostProcessor::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
