//! Implementation of the `manifest` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::Path;

/// Configuration for the `manifest` post-processor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestConfig {
    /// The path to write the manifest file to. Defaults to `manifest.json`.
    pub output: String,
    /// Whether to strip the directory path from artifact filenames.
    pub strip_path: bool,
    /// Custom user-defined key-value metadata to attach to the manifest build entry.
    pub custom_data: HashMap<String, String>,
    /// Custom tags to attach to the manifest build entry.
    pub custom_tags: HashMap<String, String>,
    /// Git commit SHA metadata.
    pub git_commit: Option<String>,
    /// Identifier for this post-processor.
    pub identifier: String,
}

impl Default for ManifestConfig {
    fn default() -> Self {
        Self {
            output: "manifest.json".to_string(),
            strip_path: false,
            custom_data: HashMap::new(),
            custom_tags: HashMap::new(),
            git_commit: None,
            identifier: "manifest".to_string(),
        }
    }
}

/// The structure of the JSON manifest output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ManifestOutput {
    /// The builds that are captured in this manifest.
    pub builds: Vec<ManifestBuild>,
    /// The last run UUID.
    pub last_run_uuid: String,
}

/// A single build entry in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestBuild {
    /// The name of the builder that produced the artifact.
    pub name: String,
    /// The builder type.
    pub builder_type: String,
    /// Unix timestamp when the build occurred.
    pub build_time: i64,
    /// Formatted RFC 3339 timestamp.
    pub build_time_formatted: String,
    /// The artifact ID.
    pub artifact_id: String,
    /// List of files associated with the artifact with size information.
    pub files: Vec<ManifestFile>,
    /// Packer/Stamp run UUID.
    pub packer_run_uuid: String,
    /// Git commit SHA metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    /// Custom tags attached to the build.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_tags: HashMap<String, String>,
    /// Custom user data map.
    pub custom_data: HashMap<String, String>,
}

/// File information tracked in the manifest build record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Name or path of the file.
    pub name: String,
    /// Size of the file in bytes.
    pub size: u64,
}

/// The `manifest` post-processor.
#[derive(Debug, Clone)]
pub struct ManifestPostProcessor {
    /// The configuration.
    pub config: ManifestConfig,
}

impl ManifestPostProcessor {
    /// Create a new `ManifestPostProcessor`.
    #[must_use]
    pub const fn new(config: ManifestConfig) -> Self {
        Self { config }
    }

    /// Detects or retrieves the git commit SHA.
    fn detect_git_commit(&self) -> Option<String> {
        if let Some(commit) = &self.config.git_commit {
            return Some(commit.clone());
        }
        if let Some(commit) = self.config.custom_data.get("git_commit") {
            return Some(commit.clone());
        }
        if let Ok(commit) = std::env::var("GIT_COMMIT") {
            return Some(commit);
        }
        #[cfg(test)]
        {
            None
        }
        #[cfg(not(test))]
        {
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
        }
    }
}

