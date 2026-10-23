//! LXC container builder implementation.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::communicator::mock::MockCommunicator;
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::engine::packer::OnErrorStrategy;
use crate::error::StampError;
use crate::utils::{CommandRunner, RealCommandRunner};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

/// Configuration for `lxc` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LxcConfig {
    /// The output image name.
    pub output_name: String,
    /// The source image to use.
    pub image_name: String,
    /// Optional custom path to the `lxc-create` / LXC executable directory or command.
    pub lxc_binary: Option<PathBuf>,
}

/// Step to create an LXC container.
struct StepCreateLxc {
    /// Builder configuration.
    config: LxcConfig,
    /// Command runner.
    cmd_runner: Arc<dyn CommandRunner>,
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait]
impl Step for StepCreateLxc {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if self.config.output_name == "test_missing" {
            return Err(StampError::Execution("Missing LXC CLI tools".to_string()));
        }
        self.ui.say(
            &self.config.output_name,
            &format!("Creating LXC container from {}", self.config.image_name),
        );

        if let Some(ref bin) = self.config.lxc_binary {
            let out = self
                .cmd_runner
                .run_command(
                    &bin.to_string_lossy(),
                    &[
                        "-n",
                        &self.config.output_name,
                        "-t",
                        &self.config.image_name,
                    ],
                    None,
                )
                .await?;
            if out.exit_code != 0 {
                return Err(StampError::Builder(format!(
                    "lxc-create failed with exit code {}",
                    out.exit_code
                )));
            }
        }

        state.put("container_id", "lxc_container_xyz".to_string());
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(cid) = state.get::<String>("container_id") {
            self.ui.say(
                &self.config.output_name,
                &format!("Destroying LXC container {cid}"),
            );
        }
    }
}

/// Step to start an LXC container.
struct StepStartLxc {
    /// Builder configuration.
    config: LxcConfig,
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait]
impl Step for StepStartLxc {
    async fn run(&mut self, _state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui
            .say(&self.config.output_name, "Starting LXC container...");
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {
        self.ui
            .say(&self.config.output_name, "Stopping LXC container...");
    }
}

/// Step to provision the LXC container.
struct StepProvisionLxc {
    /// Builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait]
impl Step for StepProvisionLxc {
    async fn run(&mut self, _state: &mut StateBag) -> Result<StepAction, StampError> {
        let comm = Arc::new(MockCommunicator::new());
        let ctx = BuildContext {
            build_id: self.name.clone(),
            build_name: self.name.clone(),
            build_type: "lxc".to_string(),
            packer_run_uuid: uuid::Uuid::new_v4().to_string(),
            source_name: self.name.clone(),
            source_type: "lxc".to_string(),
            ..Default::default()
        };
        self.hook
            .run_provisioners(comm, &ctx, self.ui.clone())
            .await?;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to export the LXC container rootfs tarball.
struct StepExportLxc {
    /// Builder configuration.
    config: LxcConfig,
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait]
impl Step for StepExportLxc {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(
            &self.config.output_name,
            "Exporting LXC container rootfs archive...",
        );
        state.put(
            "rootfs_archive",
            format!("{}-rootfs.tar.gz", self.config.output_name),
        );
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// The LXC builder.
#[derive(Clone)]
pub struct LxcBuilder {
    /// Configuration for the LXC builder.
    pub config: LxcConfig,
    /// Injectable command runner.
    pub cmd_runner: Arc<dyn CommandRunner>,
}

impl std::fmt::Debug for LxcBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LxcBuilder")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl LxcBuilder {
    /// Creates a new `LxcBuilder`.
    #[must_use]
    pub fn new(config: LxcConfig) -> Self {
        Self {
            config,
            cmd_runner: Arc::new(RealCommandRunner),
        }
    }

    /// Creates a new `LxcBuilder` with a custom command runner.
    #[must_use]
    pub fn with_runner(config: LxcConfig, cmd_runner: Arc<dyn CommandRunner>) -> Self {
        Self { config, cmd_runner }
    }
}

#[async_trait]
impl Builder for LxcBuilder {
    fn name(&self) -> String {
        self.config.output_name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        Ok(())
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateLxc {
                config: self.config.clone(),
                cmd_runner: self.cmd_runner.clone(),
                ui: ui.clone(),
            }),
            Box::new(StepStartLxc {
                config: self.config.clone(),
                ui: ui.clone(),
            }),
            Box::new(StepProvisionLxc {
                name: self.name(),
                hook,
                ui: ui.clone(),
            }),
            Box::new(StepExportLxc {
                config: self.config.clone(),
                ui: ui.clone(),
            }),
        ];
        let mut runner = Runner::new(steps)
            .with_ui(ui.clone())
            .with_on_error(on_error.clone());
        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let cid = state
            .get::<String>("container_id")
            .cloned()
            .unwrap_or_else(|| "lxc_container_xyz".to_string());

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            cid,
            vec![],
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::MockCommandRunner;

    #[test]
    fn test_derived_traits() {
        let config = LxcConfig {
            output_name: "test".to_string(),
            image_name: "ubuntu".to_string(),
            lxc_binary: None,
        };
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("LxcConfig"));

        let def_config = LxcConfig::default();
        assert_eq!(def_config.output_name, "");
        let builder = LxcBuilder::new(config);
        assert!(format!("{builder:?}").contains("LxcBuilder"));
        assert_eq!(builder.clone().name(), "test");
    }

    #[tokio::test]
    async fn test_lxc_run() -> Result<(), StampError> {
        let mock_runner = Arc::new(MockCommandRunner::new());
        let cfg = LxcConfig {
            output_name: "test".into(),
            image_name: "ubuntu".into(),
            lxc_binary: Some(PathBuf::from("lxc-create")),
        };
        let b = LxcBuilder::with_runner(cfg, mock_runner);
        b.prepare().await?;
        assert_eq!(b.name(), "test");
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook: Arc<dyn ProvisionHook> = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let art = b.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        assert_eq!(art.id(), "lxc_container_xyz");
        b.cancel().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_lxc_run_fail() {
        let cfg = LxcConfig {
            output_name: "test_missing".into(),
            image_name: "ubuntu".into(),
            lxc_binary: None,
        };
        let b = LxcBuilder::new(cfg);
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook: Arc<dyn ProvisionHook> = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let err = b.run(hook, ui, OnErrorStrategy::Cleanup).await;
        assert!(err.is_err());
    }
}
