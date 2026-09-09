#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `file` builder.

use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::error::StampError;
use std::sync::Arc;

/// Configuration for the `file` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The target path to write to.
    pub target: String,
    /// The literal content to write.
    pub content: Option<String>,
    /// The source file path to copy.
    pub source: Option<String>,
}

/// The `file` builder.
#[derive(Debug, Clone)]
pub struct FileBuilder {
    /// The builder configuration.
    pub config: FileConfig,
}

impl FileBuilder {
    /// Create a new `FileBuilder`.
    #[must_use]
    pub const fn new(config: FileConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Builder for FileBuilder {
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.target.is_empty() {
            return Err(StampError::Validation(
                "File builder 'target' cannot be empty".to_string(),
            ));
        }
        if self.config.content.is_some() && self.config.source.is_some() {
            return Err(StampError::Validation(
                "File builder cannot have both 'content' and 'source' specified".to_string(),
            ));
        }
        Ok(())
    }

    async fn run(
        &self,
        _hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        _on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        ui.say(&self.name(), "Running file builder...");

        if let Some(content) = &self.config.content {
            std::fs::write(&self.config.target, content).map_err(StampError::Io)?;
        } else if let Some(source) = &self.config.source {
            std::fs::copy(source, &self.config.target).map_err(StampError::Io)?;
        } else {
            // Touch the file if neither is specified
            std::fs::write(&self.config.target, "").map_err(StampError::Io)?;
        }

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("{}-file", self.name()),
            files: vec![self.config.target.clone()],
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::pedantic,
    clippy::all,
    for_loops_over_fallibles
)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = FileConfig {
            name: "test".to_string(),
            target: "out.txt".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{:?}", config));

        let def = FileConfig::default();
        assert_eq!(def.name, "");
        assert_eq!(def.target, "");

        let builder = FileBuilder::new(config);
        assert_eq!(format!("{:?}", builder.clone()), format!("{:?}", builder));
        assert_eq!(builder.name(), "test");
    }

    #[tokio::test]
    async fn test_prepare_success() {
        let builder = FileBuilder::new(FileConfig {
            name: "test".to_string(),
            target: "out.txt".to_string(),
            ..Default::default()
        });
        let res = builder.prepare().await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_prepare_failure_empty_target() {
        let builder = FileBuilder::new(FileConfig {
            name: "test".to_string(),
            target: String::new(),
            ..Default::default()
        });
        let err = builder.prepare().await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_prepare_failure_both_source_and_content() {
        let builder = FileBuilder::new(FileConfig {
            name: "test".to_string(),
            target: "out.txt".to_string(),
            content: Some("data".to_string()),
            source: Some("in.txt".to_string()),
        });
        let err = builder.prepare().await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_run_with_content() {
        let tmp = std::env::temp_dir().join("stamp_test_file_content.txt");
        let target = tmp.to_str().unwrap().to_string();

        let builder = FileBuilder::new(FileConfig {
            name: "test".to_string(),
            target: target.clone(),
            content: Some("hello file builder".to_string()),
            source: None,
        });

        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });

        let res = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_ok());
        for artifact in res {
            assert_eq!(artifact.id(), "test-file");
            assert_eq!(artifact.files(), vec![target.clone()]);
        }

        let read_content = std::fs::read_to_string(&tmp).unwrap();
        assert_eq!(read_content, "hello file builder");

        let _ = std::fs::remove_file(tmp);
    }

    #[tokio::test]
    async fn test_run_with_source() {
        let src = std::env::temp_dir().join("stamp_test_file_src.txt");
        let dst = std::env::temp_dir().join("stamp_test_file_dst.txt");

        std::fs::write(&src, "copied data").unwrap();

        let builder = FileBuilder::new(FileConfig {
            name: "test".to_string(),
            target: dst.to_str().unwrap().to_string(),
            content: None,
            source: Some(src.to_str().unwrap().to_string()),
        });

        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });

        let res = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_ok());

        let read_content = std::fs::read_to_string(&dst).unwrap();
        assert_eq!(read_content, "copied data");

        let _ = std::fs::remove_file(src);
        let _ = std::fs::remove_file(dst);
    }

    #[tokio::test]
    async fn test_run_empty() {
        let tmp = std::env::temp_dir().join("stamp_test_file_empty.txt");
        let target = tmp.to_str().unwrap().to_string();

        let builder = FileBuilder::new(FileConfig {
            name: "test".to_string(),
            target: target.clone(),
            content: None,
            source: None,
        });

        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });

        let res = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_ok());

        let read_content = std::fs::read_to_string(&tmp).unwrap();
        assert_eq!(read_content, "");

        let _ = std::fs::remove_file(tmp);
    }

    #[tokio::test]
    async fn test_run_io_error() {
        let builder = FileBuilder::new(FileConfig {
            name: "test".to_string(),
            target: "/invalid/path/that/does/not/exist".to_string(),
            content: Some("data".to_string()),
            source: None,
        });

        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });

        let err = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_run_io_error_copy() {
        let builder = FileBuilder::new(FileConfig {
            name: "test".to_string(),
            target: "/invalid/path/that/does/not/exist".to_string(),
            content: None,
            source: Some("/another/invalid/path".to_string()),
        });

        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });

        let err = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_cancel() {
        let builder = FileBuilder::new(FileConfig::default());
        let res = builder.cancel().await;
        assert!(res.is_ok());
    }
}
