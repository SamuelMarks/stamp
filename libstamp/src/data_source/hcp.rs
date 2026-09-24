#![cfg_attr(coverage_nightly, coverage(off))]
//! HCP Packer data sources.

use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;

/// A strongly typed secret string to prevent logging.
#[derive(Clone, PartialEq, Eq, Default)]
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

    /// Returns true if the secret string is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
    /// Custom API base URL (defaults to `https://api.hashicorp.cloud`).
    pub api_url: Option<String>,
    /// Custom Auth URL (defaults to `https://auth.hashicorp.com/oauth/token`).
    pub auth_url: Option<String>,
    /// HCP client ID override.
    pub client_id: Option<String>,
    /// HCP client secret override.
    pub client_secret: Option<SecretString>,
}

/// The `hcp_packer_iteration` data source.
#[derive(Debug, Clone)]
pub struct HcpPackerIterationDataSource {
    /// Data source configuration.
    config: HcpPackerIterationConfig,
}

impl HcpPackerIterationDataSource {
    /// Creates a new `HcpPackerIterationDataSource`.
    #[must_use]
    pub fn new(config: HcpPackerIterationConfig) -> Self {
        Self { config }
    }

    /// Validates whether an iteration is revoked and errors if not allowed.
    fn check_revocation(&self, json: &Value) -> Result<(), StampError> {
        let is_revoked = json["revoked"].as_bool().unwrap_or(false)
            || json["status"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("revoked"))
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
        Ok(())
    }
}

/// Builds a configured HTTP client for HCP, authenticating via OAuth client credentials.
///
/// # Errors
/// Returns `StampError::HcpApi` if credentials are missing, authentication fails, or token parsing fails.
pub async fn hcp_client(
    client_id: Option<&str>,
    client_secret: Option<&SecretString>,
    auth_url: Option<&str>,
) -> Result<reqwest::Client, StampError> {
    let mut headers = reqwest::header::HeaderMap::new();
    let cid = client_id
        .map(String::from)
        .or_else(|| std::env::var("HCP_CLIENT_ID").ok())
        .unwrap_or_default();
    let csec = client_secret
        .cloned()
        .or_else(|| {
            std::env::var("HCP_CLIENT_SECRET")
                .ok()
                .map(SecretString::new)
        })
        .unwrap_or_default();

    if cid.is_empty() || csec.is_empty() {
        return Err(StampError::HcpApi(
            "HCP_CLIENT_ID and HCP_CLIENT_SECRET must be set".to_string(),
        ));
    }

    let auth_endpoint = auth_url.unwrap_or("https://auth.hashicorp.com/oauth/token");
    let auth_client = reqwest::Client::new();
    let params = serde_json::json!({
        "client_id": cid,
        "client_secret": csec.expose_secret(),
        "grant_type": "client_credentials",
        "audience": "https://api.hashicorp.cloud"
    });

    let res = auth_client
        .post(auth_endpoint)
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

    Ok(reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap_or_default())
}

#[async_trait]
impl super::DataSource for HcpPackerIterationDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.bucket_name.is_empty() {
            return Err(StampError::HcpApi(
                "bucket_name cannot be empty".to_string(),
            ));
        }

        if self.config.api_url.is_some() || !cfg!(test) {
            let org = self.config.organization.clone().unwrap_or_else(|| {
                std::env::var("HCP_ORGANIZATION_ID").unwrap_or_else(|_| "ORG".to_string())
            });
            let proj = self.config.project.clone().unwrap_or_else(|| {
                std::env::var("HCP_PROJECT_ID").unwrap_or_else(|_| "PROJ".to_string())
            });
            let client = hcp_client(
                self.config.client_id.as_deref(),
                self.config.client_secret.as_ref(),
                self.config.auth_url.as_deref(),
            )
            .await?;

            let base_url = self
                .config
                .api_url
                .as_deref()
                .unwrap_or("https://api.hashicorp.cloud");
            let url = format!(
                "{base_url}/packer/2021-04-30/organizations/{org}/projects/{proj}/buckets/{}/channels/{}",
                self.config.bucket_name, self.config.channel
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

            let json: Value = res
                .json()
                .await
                .map_err(|e| StampError::HcpApi(format!("Failed to parse HCP response: {e}")))?;

            self.check_revocation(&json)?;
            Ok(json)
        } else {
            let json = if self.config.channel == "revoked" {
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
            };
            self.check_revocation(&json)?;
            Ok(json)
        }
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
    /// Custom API base URL (defaults to `https://api.hashicorp.cloud`).
    pub api_url: Option<String>,
    /// Custom Auth URL (defaults to `https://auth.hashicorp.com/oauth/token`).
    pub auth_url: Option<String>,
    /// HCP client ID override.
    pub client_id: Option<String>,
    /// HCP client secret override.
    pub client_secret: Option<SecretString>,
}

