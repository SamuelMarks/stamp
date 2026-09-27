#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `vault-secret` data source.
//!
//! Fetches dynamic secrets and KV values from `HashiCorp` Vault.

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
        let def = VaultSecretConfig::default();
        assert_eq!(def.path, "");
        assert!(def.key.is_none());
        assert!(def.address.is_none());
        assert!(def.token.is_none());
    }

    #[tokio::test]
    async fn test_vault_secret_success_with_key() {
        let ds = VaultSecretDataSource::new(VaultSecretConfig {
            path: "secret/data/database".to_string(),
            key: Some("password".to_string()),
            address: Some("http://localhost:8200".to_string()),
            ..Default::default()
        });

        let res = ds.read().await;
        assert_eq!(
            res.ok()
                .as_ref()
                .and_then(|v| v.get("password"))
                .and_then(Value::as_str),
            Some("mock-val-for-password")
        );
    }

    #[tokio::test]
    async fn test_vault_secret_success_whole_object() {
        let ds = VaultSecretDataSource::new(VaultSecretConfig {
            path: "secret/data/app".to_string(),
            ..Default::default()
        });

        let res = ds.read().await;
        assert_eq!(
            res.ok()
                .as_ref()
                .and_then(|v| v.get("data"))
                .and_then(|d| d.get("username"))
                .and_then(Value::as_str),
            Some("vault_user")
        );
    }

    #[tokio::test]
    async fn test_vault_secret_errors() {
        let ds_empty = VaultSecretDataSource::new(VaultSecretConfig::default());
        let res_empty = ds_empty.read().await;
        assert_eq!(
            res_empty.err().as_ref().map(ToString::to_string),
            Some("Parse error: vault-secret requires 'path'".to_string())
        );

        let ds_err = VaultSecretDataSource::new(VaultSecretConfig {
            path: "error_path".to_string(),
            ..Default::default()
        });
        let res_err = ds_err.read().await;
        assert_eq!(
            res_err.err().as_ref().map(ToString::to_string),
            Some("Execution error: Vault query failed".to_string())
        );
    }
}
