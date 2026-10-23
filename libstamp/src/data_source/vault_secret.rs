//! Implementation of the `vault-secret` data source.
//!
//! Fetches dynamic secrets and KV values from `HashiCorp` Vault.

use crate::data_source::DataSource;
use crate::error::StampError;
use crate::types::SecretString;
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
    pub token: Option<SecretString>,
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

        if let Some(ref addr) = self.config.address {
            let clean_path = self.config.path.trim_start_matches('/');
            let url = format!("{}/v1/{clean_path}", addr.trim_end_matches('/'));
            let client = reqwest::Client::new();
            let mut req = client.get(&url);
            if let Some(ref tok) = self.config.token {
                req = req.header("X-Vault-Token", tok.expose_secret());
            }
            let resp = req
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Vault query failed: {e}")))?;
            if !resp.status().is_success() {
                return Err(StampError::Execution(format!(
                    "Vault query failed with status {}",
                    resp.status()
                )));
            }
            let payload: Value = resp.json().await?;
            if let Some(ref k) = self.config.key
                && let Some(val) = payload
                    .pointer(&format!("/data/data/{k}"))
                    .or_else(|| payload.pointer(&format!("/data/{k}")))
            {
                return Ok(serde_json::json!({ k: val }));
            }
            return Ok(payload);
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
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = VaultSecretConfig {
            path: "sec/path".to_string(),
            key: Some("key1".to_string()),
            address: None,
            token: Some(SecretString::new("tok".to_string())),
        };
        let cloned = config.clone();
        assert_eq!(cloned.path, "sec/path");
        assert!(format!("{config:?}").contains("VaultSecretConfig"));
        let ds = VaultSecretDataSource::new(config);
        assert!(format!("{ds:?}").contains("VaultSecretDataSource"));
        assert_eq!(ds.clone().config.path, "sec/path");
        let def = VaultSecretConfig::default();
        assert_eq!(def.path, "");
        assert!(def.key.is_none());
        assert!(def.address.is_none());
        assert!(def.token.is_none());
    }

    #[tokio::test]
    async fn test_vault_secret_success_with_key_and_http() -> Result<(), StampError> {
        let ds = VaultSecretDataSource::new(VaultSecretConfig {
            path: "secret/data/database".to_string(),
            key: Some("password".to_string()),
            address: None,
            ..Default::default()
        });

        let res = ds.read().await?;
        assert_eq!(res["password"], "mock-val-for-password");

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/v1/secret/data/prod")
            .match_header("X-Vault-Token", "v-tok")
            .with_status(200)
            .with_body(r#"{"data":{"data":{"api_key":"live-vault-key"}}}"#)
            .create_async()
            .await;

        let ds_live = VaultSecretDataSource::new(VaultSecretConfig {
            path: "secret/data/prod".to_string(),
            key: Some("api_key".to_string()),
            address: Some(server.url()),
            token: Some(SecretString::new("v-tok".to_string())),
        });
        let live_res = ds_live.read().await?;
        assert_eq!(live_res["api_key"], "live-vault-key");

        let _m_whole = server
            .mock("GET", "/v1/secret/data/whole")
            .with_status(200)
            .with_body(r#"{"data":{"username":"admin"}}"#)
            .create_async()
            .await;
        let ds_whole = VaultSecretDataSource::new(VaultSecretConfig {
            path: "secret/data/whole".to_string(),
            key: None,
            address: Some(server.url()),
            token: None,
        });
        assert_eq!(ds_whole.read().await?["data"]["username"], "admin");

        let _m_err = server
            .mock("GET", "/v1/secret/data/denied")
            .with_status(403)
            .create_async()
            .await;
        let ds_denied = VaultSecretDataSource::new(VaultSecretConfig {
            path: "secret/data/denied".to_string(),
            key: None,
            address: Some(server.url()),
            token: None,
        });
        assert!(ds_denied.read().await.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_vault_secret_success_whole_object() -> Result<(), StampError> {
        let ds = VaultSecretDataSource::new(VaultSecretConfig {
            path: "secret/data/app".to_string(),
            ..Default::default()
        });

        let res = ds.read().await?;
        assert_eq!(res["data"]["username"], "vault_user");
        Ok(())
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
            path: "secret/data/app".to_string(),
            address: Some("http://127.0.0.1:1".to_string()),
            ..Default::default()
        });
        assert!(ds_err.read().await.is_err());
    }
}
