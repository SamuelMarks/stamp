//! Implementation of the `docker` builder.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `docker` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockerConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The base image to pull and use.
    pub image: String,
    /// Command to run when starting the container.
    pub run_command: Option<Vec<String>>,
    /// Whether to commit the container to an image after provisioning.
    pub commit: bool,
    /// Dockerfile instructions / changes to apply on commit or import.
    pub changes: Vec<String>,
    /// Author metadata for committed image.
    pub author: Option<String>,
    /// Commit or import message.
    pub message: Option<String>,
    /// Whether to export the container to a tarball.
    pub export_path: Option<String>,
    /// Whether to flatten the container image by re-importing the export tarball.
    pub flatten: bool,
    /// Target repository name for tagging and pushing.
    pub repository: Option<String>,
    /// Tags to assign to the image.
    pub tags: Vec<String>,
    /// Whether to push the tagged images to the remote repository.
    pub push: bool,
    /// Optional custom path to the `docker` executable.
    pub docker_binary: Option<std::path::PathBuf>,
}

impl DockerConfig {
    /// Resolves the Docker executable path from config or environment.
    #[must_use]
    pub fn docker_bin(&self) -> String {
        self.docker_binary
            .as_ref()
            .map_or_else(crate::utils::docker_executable, |p| {
                p.to_string_lossy().to_string()
            })
    }
}

/// The `docker` builder.
#[derive(Debug, Clone)]
pub struct DockerBuilder {
    /// The builder configuration.
    pub config: DockerConfig,
}

impl DockerBuilder {
    /// Create a new `DockerBuilder`.
    #[must_use]
    pub const fn new(config: DockerConfig) -> Self {
        Self { config }
    }
}

// Step: Run Container
/// Internal documentation missing.
struct StepRunContainer {
    /// Internal documentation missing.
    ui: Arc<crate::engine::ui::Ui>,
    /// Internal documentation missing.
    name: String,
    /// Internal documentation missing.
    config: DockerConfig,
}

#[async_trait::async_trait]
impl Step for StepRunContainer {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(
            &self.name,
            &format!("Pulling and running Docker image: {}", self.config.image),
        );

        // Pull image
        let pull_status = match tokio::process::Command::new(self.config.docker_bin())
            .arg("pull")
            .arg(&self.config.image)
            .status()
            .await
        {
            Ok(s) => s,
            Err(e) => {
                return Err(StampError::Execution(format!(
                    "Failed to execute docker pull: {e}"
                )));
            }
        };

        if !pull_status.success() {
            return Err(StampError::Execution("Docker pull failed".to_string()));
        }

        // Run container in background (detached)
        let mut cmd = tokio::process::Command::new(self.config.docker_bin());
        cmd.arg("run").arg("-d");

        // Expose SSH port to random host port to allow provisioning (simulated here)
        // Normally we'd use Docker communicator, but for parity we assume an SSH setup or custom port forwarding
        // Here we just map random host port to 22.
        cmd.arg("-P");

        cmd.arg(&self.config.image);
        if let Some(ref run_cmd) = self.config.run_command {
            for arg in run_cmd {
                cmd.arg(arg);
            }
        } else {
            // keep alive command
            cmd.arg("tail").arg("-f").arg("/dev/null");
        }

        let output = match cmd.output().await {
            Ok(o) => o,
            Err(e) => {
                return Err(StampError::Execution(format!(
                    "Failed to execute docker run: {e}"
                )));
            }
        };

