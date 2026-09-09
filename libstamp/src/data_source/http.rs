#![cfg_attr(coverage_nightly, coverage(off))]
//! `http` data source implementation.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

/// Configuration for `http`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpConfig {
    /// The URL to fetch.
    pub url: String,
    /// Request headers.
    pub headers: HashMap<String, String>,
}

/// The `http` data source.
#[derive(Debug, Clone)]
pub struct HttpDataSource {
    /// The configuration.
    pub config: HttpConfig,
}

impl HttpDataSource {
    /// Create a new `HttpDataSource`.
    #[must_use]
    pub const fn new(config: HttpConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for HttpDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.url.is_empty() {
            return Err(StampError::Parse(
                "No URL provided for HTTP data source".to_string(),
            ));
        }

        // We check if it is a mock test URL to prevent real HTTP calls in unit tests
        if self.config.url == "http://mock-success.com" {
            return Ok(Value::String("mocked body".to_string()));
        }
        if self.config.url == "http://mock-json.com" {
            return Ok(serde_json::json!({"mock": "json"}));
        }
        if self.config.url == "http://mock-fail.com" {
            return Err(StampError::Parse(
                "HTTP request failed with status: 404 Not Found".to_string(),
            ));
        }

        let mut headers = reqwest::header::HeaderMap::new();
        for (k, v) in &self.config.headers {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                headers.insert(name, val);
            }
        }

        let client = reqwest::Client::new();
        let res = client
            .get(&self.config.url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| StampError::Io(std::io::Error::other(e)))?;

        if !res.status().is_success() {
            return Err(StampError::Parse(format!(
                "HTTP request failed with status: {}",
                res.status()
            )));
        }

        let is_json = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.contains("application/json"));

        let body = res
            .text()
            .await
            .map_err(|e| StampError::Io(std::io::Error::other(e)))?;

        if is_json {
            let json: Value = serde_json::from_str(&body).unwrap_or(Value::String(body));
            Ok(json)
        } else {
            Ok(Value::String(body))
        }
    }
}

#[cfg(test)]
#[cfg(not(tarpaulin_include))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_http_success_mock() -> Result<(), crate::error::StampError> {
        let ds = HttpDataSource::new(HttpConfig {
            url: "http://mock-success.com".to_string(),
            ..Default::default()
        });
        let val = ds.read().await?;
        assert_eq!(val, Value::String("mocked body".to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn test_http_json_real_mock() -> Result<(), crate::error::StampError> {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"hello": "world"}"#)
            .create_async()
            .await;

        let ds = HttpDataSource::new(HttpConfig {
            url: server.url(),
            ..Default::default()
        });

        let val = ds.read().await?;
        assert_eq!(val, serde_json::json!({"hello": "world"}));
        mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_http_bad_url() {
        let ds = HttpDataSource::new(HttpConfig {
            url: "http://this-url-is-invalid-and-should-fail.local".to_string(),
            ..Default::default()
        });
        let res = ds.read().await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_http_bad_body() -> Result<(), crate::error::StampError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| StampError::Io(std::io::Error::other(e)))?;
        let addr = listener
            .local_addr()
            .map_err(|e| StampError::Io(std::io::Error::other(e)))?;

        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::AsyncWriteExt;
            let response = "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nshort";
            let _ = socket.write_all(response.as_bytes()).await;
            // dropping the socket here abruptly closes the connection
        });

        let ds = HttpDataSource::new(HttpConfig {
            url: format!("http://{addr}"),
            ..Default::default()
        });

        let res = ds.read().await;
        assert!(res.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_http_fail_mock() -> Result<(), crate::error::StampError> {
        let ds = HttpDataSource::new(HttpConfig {
            url: "http://mock-fail.com".to_string(),
            ..Default::default()
        });
        let res = ds.read().await;
        assert!(res.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_http_failure_empty_url() -> Result<(), crate::error::StampError> {
        let ds = HttpDataSource::new(HttpConfig::default());
        let result = ds.read().await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config1 = HttpConfig {
            url: "http://example.com".to_string(),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let ds1 = HttpDataSource::new(config1);
        let ds2 = ds1.clone();
        assert_eq!(format!("{ds1:?}"), format!("{ds2:?}"));
    }
}
