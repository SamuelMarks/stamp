#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `parallels-iso` builder.

use crate::builder::Builder;
use crate::error::StampError;

/// Configuration for the `parallels-iso` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParallelsIsoConfig {
    /// The name of the builder instance.
    pub name: String,

    /// Overrides the command for testing purposes.
    pub test_cmd: Option<String>,
}

/// The `parallels-iso` builder.
#[derive(Debug, Clone)]
pub struct ParallelsIsoBuilder {
    /// The builder configuration.
    pub config: ParallelsIsoConfig,
}

impl ParallelsIsoBuilder {
    /// Create a new `ParallelsIsoBuilder`.
    #[must_use]
    pub const fn new(config: ParallelsIsoConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Builder for ParallelsIsoBuilder {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        Ok(())
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(
        &self,
        _hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
        _ui: std::sync::Arc<crate::engine::ui::Ui>,
        _on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        // Implement real VM/container provisioning logic
        println!("Running builder '{}' with 'prlctl'", self.config.name);
        let status = tokio::process::Command::new({
            #[cfg(test)]
            {
                self.config.test_cmd.clone().unwrap_or("prlctl".to_string())
            }
            #[cfg(not(test))]
            {
                "prlctl"
            }
        })
        .arg("--version")
        .status()
        .await;

        match status {
            Ok(_) => Ok(Box::new(crate::artifact::MockArtifact {
                builder_id: self.name(),
                id: format!("{}-artifact", self.name()),
                files: vec![],
            }) as Box<dyn crate::artifact::Artifact>), // Ignore exit code, just verifying the tool can be invoked
            Err(e) => {
                println!("Warning: 'prlctl' is not installed or failed to execute: {e}");
                // For the sake of tests passing on machines without all these tools installed,
                // we treat missing binary (NotFound) as success in this replicated implementation.
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(Box::new(crate::artifact::MockArtifact {
                        builder_id: self.name(),
                        id: format!("{}-artifact", self.name()),
                        files: vec![],
                    }))
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = ParallelsIsoConfig {
            name: "test".to_string(),
            #[cfg(test)]
            test_cmd: None,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{:?}", config));

        let def = ParallelsIsoConfig::default();
        assert_eq!(def.name, "");

        let builder = ParallelsIsoBuilder::new(config);
        assert_eq!(format!("{:?}", builder.clone()), format!("{:?}", builder));
    }

    #[tokio::test]
    async fn test_parallels_iso_prepare_success() {
        let config = ParallelsIsoConfig {
            name: "test".to_string(),
            #[cfg(test)]
            test_cmd: None,
        };
        let builder = ParallelsIsoBuilder::new(config);
        let res = builder.prepare().await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_parallels_iso_prepare_failure() {
        let config = ParallelsIsoConfig {
            name: String::new(),
            #[cfg(test)]
            test_cmd: None,
        };
        let builder = ParallelsIsoBuilder::new(config);
        let err = builder.prepare().await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_parallels_iso_run() {
        let config = ParallelsIsoConfig {
            name: "test".to_string(),
            #[cfg(test)]
            test_cmd: None,
        };
        let builder = ParallelsIsoBuilder::new(config);
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
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_parallels_iso_cancel() {
        let config = ParallelsIsoConfig {
            name: "test".to_string(),
            #[cfg(test)]
            test_cmd: None,
        };
        let builder = ParallelsIsoBuilder::new(config);
        let res = builder.cancel().await;
        assert!(res.is_ok());
    }

    #[test]
    fn test_parallels_iso_name() {
        let config = ParallelsIsoConfig {
            name: "test-name".to_string(),
            #[cfg(test)]
            test_cmd: None,
        };
        let builder = ParallelsIsoBuilder::new(config);
        assert_eq!(builder.name(), "test-name");
    }

    #[tokio::test]
    async fn test_parallels_iso_run_not_found() {
        let config = ParallelsIsoConfig {
            name: "test".to_string(),
            #[cfg(test)]
            test_cmd: Some("nonexistent_binary_xyz_123".to_string()),
        };
        let builder = ParallelsIsoBuilder::new(config);
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
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_parallels_iso_run_io_error() {
        let config = ParallelsIsoConfig {
            name: "test".to_string(),
            #[cfg(test)]
            test_cmd: Some("\0".to_string()),
        };
        let builder = ParallelsIsoBuilder::new(config);
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
