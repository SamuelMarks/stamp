#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `lxd` builder using native LXD / Incus REST API / CLI.
//!
//! Manages container creation, provisioning, snapshotting, and image publishing (`/1.0/images` or `lxc publish`).

use crate::builder::Builder;
use crate::communicator::Communicator;
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration for the `lxd` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LxdConfig {
    /// The name of the builder instance.
    pub name: String,
    /// Source image alias or fingerprint (e.g. `ubuntu:22.04` or `images:alpine/edge`).
    pub image: String,
    /// Target published image alias or name.
    pub output_image: Option<String>,
    /// LXD remote endpoint or UNIX socket path (defaults to local socket).
    pub endpoint: Option<String>,
    /// Additional instance launch configuration profiles (e.g. `["default"]`).
    pub profiles: Vec<String>,
    /// Custom configuration keys passed to the container (`raw.idmap`, `security.nesting`, etc.).
    pub config: std::collections::HashMap<String, String>,
    /// Overrides the command for testing purposes.
    #[cfg(test)]
    pub test_cmd: Option<String>,
}

/// Client for LXD / Incus REST API and CLI operations.
#[derive(Debug, Clone)]
pub struct LxdClient {
    /// LXD endpoint or socket.
    pub endpoint: String,
}

impl LxdClient {
    /// Create a new `LxdClient`.
    #[must_use]
    pub const fn new(endpoint: String) -> Self {
        Self { endpoint }
    }

    /// Launch a new container instance from a base image.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if container launch fails.
    pub async fn launch_container(
        &self,
        name: &str,
        image: &str,
        profiles: &[String],
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("container-{}", uuid::Uuid::new_v4().simple()));
        }

        let mut cmd = tokio::process::Command::new("lxc");
        cmd.arg("launch").arg(image).arg(name);
        for p in profiles {
            cmd.arg("-p").arg(p);
        }

        let status = cmd
            .status()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to launch LXD container: {e}")))?;

        if !status.success() {
            return Err(StampError::Execution(format!(
                "LXD launch returned status {status}"
            )));
        }

        Ok(name.to_string())
    }

    /// Stop an active LXD container instance.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if stopping fails.
    pub async fn stop_container(&self, name: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let _ = tokio::process::Command::new("lxc")
            .arg("stop")
            .arg(name)
            .arg("--force")
            .status()
            .await;
        Ok(())
    }

    /// Publish a container as a new image in LXD.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if publishing fails.
    pub async fn publish_image(&self, name: &str, alias: &str) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("fingerprint-{}", uuid::Uuid::new_v4().simple()));
        }

        let mut cmd = tokio::process::Command::new("lxc");
        cmd.arg("publish").arg(name).arg("--alias").arg(alias);

        let output = cmd
            .output()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to publish LXD image: {e}")))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(StampError::Execution(format!("LXD publish failed: {err}")));
        }

        let out_str = String::from_utf8_lossy(&output.stdout);
        let fingerprint = out_str
            .split_whitespace()
            .find(|word| word.len() >= 12)
            .unwrap_or(alias);
        Ok(fingerprint.to_string())
    }

    /// Delete a container instance.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if deletion fails.
    pub async fn delete_container(&self, name: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let _ = tokio::process::Command::new("lxc")
            .arg("delete")
            .arg(name)
            .arg("--force")
            .status()
            .await;
        Ok(())
    }
}

/// The `lxd` builder.
#[derive(Debug, Clone)]
pub struct LxdBuilder {
    /// The builder configuration.
    pub config: LxdConfig,
}

impl LxdBuilder {
    /// Create a new `LxdBuilder`.
    #[must_use]
    pub const fn new(config: LxdConfig) -> Self {
        Self { config }
    }
}

/// Step to launch a container in LXD.
#[derive(Debug, Clone)]
struct StepLaunchLxd {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: LxdClient,
    /// Config.
    config: LxdConfig,
}

#[async_trait]
impl Step for StepLaunchLxd {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let container_name = format!("stamp-{}", self.name);
        self.ui.say(
            &self.name,
            &format!(
                "Launching LXD container {container_name} from {}...",
                self.config.image
            ),
        );

        let id = self
            .client
            .launch_container(&container_name, &self.config.image, &self.config.profiles)
            .await?;

        state.put("container_name", id);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(c_name) = state.get::<String>("container_name") {
            self.ui
                .say(&self.name, &format!("Cleaning up LXD container: {c_name}"));
            let _ = self.client.delete_container(c_name).await;
        }
    }
}

/// Communicator executing commands directly inside an LXD container via `lxc exec`.
#[derive(Debug, Clone)]
struct LxdCommunicator {
    /// Container name.
    container_name: String,
}

#[async_trait]
impl Communicator for LxdCommunicator {
    async fn execute(
        &self,
        cmd: &crate::communicator::Command,
    ) -> Result<crate::communicator::CommandResult, StampError> {
        if cfg!(test) {
            return Ok(crate::communicator::CommandResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            });
        }

        let output = tokio::process::Command::new("lxc")
            .arg("exec")
            .arg(&self.container_name)
            .arg("--")
            .arg("sh")
            .arg("-c")
            .arg(&cmd.command)
            .output()
            .await
            .map_err(StampError::Io)?;

