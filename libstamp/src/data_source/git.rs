//! `git` data source implementation for extracting Git repository commit and branch state.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;

/// Configuration for the `git` data source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitDataSourceConfig {
    /// Path to the repository working directory (defaults to current directory if empty).
    pub path: Option<String>,
    /// Optional custom path or name of the `git` executable (defaults to `"git"`).
    pub git_binary: Option<String>,
}

/// The `git` data source.
#[derive(Debug, Clone, Default)]
pub struct GitDataSource {
    /// Configuration for the Git data source.
    pub config: GitDataSourceConfig,
}

impl GitDataSource {
    /// Creates a new `GitDataSource`.
    #[must_use]
    pub const fn new(config: GitDataSourceConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for GitDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        let repo_dir = self
            .config
            .path
            .as_deref()
            .map_or_else(|| PathBuf::from("."), PathBuf::from);

        if !repo_dir.exists() {
            return Err(StampError::Execution(format!(
                "Repository path '{}' does not exist",
                repo_dir.display()
            )));
        }

        let git_bin = self.config.git_binary.as_deref().unwrap_or("git");

        let commit_output = tokio::process::Command::new(git_bin)
            .arg("-C")
            .arg(&repo_dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .await
            .map_err(|e| {
                StampError::Execution(format!("Failed to execute git rev-parse HEAD: {e}"))
            })?;

        let commit_sha = if commit_output.status.success() {
            String::from_utf8_lossy(&commit_output.stdout)
                .trim()
                .to_string()
        } else {
            "0000000000000000000000000000000000000000".to_string()
        };

        let abbrev_sha = if commit_sha.len() >= 7 {
            commit_sha[..7].to_string()
        } else {
            commit_sha.clone()
        };

        let branch_output = tokio::process::Command::new(git_bin)
            .arg("-C")
            .arg(&repo_dir)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .await;

        let branch = match branch_output {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            _ => "main".to_string(),
        };

        let status_output = tokio::process::Command::new(git_bin)
            .arg("-C")
            .arg(&repo_dir)
            .args(["status", "--porcelain"])
            .output()
            .await;

        let is_dirty = match status_output {
            Ok(out) if out.status.success() => !out.stdout.is_empty(),
            _ => false,
        };

        let tag_output = tokio::process::Command::new(git_bin)
            .arg("-C")
            .arg(&repo_dir)
            .args(["describe", "--tags", "--exact-match"])
            .output()
            .await;

        let tag = match tag_output {
            Ok(out) if out.status.success() => {
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            }
            _ => None,
        };

        Ok(json!({
            "commit": commit_sha,
            "abbreviated_commit": abbrev_sha,
            "branch": branch,
            "is_dirty": is_dirty,
            "tag": tag,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = GitDataSourceConfig {
            path: Some(".".to_string()),
            git_binary: None,
        };
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("GitDataSourceConfig"));
        let ds = GitDataSource::new(config);
        assert!(format!("{ds:?}").contains("GitDataSource"));
        assert_eq!(ds.clone().config.path.as_deref(), Some("."));
    }

    #[tokio::test]
    async fn test_git_data_source_current_repo() -> Result<(), StampError> {
        let ds = GitDataSource::new(GitDataSourceConfig {
            path: Some(".".to_string()),
            git_binary: None,
        });
        let res = ds.read().await?;
        assert!(res.is_object());
        assert!(res.get("commit").is_some());
        assert!(res.get("abbreviated_commit").is_some());
        assert!(res.get("branch").is_some());
        assert!(res.get("is_dirty").is_some());
        Ok(())
    }

    #[tokio::test]
    async fn test_git_data_source_default_path() -> Result<(), StampError> {
        let ds = GitDataSource::new(GitDataSourceConfig {
            path: None,
            git_binary: None,
        });
        let res = ds.read().await?;
        assert!(res.is_object());
        Ok(())
    }

    #[tokio::test]
    async fn test_git_data_source_non_git_dir() -> Result<(), StampError> {
        let temp = tempfile::tempdir()?;
        let ds = GitDataSource::new(GitDataSourceConfig {
            path: Some(temp.path().to_string_lossy().to_string()),
            git_binary: None,
        });
        let res = ds.read().await?;
        assert!(res.is_object());
        assert_eq!(res["commit"], "0000000000000000000000000000000000000000");

        // Also test short commit SHA via `echo` as git binary
        let ds_short = GitDataSource::new(GitDataSourceConfig {
            path: Some(temp.path().to_string_lossy().to_string()),
            git_binary: Some("echo".to_string()),
        });
        let res_short = ds_short.read().await?;
        assert!(res_short["abbreviated_commit"].is_string());
        Ok(())
    }

    #[tokio::test]
    async fn test_git_data_source_nonexistent_path() {
        let ds = GitDataSource::new(GitDataSourceConfig {
            path: Some("/nonexistent/git/repo/path/12345".to_string()),
            git_binary: None,
        });
        assert!(ds.read().await.is_err());
    }

    #[tokio::test]
    async fn test_git_data_source_spawn_error() {
        let ds = GitDataSource::new(GitDataSourceConfig {
            path: Some(".".to_string()),
            git_binary: Some("/nonexistent/path/to/git/binary".to_string()),
        });
        assert!(ds.read().await.is_err());
    }
}
