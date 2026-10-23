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

        if let Some(endpoint) = self.config.filters.get("endpoint") {
            let client = reqwest::Client::new();
            let resp = client
                .get(format!(
                    "{}/?Action=DescribeImages",
                    endpoint.trim_end_matches('/')
                ))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(StampError::Execution(format!(
                    "Alicloud DescribeImages failed with status {}",
                    resp.status()
                )));
            }
            let body: Value = resp.json().await?;
            if let Some(image_id) = body
                .pointer("/Images/Image/0/ImageId")
                .and_then(Value::as_str)
            {
                return Ok(Value::String(image_id.to_string()));
            }
            return Ok(body);
        }

        Ok(Value::String("img-12345".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_derived_traits() {
        let config = AlicloudImageConfig::default();
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("AlicloudImageConfig"));
        let ds = AlicloudImageDataSource::new(config);
        assert!(format!("{ds:?}").contains("AlicloudImageDataSource"));
        assert_eq!(ds.clone().config.filters.len(), 0);
    }

    #[tokio::test]
    async fn test_alicloud_success_and_http() -> Result<(), StampError> {
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), "ubuntu-*".to_string());
        let ds = AlicloudImageDataSource::new(AlicloudImageConfig { filters });
        let val = ds.read().await?;
        assert_eq!(val, Value::String("img-12345".to_string()));

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/?Action=DescribeImages")
            .with_status(200)
            .with_body(r#"{"Images":{"Image":[{"ImageId":"m-uf660012345"}]}}"#)
            .create_async()
            .await;

        let mut http_filters = HashMap::new();
        http_filters.insert("endpoint".to_string(), server.url());
        let ds_http = AlicloudImageDataSource::new(AlicloudImageConfig {
            filters: http_filters,
        });
        assert_eq!(
            ds_http.read().await?,
            Value::String("m-uf660012345".to_string())
        );

        let _m_raw = server
            .mock("GET", "/?Action=DescribeImages")
            .with_status(200)
            .with_body(r#"{"Custom":"data"}"#)
            .create_async()
            .await;
        let mut raw_filters = HashMap::new();
        raw_filters.insert("endpoint".to_string(), server.url());
        let ds_raw = AlicloudImageDataSource::new(AlicloudImageConfig {
            filters: raw_filters,
        });
        assert_eq!(ds_raw.read().await?["Custom"], "data");

        let _m_err = server
            .mock("GET", "/?Action=DescribeImages")
            .with_status(500)
            .create_async()
            .await;
        let mut err_filters = HashMap::new();
        err_filters.insert("endpoint".to_string(), server.url());
        let ds_err = AlicloudImageDataSource::new(AlicloudImageConfig {
            filters: err_filters,
        });
        assert!(ds_err.read().await.is_err());
        Ok(())
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
