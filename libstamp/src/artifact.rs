//! Artifact definitions for Stamp.

use crate::error::StampError;
use std::any::Any;
use std::collections::BTreeMap;

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
    fn destroy(&self) -> Result<(), StampError>;
}

/// A concrete machine image or file artifact produced by a Stamp builder or post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildArtifact {
    /// The ID of the builder that produced this artifact.
    pub builder_id: String,
    /// The unique ID of the artifact (e.g. AMI ID, image tag, or artifact identifier).
    pub id: String,
    /// List of files associated with the artifact.
    pub files: Vec<String>,
    /// Arbitrary key-value state metadata attached to the artifact.
    pub state_data: BTreeMap<String, String>,
}

impl BuildArtifact {
    /// Create a new `BuildArtifact` with a given ID and file list.
    #[must_use]
    pub const fn new(id: String, files: Vec<String>) -> Self {
        Self {
            builder_id: String::new(),
            id,
            files,
            state_data: BTreeMap::new(),
        }
    }

    /// Create a new `BuildArtifact` with an explicit builder ID, artifact ID, and file list.
    #[must_use]
    pub const fn with_builder(builder_id: String, id: String, files: Vec<String>) -> Self {
        Self {
            builder_id,
            id,
            files,
            state_data: BTreeMap::new(),
        }
    }

    /// Insert a state metadata entry on the artifact.
    #[must_use]
    pub fn with_state(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.state_data.insert(key.into(), value.into());
        self
    }
}

impl Artifact for BuildArtifact {
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
        if self.builder_id.is_empty() {
            format!("BuildArtifact(id: {})", self.id)
        } else {
            format!("{}: {}", self.builder_id, self.id)
        }
    }

    fn state(&self, name: &str) -> Option<Box<dyn Any>> {
        self.state_data
            .get(name)
            .map(|v| Box::new(v.clone()) as Box<dyn Any>)
    }

    fn destroy(&self) -> Result<(), StampError> {
        for f in &self.files {
            let p = std::path::Path::new(f);
            if p.exists() && p.is_file() {
                std::fs::remove_file(p)?;
            }
        }
        Ok(())
    }
}

/// A basic artifact struct for compatibility across builders and tests.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MockArtifact {
    /// The builder ID.
    pub builder_id: String,
    /// The artifact ID.
    pub id: String,
    /// The files.
    pub files: Vec<String>,
}

impl From<MockArtifact> for BuildArtifact {
    fn from(m: MockArtifact) -> Self {
        Self::with_builder(m.builder_id, m.id, m.files)
    }
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

    fn destroy(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_artifact() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let file_path = temp_dir.path().join("disk.qcow2");
        std::fs::write(&file_path, b"qcow2 data")?;
        let file_str = file_path.to_string_lossy().to_string();

        let art = BuildArtifact::with_builder(
            "qemu".to_string(),
            "qemu-img-1".to_string(),
            vec![file_str.clone()],
        )
        .with_state("region", "us-east-1");

        assert_eq!(art.builder_id(), "qemu");
        assert_eq!(art.id(), "qemu-img-1");
        assert_eq!(art.files(), vec![file_str]);
        assert_eq!(art.string(), "qemu: qemu-img-1");
        let region_any = art.state("region");
        assert!(region_any.is_some());
        if let Some(boxed) = region_any {
            assert_eq!(
                boxed.downcast_ref::<String>().map(String::as_str),
                Some("us-east-1")
            );
        }
        assert!(art.state("missing").is_none());

        art.destroy()?;
        assert!(!file_path.exists());

        let simple = BuildArtifact::new("art-simple".to_string(), vec![]);
        assert_eq!(simple.string(), "BuildArtifact(id: art-simple)");
        assert_eq!(simple.builder_id(), "");
        simple.destroy()?;
        Ok(())
    }

    #[test]
    fn test_mock_artifact() -> Result<(), StampError> {
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
        artifact.destroy()?;

        let converted: BuildArtifact = artifact.into();
        assert_eq!(converted.builder_id, "test-builder");
        assert_eq!(converted.id, "test-id");
        Ok(())
    }
}
