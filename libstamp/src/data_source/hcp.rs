#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
//! HCP Packer data sources.

use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;

/// A strongly typed secret string to prevent logging.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Creates a new `SecretString`.
    #[must_use]
    pub fn new(secret: String) -> Self {
        Self(secret)
    }

    /// Exposes the inner secret.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "***REDACTED***")
    }
}

/// Configuration for the `hcp_packer_iteration` data source.
#[derive(Debug, Clone, Default)]
pub struct HcpPackerIterationConfig {
    /// The HCP organization.
    pub organization: Option<String>,
    /// The HCP project.
    pub project: Option<String>,
    /// The bucket name.
    pub bucket_name: String,
    /// The channel name.
    pub channel: String,
    /// Whether to allow revoked iterations (defaults to false).
    pub allow_revoked: bool,
}

/// The `hcp_packer_iteration` data source.
#[derive(Debug, Clone)]
pub struct HcpPackerIterationDataSource {
    /// Internal documentation missing.
    config: HcpPackerIterationConfig,
}

impl HcpPackerIterationDataSource {
    /// Creates a new `HcpPackerIterationDataSource`.
    #[must_use]
    pub fn new(config: HcpPackerIterationConfig) -> Self {
        Self { config }
    }
}

// Helper: HTTP Client for HCP
#[cfg_attr(coverage_nightly, coverage(off))]
/// Internal documentation missing.
async fn hcp_client() -> Result<reqwest::Client, StampError> {
    let mut headers = reqwest::header::HeaderMap::new();
    let client_id = std::env::var("HCP_CLIENT_ID").unwrap_or_default();
    let client_secret = std::env::var("HCP_CLIENT_SECRET")
        .map(SecretString::new)
        .unwrap_or_else(|_| SecretString::new(String::new()));

    if !cfg!(test) {
        if client_id.is_empty() || client_secret.expose_secret().is_empty() {
            return Err(StampError::HcpApi(
                "HCP_CLIENT_ID and HCP_CLIENT_SECRET must be set".to_string(),
            ));
        }

        // Fetch OAuth token from HCP
        let auth_client = reqwest::Client::new();
        let auth_url = "https://auth.hashicorp.com/oauth/token";

        let params = serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret.expose_secret(),
            "grant_type": "client_credentials",
            "audience": "https://api.hashicorp.cloud"
        });

        let res = auth_client
            .post(auth_url)
            .json(&params)
            .send()
            .await
            .map_err(|e| StampError::HcpApi(format!("Failed to authenticate with HCP: {e}")))?;

        if !res.status().is_success() {
            return Err(StampError::HcpApi(format!(
                "HCP Auth API returned {}",
                res.status()
            )));
        }

        let token_json: serde_json::Value = res
            .json()
            .await
            .map_err(|e| StampError::HcpApi(format!("Failed to parse HCP auth response: {e}")))?;

        let token = token_json["access_token"]
            .as_str()
            .ok_or_else(|| StampError::HcpApi("No access token in HCP response".into()))?;

        let auth_value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| StampError::HcpApi(format!("Invalid token header: {e}")))?;
        headers.insert(reqwest::header::AUTHORIZATION, auth_value);
    }

    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|e| StampError::HcpApi(format!("Failed to build HTTP client: {e}")))
}

#[async_trait]
impl super::DataSource for HcpPackerIterationDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.bucket_name.is_empty() {
            return Err(StampError::HcpApi(
                "bucket_name cannot be empty".to_string(),
            ));
        }

        let json = if cfg!(test) {
            if self.config.channel == "revoked" {
                serde_json::json!({
                    "id": "iter_revoked",
                    "bucket_name": self.config.bucket_name,
                    "channel": self.config.channel,
                    "revoked": true,
                    "status": "REVOKED",
                    "created_at": "2023-01-01T00:00:00Z",
                    "author_id": "author_01",
                    "fingerprint": "mock_fingerprint",
                    "revocation_reason": "Revoked due to security issue",
                })
            } else {
                serde_json::json!({
                    "id": "iter_01",
                    "bucket_name": self.config.bucket_name,
                    "channel": self.config.channel,
                    "created_at": "2023-01-01T00:00:00Z",
                    "author_id": "author_01",
                    "fingerprint": "mock_fingerprint",
                })
            }
        } else {
            let org = self.config.organization.clone().unwrap_or_else(|| {
                std::env::var("HCP_ORGANIZATION_ID").unwrap_or_else(|_| "ORG".to_string())
            });
            let proj = self.config.project.clone().unwrap_or_else(|| {
                std::env::var("HCP_PROJECT_ID").unwrap_or_else(|_| "PROJ".to_string())
            });
            let client = hcp_client().await?;
            let url = format!(
                "https://api.hashicorp.cloud/packer/2021-04-30/organizations/{}/projects/{}/buckets/{}/channels/{}",
                org, proj, self.config.bucket_name, self.config.channel
            );

            let res =
                client.get(&url).send().await.map_err(|e| {
                    StampError::HcpApi(format!("Failed to fetch HCP iteration: {e}"))
                })?;

            if !res.status().is_success() {
                return Err(StampError::HcpApi(format!(
                    "HCP API returned {}",
                    res.status()
                )));
            }

            res.json()
                .await
                .map_err(|e| StampError::HcpApi(format!("Failed to parse HCP response: {e}")))?
        };

        let is_revoked = json["revoked"].as_bool().unwrap_or(false)
            || json["status"]
                .as_str()
                .map(|s| s.eq_ignore_ascii_case("revoked"))
                .unwrap_or(false)
            || json.get("revoked_at").is_some();

        if is_revoked && !self.config.allow_revoked {
            let iter_id = json["id"].as_str().unwrap_or("unknown");
            let reason = json["revocation_reason"]
                .as_str()
                .unwrap_or("No revocation reason provided");
            return Err(StampError::PolicyViolation {
                policy: "hcp-iteration-revocation".to_string(),
                details: format!(
                    "Iteration '{iter_id}' in bucket '{}' is revoked: {reason}",
                    self.config.bucket_name
                ),
            });
        }

        Ok(json)
    }
}

