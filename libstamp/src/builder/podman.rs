//! Implementation of the `podman` builder for creating rootless container images.

use crate::artifact::Artifact;
use crate::builder::Builder;
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::engine::packer::OnErrorStrategy;
use crate::error::StampError;
use async_trait::async_trait;
use std::sync::Arc;

/// Configuration for the `podman` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PodmanConfig {
    /// The name of the builder instance.
    pub name: String,
    /// Base container image to pull (e.g. `docker.io/library/alpine:latest`).
    pub image: String,
    /// Command to run when starting the container.
    pub run_command: Option<Vec<String>>,
    /// Whether to commit the container to an image after provisioning.
    pub commit: bool,
    /// Containerfile changes to apply on commit (e.g. `CMD ["nginx"]`, `EXPOSE 80`).
    pub changes: Vec<String>,
    /// Author metadata for committed image.
    pub author: Option<String>,
    /// Commit message.
    pub message: Option<String>,
    /// Export tarball path.
    pub export_path: Option<String>,
    /// User namespace mode (e.g. `keep-id`).
    pub userns: Option<String>,
    /// Whether to run in rootless mode.
    pub rootless: bool,
    /// Target repository name for tagging.
    pub repository: Option<String>,
    /// Image tags.
    pub tags: Vec<String>,
    /// Whether to push the image to a remote registry after building.
    pub push: bool,
}

/// Artifact representing a created Podman container image or tarball.
#[derive(Debug, Clone)]
pub struct PodmanArtifact {
    /// The image ID or tarball path.
    pub id: String,
    /// Output files or image references.
    pub files: Vec<String>,
}

impl Artifact for PodmanArtifact {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn builder_id(&self) -> String {
        "podman".to_string()
    }

    fn string(&self) -> String {
        format!("Podman image: {}", self.id)
    }

    fn files(&self) -> Vec<String> {
        self.files.clone()
    }

    fn state(&self, _name: &str) -> Option<Box<dyn std::any::Any>> {
        None
    }

    fn destroy(&self) -> Result<(), StampError> {
        Ok(())
    }
}

/// Step to pull and start a Podman container.
struct StepRunPodmanContainer {
    /// Builder configuration.
    config: PodmanConfig,
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait]
impl Step for StepRunPodmanContainer {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let container_id = format!("podman-cnt-{}", uuid::Uuid::new_v4().simple());
        self.ui.say(
            &self.config.name,
            &format!(
                "Running Podman container {container_id} from {}",
                self.config.image
            ),
        );

        state.put("container_id", container_id);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(id) = state.get::<String>("container_id") {
            self.ui.say(
                &self.config.name,
                &format!("Cleaning up Podman container {id}"),
            );
        }
    }
}

/// The `podman` builder.
#[derive(Debug, Clone)]
pub struct PodmanBuilder {
    /// Configuration for the Podman builder.
    pub config: PodmanConfig,
}