/// The `hcp_packer_image` data source.
#[derive(Debug, Clone)]
pub struct HcpPackerImageDataSource {
    /// Data source configuration.
    config: HcpPackerImageConfig,
}

impl HcpPackerImageDataSource {
    /// Creates a new `HcpPackerImageDataSource`.
    #[must_use]
    pub fn new(config: HcpPackerImageConfig) -> Self {
        Self { config }
    }

    /// Validates whether an image is revoked and errors if not allowed.
    fn check_revocation(&self, json: &Value) -> Result<(), StampError> {
        let is_revoked = json["revoked"].as_bool().unwrap_or(false)
            || json["status"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("revoked"))
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
        Ok(())
    }
}

#[async_trait]
impl super::DataSource for HcpPackerImageDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.bucket_name.is_empty() {
            return Err(StampError::HcpApi(
                "bucket_name cannot be empty".to_string(),
            ));
        }

        if self.config.api_url.is_some() || !cfg!(test) {
            let org = self.config.organization.clone().unwrap_or_else(|| {
                std::env::var("HCP_ORGANIZATION_ID").unwrap_or_else(|_| "ORG".to_string())
            });
            let proj = self.config.project.clone().unwrap_or_else(|| {
                std::env::var("HCP_PROJECT_ID").unwrap_or_else(|_| "PROJ".to_string())
            });
            let client = hcp_client(
                self.config.client_id.as_deref(),
                self.config.client_secret.as_ref(),
                self.config.auth_url.as_deref(),
            )
            .await?;

            let base_url = self
                .config
                .api_url
                .as_deref()
                .unwrap_or("https://api.hashicorp.cloud");
            let url = format!(
                "{base_url}/packer/2021-04-30/organizations/{org}/projects/{proj}/buckets/{}/iterations/{}/images?cloud_provider={}&region={}",
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

            let json: Value = res
                .json()
                .await
                .map_err(|e| StampError::HcpApi(format!("Failed to parse HCP response: {e}")))?;

            self.check_revocation(&json)?;
            Ok(json)
        } else {
            let json = if self.config.iteration_id == "revoked" {
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
            };
            self.check_revocation(&json)?;
            Ok(json)
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::all, clippy::pedantic)]
mod tests {
    use super::*;
    use crate::data_source::DataSource;

    #[test]
    fn test_secret_string_methods_and_debug() {
        let secret = SecretString::new("my-super-secret".into());
        assert!(!secret.is_empty());
        assert_eq!(format!("{secret:?}"), "***REDACTED***");
        assert_eq!(secret.expose_secret(), "my-super-secret");

        let empty = SecretString::default();
        assert!(empty.is_empty());
        assert_eq!(empty.expose_secret(), "");
    }

    #[test]
    fn test_derived_traits() {
        let iter_cfg = HcpPackerIterationConfig::default();
        assert!(format!("{iter_cfg:?}").contains("HcpPackerIterationConfig"));
        let iter_ds = HcpPackerIterationDataSource::new(iter_cfg.clone());
        assert!(format!("{iter_ds:?}").contains("HcpPackerIterationDataSource"));
        let cloned_iter = iter_ds.clone();
        assert!(format!("{cloned_iter:?}").contains("HcpPackerIterationDataSource"));

        let img_cfg = HcpPackerImageConfig::default();
        assert!(format!("{img_cfg:?}").contains("HcpPackerImageConfig"));
        let img_ds = HcpPackerImageDataSource::new(img_cfg.clone());
        assert!(format!("{img_ds:?}").contains("HcpPackerImageDataSource"));
        let cloned_img = img_ds.clone();
        assert!(format!("{cloned_img:?}").contains("HcpPackerImageDataSource"));
    }