        if !output.status.success() {
            return Err(StampError::Execution(format!(
                "Docker run failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        self.ui
            .say(&self.name, &format!("Container created: {container_id}"));
        state.put("container_id", container_id.clone());

        // Inspect to get IP
        let inspect_output = match tokio::process::Command::new(self.config.docker_bin())
            .arg("inspect")
            .arg("-f")
            .arg("{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}")
            .arg(&container_id)
            .output()
            .await
        {
            Ok(o) => o,
            Err(e) => {
                return Err(StampError::Execution(format!(
                    "Failed to execute docker inspect: {e}"
                )));
            }
        };

        let ip = String::from_utf8_lossy(&inspect_output.stdout)
            .trim()
            .to_string();
        state.put("container_ip", ip);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(container_id) = state.get::<String>("container_id") {
            self.ui.say(
                &self.name,
                &format!("Killing and removing container: {container_id}"),
            );

            let _ = tokio::process::Command::new(self.config.docker_bin())
                .arg("rm")
                .arg("-f")
                .arg(container_id)
                .status()
                .await;
        }
    }
}

// Step: Provision
/// Step to provision the running Docker container.
struct StepProvision {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Container...");

        let ip = state
            .get::<String>("container_ip")
            .cloned()
            .unwrap_or("127.0.0.1".to_string());

        let comm: Arc<dyn crate::communicator::Communicator> =
            if let Some(cid) = state.get::<String>("container_id") {
                Arc::new(crate::communicator::docker::DockerCommunicator::new(
                    crate::communicator::docker::DockerConfig::new(cid.clone()),
                ))
            } else {
                let ssh_config = SshConfig {
                    host: ip,
                    port: Port::new(22),
                    username: "root".to_string(),
                    private_key_path: None,
                    timeout: Timeout::new(Duration::from_secs(10)),
                    bastion_host: None,
                    bastion_port: None,
                    bastion_username: None,
                    bastion_private_key_file: None,
                    agent_forwarding: false,
                    pty: false,
                    connection_attempts: 1,
                    expect_disconnect: false,
                    ..Default::default()
                };
                Arc::new(SshCommunicator::new(ssh_config))
            };

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "docker".to_string(),
            user: "docker".to_string(),
            packer_run_uuid: uuid::Uuid::new_v4().to_string(),
            source_name: self.name.clone(),
            source_type: "docker".to_string(),
            ..Default::default()
        };

        if let Err(e) = self
            .hook
            .run_provisioners(comm.clone(), &build_ctx, self.ui.clone())
            .await
        {
            self.ui
                .error(&self.name, &format!("Provisioning failed: {e}"));
            if let Err(cleanup_err) = self
                .hook
                .run_error_cleanup_provisioners(comm, &build_ctx, self.ui.clone())
                .await
            {
                self.ui.error(
                    &self.name,
                    &format!("Error cleanup provisioning failed: {cleanup_err}"),
                );
            }
            return Err(e);
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

// Step: Commit Container
/// Step to commit the Docker container into an image.
struct StepCommitContainer {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Docker builder configuration.
    config: DockerConfig,
}

#[async_trait::async_trait]
impl Step for StepCommitContainer {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if !self.config.commit {
            return Ok(StepAction::Continue);
        }

        let container_id = state
            .get::<String>("container_id")
            .cloned()
            .unwrap_or_default();
        self.ui
            .say(&self.name, &format!("Committing container: {container_id}"));

        let mut cmd = tokio::process::Command::new(self.config.docker_bin());
        cmd.arg("commit");
        for change in &self.config.changes {
            cmd.arg("--change").arg(change);
        }
        if let Some(ref author) = self.config.author {
            cmd.arg("--author").arg(author);
        }
        if let Some(ref msg) = self.config.message {
            cmd.arg("--message").arg(msg);
        }
        cmd.arg(&container_id);

        let output = cmd
            .output()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to execute docker commit: {e}")))?;

        if !output.status.success() {
            return Err(StampError::Execution(format!(
                "Docker commit failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let image_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        state.put("image_id", image_id.clone());
        self.ui
            .say(&self.name, &format!("Committed image: {image_id}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

// Step: Export Container
/// Step to export the Docker container filesystem to a tar archive.
struct StepExportContainer {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Docker builder configuration.
    config: DockerConfig,
}

#[async_trait::async_trait]
impl Step for StepExportContainer {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if let Some(ref path) = self.config.export_path {
            let container_id = state
                .get::<String>("container_id")
                .cloned()
                .unwrap_or_default();
            self.ui.say(
                &self.name,
                &format!("Exporting container {container_id} to {path}"),
            );

            let status = tokio::process::Command::new(self.config.docker_bin())
                .arg("export")
                .arg("-o")
                .arg(path)
                .arg(&container_id)
                .status()
                .await
                .map_err(|e| {
                    StampError::Execution(format!("Failed to execute docker export: {e}"))
                })?;

            if !status.success() {
                return Err(StampError::Execution("Docker export failed".to_string()));
            }
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to import and flatten a container filesystem export into a single-layer image.
#[derive(Debug, Clone)]
struct StepFlattenContainer {
    /// UI reference for logging.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Docker builder configuration.
    config: DockerConfig,
}

#[async_trait::async_trait]
impl Step for StepFlattenContainer {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if !self.config.flatten {
            return Ok(StepAction::Continue);
        }

        let Some(ref export_path) = self.config.export_path else {
            return Ok(StepAction::Continue);
        };

        self.ui.say(
            &self.name,
            &format!("Flattening exported container from {export_path}..."),
        );

        let mut cmd = tokio::process::Command::new(self.config.docker_bin());
        cmd.arg("import");
        for change in &self.config.changes {
            cmd.arg("--change").arg(change);
        }
        if let Some(ref msg) = self.config.message {
            cmd.arg("--message").arg(msg);
        }
        cmd.arg(export_path);

        let output = cmd
            .output()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to execute docker import: {e}")))?;

        if !output.status.success() {
            return Err(StampError::Execution(format!(
                "Docker import (flatten) failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let image_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        self.ui
            .say(&self.name, &format!("Flattened image ID: {image_id}"));
        state.put("image_id", image_id);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to tag and push the Docker image to a registry.
#[derive(Debug, Clone)]
struct StepPushContainer {
    /// UI reference for logging.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Docker builder configuration.
    config: DockerConfig,
}

#[async_trait::async_trait]
impl Step for StepPushContainer {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let Some(ref repo) = self.config.repository else {
            return Ok(StepAction::Continue);
        };

        let image_id = state.get::<String>("image_id").cloned().unwrap_or_default();

        let tags = if self.config.tags.is_empty() {
            vec!["latest".to_string()]
        } else {
            self.config.tags.clone()
        };

        for tag in &tags {
            let full_tag = format!("{repo}:{tag}");
            self.ui.say(
                &self.name,
                &format!("Tagging image {image_id} as {full_tag}..."),
            );

            let status = tokio::process::Command::new(self.config.docker_bin())
                .arg("tag")
                .arg(&image_id)
                .arg(&full_tag)
                .status()
                .await
                .map_err(|e| StampError::Execution(format!("Failed to execute docker tag: {e}")))?;

            if !status.success() {
                return Err(StampError::Execution(format!(
                    "Docker tag failed for {full_tag}"
                )));
            }

            if self.config.push {
                self.ui.say(
                    &self.name,
                    &format!("Pushing image {full_tag} to registry..."),
                );
                let push_status = tokio::process::Command::new(self.config.docker_bin())
                    .arg("push")
                    .arg(&full_tag)
                    .status()
                    .await
                    .map_err(|e| {
                        StampError::Execution(format!("Failed to execute docker push: {e}"))
                    })?;

                if !push_status.success() {
                    return Err(StampError::Execution(format!(
                        "Docker push failed for {full_tag}"
                    )));
                }
            }
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for DockerBuilder {
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        if self.config.image.is_empty() {
            return Err(StampError::Parse("Image cannot be empty".to_string()));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepRunContainer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                hook,
            }),
            Box::new(StepCommitContainer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepExportContainer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepFlattenContainer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepPushContainer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ];

        let mut runner = Runner::new(steps)
            .with_ui(ui.clone())
            .with_on_error(on_error.clone());
        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let mut artifact_id = String::new();
        if self.config.commit || self.config.flatten {
            artifact_id = format!(
                "image:{}",
                state.get::<String>("image_id").cloned().unwrap_or_default()
            );
        }
        if let Some(ref path) = self.config.export_path {
            if artifact_id.is_empty() {
                artifact_id = format!("export:{path}");
            } else {
                artifact_id = format!("{artifact_id},export:{path}");
            }
        }
        if let Some(ref repo) = self.config.repository {
            let tag = self.config.tags.first().map_or("latest", String::as_str);
            if artifact_id.is_empty() {
                artifact_id = format!("repo:{repo}:{tag}");
            } else {
                artifact_id = format!("{artifact_id},repo:{repo}:{tag}");
            }
        }

        Ok(Box::new(crate::artifact::BuildArtifact::with_builder(
            self.name(),
            artifact_id,
            vec![],
        )))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }

    fn name(&self) -> String {
        self.config.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = DockerConfig {
            name: "test".to_string(),
            image: "ubuntu".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("DockerConfig"));

        let builder = DockerBuilder::new(config);
        assert!(format!("{builder:?}").contains("DockerBuilder"));
        assert_eq!(builder.clone().name(), "test");
    }

    #[tokio::test]
    async fn test_docker_prepare_success() -> Result<(), crate::error::StampError> {
        let config = DockerConfig {
            name: "test".to_string(),
            image: "ubuntu".to_string(),
            ..Default::default()
        };
        let builder = DockerBuilder::new(config);
        builder.prepare().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_prepare_failure_name() -> Result<(), crate::error::StampError> {
        let config = DockerConfig {
            name: String::new(),
            image: "ubuntu".to_string(),
            ..Default::default()
        };
        let builder = DockerBuilder::new(config);
        let err = builder.prepare().await;
        assert!(matches!(err, Err(crate::error::StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_prepare_failure_image() -> Result<(), crate::error::StampError> {
        let config = DockerConfig {
            name: "test".to_string(),
            image: String::new(),
            ..Default::default()
        };
        let builder = DockerBuilder::new(config);
        let err = builder.prepare().await;
        assert!(matches!(err, Err(crate::error::StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_run_mocked() -> Result<(), crate::error::StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\nif [ \"$1\" = \"commit\" ]; then\n    echo \"mocked-image-id\"\nelif [ \"$1\" = \"run\" ]; then\n    echo \"mocked-container-id\"\nfi\nexit 0\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }

        let config = DockerConfig {
            name: "test".to_string(),
            image: "ubuntu".to_string(),
            commit: true,
            run_command: Some(vec!["/bin/bash".to_string()]),
            export_path: Some("out.tar".to_string()),
            flatten: false,
            repository: Some("myorg/myimage".to_string()),
            tags: vec!["v1".to_string()],
            push: true,
            changes: vec!["EXPOSE 80".to_string()],
            author: Some("Stamp".to_string()),
            message: Some("Committed by stamp".to_string()),
            docker_binary: Some(path),
        };
        let builder = DockerBuilder::new(config);
        let artifact = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await?;

        assert!(artifact.id().contains("image:mocked-image-id"));
        assert!(artifact.id().contains("export:out.tar"));
        assert!(artifact.id().contains("repo:myorg/myimage:v1"));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_flatten_and_push() -> Result<(), crate::error::StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_flatten");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\nif [ \"$1\" = \"run\" ]; then\n    echo \"mocked-container-id\"\nelif [ \"$1\" = \"export\" ]; then\n    exit 0\nelif [ \"$1\" = \"import\" ]; then\n    echo \"mock-flattened-image\"\nfi\nexit 0\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }

        let config = DockerConfig {
            name: "test-flatten".to_string(),
            image: "alpine".to_string(),
            commit: false,
            flatten: true,
            export_path: Some("rootfs.tar".to_string()),
            repository: Some("registry.example.com/app".to_string()),
            tags: vec!["latest".to_string(), "1.0".to_string()],
            push: true,
            changes: vec!["ENTRYPOINT [\"/app\"]".to_string()],
            docker_binary: Some(path),
            ..Default::default()
        };
        let builder = DockerBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;

        assert!(artifact.id().contains("image:mock-flattened-image"));
        assert!(artifact.id().contains("export:rootfs.tar"));
        assert!(
            artifact
                .id()
                .contains("repo:registry.example.com/app:latest")
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_run_bad_exit() -> Result<(), crate::error::StampError> {
        let config = DockerConfig {
            name: "test_bad_exit".to_string(),
            image: "ubuntu".to_string(),
            docker_binary: Some(std::path::PathBuf::from("false")),
            ..Default::default()
        };
        let builder = DockerBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_run_missing() -> Result<(), crate::error::StampError> {
        let config = DockerConfig {
            name: "test_missing".to_string(),
            image: "ubuntu".to_string(),
            docker_binary: Some(std::path::PathBuf::from("/nonexistent/docker_bin")),
            ..Default::default()
        };
        let builder = DockerBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_bad_executable() {
        let config = DockerConfig {
            name: "test".to_string(),
            image: "ubuntu".to_string(),
            docker_binary: Some(std::path::PathBuf::from("non_existent_docker_executable")),
            ..Default::default()
        };
        let builder = DockerBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;

        assert!(res.is_err_and(|e| e.to_string().contains("Failed to execute docker pull:")));
    }

    #[tokio::test]
    async fn test_docker_step_run_map_err_run() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_run_err");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script =
                "#!/bin/sh\nif [ \"$1\" = \"pull\" ]; then\n    rm \"$0\"\n    exit 0\nfi\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut step = StepRunContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test".into(),
            config: DockerConfig {
                image: "ubuntu".to_string(),
                docker_binary: Some(path),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        let res = step.run(&mut state).await;

        assert!(res.is_err_and(|e| e.to_string().contains("Failed to execute docker run")));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_step_run_map_err_inspect() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_insp_err");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\nif [ \"$1\" = \"pull\" ]; then\n    exit 0\nelif [ \"$1\" = \"run\" ]; then\n    rm \"$0\"\n    exit 0\nfi\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut step = StepRunContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test".into(),
            config: DockerConfig {
                image: "ubuntu".to_string(),
                docker_binary: Some(path),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        let res = step.run(&mut state).await;

        assert!(res.is_err_and(|e| e.to_string().contains("Failed to execute docker inspect")));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_step_commit_map_err() {
        let mut step = StepCommitContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test".into(),
            config: DockerConfig {
                commit: true,
                docker_binary: Some(std::path::PathBuf::from(
                    "non_existent_docker_executable_commit",
                )),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("container_id", "123".to_string());
        let res = step.run(&mut state).await;
        assert!(res.is_err_and(|e| e.to_string().contains("Failed to execute docker commit")));
    }

    #[tokio::test]
    async fn test_docker_step_export_map_err() {
        let mut step = StepExportContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test".into(),
            config: DockerConfig {
                export_path: Some("out.tar".into()),
                docker_binary: Some(std::path::PathBuf::from(
                    "non_existent_docker_executable_export",
                )),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("container_id", "123".to_string());
        let res = step.run(&mut state).await;
        assert!(res.is_err_and(|e| e.to_string().contains("Failed to execute docker export")));
    }

    #[tokio::test]
    async fn test_docker_cancel() -> Result<(), crate::error::StampError> {
        let config = DockerConfig {
            name: "test".to_string(),
            image: "ubuntu".to_string(),
            ..Default::default()
        };
        let builder = DockerBuilder::new(config);
        builder.cancel().await?;
        Ok(())
    }

    #[test]
    fn test_docker_name() {
        let config = DockerConfig {
            name: "test-name".to_string(),
            image: "ubuntu".to_string(),
            ..Default::default()
        };
        let builder = DockerBuilder::new(config);
        assert_eq!(builder.name(), "test-name");
    }

    #[tokio::test]
    async fn test_docker_step_run_pull_failure() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_pull_fail");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\nexit 1\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut step = StepRunContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test_pull_failure".into(),
            config: DockerConfig {
                name: "test_pull_failure".to_string(),
                image: "ubuntu".to_string(),
                docker_binary: Some(path),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        let res = step.run(&mut state).await;
        assert!(res.is_err_and(|e| e.to_string().contains("Docker pull failed")));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_step_run_run_failure() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_run_fail2");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\nif [ \"$1\" = \"pull\" ]; then\n    exit 0\nfi\nexit 1\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut step = StepRunContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test_run_failure".into(),
            config: DockerConfig {
                name: "test_run_failure".to_string(),
                image: "ubuntu".to_string(),
                docker_binary: Some(path),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        let res = step.run(&mut state).await;
        assert!(res.is_err_and(|e| e.to_string().contains("Docker run failed")));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_step_commit_failure() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_commit_fail");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\nexit 1\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut step = StepCommitContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test_commit_failure".into(),
            config: DockerConfig {
                name: "test_commit_failure".to_string(),
                commit: true,
                docker_binary: Some(path),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("container_id", "123".to_string());
        let res = step.run(&mut state).await;
        assert!(res.is_err_and(|e| e.to_string().contains("Docker commit failed")));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_step_export_failure() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_export_fail");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\nexit 1\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut step = StepExportContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test_export_failure".into(),
            config: DockerConfig {
                name: "test_export_failure".to_string(),
                export_path: Some("out.tar".into()),
                docker_binary: Some(path),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("container_id", "123".to_string());
        let res = step.run(&mut state).await;
        assert!(matches!(
            res,
            Err(StampError::Execution(ref msg)) if msg.contains("Docker export failed")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_step_flatten_failure() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_flatten_fail");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\nexit 1\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut step = StepFlattenContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test_flatten_failure".into(),
            config: DockerConfig {
                name: "test_flatten_failure".to_string(),
                flatten: true,
                export_path: Some("out.tar".into()),
                docker_binary: Some(path),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        let res = step.run(&mut state).await;
        assert!(matches!(
            res,
            Err(StampError::Execution(ref msg)) if msg.contains("Docker import (flatten) failed")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_step_flatten_map_err() {
        let mut step = StepFlattenContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test_flatten_map_err".into(),
            config: DockerConfig {
                name: "test_flatten_map_err".to_string(),
                flatten: true,
                export_path: Some("out.tar".into()),
                docker_binary: Some(std::path::PathBuf::from(
                    "non_existent_docker_executable_flatten",
                )),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        let res = step.run(&mut state).await;
        assert!(matches!(
            res,
            Err(StampError::Execution(ref msg)) if msg.contains("Failed to execute docker import")
        ));
    }

    #[tokio::test]
    async fn test_docker_step_tag_failure() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_tag_fail");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\nexit 1\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut step = StepPushContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test_tag_failure".into(),
            config: DockerConfig {
                name: "test_tag_failure".to_string(),
                repository: Some("myrepo".to_string()),
                tags: vec!["v1".to_string()],
                docker_binary: Some(path),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("image_id", "img123".to_string());
        let res = step.run(&mut state).await;
        assert!(matches!(
            res,
            Err(StampError::Execution(ref msg)) if msg.contains("Docker tag failed")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_step_tag_map_err() {
        let mut step = StepPushContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test_tag_map_err".into(),
            config: DockerConfig {
                name: "test_tag_map_err".to_string(),
                repository: Some("myrepo".to_string()),
                tags: vec!["v1".to_string()],
                docker_binary: Some(std::path::PathBuf::from(
                    "non_existent_docker_executable_tag",
                )),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("image_id", "img123".to_string());
        let res = step.run(&mut state).await;
        assert!(matches!(
            res,
            Err(StampError::Execution(ref msg)) if msg.contains("Failed to execute docker tag")
        ));
    }

    #[tokio::test]
    async fn test_docker_step_push_failure() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_push_fail");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\nif [ \"$1\" = \"push\" ]; then\n  exit 1\nfi\nexit 0\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut step = StepPushContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test_push_failure".into(),
            config: DockerConfig {
                name: "test_push_failure".to_string(),
                repository: Some("myrepo".to_string()),
                tags: vec!["v1".to_string()],
                push: true,
                docker_binary: Some(path),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("image_id", "img123".to_string());
        let res = step.run(&mut state).await;
        assert!(matches!(
            res,
            Err(StampError::Execution(ref msg)) if msg.contains("Docker push failed")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn test_docker_step_push_map_err() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mock_docker_push_map_err");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script =
                "#!/bin/sh\nif [ \"$1\" = \"tag\" ]; then\n  rm -f \"$0\"\n  exit 0\nfi\nexit 0\n";
            std::fs::write(&path, script)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut step = StepPushContainer {
            ui: std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test_push_map_err".into(),
            config: DockerConfig {
                name: "test_push_map_err".to_string(),
                repository: Some("myrepo".to_string()),
                tags: vec!["v1".to_string()],
                push: true,
                docker_binary: Some(path),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("image_id", "img123".to_string());
        let res = step.run(&mut state).await;
        assert!(matches!(
            res,
            Err(StampError::Execution(ref msg)) if msg.contains("Failed to execute docker push")
        ));
        Ok(())
    }
}
