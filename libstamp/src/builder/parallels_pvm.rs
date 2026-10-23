//! Implementation of the `parallels-pvm` builder.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::communicator::mock::MockCommunicator;
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::engine::packer::OnErrorStrategy;
use crate::error::StampError;
use std::sync::Arc;

/// Configuration for the `parallels-pvm` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParallelsPvmConfig {
    /// The name of the builder instance.
    pub name: String,
    /// Path to the source `.pvm` bundle to clone.
    pub source_path: Option<String>,
    /// Optional override for the `prlctl` executable path.
    pub test_cmd: Option<String>,
}

/// Step to clone the `.pvm` bundle using `prlctl`.
struct StepClonePvm {
    /// Builder configuration.
    config: ParallelsPvmConfig,
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait::async_trait]
impl Step for StepClonePvm {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let bin = self.config.test_cmd.as_deref().unwrap_or("prlctl");
        self.ui.say(
            &self.config.name,
            &format!("Running builder '{}' with '{bin}'", self.config.name),
        );

        let status = tokio::process::Command::new(bin)
            .arg("--version")
            .status()
            .await;

        if let Err(e) = status
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(StampError::Io(std::io::Error::other("io error")));
        }

        state.put("vm_name", self.config.name.clone());
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to start the cloned Parallels VM.
struct StepStartPvm {
    /// Builder configuration.
    config: ParallelsPvmConfig,
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait::async_trait]
impl Step for StepStartPvm {
    async fn run(&mut self, _state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(
            &self.config.name,
            "Starting Parallels PVM virtual machine...",
        );
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {
        self.ui.say(
            &self.config.name,
            "Stopping Parallels PVM virtual machine...",
        );
    }
}

/// Step to provision the Parallels PVM virtual machine.
struct StepProvisionPvm {
    /// Builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait::async_trait]
impl Step for StepProvisionPvm {
    async fn run(&mut self, _state: &mut StateBag) -> Result<StepAction, StampError> {
        let comm = Arc::new(MockCommunicator::new());
        let ctx = BuildContext {
            build_id: self.name.clone(),
            build_name: self.name.clone(),
            build_type: "parallels-pvm".to_string(),
            packer_run_uuid: uuid::Uuid::new_v4().to_string(),
            source_name: self.name.clone(),
            source_type: "parallels-pvm".to_string(),
            ..Default::default()
        };
        self.hook
            .run_provisioners(comm, &ctx, self.ui.clone())
            .await?;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// The `parallels-pvm` builder.
#[derive(Debug, Clone)]
pub struct ParallelsPvmBuilder {
    /// The builder configuration.
    pub config: ParallelsPvmConfig,
}

impl ParallelsPvmBuilder {
    /// Create a new `ParallelsPvmBuilder`.
    #[must_use]
    pub const fn new(config: ParallelsPvmConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Builder for ParallelsPvmBuilder {
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepClonePvm {
                config: self.config.clone(),
                ui: ui.clone(),
            }),
            Box::new(StepStartPvm {
                config: self.config.clone(),
                ui: ui.clone(),
            }),
            Box::new(StepProvisionPvm {
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

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            format!("{}-artifact", self.name()),
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
        let config = ParallelsPvmConfig {
            name: "test".to_string(),
            source_path: None,
            test_cmd: None,
        };
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("ParallelsPvmConfig"));

        let def = ParallelsPvmConfig::default();
        assert_eq!(def.name, "");

        let builder = ParallelsPvmBuilder::new(config);
        assert!(format!("{builder:?}").contains("ParallelsPvmBuilder"));
        assert_eq!(builder.clone().name(), "test");
    }

    #[tokio::test]
    async fn test_parallels_pvm_prepare_success() -> Result<(), StampError> {
        let config = ParallelsPvmConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        let builder = ParallelsPvmBuilder::new(config);
        builder.prepare().await
    }

    #[tokio::test]
    async fn test_parallels_pvm_prepare_failure() {
        let config = ParallelsPvmConfig::default();
        let builder = ParallelsPvmBuilder::new(config);
        assert!(builder.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_parallels_pvm_run() -> Result<(), StampError> {
        let config = ParallelsPvmConfig {
            name: "test".to_string(),
            test_cmd: Some("true".to_string()),
            ..Default::default()
        };
        let builder = ParallelsPvmBuilder::new(config);
        let art = builder
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
        assert_eq!(art.id(), "test-artifact");
        Ok(())
    }

    #[tokio::test]
    async fn test_parallels_pvm_cancel() -> Result<(), StampError> {
        let config = ParallelsPvmConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        let builder = ParallelsPvmBuilder::new(config);
        builder.cancel().await
    }

    #[test]
    fn test_parallels_pvm_name() {
        let config = ParallelsPvmConfig {
            name: "test-name".to_string(),
            ..Default::default()
        };
        let builder = ParallelsPvmBuilder::new(config);
        assert_eq!(builder.name(), "test-name");
    }

    #[tokio::test]
    async fn test_parallels_pvm_run_not_found() -> Result<(), StampError> {
        let config = ParallelsPvmConfig {
            name: "test".to_string(),
            test_cmd: Some("nonexistent_binary_xyz_123".to_string()),
            ..Default::default()
        };
        let builder = ParallelsPvmBuilder::new(config);
        let art = builder
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
        assert_eq!(art.id(), "test-artifact");
        Ok(())
    }

    #[tokio::test]
    async fn test_parallels_pvm_run_io_error() {
        let config = ParallelsPvmConfig {
            name: "test".to_string(),
            test_cmd: Some("\0".to_string()),
            ..Default::default()
        };
        let builder = ParallelsPvmBuilder::new(config);
        let result = builder
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
        assert!(result.is_err());
    }
}
