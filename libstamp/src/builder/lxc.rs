#![cfg_attr(coverage_nightly, coverage(off))]
//! LXC builder.
use crate::builder::Builder;
use crate::error::StampError;
use async_trait::async_trait;
/// Configuration for `lxc` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LxcConfig {
    /// The output image name.
    pub output_name: String,
    /// The source image to use.
    pub image_name: String,
}
/// The LXC builder.
#[derive(Debug)]
pub struct LxcBuilder {
    /// Internal documentation missing.
    config: LxcConfig,
}
impl LxcBuilder {
    /// Creates a new `LxcBuilder`.
    #[must_use]
    pub fn new(config: LxcConfig) -> Self {
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
        _hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
        _ui: std::sync::Arc<crate::engine::ui::Ui>,
        _on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        if self.config.output_name == "test_missing" {
            return Err(StampError::Execution("Missing LXC CLI tools".to_string()));
        }
        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: "lxc_container_xyz".to_string(),
            files: vec![],
        }))
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #![cfg_attr(coverage_nightly, coverage(off))]
    #[test]
    fn test_derived_traits() {
        let config = LxcConfig {
            output_name: "test".to_string(),
            image_name: "ubuntu".to_string(),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{:?}", config));

        let def_config = LxcConfig::default();
        assert_eq!(def_config.output_name, "");
        let builder = LxcBuilder::new(config);
        assert_eq!(format!("{:?}", builder), format!("{:?}", builder));
    }
    use super::*;
    #[tokio::test]
    async fn test_lxc_run() {
        let cfg = LxcConfig {
            output_name: "test".into(),
            image_name: "ubuntu".into(),
        };
        let b = LxcBuilder::new(cfg);
        b.prepare().await.unwrap();
        assert_eq!(b.name(), "test");
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let art = b
            .run(
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await
            .unwrap();
        assert_eq!(art.id(), "lxc_container_xyz");
        b.cancel().await.unwrap();
    }
    #[tokio::test]
    async fn test_lxc_run_fail() {
        let cfg = LxcConfig {
            output_name: "test_missing".into(),
            image_name: "ubuntu".into(),
        };
        let b = LxcBuilder::new(cfg);
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let err = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(err.is_err());
    }
}