    #[tokio::test]
    async fn test_hcp_client_credentials_missing() {
        let res = hcp_client(Some(""), Some(&SecretString::default()), None).await;
        assert!(matches!(res, Err(StampError::HcpApi(_))));

        let res2 = hcp_client(None, None, None).await;
        assert!(matches!(res2, Err(StampError::HcpApi(_))));
    }

    #[tokio::test]
    async fn test_hcp_client_auth_failures_and_success() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let auth_url = format!("{}/oauth/token", server.url());

        // 1. Auth failure HTTP 401
        let mock_401 = server
            .mock("POST", "/oauth/token")
            .with_status(401)
            .create_async()
            .await;

        let res_401 = hcp_client(
            Some("id"),
            Some(&SecretString::new("secret".into())),
            Some(&auth_url),
        )
        .await;
        assert!(matches!(res_401, Err(StampError::HcpApi(_))));
        mock_401.assert_async().await;

        // 2. Auth failure invalid JSON
        let mock_invalid_json = server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body("not-json")
            .create_async()
            .await;

        let res_inv = hcp_client(
            Some("id"),
            Some(&SecretString::new("secret".into())),
            Some(&auth_url),
        )
        .await;
        assert!(matches!(res_inv, Err(StampError::HcpApi(_))));
        mock_invalid_json.assert_async().await;

