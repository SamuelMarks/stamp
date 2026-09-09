#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `shell_local` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;

/// Configuration for the `shell_local` post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellLocalConfig {
    /// Identifier for this post-processor.
    pub identifier: String,
}

/// The `shell_local` post-processor.
#[derive(Debug, Clone)]
pub struct ShellLocalPostProcessor {
    /// The configuration.
    pub config: ShellLocalConfig,
}

impl ShellLocalPostProcessor {
    /// Create a new `ShellLocalPostProcessor`.
    #[must_use]
    pub const fn new(config: ShellLocalConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PostProcessor for ShellLocalPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty() {
            return Err(StampError::Parse("Identifier is empty".to_string()));
        }

        // Pass through or mutate artifact
        let mut new_artifact = artifact.clone();
        new_artifact.id = format!("{}-{}", artifact.id, self.config.identifier);

        Ok(new_artifact)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shell_local_process_success() -> Result<(), StampError> {
        let config = ShellLocalConfig {
            identifier: "processed".to_string(),
        };
        let processor = ShellLocalPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "base-processed");
        Ok(())
    }

    #[tokio::test]
    async fn test_shell_local_process_failure() -> Result<(), StampError> {
        let config = ShellLocalConfig {
            identifier: String::new(),
        };
        let processor = ShellLocalPostProcessor::new(config);
        let artifact = Artifact::new("base".to_string(), vec![]);

        let err = processor.process(artifact).await;
        assert!(matches!(err, Err(StampError::Parse(_))));
        Ok(())
    }

    #[test]
    fn test_shell_local_config_derived_traits() {
        let config1 = ShellLocalConfig {
            identifier: "test".to_string(),
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
    }

    #[test]
    fn test_shell_local_post_processor_derived_traits() {
        let config = ShellLocalConfig {
            identifier: "test".to_string(),
        };
        let processor1 = ShellLocalPostProcessor::new(config);
        let processor2 = processor1.clone();
        assert_eq!(format!("{processor1:?}"), format!("{processor2:?}"));
    }
}