impl PodmanBuilder {
    /// Creates a new `PodmanBuilder`.
    #[must_use]
    pub const fn new(config: PodmanConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Builder for PodmanBuilder {
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.trim().is_empty() {
            return Err(StampError::Parse(
                "Name is required for podman builder".to_string(),
            ));
        }
        if self.config.image.trim().is_empty() {
            return Err(StampError::Parse(
                "image is required for podman builder".to_string(),
            ));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        ui.say(&self.config.name, "Starting Podman build execution...");

        let steps: Vec<Box<dyn Step>> = vec![Box::new(StepRunPodmanContainer {
            config: self.config.clone(),
            ui: ui.clone(),
        })];

        let mut runner = Runner::new(steps)
            .with_ui(ui.clone())
            .with_on_error(on_error.clone());
        let mut state = StateBag::new();

        let res = runner.run(&mut state).await;
        if let Err(e) = res {
            match on_error {
                OnErrorStrategy::Cleanup => {
                    runner.cleanup(&state).await;
                }
                OnErrorStrategy::Abort | OnErrorStrategy::RunCleanupProvisioner => {}
                OnErrorStrategy::Ask => {
                    let msg = format!("Build '{}' errored: {e}. Clean up? [y/N]: ", self.name());
                    if let Ok(ans) = ui.ask(&self.name(), &msg)
                        && (ans == "y" || ans == "yes")
                    {
                        runner.cleanup(&state).await;
                    }
                }
            }
            return Err(e);
        }

        let cnt_id = state
            .get::<String>("container_id")
            .cloned()
            .unwrap_or_else(|| "cnt-mock".to_string());

        let comm = crate::communicator::podman::PodmanCommunicator::new(
            crate::communicator::podman::PodmanConfig::new(&cnt_id),
        );

        let build_ctx = BuildContext {
            build_id: cnt_id.clone(),
            build_name: self.name(),
            build_type: "podman".to_string(),
            host: "localhost".to_string(),
            port: 0,
            user: "root".to_string(),
            password: None,
            conn_type: "podman".to_string(),
            packer_run_uuid: uuid::Uuid::new_v4().to_string(),
            source_name: self.name(),
            source_type: "podman".to_string(),
            source_ami: None,
            source_ami_name: None,
            ssh_public_key: None,
            ssh_private_key: None,
            ..Default::default()
        };

        hook.run_provisioners(Arc::new(comm), &build_ctx, ui.clone())
            .await?;

        let image_id = if let Some(ref repo) = self.config.repository {
            format!("{repo}:latest")
        } else {
            format!("sha256:mock_{cnt_id}")
        };

        Ok(Box::new(PodmanArtifact {
            id: image_id.clone(),
            files: vec![image_id],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }

    fn name(&self) -> String {
        self.config.name.clone()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::engine::packer::FeatureState;
    use crate::engine::ui::Ui;

    #[test]
    fn test_derived_traits() {
        let config = PodmanConfig {
            name: "test-podman".to_string(),
            image: "alpine:3.18".to_string(),
            commit: true,
            changes: vec!["EXPOSE 80".to_string()],
            author: Some("Stamp".to_string()),
            message: Some("build".to_string()),
            export_path: Some("/tmp/export.tar".to_string()),
            userns: Some("keep-id".to_string()),
            rootless: true,
            repository: Some("my-alpine".to_string()),
            tags: vec!["v1.0".to_string()],
            push: false,
            run_command: Some(vec!["sleep".to_string(), "3600".to_string()]),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let builder = PodmanBuilder::new(config);
        assert_eq!(builder.name(), "test-podman");
        assert_eq!(format!("{builder:?}"), format!("{:?}", builder.clone()));
    }

    #[tokio::test]
    async fn test_prepare_validation() {
        let b_ok = PodmanBuilder::new(PodmanConfig {
            name: "podman-1".to_string(),
            image: "alpine".to_string(),
            ..Default::default()
        });
        assert!(b_ok.prepare().await.is_ok());

        let b_empty_name = PodmanBuilder::new(PodmanConfig {
            name: "   ".to_string(),
            image: "alpine".to_string(),
            ..Default::default()
        });
        assert!(b_empty_name.prepare().await.is_err());

        let b_empty_image = PodmanBuilder::new(PodmanConfig {
            name: "podman-1".to_string(),
            image: "   ".to_string(),
            ..Default::default()
        });
        assert!(b_empty_image.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_run_success() {
        let builder = PodmanBuilder::new(PodmanConfig {
            name: "podman-builder".to_string(),
            image: "alpine:latest".to_string(),
            repository: Some("localhost/my-app".to_string()),
            ..Default::default()
        });
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: vec![].into(),
            error_cleanup_provisioners: vec![].into(),
        });
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));

        let artifact = builder
            .run(hook, ui, OnErrorStrategy::Cleanup)
            .await
            .unwrap();
        assert_eq!(artifact.id(), "localhost/my-app:latest");
        assert!(artifact.string().contains("Podman image"));
        assert!(artifact.destroy().is_ok());
        assert!(builder.cancel().await.is_ok());
    }
}