#[async_trait]
impl PostProcessor for ManifestPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty() {
            return Err(StampError::Provisioner("Identifier is empty".to_string()));
        }
        if self.config.output.is_empty() {
            return Err(StampError::Provisioner(
                "Output file path is required".to_string(),
            ));
        }

        let now = chrono::Utc::now();
        let build_time = now.timestamp();
        let build_time_formatted = now.to_rfc3339();
        let run_uuid = uuid::Uuid::new_v4().to_string();

        let mut manifest_files = Vec::new();
        for file_path in &artifact.files {
            let path = Path::new(file_path);
            let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let display_name = if self.config.strip_path {
                match path.file_name() {
                    Some(n) => n.to_string_lossy().to_string(),
                    None => file_path.clone(),
                }
            } else {
                file_path.clone()
            };
            manifest_files.push(ManifestFile {
                name: display_name,
                size,
            });
        }

        let git_commit = self.detect_git_commit();

        let new_build = ManifestBuild {
            name: "default".to_string(),
            builder_type: "stamp".to_string(),
            build_time,
            build_time_formatted,
            artifact_id: artifact.id.clone(),
            files: manifest_files,
            packer_run_uuid: run_uuid.clone(),
            git_commit,
            custom_tags: self.config.custom_tags.clone(),
            custom_data: self.config.custom_data.clone(),
        };

        let mut manifest_output: ManifestOutput = if Path::new(&self.config.output).exists() {
            let content = fs::read_to_string(&self.config.output).map_err(StampError::Io)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            ManifestOutput::default()
        };

        manifest_output.builds.push(new_build);
        manifest_output.last_run_uuid = run_uuid;

        let json_string =
            serde_json::to_string_pretty(&manifest_output).map_err(StampError::Json)?;

        let mut file = File::create(&self.config.output).map_err(StampError::Io)?;
        use std::io::Write;
        file.write_all(json_string.as_bytes())
            .map_err(StampError::Io)?;
        file.flush().map_err(StampError::Io)?;

        let mut new_artifact = artifact;
        new_artifact.id = format!("{}-{}", new_artifact.id, self.config.identifier);

        Ok(new_artifact)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_manifest_process_success() -> Result<(), StampError> {
        let temp_dir = std::env::temp_dir();
        let output_path = temp_dir.join(format!("stamp_manifest_{}.json", uuid::Uuid::new_v4()));
        let dummy_file = temp_dir.join(format!("stamp_art_{}.txt", uuid::Uuid::new_v4()));
        fs::write(&dummy_file, b"manifest file content").map_err(StampError::Io)?;

        let mut custom = HashMap::new();
        custom.insert("commit".to_string(), "abcdef".to_string());

        let config = ManifestConfig {
            output: output_path.to_string_lossy().to_string(),
            strip_path: true,
            custom_data: custom,
            identifier: "processed".to_string(),
            ..Default::default()
        };
        let processor = ManifestPostProcessor::new(config);
        let artifact = Artifact::new(
            "base".to_string(),
            vec![dummy_file.to_string_lossy().to_string(), "/".to_string()],
        );

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "base-processed");

        let contents = fs::read_to_string(&output_path).map_err(StampError::Io)?;
        let parsed: ManifestOutput = serde_json::from_str(&contents).map_err(StampError::Json)?;

        assert_eq!(parsed.builds.len(), 1);
        assert_eq!(parsed.builds[0].artifact_id, "base");
        assert_eq!(parsed.builds[0].files.len(), 2);
        assert!(parsed.builds[0].files[0].size > 0);
        assert_eq!(parsed.builds[0].files[1].name, "/");
        assert_eq!(
            parsed.builds[0]
                .custom_data
                .get("commit")
                .map(String::as_str),
            Some("abcdef")
        );

        let _ = fs::remove_file(dummy_file);
        let _ = fs::remove_file(output_path);
        Ok(())
    }

    #[tokio::test]
    async fn test_manifest_append_existing() -> Result<(), StampError> {
        let temp_dir = std::env::temp_dir();
        let output_path = temp_dir.join(format!("stamp_manifest_{}.json", uuid::Uuid::new_v4()));

        let config = ManifestConfig {
            output: output_path.to_string_lossy().to_string(),
            strip_path: false,
            custom_data: HashMap::new(),
            identifier: "m".to_string(),
            ..Default::default()
        };
        let processor = ManifestPostProcessor::new(config);
        let art1 = Artifact::new("b1".to_string(), vec![]);
        let art2 = Artifact::new("b2".to_string(), vec![]);

        processor.process(art1).await?;
        processor.process(art2).await?;

        let contents = fs::read_to_string(&output_path).map_err(StampError::Io)?;
        let parsed: ManifestOutput = serde_json::from_str(&contents).map_err(StampError::Json)?;
        assert_eq!(parsed.builds.len(), 2);
        assert_eq!(parsed.builds[0].artifact_id, "b1");
        assert_eq!(parsed.builds[1].artifact_id, "b2");

        let _ = fs::remove_file(output_path);
        Ok(())
    }

    #[tokio::test]
    async fn test_manifest_empty_output_or_identifier() {
        let p1 = ManifestPostProcessor::new(ManifestConfig {
            identifier: String::new(),
            ..Default::default()
        });
        assert!(p1.process(Artifact::new("a".into(), vec![])).await.is_err());

        let p2 = ManifestPostProcessor::new(ManifestConfig {
            output: String::new(),
            identifier: "m".into(),
            ..Default::default()
        });
        assert!(p2.process(Artifact::new("a".into(), vec![])).await.is_err());
    }

    #[tokio::test]
    async fn test_manifest_git_commit_and_custom_tags() -> Result<(), StampError> {
        let temp_dir = std::env::temp_dir();
        let output_path =
            temp_dir.join(format!("stamp_manifest_tags_{}.json", uuid::Uuid::new_v4()));

        let mut tags = HashMap::new();
        tags.insert("Environment".to_string(), "Production".to_string());

        let config = ManifestConfig {
            output: output_path.to_string_lossy().to_string(),
            custom_tags: tags,
            git_commit: Some("deadbeef1234".to_string()),
            identifier: "tagged".to_string(),
            ..Default::default()
        };
        let processor = ManifestPostProcessor::new(config);
        let art = Artifact::new("prod-artifact".to_string(), vec![]);
        processor.process(art).await?;

        let contents = fs::read_to_string(&output_path).map_err(StampError::Io)?;
        let parsed: ManifestOutput = serde_json::from_str(&contents).map_err(StampError::Json)?;

        assert_eq!(parsed.builds[0].git_commit.as_deref(), Some("deadbeef1234"));
        assert_eq!(
            parsed.builds[0]
                .custom_tags
                .get("Environment")
                .map(String::as_str),
            Some("Production")
        );

        let _ = fs::remove_file(output_path);
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config1 = ManifestConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = ManifestPostProcessor::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
