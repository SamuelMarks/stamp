#![cfg_attr(coverage_nightly, coverage(off))]
//! `terraform` data source implementation for ingesting Terraform state outputs.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

/// Configuration for the `terraform` data source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TerraformDataSourceConfig {
    /// Path to the local `terraform.tfstate` file.
    pub state_path: Option<String>,
    /// Target output variable name to extract (optional).
    pub output: Option<String>,
}

/// The `terraform` data source.
#[derive(Debug, Clone, Default)]
pub struct TerraformDataSource {
    /// Configuration for the Terraform data source.
    pub config: TerraformDataSourceConfig,
}

impl TerraformDataSource {
    /// Creates a new `TerraformDataSource`.
    #[must_use]
    pub const fn new(config: TerraformDataSourceConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for TerraformDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        let state_path = self
            .config
            .state_path
            .as_deref()
            .map_or_else(|| PathBuf::from("terraform.tfstate"), PathBuf::from);

        if !state_path.exists() {
            return Err(StampError::Execution(format!(
                "Terraform state file '{}' does not exist",
                state_path.display()
            )));
        }

        let bytes = tokio::fs::read(&state_path).await.map_err(|e| {
            StampError::Execution(format!(
                "Failed to read Terraform state file '{}': {e}",
                state_path.display()
            ))
        })?;

        let parsed: Value = serde_json::from_slice(&bytes).map_err(|e| {
            StampError::Parse(format!(
                "Failed to parse Terraform state JSON in '{}': {e}",
                state_path.display()
            ))
        })?;

        let outputs = parsed
            .get("outputs")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                StampError::Parse(format!(
                    "No 'outputs' block found in Terraform state file '{}'",
                    state_path.display()
                ))
            })?;

        let mut simplified_outputs = serde_json::Map::new();
        for (k, v) in outputs {
            if let Some(val) = v.get("value") {
                simplified_outputs.insert(k.clone(), val.clone());
            } else {
                simplified_outputs.insert(k.clone(), v.clone());
            }
        }

        if let Some(ref target_output) = self.config.output {
            if let Some(val) = simplified_outputs.get(target_output) {
                Ok(val.clone())
            } else {
                Err(StampError::Execution(format!(
                    "Output '{target_output}' not found in Terraform state file '{}'",
                    state_path.display()
                )))
            }
        } else {
            Ok(Value::Object(simplified_outputs))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = TerraformDataSourceConfig {
            state_path: Some("terraform.tfstate".to_string()),
            output: Some("vpc_id".to_string()),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let ds = TerraformDataSource::new(config);
        assert_eq!(format!("{ds:?}"), format!("{:?}", ds.clone()));
    }

    #[tokio::test]
    async fn test_terraform_data_source_all_outputs() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir();
        let state_file = temp_dir.join("test_terraform.tfstate");
        let state_content = r#"{
            "version": 4,
            "terraform_version": "1.5.0",
            "outputs": {
                "vpc_id": {
                    "value": "vpc-12345678",
                    "type": "string"
                },
                "subnet_ids": {
                    "value": ["subnet-1", "subnet-2"],
                    "type": ["list", "string"]
                }
            }
        }"#;
        tokio::fs::write(&state_file, state_content).await?;

        let ds = TerraformDataSource::new(TerraformDataSourceConfig {
            state_path: Some(state_file.to_string_lossy().to_string()),
            output: None,
        });

        let res = ds.read().await?;
        assert_eq!(res["vpc_id"], "vpc-12345678");
        assert_eq!(res["subnet_ids"][0], "subnet-1");

        let _ = tokio::fs::remove_file(state_file).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_terraform_data_source_specific_output() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp_dir = std::env::temp_dir();
        let state_file = temp_dir.join("test_terraform_specific.tfstate");
        let state_content = r#"{
            "outputs": {
                "ami_id": {
                    "value": "ami-99999999"
                }
            }
        }"#;
        tokio::fs::write(&state_file, state_content).await?;

        let ds = TerraformDataSource::new(TerraformDataSourceConfig {
            state_path: Some(state_file.to_string_lossy().to_string()),
            output: Some("ami_id".to_string()),
        });

        let res = ds.read().await?;
        assert_eq!(res, "ami-99999999");

        let ds_missing = TerraformDataSource::new(TerraformDataSourceConfig {
            state_path: Some(state_file.to_string_lossy().to_string()),
            output: Some("nonexistent".to_string()),
        });
        assert!(ds_missing.read().await.is_err());

        let _ = tokio::fs::remove_file(state_file).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_terraform_data_source_missing_file() {
        let ds = TerraformDataSource::new(TerraformDataSourceConfig {
            state_path: Some("/nonexistent/terraform/state/file.tfstate".to_string()),
            output: None,
        });
        assert!(ds.read().await.is_err());
    }

    #[tokio::test]
    async fn test_terraform_data_source_invalid_json() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir();
        let state_file = temp_dir.join("test_terraform_invalid.tfstate");
        tokio::fs::write(&state_file, "not json").await?;

        let ds = TerraformDataSource::new(TerraformDataSourceConfig {
            state_path: Some(state_file.to_string_lossy().to_string()),
            output: None,
        });
        assert!(ds.read().await.is_err());

        let _ = tokio::fs::remove_file(state_file).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_terraform_data_source_no_outputs_block() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp_dir = std::env::temp_dir();
        let state_file = temp_dir.join("test_terraform_no_outputs.tfstate");
        tokio::fs::write(&state_file, "{\"version\": 4}").await?;

        let ds = TerraformDataSource::new(TerraformDataSourceConfig {
            state_path: Some(state_file.to_string_lossy().to_string()),
            output: None,
        });
        assert!(ds.read().await.is_err());

        let _ = tokio::fs::remove_file(state_file).await;
        Ok(())
    }
}
