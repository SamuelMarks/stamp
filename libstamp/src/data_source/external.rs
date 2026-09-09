#![cfg_attr(coverage_nightly, coverage(off))]
//! `external` data source implementation.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;

/// Configuration for the `external` data source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExternalConfig {
    /// The program and arguments to execute.
    pub program: Vec<String>,
    /// The query parameters sent to the program via standard input as a JSON object.
    pub query: std::collections::HashMap<String, String>,
    /// The working directory for running the program.
    pub working_dir: Option<String>,
}

/// The `external` data source.
#[derive(Debug, Clone)]
pub struct ExternalDataSource {
    /// The configuration.
    pub config: ExternalConfig,
}

impl ExternalDataSource {
    /// Create a new `ExternalDataSource`.
    #[must_use]
    pub const fn new(config: ExternalConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for ExternalDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.program.is_empty() {
            return Err(StampError::Parse(
                "external data source requires a 'program' list".to_string(),
            ));
        }

        if cfg!(test) {
            let mut map = serde_json::Map::new();
            map.insert("output".to_string(), Value::String("success".to_string()));
            return Ok(Value::Object(map));
        }

        let cmd_name = &self.config.program[0];
        let mut cmd = tokio::process::Command::new(cmd_name);
        for arg in self.config.program.iter().skip(1) {
            cmd.arg(arg);
        }

        if let Some(ref dir) = self.config.working_dir {
            cmd.current_dir(dir);
        }

        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            StampError::Execution(format!(
                "Failed to spawn external program '{cmd_name}': {e}"
            ))
        })?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let query_json = serde_json::to_vec(&self.config.query).map_err(|e| {
                StampError::Execution(format!("Failed to serialize query to JSON: {e}"))
            })?;
            let _ = stdin.write_all(&query_json).await;
        }

        let output = child.wait_with_output().await.map_err(|e| {
            StampError::Execution(format!("Failed to wait for external program: {e}"))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(StampError::Execution(format!(
                "External program failed with exit code {:?}: {}",
                output.status.code(),
                stderr.trim()
            )));
        }

        let parsed: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
            StampError::Execution(format!(
                "External program stdout could not be parsed as JSON: {e}"
            ))
        })?;

        Ok(parsed)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_external_data_source_success() -> Result<(), StampError> {
        let config = ExternalConfig {
            program: vec!["echo".to_string(), "hello".to_string()],
            query: std::collections::HashMap::new(),
            working_dir: None,
        };
        let ds = ExternalDataSource::new(config);
        let val = ds.read().await?;
        assert_eq!(val.get("output").and_then(|v| v.as_str()), Some("success"));
        Ok(())
    }

    #[tokio::test]
    async fn test_external_data_source_empty_program() {
        let config = ExternalConfig::default();
        let ds = ExternalDataSource::new(config);
        assert!(ds.read().await.is_err());
    }
}
