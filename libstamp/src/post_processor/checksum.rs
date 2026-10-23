//! Implementation of the `checksum` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use md5::{Digest, Md5 as Md5Hasher};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

/// Supported cryptographic hashing algorithms for the `checksum` post-processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ChecksumAlgorithm {
    /// SHA-256 (256-bit secure hash algorithm).
    #[default]
    Sha256,
    /// SHA-512 (512-bit secure hash algorithm).
    Sha512,
    /// MD5 (128-bit legacy message digest).
    Md5,
}

impl ChecksumAlgorithm {
    /// Return the canonical string name of the algorithm.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
            Self::Md5 => "md5",
        }
    }
}

/// Configuration for the `checksum` post-processor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksumConfig {
    /// List of checksum algorithms to compute.
    pub checksum_types: Vec<ChecksumAlgorithm>,
    /// Path format for output checksum files. Defaults to `{{ .BuildName }}_{{ .ChecksumType }}.checksum`.
    pub output: String,
    /// Whether to keep the input artifact files. Defaults to true.
    pub keep_input_artifact: bool,
    /// Identifier for this post-processor.
    pub identifier: String,
}

impl Default for ChecksumConfig {
    fn default() -> Self {
        Self {
            checksum_types: vec![ChecksumAlgorithm::Sha256],
            output: "packer_{{ .ChecksumType }}.checksum".to_string(),
            keep_input_artifact: true,
            identifier: "checksum".to_string(),
        }
    }
}

/// The `checksum` post-processor.
#[derive(Debug, Clone)]
pub struct ChecksumPostProcessor {
    /// The post-processor configuration.
    pub config: ChecksumConfig,
}

impl ChecksumPostProcessor {
    /// Create a new `ChecksumPostProcessor`.
    #[must_use]
    pub const fn new(config: ChecksumConfig) -> Self {
        Self { config }
    }

    /// Computes the cryptographic hash of a file using the specified algorithm.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Io`] if reading the file fails.
    pub fn compute_file_checksum(
        file_path: &Path,
        algo: ChecksumAlgorithm,
    ) -> Result<String, StampError> {
        let bytes = fs::read(file_path).map_err(StampError::Io)?;

        match algo {
            ChecksumAlgorithm::Sha256 => {
                let mut hasher = sha2::Sha256::new();
                hasher.update(&bytes);
                Ok(hex::encode(hasher.finalize()))
            }
            ChecksumAlgorithm::Sha512 => {
                let mut hasher = sha2::Sha512::new();
                hasher.update(&bytes);
                Ok(hex::encode(hasher.finalize()))
            }
            ChecksumAlgorithm::Md5 => {
                let mut hasher = Md5Hasher::new();
                hasher.update(&bytes);
                Ok(hex::encode(hasher.finalize()))
            }
        }
    }
}

