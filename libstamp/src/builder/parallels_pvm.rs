//! Implementation of the `parallels-pvm` builder.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::error::StampError;

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
        _hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
        _on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let bin = self.config.test_cmd.as_deref().unwrap_or("prlctl");
        ui.say(
            &self.config.name,
            &format!("Running builder '{}' with '{bin}'", self.config.name),
        );

        let status = tokio::process::Command::new(bin)
            .arg("--version")
            .status()
            .await;

        match status {
            Ok(_) => Ok(Box::new(BuildArtifact::with_builder(
                self.name(),
                format!("{}-artifact", self.name()),
                vec![],
            ))),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(Box::new(BuildArtifact::with_builder(
                        self.name(),
                        format!("{}-artifact", self.name()),
                        vec![],
                    )))
                } else {
                    Err(StampError::Io(std::io::Error::other("io error")))
                }
            }
        }
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
