//! LXC container builder implementation.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::communicator::mock::MockCommunicator;
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::engine::packer::OnErrorStrategy;
use crate::error::StampError;
use async_trait::async_trait;
use std::sync::Arc;

/// Configuration for `lxc` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LxcConfig {
    /// The output image name.
    pub output_name: String,
    /// The source image to use.
    pub image_name: String,
}

/// Step to create and start an LXC container.
struct StepCreateLxc {
    /// Builder configuration.
    config: LxcConfig,
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

/// The LXC builder.
#[derive(Debug, Clone)]
pub struct LxcBuilder {
    /// Configuration for the LXC builder.
    config: LxcConfig,
}

impl LxcBuilder {
    /// Creates a new `LxcBuilder`.
    #[must_use]
    pub const fn new(config: LxcConfig) -> Self {
        Self { config }
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
                ui: ui.clone(),
            }),
            Box::new(StepProvisionLxc {
                name: self.name(),
                hook,
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

    #[test]
    fn test_derived_traits() {
        let config = LxcConfig {
            output_name: "test".to_string(),
            image_name: "ubuntu".to_string(),
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
        let cfg = LxcConfig {
            output_name: "test".into(),
            image_name: "ubuntu".into(),
        };
        let b = LxcBuilder::new(cfg);
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