#[async_trait]
impl PostProcessor for ChecksumPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty() {
            return Err(StampError::Provisioner("Identifier is empty".to_string()));
        }
        if self.config.checksum_types.is_empty() {
            return Err(StampError::Provisioner(
                "checksum_types is required".to_string(),
            ));
        }

        let mut output_files = artifact.files.clone();

        for &algo in &self.config.checksum_types {
            let mut lines = Vec::new();

            for f in &artifact.files {
                let path = Path::new(f);
                if path.exists() && path.is_file() {
                    let hash = Self::compute_file_checksum(path, algo)?;
                    let file_name = path
                        .file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .unwrap_or(f.as_str());
                    lines.push(format!("{hash}  {file_name}"));
                }
            }

            if !lines.is_empty() {
                let out_name = self
                    .config
                    .output
                    .replace("{{ .ChecksumType }}", algo.as_str())
                    .replace("{{ .BuildName }}", &artifact.id);

                let mut out_file = File::create(&out_name).map_err(StampError::Io)?;
                for line in lines {
                    writeln!(out_file, "{line}").map_err(StampError::Io)?;
                }
                out_file.flush().map_err(StampError::Io)?;
                output_files.push(out_name);
            }
        }

        let mut new_artifact = artifact;
        new_artifact.id = format!("{}-{}", new_artifact.id, self.config.identifier);
        new_artifact.files = output_files;

        Ok(new_artifact)
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn test_checksum_sha256_sha512_md5() -> Result<(), StampError> {
        let tmp_dir = tempfile::tempdir()?;
        let test_file = tmp_dir.path().join("stamp_test.iso");
        fs::write(&test_file, b"hello world")?;

        let out_template = tmp_dir.path().join("stamp_check_{{ .ChecksumType }}.txt");

        let config = ChecksumConfig {
            checksum_types: vec![
                ChecksumAlgorithm::Sha256,
                ChecksumAlgorithm::Sha512,
                ChecksumAlgorithm::Md5,
            ],
            output: out_template.to_string_lossy().to_string(),
            keep_input_artifact: true,
            identifier: "checksummed".to_string(),
        };

        let processor = ChecksumPostProcessor::new(config);
        let artifact = Artifact::new(
            "my_artifact".to_string(),
            vec![
                test_file.to_string_lossy().to_string(),
                "/nonexistent_file_checksum_ignored.iso".to_string(),
            ],
        );

        let result = processor.process(artifact).await?;
        assert_eq!(result.id, "my_artifact-checksummed");
        assert_eq!(result.files.len(), 5); // original 2 files + 3 checksum files

        assert!(processor.keep_input_artifact());

        // Verify computed hashes match known values for "hello world"
        // sha256: b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        // md5: 5eb63bbbe01eeed093cb22bb8f5acdc3
        let sha256_hash =
            ChecksumPostProcessor::compute_file_checksum(&test_file, ChecksumAlgorithm::Sha256)?;
        assert_eq!(
            sha256_hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );

        let md5_hash =
            ChecksumPostProcessor::compute_file_checksum(&test_file, ChecksumAlgorithm::Md5)?;
        assert_eq!(md5_hash, "5eb63bbbe01eeed093cb22bb8f5acdc3");

        let sha512_hash =
            ChecksumPostProcessor::compute_file_checksum(&test_file, ChecksumAlgorithm::Sha512)?;
        assert_ne!(sha512_hash, "");
        Ok(())
    }

    #[tokio::test]
    async fn test_checksum_empty_types_and_identifier() {
        let config_no_types = ChecksumConfig {
            checksum_types: vec![],
            ..Default::default()
        };
        let processor = ChecksumPostProcessor::new(config_no_types);
        let artifact = Artifact::new("base".to_string(), vec![]);
        assert!(processor.process(artifact.clone()).await.is_err());

        let config_no_id = ChecksumConfig {
            identifier: String::new(),
            ..Default::default()
        };
        let processor_no_id = ChecksumPostProcessor::new(config_no_id);
        assert!(processor_no_id.process(artifact).await.is_err());
    }

    #[tokio::test]
    async fn test_checksum_output_create_failure() -> Result<(), StampError> {
        let tmp_dir = tempfile::tempdir()?;
        let test_file = tmp_dir.path().join("stamp_test.iso");
        fs::write(&test_file, b"test content")?;

        let config = ChecksumConfig {
            checksum_types: vec![ChecksumAlgorithm::Sha256],
            output: "/nonexistent_dir_99999/checksum_{{ .ChecksumType }}.txt".to_string(),
            keep_input_artifact: true,
            identifier: "checksummed".to_string(),
        };
        let processor = ChecksumPostProcessor::new(config);
        let artifact = Artifact::new(
            "my_artifact".to_string(),
            vec![test_file.to_string_lossy().to_string()],
        );
        let res = processor.process(artifact).await;
        assert!(res.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_checksum_unreadable_file_failure() -> Result<(), StampError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tmp_dir = tempfile::tempdir()?;
            let test_file = tmp_dir.path().join("stamp_unreadable.iso");
            fs::write(&test_file, b"unreadable content")?;
            fs::set_permissions(&test_file, fs::Permissions::from_mode(0o000))?;

            let config = ChecksumConfig {
                checksum_types: vec![ChecksumAlgorithm::Sha256],
                output: tmp_dir
                    .path()
                    .join("check_{{ .ChecksumType }}.txt")
                    .to_string_lossy()
                    .to_string(),
                keep_input_artifact: true,
                identifier: "checksummed".to_string(),
            };
            let processor = ChecksumPostProcessor::new(config);
            let artifact = Artifact::new(
                "my_artifact".to_string(),
                vec![test_file.to_string_lossy().to_string()],
            );
            let res = processor.process(artifact).await;
            assert!(res.is_err());

            fs::set_permissions(&test_file, fs::Permissions::from_mode(0o644))?;
        }
        Ok(())
    }

    #[test]
    fn test_checksum_compute_missing_file() {
        let res = ChecksumPostProcessor::compute_file_checksum(
            Path::new("/nonexistent_path_checksum_12345.iso"),
            ChecksumAlgorithm::Sha256,
        );
        assert!(res.is_err());
    }

    #[test]
    fn test_checksum_derived_traits() {
        let algo1 = ChecksumAlgorithm::Sha256;
        let algo2 = ChecksumAlgorithm::default();
        assert_eq!(algo1, algo2);
        assert_eq!(algo1.as_str(), "sha256");
        assert_eq!(ChecksumAlgorithm::Sha512.as_str(), "sha512");
        assert_eq!(ChecksumAlgorithm::Md5.as_str(), "md5");

        let config1 = ChecksumConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert!(format!("{config1:?}").contains("ChecksumConfig"));

        let p1 = ChecksumPostProcessor::new(config1);
        let p2 = p1.clone();
        assert!(format!("{p1:?}").contains("ChecksumPostProcessor"));
        assert_eq!(p1.config, p2.config);
    }
}
