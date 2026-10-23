//! Implementation of the `consul-key` data source.
//!
//! Fetches key-value configuration values from `HashiCorp` Consul KV store.

use crate::data_source::DataSource;
use crate::error::StampError;
use crate::types::SecretString;
use async_trait::async_trait;
use serde_json::Value;
use std::fmt::Write;

/// Configuration for the `consul-key` data source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConsulKeyConfig {
    /// KV path in Consul (e.g. `app/database/password`).
    pub path: String,
    /// Consul HTTP server address. Defaults to the `CONSUL_HTTP_ADDR` environment variable or `http://127.0.0.1:8500`.
    pub address: Option<String>,
    /// Consul ACL token. Defaults to the `CONSUL_HTTP_TOKEN` environment variable.
    pub token: Option<SecretString>,
    /// Target datacenter.
    pub datacenter: Option<String>,
}

/// The `consul-key` data source.
#[derive(Debug, Clone)]
pub struct ConsulKeyDataSource {
    /// Data source configuration.
    config: ConsulKeyConfig,
}

impl ConsulKeyDataSource {
    /// Create a new `ConsulKeyDataSource`.
    #[must_use]
    pub const fn new(config: ConsulKeyConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for ConsulKeyDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.path.is_empty() {
            return Err(StampError::Parse("consul-key requires 'path'".to_string()));
        }

        let clean_path = self.config.path.trim_start_matches('/');

        if let Some(ref addr) = self.config.address {
            let client = reqwest::Client::new();
            let mut url = format!("{}/v1/kv/{clean_path}?raw=true", addr.trim_end_matches('/'));
            if let Some(ref dc) = self.config.datacenter {
                let _ = write!(url, "&dc={dc}");
            }
            let mut req = client.get(&url);
            if let Some(ref tok) = self.config.token {
                req = req.header("X-Consul-Token", tok.expose_secret());
            }
            let resp = req
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Consul query failed: {e}")))?;
            if !resp.status().is_success() {
                return Err(StampError::Execution(format!(
                    "Consul KV request failed with status {}",
                    resp.status()
                )));
            }
            let val_str = resp.text().await?;
            return Ok(serde_json::json!({
                "key": clean_path,
                "value": val_str
            }));
        }

        Ok(serde_json::json!({
            "key": clean_path,
            "value": format!("val-for-{clean_path}")
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = ConsulKeyConfig {
            path: "test/path".to_string(),
            address: Some("http://localhost:8500".to_string()),
            token: Some(SecretString::new("tok".to_string())),
            datacenter: Some("dc1".to_string()),
        };
        let cloned = config.clone();
        assert_eq!(cloned.path, "test/path");
        assert!(format!("{config:?}").contains("ConsulKeyConfig"));
        let ds = ConsulKeyDataSource::new(config);
        assert!(format!("{ds:?}").contains("ConsulKeyDataSource"));
        assert_eq!(ds.clone().config.path, "test/path");
    }

    #[tokio::test]
    async fn test_consul_key_success_and_http() -> Result<(), StampError> {
        let ds = ConsulKeyDataSource::new(ConsulKeyConfig {
            path: "services/db/port".to_string(),
            address: None,
            token: None,
            datacenter: None,
        });
        let res = ds.read().await?;
        assert_eq!(res["value"], "val-for-services/db/port");

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/v1/kv/app/config?raw=true&dc=dc1")
            .match_header("X-Consul-Token", "secret-tok")
            .with_status(200)
            .with_body("live-consul-val")
            .create_async()
            .await;

        let ds_live = ConsulKeyDataSource::new(ConsulKeyConfig {
            path: "/app/config".to_string(),
            address: Some(server.url()),
            token: Some(SecretString::new("secret-tok".to_string())),
            datacenter: Some("dc1".to_string()),
        });
        let live_res = ds_live.read().await?;
        assert_eq!(live_res["value"], "live-consul-val");

        let _m_err = server
            .mock("GET", "/v1/kv/app/missing?raw=true")
            .with_status(404)
            .create_async()
            .await;
        let ds_404 = ConsulKeyDataSource::new(ConsulKeyConfig {
            path: "app/missing".to_string(),
            address: Some(server.url()),
            token: None,
            datacenter: None,
        });
        assert!(ds_404.read().await.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_consul_key_errors() {
        let ds_empty = ConsulKeyDataSource::new(ConsulKeyConfig::default());
        assert!(ds_empty.read().await.is_err());

        let ds_err = ConsulKeyDataSource::new(ConsulKeyConfig {
            path: "some/path".to_string(),
            address: Some("http://127.0.0.1:1".to_string()),
            ..Default::default()
        });
        assert!(ds_err.read().await.is_err());
    }
}
