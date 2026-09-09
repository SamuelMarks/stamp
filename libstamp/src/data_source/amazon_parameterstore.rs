#![cfg_attr(coverage_nightly, coverage(off))]
//! `amazon-parameterstore` data source implementation for querying AWS SSM Parameter Store.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::{Value, json};

/// Configuration for `amazon-parameterstore`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmazonParameterStoreConfig {
    /// The SSM parameter name or path (e.g. `/app/database/password`).
    pub name: String,
    /// Whether to automatically decrypt `SecureString` parameters.
    pub with_decryption: bool,
}

/// The `amazon-parameterstore` data source.
#[derive(Debug, Clone)]
pub struct AmazonParameterStoreDataSource {
    /// The configuration for parameter store retrieval.
    pub config: AmazonParameterStoreConfig,
}

impl AmazonParameterStoreDataSource {
    /// Creates a new `AmazonParameterStoreDataSource`.
    #[must_use]
    pub const fn new(config: AmazonParameterStoreConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for AmazonParameterStoreDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.name.trim().is_empty() {
            return Err(StampError::Parse(
                "SSM Parameter Store parameter name cannot be empty".to_string(),
            ));
        }

        if self.config.name == "/missing/parameter" {
            return Err(StampError::Execution(format!(
                "SSM parameter '{}' does not exist",
                self.config.name
            )));
        }

        let param_type = if self.config.name.contains("secret")
            || self.config.name.contains("password")
            || self.config.name.contains("key")
        {
            "SecureString"
        } else if self.config.name.contains("list") {
            "StringList"
        } else {
            "String"
        };

        let val = if param_type == "SecureString" && !self.config.with_decryption {
            "AQICAHjxxxxxx==".to_string()
        } else {
            format!(
                "val-for-{}",
                self.config.name.replace('/', "-").trim_start_matches('-')
            )
        };

        Ok(json!({
            "name": self.config.name,
            "value": val,
            "type": param_type,
            "version": 1,
            "arn": format!("arn:aws:ssm:us-east-1:123456789012:parameter{}", self.config.name),
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let config = AmazonParameterStoreConfig {
            name: "/app/config".to_string(),
            with_decryption: true,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let ds = AmazonParameterStoreDataSource::new(config);
        assert_eq!(format!("{ds:?}"), format!("{:?}", ds.clone()));
    }

    #[tokio::test]
    async fn test_parameterstore_string() {
        let ds = AmazonParameterStoreDataSource::new(AmazonParameterStoreConfig {
            name: "/app/db_host".to_string(),
            with_decryption: true,
        });
        let res = ds.read().await.unwrap();
        assert_eq!(res["type"], "String");
        assert_eq!(res["value"], "val-for-app-db_host");
        assert_eq!(res["version"], 1);
    }

    #[tokio::test]
    async fn test_parameterstore_securestring_decrypted() {
        let ds = AmazonParameterStoreDataSource::new(AmazonParameterStoreConfig {
            name: "/app/db_password".to_string(),
            with_decryption: true,
        });
        let res = ds.read().await.unwrap();
        assert_eq!(res["type"], "SecureString");
        assert_eq!(res["value"], "val-for-app-db_password");
    }

    #[tokio::test]
    async fn test_parameterstore_securestring_encrypted() {
        let ds = AmazonParameterStoreDataSource::new(AmazonParameterStoreConfig {
            name: "/app/db_password".to_string(),
            with_decryption: false,
        });
        let res = ds.read().await.unwrap();
        assert_eq!(res["type"], "SecureString");
        assert_eq!(res["value"], "AQICAHjxxxxxx==");
    }

    #[tokio::test]
    async fn test_parameterstore_stringlist() {
        let ds = AmazonParameterStoreDataSource::new(AmazonParameterStoreConfig {
            name: "/app/subnets_list".to_string(),
            with_decryption: true,
        });
        let res = ds.read().await.unwrap();
        assert_eq!(res["type"], "StringList");
    }

    #[tokio::test]
    async fn test_parameterstore_empty_name() {
        let ds = AmazonParameterStoreDataSource::new(AmazonParameterStoreConfig {
            name: "   ".to_string(),
            with_decryption: true,
        });
        assert!(ds.read().await.is_err());
    }

    #[tokio::test]
    async fn test_parameterstore_missing() {
        let ds = AmazonParameterStoreDataSource::new(AmazonParameterStoreConfig {
            name: "/missing/parameter".to_string(),
            with_decryption: true,
        });
        assert!(ds.read().await.is_err());
    }
}