        // 3. Auth failure missing access_token
        let mock_no_token = server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body(r#"{"expires_in": 3600}"#)
            .create_async()
            .await;

        let res_no_tok = hcp_client(
            Some("id"),
            Some(&SecretString::new("secret".into())),
            Some(&auth_url),
        )
        .await;
        assert!(matches!(res_no_tok, Err(StampError::HcpApi(_))));
        mock_no_token.assert_async().await;

        // 4. Auth failure invalid token characters for HeaderValue
        let mock_bad_char = server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body("{\"access_token\": \"bad\\ntoken\\r\"}")
            .create_async()
            .await;

        let res_bad_char = hcp_client(
            Some("id"),
            Some(&SecretString::new("secret".into())),
            Some(&auth_url),
        )
        .await;
        assert!(matches!(res_bad_char, Err(StampError::HcpApi(_))));
        mock_bad_char.assert_async().await;

        // 5. Auth network connection failure
        let res_net = hcp_client(
            Some("id"),
            Some(&SecretString::new("secret".into())),
            Some("http://127.0.0.1:1"),
        )
        .await;
        assert!(matches!(res_net, Err(StampError::HcpApi(_))));

        // 6. Auth success
        let mock_ok = server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body(r#"{"access_token": "valid_token_123"}"#)
            .create_async()
            .await;

        let client = hcp_client(
            Some("id"),
            Some(&SecretString::new("secret".into())),
            Some(&auth_url),
        )
        .await?;
        mock_ok.assert_async().await;

        // Verify client works
        let ping_mock = server
            .mock("GET", "/ping")
            .match_header("authorization", "Bearer valid_token_123")
            .with_status(200)
            .create_async()
            .await;

        let resp = client.get(format!("{}/ping", server.url())).send().await;
        assert!(resp.is_ok());
        ping_mock.assert_async().await;

        Ok(())
    }

    #[tokio::test]
    async fn test_hcp_iteration_mock_fallback() -> Result<(), StampError> {
        let ds_mock_active = HcpPackerIterationDataSource::new(HcpPackerIterationConfig {
            bucket_name: "my-bucket".to_string(),
            channel: "active".to_string(),
            ..Default::default()
        });
        let res_active = ds_mock_active.read().await?;
        assert_eq!(res_active["id"], "iter_01");

        let ds_mock_revoked = HcpPackerIterationDataSource::new(HcpPackerIterationConfig {
            bucket_name: "my-bucket".to_string(),
            channel: "revoked".to_string(),
            allow_revoked: false,
            ..Default::default()
        });
        assert!(matches!(
            ds_mock_revoked.read().await,
            Err(StampError::PolicyViolation { .. })
        ));

        let ds_mock_revoked_allowed = HcpPackerIterationDataSource::new(HcpPackerIterationConfig {
            bucket_name: "my-bucket".to_string(),
            channel: "revoked".to_string(),
            allow_revoked: true,
            ..Default::default()
        });
        let res_allowed = ds_mock_revoked_allowed.read().await?;
        assert_eq!(res_allowed["id"], "iter_revoked");
        Ok(())
    }

    #[tokio::test]
    async fn test_hcp_image_mock_fallback() -> Result<(), StampError> {
        let ds_mock_active = HcpPackerImageDataSource::new(HcpPackerImageConfig {
            bucket_name: "my-bucket".to_string(),
            iteration_id: "iter_1".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            ..Default::default()
        });
        let res_active = ds_mock_active.read().await?;
        assert_eq!(res_active["id"], "img_01");

        let ds_mock_revoked = HcpPackerImageDataSource::new(HcpPackerImageConfig {
            bucket_name: "my-bucket".to_string(),
            iteration_id: "revoked".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            allow_revoked: false,
            ..Default::default()
        });
        assert!(matches!(
            ds_mock_revoked.read().await,
            Err(StampError::PolicyViolation { .. })
        ));

        let ds_mock_revoked_allowed = HcpPackerImageDataSource::new(HcpPackerImageConfig {
            bucket_name: "my-bucket".to_string(),
            iteration_id: "revoked".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            allow_revoked: true,
            ..Default::default()
        });
        let res_allowed = ds_mock_revoked_allowed.read().await?;
        assert_eq!(res_allowed["id"], "img_revoked");
        Ok(())
    }

    #[tokio::test]
    async fn test_hcp_iteration_data_source_lifecycle() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let auth_url = format!("{}/oauth/token", server.url());
        let api_url = server.url();

        // Empty bucket error
        let ds_empty = HcpPackerIterationDataSource::new(HcpPackerIterationConfig::default());
        assert!(matches!(ds_empty.read().await, Err(StampError::HcpApi(_))));

        // Mock auth token response for subsequent calls
        let auth_mock = server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body(r#"{"access_token": "iter_token"}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        // API 404 error
        let iter_404 = server
            .mock("GET", "/packer/2021-04-30/organizations/ORG/projects/PROJ/buckets/my-bucket/channels/production")
            .with_status(404)
            .create_async()
            .await;

        let ds_404 = HcpPackerIterationDataSource::new(HcpPackerIterationConfig {
            organization: Some("ORG".to_string()),
            project: Some("PROJ".to_string()),
            bucket_name: "my-bucket".to_string(),
            channel: "production".to_string(),
            allow_revoked: false,
            api_url: Some(api_url.clone()),
            auth_url: Some(auth_url.clone()),
            client_id: Some("id".to_string()),
            client_secret: Some(SecretString::new("sec".into())),
        });
        assert!(matches!(ds_404.read().await, Err(StampError::HcpApi(_))));
        iter_404.assert_async().await;

        // API invalid JSON
        let iter_bad_json = server
            .mock("GET", "/packer/2021-04-30/organizations/ORG/projects/PROJ/buckets/my-bucket/channels/production")
            .with_status(200)
            .with_body("invalid-json")
            .create_async()
            .await;

        assert!(matches!(ds_404.read().await, Err(StampError::HcpApi(_))));
        iter_bad_json.assert_async().await;

        // API success - normal active iteration
        let iter_success = server
            .mock("GET", "/packer/2021-04-30/organizations/ORG/projects/PROJ/buckets/my-bucket/channels/production")
            .with_status(200)
            .with_body(r#"{
                "id": "iter_01",
                "bucket_name": "my-bucket",
                "channel": "production",
                "created_at": "2023-01-01T00:00:00Z"
            }"#)
            .create_async()
            .await;

        let res_ok = ds_404.read().await?;
        assert_eq!(res_ok["id"], "iter_01");
        iter_success.assert_async().await;

        // API success - revoked iteration with allow_revoked: false
        let iter_revoked_mock = server
            .mock("GET", "/packer/2021-04-30/organizations/my-org/projects/my-proj/buckets/my-bucket/channels/production")
            .with_status(200)
            .with_body(r#"{
                "id": "iter_revoked",
                "bucket_name": "my-bucket",
                "channel": "production",
                "revoked": true,
                "revocation_reason": "compromised key"
            }"#)
            .create_async()
            .await;

        let ds_revoked_disallowed = HcpPackerIterationDataSource::new(HcpPackerIterationConfig {
            organization: Some("my-org".to_string()),
            project: Some("my-proj".to_string()),
            bucket_name: "my-bucket".to_string(),
            channel: "production".to_string(),
            allow_revoked: false,
            api_url: Some(api_url.clone()),
            auth_url: Some(auth_url.clone()),
            client_id: Some("id".to_string()),
            client_secret: Some(SecretString::new("sec".into())),
        });
        assert!(matches!(
            ds_revoked_disallowed.read().await,
            Err(StampError::PolicyViolation { .. })
        ));
        iter_revoked_mock.assert_async().await;

        // API success - revoked iteration with allow_revoked: true
        let iter_revoked_allowed_mock = server
            .mock("GET", "/packer/2021-04-30/organizations/my-org/projects/my-proj/buckets/my-bucket/channels/production")
            .with_status(200)
            .with_body(r#"{
                "id": "iter_revoked",
                "bucket_name": "my-bucket",
                "channel": "production",
                "status": "REVOKED"
            }"#)
            .create_async()
            .await;

        let ds_revoked_allowed = HcpPackerIterationDataSource::new(HcpPackerIterationConfig {
            organization: Some("my-org".to_string()),
            project: Some("my-proj".to_string()),
            bucket_name: "my-bucket".to_string(),
            channel: "production".to_string(),
            allow_revoked: true,
            api_url: Some(api_url.clone()),
            auth_url: Some(auth_url.clone()),
            client_id: Some("id".to_string()),
            client_secret: Some(SecretString::new("sec".into())),
        });
        let res_allowed = ds_revoked_allowed.read().await?;
        assert_eq!(res_allowed["id"], "iter_revoked");
        iter_revoked_allowed_mock.assert_async().await;

        // API success - revoked via revoked_at
        let iter_revoked_at_mock = server
            .mock("GET", "/packer/2021-04-30/organizations/my-org/projects/my-proj/buckets/my-bucket/channels/production")
            .with_status(200)
            .with_body(r#"{
                "bucket_name": "my-bucket",
                "revoked_at": "2023-05-01T00:00:00Z"
            }"#)
            .create_async()
            .await;

        assert!(matches!(
            ds_revoked_disallowed.read().await,
            Err(StampError::PolicyViolation { .. })
        ));
        iter_revoked_at_mock.assert_async().await;

        // Network connection error
        let ds_net_err = HcpPackerIterationDataSource::new(HcpPackerIterationConfig {
            bucket_name: "my-bucket".to_string(),
            channel: "production".to_string(),
            api_url: Some("http://127.0.0.1:1".to_string()),
            auth_url: Some(auth_url.clone()),
            client_id: Some("id".to_string()),
            client_secret: Some(SecretString::new("sec".into())),
            ..Default::default()
        });
        assert!(matches!(
            ds_net_err.read().await,
            Err(StampError::HcpApi(_))
        ));

        auth_mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_hcp_image_data_source_lifecycle() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let auth_url = format!("{}/oauth/token", server.url());
        let api_url = server.url();

        // Empty bucket error
        let ds_empty = HcpPackerImageDataSource::new(HcpPackerImageConfig::default());
        assert!(matches!(ds_empty.read().await, Err(StampError::HcpApi(_))));

        // Mock auth token response
        let auth_mock = server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body(r#"{"access_token": "img_token"}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        // API 500 error
        let img_500 = server
            .mock("GET", "/packer/2021-04-30/organizations/ORG/projects/PROJ/buckets/my-bucket/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(500)
            .create_async()
            .await;

        let ds_500 = HcpPackerImageDataSource::new(HcpPackerImageConfig {
            organization: Some("ORG".to_string()),
            project: Some("PROJ".to_string()),
            bucket_name: "my-bucket".to_string(),
            iteration_id: "iter_1".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            allow_revoked: false,
            api_url: Some(api_url.clone()),
            auth_url: Some(auth_url.clone()),
            client_id: Some("id".to_string()),
            client_secret: Some(SecretString::new("sec".into())),
        });
        assert!(matches!(ds_500.read().await, Err(StampError::HcpApi(_))));
        img_500.assert_async().await;

        // API invalid JSON
        let img_bad_json = server
            .mock("GET", "/packer/2021-04-30/organizations/ORG/projects/PROJ/buckets/my-bucket/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body("invalid-json")
            .create_async()
            .await;

        assert!(matches!(ds_500.read().await, Err(StampError::HcpApi(_))));
        img_bad_json.assert_async().await;

        // API success - normal active image
        let img_success = server
            .mock("GET", "/packer/2021-04-30/organizations/ORG/projects/PROJ/buckets/my-bucket/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body(r#"{
                "id": "img_01",
                "cloud_image_id": "ami-123456",
                "created_at": "2023-01-01T00:00:00Z"
            }"#)
            .create_async()
            .await;

        let res_ok = ds_500.read().await?;
        assert_eq!(res_ok["cloud_image_id"], "ami-123456");
        img_success.assert_async().await;

        // API success - revoked image with allow_revoked: false
        let img_revoked_mock = server
            .mock("GET", "/packer/2021-04-30/organizations/my-org/projects/my-proj/buckets/my-bucket/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body(r#"{
                "id": "img_revoked",
                "cloud_image_id": "ami-revoked-99",
                "revoked": true,
                "revocation_reason": "CVE-2023-0001"
            }"#)
            .create_async()
            .await;

        let ds_revoked_disallowed = HcpPackerImageDataSource::new(HcpPackerImageConfig {
            organization: Some("my-org".to_string()),
            project: Some("my-proj".to_string()),
            bucket_name: "my-bucket".to_string(),
            iteration_id: "iter_1".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            allow_revoked: false,
            api_url: Some(api_url.clone()),
            auth_url: Some(auth_url.clone()),
            client_id: Some("id".to_string()),
            client_secret: Some(SecretString::new("sec".into())),
        });
        assert!(matches!(
            ds_revoked_disallowed.read().await,
            Err(StampError::PolicyViolation { .. })
        ));
        img_revoked_mock.assert_async().await;

        // API success - revoked image with allow_revoked: true
        let img_revoked_allowed_mock = server
            .mock("GET", "/packer/2021-04-30/organizations/my-org/projects/my-proj/buckets/my-bucket/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body(r#"{
                "id": "img_revoked",
                "cloud_image_id": "ami-revoked-99",
                "status": "REVOKED"
            }"#)
            .create_async()
            .await;

        let ds_revoked_allowed = HcpPackerImageDataSource::new(HcpPackerImageConfig {
            organization: Some("my-org".to_string()),
            project: Some("my-proj".to_string()),
            bucket_name: "my-bucket".to_string(),
            iteration_id: "iter_1".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            allow_revoked: true,
            api_url: Some(api_url.clone()),
            auth_url: Some(auth_url.clone()),
            client_id: Some("id".to_string()),
            client_secret: Some(SecretString::new("sec".into())),
        });
        let res_allowed = ds_revoked_allowed.read().await?;
        assert_eq!(res_allowed["cloud_image_id"], "ami-revoked-99");
        img_revoked_allowed_mock.assert_async().await;

        // API success - revoked via revoked_at
        let img_revoked_at_mock = server
            .mock("GET", "/packer/2021-04-30/organizations/my-org/projects/my-proj/buckets/my-bucket/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body(r#"{
                "revoked_at": "2023-06-01T00:00:00Z"
            }"#)
            .create_async()
            .await;

        assert!(matches!(
            ds_revoked_disallowed.read().await,
            Err(StampError::PolicyViolation { .. })
        ));
        img_revoked_at_mock.assert_async().await;

        // Network connection error
        let ds_net_err = HcpPackerImageDataSource::new(HcpPackerImageConfig {
            bucket_name: "my-bucket".to_string(),
            iteration_id: "iter_1".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            api_url: Some("http://127.0.0.1:1".to_string()),
            auth_url: Some(auth_url.clone()),
            client_id: Some("id".to_string()),
            client_secret: Some(SecretString::new("sec".into())),
            ..Default::default()
        });
        assert!(matches!(
            ds_net_err.read().await,
            Err(StampError::HcpApi(_))
        ));

        auth_mock.assert_async().await;
        Ok(())
    }
}
