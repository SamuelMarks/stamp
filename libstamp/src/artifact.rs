#![cfg_attr(coverage_nightly, coverage(off))]
//! Artifact definitions for Stamp.

use std::any::Any;

/// Represents an artifact created by a builder.
pub trait Artifact: Send + Sync + std::fmt::Debug {
    /// Returns the ID of the builder that created this artifact.
    fn builder_id(&self) -> String;

    /// Returns a list of files that make up this artifact.
    fn files(&self) -> Vec<String>;

    /// Returns the unique ID of this artifact.
    fn id(&self) -> String;

    /// Returns a human-readable string representation of this artifact.
    fn string(&self) -> String;

    /// Returns state information for this artifact by name.
    fn state(&self, name: &str) -> Option<Box<dyn Any>>;

    /// Destroys this artifact.
    ///
    /// # Errors
    /// Returns `StampError` if the artifact cannot be destroyed.
    fn destroy(&self) -> Result<(), crate::error::StampError>;
}

/// A basic mock artifact for use in testing and stubbed builders.
#[derive(Debug, Clone)]
pub struct MockArtifact {
    /// The builder ID.
    pub builder_id: String,
    /// The artifact ID.
    pub id: String,
    /// The files.
    pub files: Vec<String>,
}

impl Artifact for MockArtifact {
    fn builder_id(&self) -> String {
        self.builder_id.clone()
    }

    fn files(&self) -> Vec<String> {
        self.files.clone()
    }

    fn id(&self) -> String {
        self.id.clone()
    }

    fn string(&self) -> String {
        format!("MockArtifact(id: {})", self.id)
    }

    fn state(&self, _name: &str) -> Option<Box<dyn Any>> {
        None
    }

    fn destroy(&self) -> Result<(), crate::error::StampError> {
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_artifact() {
        let artifact = MockArtifact {
            builder_id: "test-builder".to_string(),
            id: "test-id".to_string(),
            files: vec!["file1.txt".to_string()],
        };

        assert_eq!(artifact.builder_id(), "test-builder");
        assert_eq!(artifact.id(), "test-id");
        assert_eq!(artifact.files(), vec!["file1.txt"]);
        assert_eq!(artifact.string(), "MockArtifact(id: test-id)");
        assert!(artifact.state("anything").is_none());
        assert!(artifact.destroy().is_ok());
    }
}
