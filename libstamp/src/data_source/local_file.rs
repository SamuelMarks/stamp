#![cfg_attr(coverage_nightly, coverage(off))]
//! `file` / `local_file` data source implementation for reading local filesystem files.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Configuration for the `file` data source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalFileConfig {
    /// Path to the file to read.
    pub path: String,
}

/// The `file` / `local_file` data source.
#[derive(Debug, Clone, Default)]
pub struct LocalFileDataSource {
    /// Configuration for local file data source.
    pub config: LocalFileConfig,
}

impl LocalFileDataSource {
    /// Creates a new `LocalFileDataSource`.
    #[must_use]
    pub const fn new(config: LocalFileConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for LocalFileDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.path.trim().is_empty() {
            return Err(StampError::Parse(
                "File data source path cannot be empty".to_string(),
            ));
        }

        let file_path = PathBuf::from(&self.config.path);
        let bytes = tokio::fs::read(&file_path).await.map_err(|e| {
            StampError::Execution(format!(
                "Failed to read file '{}': {e}",
                file_path.display()
            ))
        })?;

        let content_utf8 = String::from_utf8_lossy(&bytes).to_string();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let sha256_hex = hex::encode(hasher.finalize());
        let size = bytes.len();

        Ok(json!({
            "path": self.config.path,
            "content": content_utf8,
            "content_base64": b64,
            "sha256": sha256_hex,
            "size": size,
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = LocalFileConfig {
            path: "/path/to/file".to_string(),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let ds = LocalFileDataSource::new(config);
        assert_eq!(format!("{ds:?}"), format!("{:?}", ds.clone()));
    }

    #[tokio::test]
    async fn test_read_existing_file() {
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join("test_stamp_data_source_file.txt");
        tokio::fs::write(&file_path, "hello world stamp")
            .await
            .unwrap();

        let ds = LocalFileDataSource::new(LocalFileConfig {
            path: file_path.to_string_lossy().to_string(),
        });
        let res = ds.read().await.unwrap();
        assert_eq!(res["content"], "hello world stamp");
        assert_eq!(res["size"], 17);
        assert!(res["sha256"].is_string());
        assert!(res["content_base64"].is_string());

        let _ = tokio::fs::remove_file(file_path).await;
    }

    #[tokio::test]
    async fn test_read_empty_path() {
        let ds = LocalFileDataSource::new(LocalFileConfig {
            path: String::new(),
        });
        assert!(ds.read().await.is_err());
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let ds = LocalFileDataSource::new(LocalFileConfig {
            path: "/path/that/definitely/does/not/exist_stamp_123.txt".to_string(),
        });
        assert!(ds.read().await.is_err());
    }
}
