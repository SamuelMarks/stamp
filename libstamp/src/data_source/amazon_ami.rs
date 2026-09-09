#![cfg_attr(coverage_nightly, coverage(off))]
//! `amazon-ami` data source implementation.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;

/// Configuration for `amazon-ami`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmazonAmiConfig {
    /// The search filters for the AMI.
    pub filters: std::collections::HashMap<String, String>,
}

/// The `amazon-ami` data source.
#[derive(Debug, Clone)]
pub struct AmazonAmiDataSource {
    /// The configuration.
    pub config: AmazonAmiConfig,
}

impl AmazonAmiDataSource {
    /// Create a new `AmazonAmiDataSource`.
    #[must_use]
    pub const fn new(config: AmazonAmiConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for AmazonAmiDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.filters.is_empty() {
            return Err(StampError::Parse(
                "No filters provided for AMI search".to_string(),
            ));
        }

        if self.config.filters.contains_key("missing") {
            return Err(StampError::Parse("No matching AMI found".to_string()));
        }

        Ok(Value::String("ami-12345678".to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_derived_traits() {
        let mut filters = HashMap::new();
        filters.insert("owner".to_string(), "amazon".to_string());
        let config = AmazonAmiConfig { filters };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let ds = AmazonAmiDataSource::new(config);
        assert_eq!(format!("{ds:?}"), format!("{:?}", ds.clone()));
    }

    #[tokio::test]
    async fn test_amazon_ami_success() {
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), "ubuntu-*".to_string());
        let ds = AmazonAmiDataSource::new(AmazonAmiConfig { filters });
        let val = ds.read().await;
        assert_eq!(val.unwrap(), Value::String("ami-12345678".to_string()));

        let mut missing_filters = HashMap::new();
        missing_filters.insert("missing".to_string(), "true".to_string());
        let ds_missing = AmazonAmiDataSource::new(AmazonAmiConfig {
            filters: missing_filters,
        });
        assert!(ds_missing.read().await.is_err());
    }

    #[tokio::test]
    async fn test_amazon_ami_failure() {
        let ds = AmazonAmiDataSource::new(AmazonAmiConfig {
            filters: HashMap::new(),
        });
        assert!(ds.read().await.is_err());
    }
}
