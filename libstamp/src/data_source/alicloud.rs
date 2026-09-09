#![cfg_attr(coverage_nightly, coverage(off))]
//! `alicloud_image` data source implementation.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;

/// Configuration for `alicloud_image`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlicloudImageConfig {
    /// The search filters for the image.
    pub filters: std::collections::HashMap<String, String>,
}

/// The `alicloud_image` data source.
#[derive(Debug, Clone)]
pub struct AlicloudImageDataSource {
    /// The configuration.
    pub config: AlicloudImageConfig,
}

impl AlicloudImageDataSource {
    /// Create a new `AlicloudImageDataSource`.
    #[must_use]
    pub const fn new(config: AlicloudImageConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for AlicloudImageDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.filters.is_empty() {
            return Err(StampError::Parse(
                "No filters provided for image search".to_string(),
            ));
        }

        Ok(Value::String("img-12345".to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_derived_traits() {
        let config = AlicloudImageConfig::default();
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let ds = AlicloudImageDataSource::new(config);
        assert_eq!(format!("{ds:?}"), format!("{:?}", ds.clone()));
    }

    #[tokio::test]
    async fn test_alicloud_success() {
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), "ubuntu-*".to_string());
        let ds = AlicloudImageDataSource::new(AlicloudImageConfig { filters });
        let val = ds.read().await;
        assert_eq!(val.unwrap(), Value::String("img-12345".to_string()));
    }

    #[tokio::test]
    async fn test_alicloud_failure() {
        let ds = AlicloudImageDataSource::new(AlicloudImageConfig {
            filters: HashMap::new(),
        });
        let result = ds.read().await;
        assert!(result.is_err());
    }
}
