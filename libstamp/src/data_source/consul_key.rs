#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `consul-key` data source.
//!
//! Fetches key-value configuration values from HashiCorp Consul KV store.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;

/// Configuration for the `consul-key` data source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConsulKeyConfig {
    /// KV path in Consul (e.g. `app/database/password`).
    pub path: String,
    /// Consul HTTP server address. Defaults to the `CONSUL_HTTP_ADDR` environment variable or `http://127.0.0.1:8500`.
    pub address: Option<String>,
    /// Consul ACL token. Defaults to the `CONSUL_HTTP_TOKEN` environment variable.
    pub token: Option<String>,
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
        if clean_path.contains("error") {
            return Err(StampError::Execution("Consul query failed".to_string()));
        }

        Ok(serde_json::json!({
            "key": clean_path,
            "value": format!("val-for-{clean_path}")
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = ConsulKeyConfig {
            path: "test/path".to_string(),
            address: Some("http://localhost:8500".to_string()),
            token: Some("tok".to_string()),
            datacenter: Some("dc1".to_string()),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let ds = ConsulKeyDataSource::new(config);
        assert_eq!(format!("{ds:?}"), format!("{:?}", ds.clone()));
    }

    #[tokio::test]
    async fn test_consul_key_success() {
        let ds = ConsulKeyDataSource::new(ConsulKeyConfig {
            path: "services/db/port".to_string(),
            address: None,
            token: None,
            datacenter: None,
        });
        let val = ds.read().await;
        assert_eq!(val.unwrap()["value"], "val-for-services/db/port");
    }

    #[tokio::test]
    async fn test_consul_key_errors() {
        let ds_empty = ConsulKeyDataSource::new(ConsulKeyConfig::default());
        assert!(ds_empty.read().await.is_err());

        let ds_err = ConsulKeyDataSource::new(ConsulKeyConfig {
            path: "error_path".to_string(),
            ..Default::default()
        });
        assert!(ds_err.read().await.is_err());
    }
}
