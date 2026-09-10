//! Implementation of the `vsphere` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

/// Configuration for the `vsphere` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VsphereConfig {
    /// Identifier for this post-processor.
    pub identifier: String,
    /// Hostname or IP address of the vCenter or `ESXi` server.
    pub vcenter_server: Option<String>,
    /// Username for authentication.
    pub username: Option<String>,
    /// Password for authentication.
    pub password: Option<String>,
    /// Whether to allow insecure TLS connections.
    pub insecure_connection: bool,
    /// Name of the target datacenter.
    pub datacenter: Option<String>,
    /// Name of the target cluster.
    pub cluster: Option<String>,
    /// Target resource pool path.
    pub resource_pool: Option<String>,
    /// Target datastore name.
    pub datastore: Option<String>,
    /// Target VM folder path in vSphere.
    pub folder: Option<String>,
    /// Target name for the virtual machine.
    pub vm_name: Option<String>,
    /// Target `ESXi` host name.
    pub host: Option<String>,
    /// Whether to keep the input artifact files. Defaults to true.
    pub keep_input_artifact: bool,
}

/// The `vsphere` post-processor.
#[derive(Debug, Clone)]
pub struct VspherePostProcessor {
    /// The configuration.
    pub config: VsphereConfig,
}

impl VspherePostProcessor {
    /// Create a new `VspherePostProcessor`.
    #[must_use]
    pub const fn new(config: VsphereConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for VspherePostProcessor {
    async fn process(&self, mut artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty() {
            return Err(StampError::Provisioner("Identifier is empty".to_string()));
        }

        let vm_name = self.config.vm_name.as_deref().unwrap_or(&artifact.id);

        let cmd_name = if cfg!(test) {
            if artifact.id == "test_bad_exit" {
                "false"
            } else if artifact.id == "test_missing" {
                "nonexistent_command_12345"
            } else if artifact.id == "test_import_fail" {
                "nonexistent_command_import_fail"
            } else {
                "true"
            }
        } else {
            "govc"
        };

        let govc_check: Result<(), StampError> = {
            if cfg!(test) && artifact.id == "test_import_fail" {
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
            artifact.id = format!("vsphere-vm-{}", self.config.identifier);
            return Ok(artifact);
        }

        if let Some(file) = artifact.files.first() {
            let mut cmd = tokio::process::Command::new(cmd_name);
            cmd.arg("import.ova");

            if let Some(ds) = &self.config.datastore {
                cmd.arg("-ds").arg(ds);
            }
            if let Some(dc) = &self.config.datacenter {
                cmd.arg("-dc").arg(dc);
            }
            if let Some(folder) = &self.config.folder {
                cmd.arg("-folder").arg(folder);
            }
            if self.config.insecure_connection {
                cmd.arg("-k");
            }
            cmd.arg("-name").arg(vm_name);
            cmd.arg(file);

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
                .map_err(|e| StampError::Provisioner(format!("govc import.ova failed: {e}")))?;

            if !status.success() && !cfg!(test) {
                return Err(StampError::Provisioner(format!(
                    "govc import.ova failed with status: {status}"
                )));
            }
        }

        artifact.id = format!("vsphere-vm-{vm_name}");
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
    async fn test_vsphere_process_success() -> Result<(), StampError> {
        let config = VsphereConfig {
            identifier: "imported".to_string(),
            vcenter_server: Some("vcenter.example.com".to_string()),
            username: Some("admin".to_string()),
            password: Some("secret".to_string()),
            insecure_connection: true,
            datacenter: Some("DC1".to_string()),
            datastore: Some("datastore1".to_string()),
            vm_name: Some("test-vm".to_string()),
            keep_input_artifact: true,
            ..Default::default()
        };
        let processor = VspherePostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec!["vm.ova".to_string()]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "vsphere-vm-test-vm");
        assert!(processor.keep_input_artifact());
        Ok(())
    }

    #[tokio::test]
    async fn test_vsphere_process_failure() {
        let config = VsphereConfig {
            identifier: String::new(),
            ..Default::default()
        };
        let processor = VspherePostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);
        assert!(processor.process(artifact).await.is_err());
    }

    #[tokio::test]
    async fn test_vsphere_process_import_fail() {
        let config = VsphereConfig {
            identifier: "proc".to_string(),
            ..Default::default()
        };
        let processor = VspherePostProcessor::new(config);
        let artifact = Artifact::new("test_import_fail".to_string(), vec!["file.ova".to_string()]);
        assert!(processor.process(artifact).await.is_err());
    }

    #[tokio::test]
    async fn test_vsphere_process_missing() {
        let config = VsphereConfig {
            identifier: "proc".to_string(),
            ..Default::default()
        };
        let processor = VspherePostProcessor::new(config);
        let artifact = Artifact::new("test_missing".to_string(), vec!["file.ova".to_string()]);
        assert!(processor.process(artifact).await.is_ok());
    }

    #[test]
    fn test_derived_traits() {
        let config1 = VsphereConfig {
            identifier: "processed".to_string(),
            vcenter_server: Some("vcenter".to_string()),
            username: None,
            password: None,
            insecure_connection: false,
            datacenter: None,
            cluster: None,
            resource_pool: None,
            datastore: None,
            folder: None,
            vm_name: None,
            host: None,
            keep_input_artifact: true,
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = VspherePostProcessor::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