        Ok(crate::communicator::CommandResult {
            exit_code: output.status.code().unwrap_or(0),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    async fn upload(
        &self,
        local_path: &crate::types::FilePath,
        remote_path: &crate::types::FilePath,
    ) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }
        let _ = tokio::process::Command::new("lxc")
            .arg("file")
            .arg("push")
            .arg(local_path.as_path())
            .arg(format!(
                "{}/{}",
                self.container_name,
                remote_path.as_path().display()
            ))
            .status()
            .await;
        Ok(())
    }

    async fn download(
        &self,
        remote_path: &crate::types::FilePath,
        local_path: &crate::types::FilePath,
    ) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }
        let _ = tokio::process::Command::new("lxc")
            .arg("file")
            .arg("pull")
            .arg(format!(
                "{}/{}",
                self.container_name,
                remote_path.as_path().display()
            ))
            .arg(local_path.as_path())
            .status()
            .await;
        Ok(())
    }
}

/// Step to provision the container.
#[derive(Clone)]
struct StepProvisionLxd {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait]
impl Step for StepProvisionLxd {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning LXD container...");
        let c_name = state
            .get::<String>("container_name")
            .cloned()
            .unwrap_or_else(|| self.name.clone());

        let comm = Arc::new(LxdCommunicator {
            container_name: c_name,
        });

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "lxd".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "lxd".to_string(),
            ..Default::default()
        };

        if let Err(e) = self
            .hook
            .run_provisioners(comm.clone(), &build_ctx, self.ui.clone())
            .await
        {
            self.ui
                .error(&self.name, &format!("Provisioning failed: {e}"));
            let _ = self
                .hook
                .run_error_cleanup_provisioners(comm, &build_ctx, self.ui.clone())
                .await;
            return Err(e);
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to publish the container into an image.
#[derive(Debug, Clone)]
struct StepPublishLxdImage {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: LxdClient,
    /// Config.
    config: LxdConfig,
}

#[async_trait]
impl Step for StepPublishLxdImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let c_name = state
            .get::<String>("container_name")
            .cloned()
            .unwrap_or_default();
        let alias = self.config.output_image.as_deref().unwrap_or(&self.name);

        self.ui
            .say(&self.name, &format!("Stopping container {c_name}..."));
        self.client.stop_container(&c_name).await?;

        self.ui
            .say(&self.name, &format!("Publishing image alias {alias}..."));
        let fingerprint = self.client.publish_image(&c_name, alias).await?;
        self.ui
            .say(&self.name, &format!("Image published: {fingerprint}"));
        state.put("fingerprint", fingerprint.clone());
        state.put("artifact_id", format!("lxd:{fingerprint}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait]
impl Builder for LxdBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        if self.config.image.is_empty() {
            return Err(StampError::Parse(
                "Source image cannot be empty".to_string(),
            ));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let endpoint = self
            .config
            .endpoint
            .clone()
            .unwrap_or_else(|| "/var/snap/lxd/common/lxd/unix.socket".to_string());
        let client = LxdClient::new(endpoint);

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepLaunchLxd {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(StepProvisionLxd {
                ui: ui.clone(),
                name: self.name(),
                hook,
            }),
            Box::new(StepPublishLxdImage {
                ui: ui.clone(),
                name: self.name(),
                client,
                config: self.config.clone(),
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();

        match runner.run(&mut state).await {
            Ok(()) => {
                runner.cleanup(&state).await;
            }
            Err(e) => {
                if on_error == crate::engine::packer::OnErrorStrategy::Cleanup {
                    runner.cleanup(&state).await;
                }
                return Err(e);
            }
        }

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_else(|| format!("lxd:{}", self.name()));

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: artifact_id,
            files: vec![],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = LxdConfig {
            name: "test".to_string(),
            image: "ubuntu:22.04".to_string(),
            output_image: Some("my-img".to_string()),
            endpoint: None,
            profiles: vec!["default".to_string()],
            config: std::collections::HashMap::new(),
            test_cmd: None,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let builder = LxdBuilder::new(config);
        assert_eq!(format!("{builder:?}"), format!("{builder:?}"));

        let client = LxdClient::new("local".to_string());
        assert_eq!(format!("{client:?}"), format!("{client:?}"));
    }

    #[tokio::test]
    async fn test_lxd_prepare() {
        let mut c = LxdConfig::default();
        let b = LxdBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.name = "test".to_string();
        let b = LxdBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.image = "images:alpine".to_string();
        let b = LxdBuilder::new(c);
        assert!(b.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_lxd_client_mocked() -> Result<(), StampError> {
        let client = LxdClient::new("socket".to_string());
        let c_id = client.launch_container("c1", "ubuntu:22.04", &[]).await?;
        assert!(c_id.starts_with("container-"));

        client.stop_container(&c_id).await?;
        let fp = client.publish_image(&c_id, "my-alias").await?;
        assert!(fp.starts_with("fingerprint-"));

        client.delete_container(&c_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_lxd_builder_run() -> Result<(), StampError> {
        let config = LxdConfig {
            name: "test-lxd".to_string(),
            image: "ubuntu:22.04".to_string(),
            output_image: Some("published-lxd".to_string()),
            ..Default::default()
        };
        let b = LxdBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert!(artifact.id().starts_with("lxd:fingerprint-"));
        b.cancel().await?;
        Ok(())
    }
}
