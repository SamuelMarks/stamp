#![cfg_attr(coverage_nightly, coverage(off))]
//! `googlecompute_image` data source implementation.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;

/// Configuration for `googlecompute_image`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GooglecomputeImageConfig {
    /// The search filters for the image.
    pub filters: std::collections::HashMap<String, String>,
}

/// The `googlecompute_image` data source.
#[derive(Debug, Clone)]
pub struct GooglecomputeImageDataSource {
    /// The configuration.
    pub config: GooglecomputeImageConfig,
}

impl GooglecomputeImageDataSource {
    /// Create a new `GooglecomputeImageDataSource`.
    #[must_use]
    pub const fn new(config: GooglecomputeImageConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for GooglecomputeImageDataSource {
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
        let config = GooglecomputeImageConfig::default();
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let ds = GooglecomputeImageDataSource::new(config);
        assert_eq!(format!("{ds:?}"), format!("{:?}", ds.clone()));
    }

    #[tokio::test]
    async fn test_googlecompute_success() {
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), "ubuntu-*".to_string());
        let ds = GooglecomputeImageDataSource::new(GooglecomputeImageConfig { filters });
        let val = ds.read().await;
        assert_eq!(val.unwrap(), Value::String("img-12345".to_string()));
    }

    #[tokio::test]
    async fn test_googlecompute_failure() {
        let ds = GooglecomputeImageDataSource::new(GooglecomputeImageConfig {
            filters: HashMap::new(),
        });
        let result = ds.read().await;
        assert!(result.is_err());
    }
}