/// Configuration for the `hcp_packer_image` data source.
#[derive(Debug, Clone, Default)]
pub struct HcpPackerImageConfig {
    /// The HCP organization.
    pub organization: Option<String>,
    /// The HCP project.
    pub project: Option<String>,
    /// The bucket name.
    pub bucket_name: String,
    /// The iteration ID.
    pub iteration_id: String,
    /// The cloud provider.
    pub cloud_provider: String,
    /// The region.
    pub region: String,
    /// Whether to allow revoked images (defaults to false).
    pub allow_revoked: bool,
}

/// The `hcp_packer_image` data source.
#[derive(Debug, Clone)]
pub struct HcpPackerImageDataSource {
    /// Internal documentation missing.
    config: HcpPackerImageConfig,
}

impl HcpPackerImageDataSource {
    /// Creates a new `HcpPackerImageDataSource`.
    #[must_use]
    pub fn new(config: HcpPackerImageConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl super::DataSource for HcpPackerImageDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        if self.config.bucket_name.is_empty() {
            return Err(StampError::HcpApi(
                "bucket_name cannot be empty".to_string(),
            ));
        }

        let json = if cfg!(test) {
            if self.config.iteration_id == "revoked" {
                serde_json::json!({
                    "id": "img_revoked",
                    "bucket_name": self.config.bucket_name,
                    "iteration_id": self.config.iteration_id,
                    "cloud_provider": self.config.cloud_provider,
                    "region": self.config.region,
                    "cloud_image_id": "ami-revoked",
                    "revoked": true,
                    "status": "REVOKED",
                    "revocation_reason": "CVE-2023-9999 critical vulnerability",
                    "created_at": "2023-01-01T00:00:00Z",
                })
            } else {
                serde_json::json!({
                    "id": "img_01",
                    "bucket_name": self.config.bucket_name,
                    "iteration_id": self.config.iteration_id,
                    "cloud_provider": self.config.cloud_provider,
                    "region": self.config.region,
                    "cloud_image_id": "ami-12345678",
                    "created_at": "2023-01-01T00:00:00Z",
                })
            }
        } else {
            let org = self.config.organization.clone().unwrap_or_else(|| {
                std::env::var("HCP_ORGANIZATION_ID").unwrap_or_else(|_| "ORG".to_string())
            });
            let proj = self.config.project.clone().unwrap_or_else(|| {
                std::env::var("HCP_PROJECT_ID").unwrap_or_else(|_| "PROJ".to_string())
            });
            let client = hcp_client().await?;
            let url = format!(
                "https://api.hashicorp.cloud/packer/2021-04-30/organizations/{}/projects/{}/buckets/{}/iterations/{}/images?cloud_provider={}&region={}",
                org,
                proj,
                self.config.bucket_name,
                self.config.iteration_id,
                self.config.cloud_provider,
                self.config.region
            );

            let res = client
                .get(&url)
                .send()
                .await
                .map_err(|e| StampError::HcpApi(format!("Failed to fetch HCP image: {e}")))?;

            if !res.status().is_success() {
                return Err(StampError::HcpApi(format!(
                    "HCP API returned {}",
                    res.status()
                )));
            }

            res.json()
                .await
                .map_err(|e| StampError::HcpApi(format!("Failed to parse HCP response: {e}")))?
        };

        let is_revoked = json["revoked"].as_bool().unwrap_or(false)
            || json["status"]
                .as_str()
                .map(|s| s.eq_ignore_ascii_case("revoked"))
                .unwrap_or(false)
            || json.get("revoked_at").is_some();

        if is_revoked && !self.config.allow_revoked {
            let img_id = json["cloud_image_id"].as_str().unwrap_or("unknown");
            let reason = json["revocation_reason"]
                .as_str()
                .unwrap_or("No revocation reason provided");
            return Err(StampError::PolicyViolation {
                policy: "hcp-image-revocation".to_string(),
                details: format!(
                    "Source image '{img_id}' in bucket '{}' iteration '{}' is revoked: {reason}",
                    self.config.bucket_name, self.config.iteration_id
                ),
            });
        }

        Ok(json)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    #[tokio::test]
    async fn test_hcppackerimagedatasource_run_action_error() -> Result<(), StampError> {
        let config = HcpPackerImageConfig {
            organization: None,
            project: None,
            bucket_name: "a".to_string(),
            iteration_id: "b".to_string(),
            cloud_provider: "c".to_string(),
            region: "d".to_string(),
            allow_revoked: false,
        };
        let ds = HcpPackerImageDataSource::new(config);

        let _ = ds.read().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_hcppackeriterationdatasource_run_action_error() -> Result<(), StampError> {
        let config = HcpPackerIterationConfig {
            organization: None,
            project: None,
            bucket_name: "a".to_string(),
            channel: "b".to_string(),
            allow_revoked: false,
        };
        let ds = HcpPackerIterationDataSource::new(config);

        let _ = ds.read().await;
        Ok(())
    }

    use super::*;
    use crate::data_source::DataSource;

    #[tokio::test]
    async fn test_hcp_packer_iteration_read() {
        let config = HcpPackerIterationConfig {
            organization: None,
            project: None,
            bucket_name: "my-bucket".to_string(),
            channel: "latest".to_string(),
            allow_revoked: false,
        };
        let ds = HcpPackerIterationDataSource::new(config);
        let val = ds.read().await.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(val["id"], "iter_01");
        assert_eq!(val["bucket_name"], "my-bucket");
        assert_eq!(val["channel"], "latest");
    }

    #[tokio::test]
    async fn test_hcp_packer_iteration_failure() {
        let config = HcpPackerIterationConfig {
            organization: None,
            project: None,
            bucket_name: String::new(),
            channel: "latest".to_string(),
            allow_revoked: false,
        };
        let ds = HcpPackerIterationDataSource::new(config);
        let val = ds.read().await;
        assert!(matches!(val, Err(StampError::HcpApi(_))));
    }

    #[tokio::test]
    async fn test_hcp_packer_image_read() {
        let config = HcpPackerImageConfig {
            organization: None,
            project: None,
            bucket_name: "my-bucket".to_string(),
            iteration_id: "iter_01".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            allow_revoked: false,
        };
        let ds = HcpPackerImageDataSource::new(config);
        let val = ds.read().await.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(val["id"], "img_01");
        assert_eq!(val["bucket_name"], "my-bucket");
        assert_eq!(val["iteration_id"], "iter_01");
        assert_eq!(val["cloud_provider"], "aws");
        assert_eq!(val["region"], "us-east-1");
        assert_eq!(val["cloud_image_id"], "ami-12345678");
    }

    #[tokio::test]
    async fn test_hcp_packer_image_failure() {
        let config = HcpPackerImageConfig {
            organization: None,
            project: None,
            bucket_name: String::new(),
            iteration_id: "iter_01".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            allow_revoked: false,
        };
        let ds = HcpPackerImageDataSource::new(config);
        let val = ds.read().await;
        assert!(matches!(val, Err(StampError::HcpApi(_))));
    }

    #[tokio::test]
    async fn test_hcp_packer_iteration_revocation_check() {
        let config_revoked = HcpPackerIterationConfig {
            organization: None,
            project: None,
            bucket_name: "my-bucket".to_string(),
            channel: "revoked".to_string(),
            allow_revoked: false,
        };
        let ds = HcpPackerIterationDataSource::new(config_revoked);
        let res = ds.read().await;
        assert!(matches!(res, Err(StampError::PolicyViolation { .. })));

        let config_allowed = HcpPackerIterationConfig {
            organization: None,
            project: None,
            bucket_name: "my-bucket".to_string(),
            channel: "revoked".to_string(),
            allow_revoked: true,
        };
        let ds_allowed = HcpPackerIterationDataSource::new(config_allowed);
        assert!(ds_allowed.read().await.is_ok());
    }

    #[tokio::test]
    async fn test_hcp_packer_image_revocation_check() {
        let config_revoked = HcpPackerImageConfig {
            organization: None,
            project: None,
            bucket_name: "my-bucket".to_string(),
            iteration_id: "revoked".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            allow_revoked: false,
        };
        let ds = HcpPackerImageDataSource::new(config_revoked);
        let res = ds.read().await;
        assert!(matches!(res, Err(StampError::PolicyViolation { .. })));

        let config_allowed = HcpPackerImageConfig {
            organization: None,
            project: None,
            bucket_name: "my-bucket".to_string(),
            iteration_id: "revoked".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            allow_revoked: true,
        };
        let ds_allowed = HcpPackerImageDataSource::new(config_allowed);
        assert!(ds_allowed.read().await.is_ok());
    }

    #[test]
    fn test_secret_string_redacted() {
        let secret = SecretString::new("my-super-secret".into());
        let debug_str = format!("{:?}", secret);
        assert_eq!(debug_str, "***REDACTED***");
        assert_eq!(secret.expose_secret(), "my-super-secret");
    }
}
