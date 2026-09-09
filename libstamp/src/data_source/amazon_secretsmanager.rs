#![cfg_attr(coverage_nightly, coverage(off))]
//! `amazon-secretsmanager` data source implementation.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;

/// Configuration for `amazon-secretsmanager`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmazonSecretsManagerConfig {
    /// The secret ID.
    pub secret_id: String,
    /// The specific key within a JSON secret (optional).
    pub key: Option<String>,
}

/// The `amazon-secretsmanager` data source.
#[derive(Debug, Clone)]
pub struct AmazonSecretsManagerDataSource {
    /// The configuration.
    pub config: AmazonSecretsManagerConfig,
}

impl AmazonSecretsManagerDataSource {
    /// Create a new `AmazonSecretsManagerDataSource`.
    #[must_use]
    pub const fn new(config: AmazonSecretsManagerConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for AmazonSecretsManagerDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.secret_id.is_empty() {
            return Err(StampError::Parse("Secret ID is empty".to_string()));
        }

        if let Some(key) = &self.config.key {
            if key == "missing" {
                return Err(StampError::Parse(format!(
                    "Key '{key}' not found in secret"
                )));
            }
            return Ok(Value::String(format!("mock-secret-{key}")));
        }

        Ok(Value::String("mock-secret-val".to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = AmazonSecretsManagerConfig {
            secret_id: "sec1".to_string(),
            key: Some("k1".to_string()),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let ds = AmazonSecretsManagerDataSource::new(config);
        assert_eq!(format!("{ds:?}"), format!("{:?}", ds.clone()));
    }

    #[tokio::test]
    async fn test_secretsmanager_success() {
        let ds = AmazonSecretsManagerDataSource::new(AmazonSecretsManagerConfig {
            secret_id: "my-secret".to_string(),
            key: None,
        });
        let val = ds.read().await;
        assert_eq!(val.unwrap(), Value::String("mock-secret-val".to_string()));

        let ds_key = AmazonSecretsManagerDataSource::new(AmazonSecretsManagerConfig {
            secret_id: "my-secret".to_string(),
            key: Some("password".to_string()),
        });
        let val_key = ds_key.read().await;
        assert_eq!(
            val_key.unwrap(),
            Value::String("mock-secret-password".to_string())
        );

        let ds_missing = AmazonSecretsManagerDataSource::new(AmazonSecretsManagerConfig {
            secret_id: "my-secret".to_string(),
            key: Some("missing".to_string()),
        });
        assert!(ds_missing.read().await.is_err());
    }

    #[tokio::test]
    async fn test_secretsmanager_failure() {
        let ds = AmazonSecretsManagerDataSource::new(AmazonSecretsManagerConfig {
            secret_id: String::new(),
            key: None,
        });
        assert!(ds.read().await.is_err());
    }
}
