//! Pipeline execution engine for post-processors.
//!
//! Supports sequential pipeline branching (arrays of arrays), intermediate artifact cleanup
//! via `keep_input_artifact`, and chaining of transformed artifacts.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// A single sequential branch in a post-processor pipeline.
pub struct PipelineBranch {
    /// The list of post-processors executed sequentially in this branch.
    pub processors: Vec<Box<dyn PostProcessor>>,
}

impl PipelineBranch {
    /// Create a new `PipelineBranch`.
    #[must_use]
    pub fn new(processors: Vec<Box<dyn PostProcessor>>) -> Self {
        Self { processors }
    }
}

/// The pipeline execution engine for post-processors.
pub struct PostProcessorPipeline {
    /// The branches of the pipeline to execute.
    pub branches: Vec<PipelineBranch>,
}

impl PostProcessorPipeline {
    /// Create a new `PostProcessorPipeline`.
    #[must_use]
    pub fn new(branches: Vec<PipelineBranch>) -> Self {
        Self { branches }
    }

    /// Execute the pipeline on an initial artifact across all branches.
    ///
    /// For each branch:
    /// - The initial artifact is passed into the first post-processor.
    /// - Output artifacts are sequentially fed as inputs to downstream post-processors.
    /// - Intermediate artifact files are cleaned up if `keep_input_artifact` is false.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError`] if any post-processor fails.
    pub async fn execute(
        &self,
        initial_artifact: &Artifact,
        ui: Arc<crate::engine::ui::Ui>,
    ) -> Result<Vec<Artifact>, StampError> {
        let mut final_artifacts = Vec::new();

        if self.branches.is_empty() {
            return Ok(vec![initial_artifact.clone()]);
        }

        for (branch_idx, branch) in self.branches.iter().enumerate() {
            ui.say(
                "pipeline",
                &format!(
                    "Executing post-processor branch {} with {} processor(s)",
                    branch_idx + 1,
                    branch.processors.len()
                ),
            );

            let mut current_artifact = initial_artifact.clone();

            for (proc_idx, processor) in branch.processors.iter().enumerate() {
                let previous_files = current_artifact.files.clone();
                let keep_input = processor.keep_input_artifact();

                ui.say(
                    "pipeline",
                    &format!(
                        "Branch {}: running step {} (keep_input={keep_input})",
                        branch_idx + 1,
                        proc_idx + 1
                    ),
                );

                let next_artifact = processor.process(current_artifact).await?;

                // Intermediate artifact cleanup
                if !keep_input {
                    for file in &previous_files {
                        if !next_artifact.files.contains(file) {
                            let path = Path::new(file);
                            if path.exists() {
                                let _ = fs::remove_file(path);
                            }
                        }
                    }
                }

                current_artifact = next_artifact;
            }

            final_artifacts.push(current_artifact);
        }

        Ok(final_artifacts)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct MockTransformer {
        suffix: &'static str,
        keep: bool,
    }

    #[async_trait]
    impl PostProcessor for MockTransformer {
        async fn process(&self, mut artifact: Artifact) -> Result<Artifact, StampError> {
            artifact.id = format!("{}-{}", artifact.id, self.suffix);
            artifact
                .files
                .push(format!("generated_{}.txt", self.suffix));
            Ok(artifact)
        }

        fn keep_input_artifact(&self) -> bool {
            self.keep
        }
    }

    struct FailingTransformer;

    #[async_trait]
    impl PostProcessor for FailingTransformer {
        async fn process(&self, _artifact: Artifact) -> Result<Artifact, StampError> {
            Err(StampError::Provisioner("Mock failure".to_string()))
        }
    }

    #[tokio::test]
    async fn test_pipeline_empty() -> Result<(), StampError> {
        let pipeline = PostProcessorPipeline::new(vec![]);
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let init = Artifact::new("base".to_string(), vec!["file1.iso".to_string()]);
        let results = pipeline.execute(&init, ui).await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "base");
        Ok(())
    }

    #[tokio::test]
    async fn test_pipeline_sequential_branching_and_cleanup() -> Result<(), StampError> {
        let tmp_dir = std::env::temp_dir();
        let intermediate_file = tmp_dir.join(format!("stamp_pipe_{}.raw", uuid::Uuid::new_v4()));
        fs::write(&intermediate_file, "raw disk content").map_err(StampError::Io)?;

        // Branch 1: step 1 (keep=false), step 2 (keep=true)
        let branch1 = PipelineBranch::new(vec![
            Box::new(MockTransformer {
                suffix: "step1",
                keep: false,
            }),
            Box::new(MockTransformer {
                suffix: "step2",
                keep: true,
            }),
        ]);

        // Branch 2: step A (keep=true)
        let branch2 = PipelineBranch::new(vec![Box::new(MockTransformer {
            suffix: "stepA",
            keep: true,
        })]);

        let pipeline = PostProcessorPipeline::new(vec![branch1, branch2]);
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let init = Artifact::new(
            "vm".to_string(),
            vec![intermediate_file.to_string_lossy().to_string()],
        );
        let results = pipeline.execute(&init, ui).await?;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "vm-step1-step2");
        assert_eq!(results[1].id, "vm-stepA");

        // Clean up temporary file if still existing
        if intermediate_file.exists() {
            let _ = fs::remove_file(intermediate_file);
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_pipeline_failure() {
        let branch = PipelineBranch::new(vec![Box::new(FailingTransformer)]);
        let pipeline = PostProcessorPipeline::new(vec![branch]);
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let init = Artifact::new("base".to_string(), vec![]);
        assert!(pipeline.execute(&init, ui).await.is_err());
    }
}
