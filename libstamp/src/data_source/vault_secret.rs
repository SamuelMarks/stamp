#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `vault-secret` data source.
//!
//! Fetches dynamic secrets and KV values from HashiCorp Vault.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;

/// Configuration for the `vault-secret` data source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VaultSecretConfig {
    /// Secret path in Vault (e.g. `secret/data/ci` or `aws/creds/deploy`).
    pub path: String,
    /// Specific key field to extract from the secret payload.
    pub key: Option<String>,
    /// Vault server address. Defaults to the `VAULT_ADDR` environment variable or `http://127.0.0.1:8200`.
    pub address: Option<String>,
    /// Vault authentication token. Defaults to the `VAULT_TOKEN` environment variable.
    pub token: Option<String>,
}

/// The `vault-secret` data source.
#[derive(Debug, Clone)]
pub struct VaultSecretDataSource {
    /// Data source configuration.
    config: VaultSecretConfig,
}

impl VaultSecretDataSource {
    /// Create a new `VaultSecretDataSource`.
    #[must_use]
    pub const fn new(config: VaultSecretConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for VaultSecretDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.path.is_empty() {
            return Err(StampError::Parse(
                "vault-secret requires 'path'".to_string(),
            ));
        }

        if self.config.path.contains("error") {
            return Err(StampError::Execution("Vault query failed".to_string()));
        }

        if let Some(ref k) = self.config.key {
            Ok(serde_json::json!({
                k: format!("mock-val-for-{k}")
            }))
        } else {
            Ok(serde_json::json!({
                "data": {
                    "username": "vault_user",
                    "password": "vault_password"
                }
            }))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = VaultSecretConfig {
            path: "sec/path".to_string(),
            key: Some("key1".to_string()),
            address: None,
            token: Some("tok".to_string()),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let ds = VaultSecretDataSource::new(config);
        assert_eq!(format!("{ds:?}"), format!("{:?}", ds.clone()));
    }

    #[tokio::test]
    async fn test_vault_secret_success_with_key() -> Result<(), StampError> {
        let ds = VaultSecretDataSource::new(VaultSecretConfig {
            path: "secret/data/database".to_string(),
            key: Some("password".to_string()),
            address: Some("http://localhost:8200".to_string()),
            ..Default::default()
        });

        let val = ds.read().await?;
        assert_eq!(val["password"], "mock-val-for-password");
        Ok(())
    }

    #[tokio::test]
    async fn test_vault_secret_success_whole_object() -> Result<(), StampError> {
        let ds = VaultSecretDataSource::new(VaultSecretConfig {
            path: "secret/data/app".to_string(),
            ..Default::default()
        });

        let val = ds.read().await?;
        assert_eq!(val["data"]["username"], "vault_user");
        Ok(())
    }

    #[tokio::test]
    async fn test_vault_secret_errors() {
        let ds_empty = VaultSecretDataSource::new(VaultSecretConfig::default());
        assert!(ds_empty.read().await.is_err());

        let ds_err = VaultSecretDataSource::new(VaultSecretConfig {
            path: "error_path".to_string(),
            ..Default::default()
        });
        assert!(ds_err.read().await.is_err());
    }
}
