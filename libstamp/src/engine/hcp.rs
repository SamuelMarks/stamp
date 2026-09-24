#![cfg_attr(coverage_nightly, coverage(off))]
//! `HashiCorp` Cloud Platform (HCP) Packer Registry client and build workflow.
//!
//! Provides API integration for managing HCP Packer buckets, build iterations,
//! image artifacts, and release channels, as well as enforcing image revocation policies.

use crate::artifact::Artifact;
use crate::error::StampError;
use crate::template::HcpPackerRegistryConfig;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Strongly-typed organization identifier for HCP.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HcpOrganizationId(pub String);

impl std::fmt::Display for HcpOrganizationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Strongly-typed project identifier for HCP.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HcpProjectId(pub String);

impl std::fmt::Display for HcpProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Strongly-typed bucket name for HCP Packer Registry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HcpBucketName(pub String);

impl std::fmt::Display for HcpBucketName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Strongly-typed iteration identifier in an HCP bucket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HcpIterationId(pub String);

impl std::fmt::Display for HcpIterationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Strongly-typed release channel name in an HCP bucket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HcpChannelName(pub String);

impl std::fmt::Display for HcpChannelName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Strongly-typed image identifier registered within an iteration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HcpImageId(pub String);

impl std::fmt::Display for HcpImageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Configuration settings for connecting to the HCP Packer Registry API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HcpClientConfig {
    /// `OAuth2` Client ID from HCP service principal.
    pub client_id: Option<String>,
    /// `OAuth2` Client Secret from HCP service principal.
    pub client_secret: Option<String>,
    /// Direct Bearer token override, if already obtained.
    pub auth_token: Option<String>,
    /// Target HCP organization identifier.
    pub organization_id: HcpOrganizationId,
    /// Target HCP project identifier.
    pub project_id: HcpProjectId,
    /// Base URL for the HCP Packer API endpoint.
    pub api_url: String,
    /// Base URL for the HCP authentication endpoint.
    pub auth_url: String,
}

impl Default for HcpClientConfig {
    fn default() -> Self {
        Self {
            client_id: None,
            client_secret: None,
            auth_token: None,
            organization_id: HcpOrganizationId("default-org".to_string()),
            project_id: HcpProjectId("default-project".to_string()),
            api_url: "https://api.hashicorp.cloud".to_string(),
            auth_url: "https://auth.hashicorp.com/oauth/token".to_string(),
        }
    }
}

/// Details of an iteration registered in an HCP Packer bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HcpBuildIteration {
    /// The unique iteration ID assigned by HCP.
    pub id: HcpIterationId,
    /// The name of the bucket containing this iteration.
    pub bucket_name: HcpBucketName,
    /// Unique fingerprint identifying this build.
    pub fingerprint: String,
    /// Optional human-readable description for the iteration.
    pub description: Option<String>,
    /// Labels associated with this iteration.
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// Status of the iteration (e.g. "BUILDING", "READY", "REVOKED").
    #[serde(default)]
    pub status: String,
    /// Whether the iteration has been marked as revoked.
    #[serde(default)]
    pub revoked: bool,
    /// Optional reason if the iteration was revoked.
    pub revocation_reason: Option<String>,
    /// RFC3339 creation timestamp.
    #[serde(default)]
    pub created_at: String,
}

/// Details of a cloud image registered within an HCP Packer iteration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HcpBuildImage {
    /// Unique image record identifier in HCP.
    pub id: HcpImageId,
    /// Iteration ID containing this image.
    pub iteration_id: HcpIterationId,
    /// Component / builder name that produced this image.
    #[serde(default)]
    pub component_type: String,
    /// Target cloud provider (e.g. "aws", "azure", "gcp").
    pub cloud_provider: String,
    /// Target cloud region (e.g. "us-east-1").
    pub region: String,
    /// Cloud provider image ID (e.g. "ami-12345678").
    pub cloud_image_id: String,
    /// Labels attached to this registered image.
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// Whether this image has been revoked in HCP.
    #[serde(default)]
    pub revoked: bool,
    /// Reason explaining why the image was revoked, if applicable.
    pub revocation_reason: Option<String>,
}

/// Client for interacting with the HCP Packer Registry API.
#[derive(Debug, Clone)]
pub struct HcpRegistryClient {
    /// Configuration for this client instance.
    config: HcpClientConfig,
    /// HTTP client instance.
    http: reqwest::Client,
}

impl HcpRegistryClient {
    /// Creates a new `HcpRegistryClient` with the given configuration.
    #[must_use]
    pub fn new(config: HcpClientConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    /// Initializes an `HcpRegistryClient` by inspecting standard environment variables.
    ///
    /// Reads:
    /// - `HCP_CLIENT_ID`
    /// - `HCP_CLIENT_SECRET`
    /// - `HCP_AUTH_TOKEN`
    /// - `HCP_ORGANIZATION_ID`
    /// - `HCP_PROJECT_ID`
    /// - `HCP_API_URL`
    /// - `HCP_AUTH_URL`
    ///
    /// # Errors
    /// Returns `StampError::HcpApi` if required credentials are missing.
    pub fn from_env() -> Result<Self, StampError> {
        let client_id = std::env::var("HCP_CLIENT_ID").ok();
        let client_secret = std::env::var("HCP_CLIENT_SECRET").ok();
        let auth_token = std::env::var("HCP_AUTH_TOKEN").ok();

        if auth_token.is_none() && (client_id.is_none() || client_secret.is_none()) {
            return Err(StampError::HcpApi(
                "HCP credentials missing: must provide HCP_AUTH_TOKEN or both HCP_CLIENT_ID and HCP_CLIENT_SECRET".to_string(),
            ));
        }

        let organization_id = HcpOrganizationId(
            std::env::var("HCP_ORGANIZATION_ID").unwrap_or_else(|_| "default-org".to_string()),
        );
        let project_id = HcpProjectId(
            std::env::var("HCP_PROJECT_ID").unwrap_or_else(|_| "default-project".to_string()),
        );
        let api_url = std::env::var("HCP_API_URL")
            .unwrap_or_else(|_| "https://api.hashicorp.cloud".to_string());
        let auth_url = std::env::var("HCP_AUTH_URL")
            .unwrap_or_else(|_| "https://auth.hashicorp.com/oauth/token".to_string());

        Ok(Self::new(HcpClientConfig {
            client_id,
            client_secret,
            auth_token,
            organization_id,
            project_id,
            api_url,
            auth_url,
        }))
    }

    /// Obtains a valid Bearer token for authenticating against HCP APIs.
    ///
    /// # Errors
    /// Returns `StampError::HcpApi` if authentication fails or token cannot be obtained.
    pub async fn authenticate(&self) -> Result<String, StampError> {
        if let Some(token) = &self.config.auth_token {
            return Ok(token.clone());
        }

        let client_id = self.config.client_id.as_deref().ok_or_else(|| {
            StampError::HcpApi("HCP client_id is required for OAuth authentication".to_string())
        })?;
        let client_secret = self.config.client_secret.as_deref().ok_or_else(|| {
            StampError::HcpApi("HCP client_secret is required for OAuth authentication".to_string())
        })?;

        let body = serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "grant_type": "client_credentials",
            "audience": "https://api.hashicorp.cloud"
        });

        let response = self
            .http
            .post(&self.config.auth_url)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::HcpApi(format!("HCP auth request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(StampError::HcpApi(format!(
                "HCP auth endpoint returned {status}: {text}"
            )));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| StampError::HcpApi(format!("Failed to parse HCP auth response: {e}")))?;

        json["access_token"]
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| {
                StampError::HcpApi("Missing access_token in HCP auth response".to_string())
            })
    }

    /// Prepares HTTP headers with the Bearer authorization token.
    async fn auth_headers(&self) -> Result<HeaderMap, StampError> {
        let token = self.authenticate().await?;
        let mut headers = HeaderMap::new();
        let auth_val = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| StampError::HcpApi(format!("Invalid Authorization header: {e}")))?;
        headers.insert(AUTHORIZATION, auth_val);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    /// Creates a new build iteration within an HCP Packer bucket.
    ///
    /// # Errors
    /// Returns `StampError::HcpApi` if creation fails.
    pub async fn create_iteration(
        &self,
        bucket_name: &HcpBucketName,
        fingerprint: &str,
        description: Option<&str>,
        labels: &HashMap<String, String>,
    ) -> Result<HcpBuildIteration, StampError> {
        let headers = self.auth_headers().await?;
        let url = format!(
            "{}/packer/2021-04-30/organizations/{}/projects/{}/buckets/{}/iterations",
            self.config.api_url, self.config.organization_id, self.config.project_id, bucket_name
        );

        let mut body = serde_json::json!({
            "fingerprint": fingerprint,
            "labels": labels,
        });

        if let Some(desc) = description
            && let Some(map) = body.as_object_mut()
        {
            map.insert("description".to_string(), serde_json::json!(desc));
        }

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::HcpApi(format!("Failed to create HCP iteration: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(StampError::HcpApi(format!(
                "HCP create iteration returned {status}: {text}"
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            StampError::HcpApi(format!("Failed to parse create iteration response: {e}"))
        })?;

        let iteration_id = json["iteration"]["id"]
            .as_str()
            .or_else(|| json["id"].as_str())
            .unwrap_or("iter_unknown");

        Ok(HcpBuildIteration {
            id: HcpIterationId(iteration_id.to_string()),
            bucket_name: bucket_name.clone(),
            fingerprint: fingerprint.to_string(),
            description: description.map(ToString::to_string),
            labels: labels.clone(),
            status: "READY".to_string(),
            revoked: false,
            revocation_reason: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Registers a build image artifact into an HCP Packer iteration.
    ///
    /// # Errors
    /// Returns `StampError::HcpApi` if registration fails.
    #[allow(clippy::too_many_arguments)]
    pub async fn register_image(
        &self,
        bucket_name: &HcpBucketName,
        iteration_id: &HcpIterationId,
        component_type: &str,
        cloud_provider: &str,
        region: &str,
        cloud_image_id: &str,
        labels: &HashMap<String, String>,
    ) -> Result<HcpBuildImage, StampError> {
        let headers = self.auth_headers().await?;
        let url = format!(
            "{}/packer/2021-04-30/organizations/{}/projects/{}/buckets/{}/iterations/{}/images",
            self.config.api_url,
            self.config.organization_id,
            self.config.project_id,
            bucket_name,
            iteration_id
        );

        let body = serde_json::json!({
            "component_type": component_type,
            "cloud_provider": cloud_provider,
            "region": region,
            "cloud_image_id": cloud_image_id,
            "labels": labels,
        });

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::HcpApi(format!("Failed to register HCP image: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(StampError::HcpApi(format!(
                "HCP register image returned {status}: {text}"
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            StampError::HcpApi(format!("Failed to parse register image response: {e}"))
        })?;

        let image_id = json["image"]["id"]
            .as_str()
            .or_else(|| json["id"].as_str())
            .unwrap_or("img_unknown");

        Ok(HcpBuildImage {
            id: HcpImageId(image_id.to_string()),
            iteration_id: iteration_id.clone(),
            component_type: component_type.to_string(),
            cloud_provider: cloud_provider.to_string(),
            region: region.to_string(),
            cloud_image_id: cloud_image_id.to_string(),
            labels: labels.clone(),
            revoked: false,
            revocation_reason: None,
        })
    }

    /// Assigns an iteration to a release channel in the specified bucket.
    ///
    /// # Errors
    /// Returns `StampError::HcpApi` if channel assignment fails.
    pub async fn assign_channel(
        &self,
        bucket_name: &HcpBucketName,
        iteration_id: &HcpIterationId,
        channel: &HcpChannelName,
    ) -> Result<(), StampError> {
        let headers = self.auth_headers().await?;
        let url = format!(
            "{}/packer/2021-04-30/organizations/{}/projects/{}/buckets/{}/channels/{}",
            self.config.api_url,
            self.config.organization_id,
            self.config.project_id,
            bucket_name,
            channel
        );

        let body = serde_json::json!({
            "iteration_id": iteration_id.0,
        });

        let response = self
            .http
            .patch(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::HcpApi(format!("Failed to assign HCP channel: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(StampError::HcpApi(format!(
                "HCP assign channel returned {status}: {text}"
            )));
        }

        Ok(())
    }

    /// Retrieves iteration information assigned to a release channel.
    ///
    /// # Errors
    /// Returns `StampError::HcpApi` if retrieval fails, or `StampError::PolicyViolation`
    /// if the iteration is revoked.
    pub async fn get_channel_iteration(
        &self,
        bucket_name: &HcpBucketName,
        channel: &HcpChannelName,
        allow_revoked: bool,
    ) -> Result<HcpBuildIteration, StampError> {
        let headers = self.auth_headers().await?;
        let url = format!(
            "{}/packer/2021-04-30/organizations/{}/projects/{}/buckets/{}/channels/{}",
            self.config.api_url,
            self.config.organization_id,
            self.config.project_id,
            bucket_name,
            channel
        );

        let response = self
            .http
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| {
                StampError::HcpApi(format!("Failed to fetch HCP channel iteration: {e}"))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(StampError::HcpApi(format!(
                "HCP get channel iteration returned {status}: {text}"
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            StampError::HcpApi(format!(
                "Failed to parse get channel iteration response: {e}"
            ))
        })?;

        let iter_obj = json.get("iteration").unwrap_or(&json);
        let id = iter_obj["id"].as_str().unwrap_or("").to_string();
        let status = iter_obj["status"].as_str().unwrap_or("READY").to_string();
        let revoked = iter_obj["revoked"].as_bool().unwrap_or(false)
            || iter_obj.get("revoked_at").is_some()
            || status.eq_ignore_ascii_case("revoked");
        let revocation_reason = iter_obj["revocation_reason"]
            .as_str()
            .map(ToString::to_string);

        if revoked && !allow_revoked {
            return Err(StampError::PolicyViolation {
                policy: "hcp-iteration-revocation".to_string(),
                details: format!(
                    "Iteration '{id}' in bucket '{bucket_name}' is revoked: {}",
                    revocation_reason
                        .as_deref()
                        .unwrap_or("No revocation reason provided")
                ),
            });
        }

        Ok(HcpBuildIteration {
            id: HcpIterationId(id),
            bucket_name: bucket_name.clone(),
            fingerprint: iter_obj["fingerprint"].as_str().unwrap_or("").to_string(),
            description: iter_obj["description"].as_str().map(ToString::to_string),
            labels: HashMap::new(),
            status,
            revoked,
            revocation_reason,
            created_at: iter_obj["created_at"].as_str().unwrap_or("").to_string(),
        })
    }

    /// Retrieves image metadata for a given iteration, provider, and region.
    ///
    /// # Errors
    /// Returns `StampError::HcpApi` if query fails, or `StampError::PolicyViolation`
    /// if the target image is revoked.
    pub async fn get_image(
        &self,
        bucket_name: &HcpBucketName,
        iteration_id: &HcpIterationId,
        cloud_provider: &str,
        region: &str,
        allow_revoked: bool,
    ) -> Result<HcpBuildImage, StampError> {
        let headers = self.auth_headers().await?;
        let url = format!(
            "{}/packer/2021-04-30/organizations/{}/projects/{}/buckets/{}/iterations/{}/images?cloud_provider={}&region={}",
            self.config.api_url,
            self.config.organization_id,
            self.config.project_id,
            bucket_name,
            iteration_id,
            cloud_provider,
            region
        );

        let response = self
            .http
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| StampError::HcpApi(format!("Failed to fetch HCP image: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(StampError::HcpApi(format!(
                "HCP get image returned {status}: {text}"
            )));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| StampError::HcpApi(format!("Failed to parse get image response: {e}")))?;

        let img_obj = if let Some(images) = json["images"].as_array() {
            images.first().unwrap_or(&json)
        } else if let Some(img) = json.get("image") {
            img
        } else {
            &json
        };

        let id = img_obj["id"].as_str().unwrap_or("").to_string();
        let cloud_image_id = img_obj["cloud_image_id"].as_str().unwrap_or("").to_string();
        let status = img_obj["status"].as_str().unwrap_or("READY").to_string();
        let revoked = img_obj["revoked"].as_bool().unwrap_or(false)
            || img_obj.get("revoked_at").is_some()
            || status.eq_ignore_ascii_case("revoked");
        let revocation_reason = img_obj["revocation_reason"]
            .as_str()
            .map(ToString::to_string);

        if revoked && !allow_revoked {
            return Err(StampError::PolicyViolation {
                policy: "hcp-image-revocation".to_string(),
                details: format!(
                    "Source image '{cloud_image_id}' in bucket '{bucket_name}' iteration '{iteration_id}' is revoked: {}",
                    revocation_reason
                        .as_deref()
                        .unwrap_or("No revocation reason provided")
                ),
            });
        }

        Ok(HcpBuildImage {
            id: HcpImageId(id),
            iteration_id: iteration_id.clone(),
            component_type: img_obj["component_type"].as_str().unwrap_or("").to_string(),
            cloud_provider: cloud_provider.to_string(),
            region: region.to_string(),
            cloud_image_id,
            labels: HashMap::new(),
            revoked,
            revocation_reason,
        })
    }

    /// Coordinates pushing generated build artifacts to the HCP Packer Registry.
    ///
    /// Executes the full registration lifecycle:
    /// 1. Creates a new build iteration in the configured HCP bucket.
    /// 2. Registers metadata and image IDs for each completed artifact.
    /// 3. Assigns the created iteration to any configured release channels.
    ///
    /// # Errors
    /// Returns `StampError` if iteration creation, image registration, or channel assignment fails.
    pub async fn push_build_artifacts(
        &self,
        registry: &HcpPackerRegistryConfig,
        artifacts: &[Box<dyn Artifact>],
    ) -> Result<HcpBuildIteration, StampError> {
        let bucket = HcpBucketName(registry.bucket_name.0.clone());
        let fingerprint = format!("stamp-{}", uuid::Uuid::new_v4());

        let mut merged_labels = registry.labels.clone();
        merged_labels.extend(registry.bucket_labels.clone());
        merged_labels.extend(registry.build_labels.clone());

        let iteration = self
            .create_iteration(
                &bucket,
                &fingerprint,
                registry.description.as_deref(),
                &merged_labels,
            )
            .await?;

        for artifact in artifacts {
            let art_id = artifact.id();
            let builder_id = artifact.builder_id();

            let (cloud_provider, region, image_id) = parse_artifact_metadata(&builder_id, &art_id);

            let _ = self
                .register_image(
                    &bucket,
                    &iteration.id,
                    &builder_id,
                    &cloud_provider,
                    &region,
                    &image_id,
                    &merged_labels,
                )
                .await?;
        }

        for channel in &registry.channels {
            self.assign_channel(&bucket, &iteration.id, &HcpChannelName(channel.clone()))
                .await?;
        }

        Ok(iteration)
    }

    /// Revoke an iteration in an HCP Packer bucket.
    ///
    /// # Errors
    /// Returns `StampError::HcpApi` if the revocation request fails.
    pub async fn revoke_iteration(
        &self,
        bucket_name: &HcpBucketName,
        iteration_id: &HcpIterationId,
        reason: &str,
    ) -> Result<(), StampError> {
        let headers = self.auth_headers().await?;
        let url = format!(
            "{}/packer/2021-04-30/organizations/{}/projects/{}/buckets/{}/iterations/{}/revoke",
            self.config.api_url,
            self.config.organization_id,
            self.config.project_id,
            bucket_name,
            iteration_id
        );

        let payload = serde_json::json!({
            "revocation_reason": reason
        });

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::HcpApi(format!("Network error revoking iteration: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StampError::HcpApi(format!(
                "HCP API error revoking iteration (status {status}): {body}"
            )));
        }
        Ok(())
    }

    /// Mark an iteration as complete and active in an HCP Packer bucket.
    ///
    /// # Errors
    /// Returns `StampError::HcpApi` if iteration activation fails.
    pub async fn activate_iteration(
        &self,
        bucket_name: &HcpBucketName,
        iteration_id: &HcpIterationId,
    ) -> Result<(), StampError> {
        let headers = self.auth_headers().await?;
        let url = format!(
            "{}/packer/2021-04-30/organizations/{}/projects/{}/buckets/{}/iterations/{}/complete",
            self.config.api_url,
            self.config.organization_id,
            self.config.project_id,
            bucket_name,
            iteration_id
        );

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| StampError::HcpApi(format!("Network error completing iteration: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StampError::HcpApi(format!(
                "HCP API error activating iteration (status {status}): {body}"
            )));
        }
        Ok(())
    }
}

/// Parses artifact identifier strings to infer cloud provider, region, and clean image ID.
fn parse_artifact_metadata(builder_id: &str, artifact_id: &str) -> (String, String, String) {
    if let Some((region, ami)) = artifact_id.split_once(':')
        && ami.starts_with("ami-")
    {
        return ("aws".to_string(), region.to_string(), ami.to_string());
    }

    if artifact_id.starts_with("ami-") {
        return (
            "aws".to_string(),
            "us-east-1".to_string(),
            artifact_id.to_string(),
        );
    }

    if builder_id.contains("azure") {
        return (
            "azure".to_string(),
            "eastus".to_string(),
            artifact_id.to_string(),
        );
    }

    if builder_id.contains("google") || builder_id.contains("gce") {
        return (
            "gcp".to_string(),
            "global".to_string(),
            artifact_id.to_string(),
        );
    }

    if builder_id.contains("docker") {
        return (
            "docker".to_string(),
            "local".to_string(),
            artifact_id.to_string(),
        );
    }

    (
        "unknown".to_string(),
        "default".to_string(),
        artifact_id.to_string(),
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::all, clippy::pedantic)]
mod tests {
    use super::*;

    /// Tests derived traits (Display, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize) for all HCP types.
    #[test]
    fn test_types_display_and_traits() -> Result<(), serde_json::Error> {
        let org = HcpOrganizationId("org-123".to_string());
        assert_eq!(org.to_string(), "org-123");
        assert_eq!(format!("{org:?}"), r#"HcpOrganizationId("org-123")"#);
        assert_eq!(org, org.clone());
        let mut map = HashMap::new();
        map.insert(org.clone(), 1);
        assert_eq!(map.get(&org), Some(&1));
        let org_json = serde_json::to_string(&org)?;
        let org_de: HcpOrganizationId = serde_json::from_str(&org_json)?;
        assert_eq!(org, org_de);

        let proj = HcpProjectId("proj-123".to_string());
        assert_eq!(proj.to_string(), "proj-123");
        assert_eq!(format!("{proj:?}"), r#"HcpProjectId("proj-123")"#);
        assert_eq!(proj, proj.clone());
        let mut pmap = HashMap::new();
        pmap.insert(proj.clone(), 1);
        assert_eq!(pmap.get(&proj), Some(&1));
        let proj_json = serde_json::to_string(&proj)?;
        let proj_de: HcpProjectId = serde_json::from_str(&proj_json)?;
        assert_eq!(proj, proj_de);

        let bucket = HcpBucketName("bucket-abc".to_string());
        assert_eq!(bucket.to_string(), "bucket-abc");
        assert_eq!(format!("{bucket:?}"), r#"HcpBucketName("bucket-abc")"#);
        assert_eq!(bucket, bucket.clone());
        let mut bmap = HashMap::new();
        bmap.insert(bucket.clone(), 1);
        assert_eq!(bmap.get(&bucket), Some(&1));
        let bucket_json = serde_json::to_string(&bucket)?;
        let bucket_de: HcpBucketName = serde_json::from_str(&bucket_json)?;
        assert_eq!(bucket, bucket_de);

        let iter = HcpIterationId("iter-1".to_string());
        assert_eq!(iter.to_string(), "iter-1");
        assert_eq!(format!("{iter:?}"), r#"HcpIterationId("iter-1")"#);
        assert_eq!(iter, iter.clone());
        let mut imap = HashMap::new();
        imap.insert(iter.clone(), 1);
        assert_eq!(imap.get(&iter), Some(&1));
        let iter_json = serde_json::to_string(&iter)?;
        let iter_de: HcpIterationId = serde_json::from_str(&iter_json)?;
        assert_eq!(iter, iter_de);

        let chan = HcpChannelName("prod".to_string());
        assert_eq!(chan.to_string(), "prod");
        assert_eq!(format!("{chan:?}"), r#"HcpChannelName("prod")"#);
        assert_eq!(chan, chan.clone());
        let mut cmap = HashMap::new();
        cmap.insert(chan.clone(), 1);
        assert_eq!(cmap.get(&chan), Some(&1));
        let chan_json = serde_json::to_string(&chan)?;
        let chan_de: HcpChannelName = serde_json::from_str(&chan_json)?;
        assert_eq!(chan, chan_de);

        let img = HcpImageId("img-1".to_string());
        assert_eq!(img.to_string(), "img-1");
        assert_eq!(format!("{img:?}"), r#"HcpImageId("img-1")"#);
        assert_eq!(img, img.clone());
        let mut img_map = HashMap::new();
        img_map.insert(img.clone(), 1);
        assert_eq!(img_map.get(&img), Some(&1));
        let img_json = serde_json::to_string(&img)?;
        let img_de: HcpImageId = serde_json::from_str(&img_json)?;
        assert_eq!(img, img_de);

        let build_iter = HcpBuildIteration {
            id: iter.clone(),
            bucket_name: bucket.clone(),
            fingerprint: "fp-1".to_string(),
            description: Some("desc".to_string()),
            labels: HashMap::from([("k".to_string(), "v".to_string())]),
            status: "READY".to_string(),
            revoked: false,
            revocation_reason: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(build_iter, build_iter.clone());
        assert!(format!("{build_iter:?}").contains("HcpBuildIteration"));
        let bi_json = serde_json::to_string(&build_iter)?;
        let bi_de: HcpBuildIteration = serde_json::from_str(&bi_json)?;
        assert_eq!(build_iter, bi_de);

        let build_img = HcpBuildImage {
            id: img.clone(),
            iteration_id: iter.clone(),
            component_type: "amazon-ebs".to_string(),
            cloud_provider: "aws".to_string(),
            region: "us-east-1".to_string(),
            cloud_image_id: "ami-123".to_string(),
            labels: HashMap::from([("k".to_string(), "v".to_string())]),
            revoked: false,
            revocation_reason: None,
        };
        assert_eq!(build_img, build_img.clone());
        assert!(format!("{build_img:?}").contains("HcpBuildImage"));
        let bimg_json = serde_json::to_string(&build_img)?;
        let bimg_de: HcpBuildImage = serde_json::from_str(&bimg_json)?;
        assert_eq!(build_img, bimg_de);

        let cfg = HcpClientConfig::default();
        assert_eq!(cfg, cfg.clone());
        assert!(format!("{cfg:?}").contains("HcpClientConfig"));
        let client = HcpRegistryClient::new(cfg);
        let client_clone = client.clone();
        assert!(format!("{client_clone:?}").contains("HcpRegistryClient"));
        Ok(())
    }

    /// Tests default configuration values.
    #[test]
    fn test_client_config_default() {
        let cfg = HcpClientConfig::default();
        assert_eq!(cfg.organization_id.0, "default-org");
        assert_eq!(cfg.project_id.0, "default-project");
        assert_eq!(cfg.api_url, "https://api.hashicorp.cloud");
        assert_eq!(cfg.auth_url, "https://auth.hashicorp.com/oauth/token");
    }

    /// Tests client initialization from environment variables across all configurations.
    #[test]
    fn test_client_from_env_all_cases() -> Result<(), StampError> {
        let _guard = crate::utils::ENV_MUTEX.lock();

        // Clean env
        unsafe {
            std::env::remove_var("HCP_CLIENT_ID");
            std::env::remove_var("HCP_CLIENT_SECRET");
            std::env::remove_var("HCP_AUTH_TOKEN");
            std::env::remove_var("HCP_ORGANIZATION_ID");
            std::env::remove_var("HCP_PROJECT_ID");
            std::env::remove_var("HCP_API_URL");
            std::env::remove_var("HCP_AUTH_URL");
        }

        // 1. All missing -> error
        assert!(HcpRegistryClient::from_env().is_err());

        // 2. Only client_id set -> error
        unsafe {
            std::env::set_var("HCP_CLIENT_ID", "my-id");
        }
        assert!(HcpRegistryClient::from_env().is_err());

        // 3. Only client_secret set -> error
        unsafe {
            std::env::remove_var("HCP_CLIENT_ID");
            std::env::set_var("HCP_CLIENT_SECRET", "my-secret");
        }
        assert!(HcpRegistryClient::from_env().is_err());

        // 4. Client id and secret set with custom org/proj/urls -> success
        unsafe {
            std::env::set_var("HCP_CLIENT_ID", "my-id");
            std::env::set_var("HCP_CLIENT_SECRET", "my-secret");
            std::env::set_var("HCP_ORGANIZATION_ID", "my-org");
            std::env::set_var("HCP_PROJECT_ID", "my-proj");
            std::env::set_var("HCP_API_URL", "https://custom.api");
            std::env::set_var("HCP_AUTH_URL", "https://custom.auth");
        }
        let c = HcpRegistryClient::from_env()?;
        assert_eq!(c.config.client_id.as_deref(), Some("my-id"));
        assert_eq!(c.config.client_secret.as_deref(), Some("my-secret"));
        assert_eq!(c.config.organization_id.0, "my-org");
        assert_eq!(c.config.project_id.0, "my-proj");
        assert_eq!(c.config.api_url, "https://custom.api");
        assert_eq!(c.config.auth_url, "https://custom.auth");

        // 5. Auth token set -> success (and tests fallback closures)
        unsafe {
            std::env::remove_var("HCP_CLIENT_ID");
            std::env::remove_var("HCP_CLIENT_SECRET");
            std::env::remove_var("HCP_ORGANIZATION_ID");
            std::env::remove_var("HCP_PROJECT_ID");
            std::env::remove_var("HCP_API_URL");
            std::env::remove_var("HCP_AUTH_URL");
            std::env::set_var("HCP_AUTH_TOKEN", "bearer-token-123");
        }
        let c_tok = HcpRegistryClient::from_env()?;
        assert_eq!(c_tok.config.auth_token.as_deref(), Some("bearer-token-123"));
        assert_eq!(c_tok.config.organization_id.0, "default-org");
        assert_eq!(c_tok.config.project_id.0, "default-project");
        assert_eq!(c_tok.config.api_url, "https://api.hashicorp.cloud");
        assert_eq!(
            c_tok.config.auth_url,
            "https://auth.hashicorp.com/oauth/token"
        );

        // Clean env
        unsafe {
            std::env::remove_var("HCP_CLIENT_ID");
            std::env::remove_var("HCP_CLIENT_SECRET");
            std::env::remove_var("HCP_AUTH_TOKEN");
            std::env::remove_var("HCP_ORGANIZATION_ID");
            std::env::remove_var("HCP_PROJECT_ID");
            std::env::remove_var("HCP_API_URL");
            std::env::remove_var("HCP_AUTH_URL");
        }
        Ok(())
    }

    /// Tests unauthenticated client failures across all API methods.
    #[tokio::test]
    async fn test_unauthenticated_client_errors() {
        let unauth_client = HcpRegistryClient::new(HcpClientConfig {
            client_id: None,
            client_secret: None,
            auth_token: None,
            ..Default::default()
        });
        let bucket = HcpBucketName("b".to_string());
        let iter_id = HcpIterationId("i".to_string());
        let chan = HcpChannelName("c".to_string());

        assert!(matches!(
            unauth_client
                .create_iteration(&bucket, "fp", None, &HashMap::new())
                .await,
            Err(StampError::HcpApi(_))
        ));
        assert!(matches!(
            unauth_client
                .register_image(
                    &bucket,
                    &iter_id,
                    "comp",
                    "aws",
                    "us-east-1",
                    "ami-1",
                    &HashMap::new()
                )
                .await,
            Err(StampError::HcpApi(_))
        ));
        assert!(matches!(
            unauth_client.assign_channel(&bucket, &iter_id, &chan).await,
            Err(StampError::HcpApi(_))
        ));
        assert!(matches!(
            unauth_client
                .get_channel_iteration(&bucket, &chan, false)
                .await,
            Err(StampError::HcpApi(_))
        ));
        assert!(matches!(
            unauth_client
                .get_image(&bucket, &iter_id, "aws", "us-east-1", false)
                .await,
            Err(StampError::HcpApi(_))
        ));
        assert!(matches!(
            unauth_client
                .revoke_iteration(&bucket, &iter_id, "reason")
                .await,
            Err(StampError::HcpApi(_))
        ));
        assert!(matches!(
            unauth_client.activate_iteration(&bucket, &iter_id).await,
            Err(StampError::HcpApi(_))
        ));
    }

    /// Tests artifact identifier metadata extraction.
    #[test]
    fn test_parse_artifact_metadata() {
        let (p, r, id) =
            parse_artifact_metadata("amazon-ebs.test", "us-west-2:ami-0123456789abcdef0");
        assert_eq!(p, "aws");
        assert_eq!(r, "us-west-2");
        assert_eq!(id, "ami-0123456789abcdef0");

        let (p2, r2, id2) = parse_artifact_metadata("amazon-ebs", "ami-11112222");
        assert_eq!(p2, "aws");
        assert_eq!(r2, "us-east-1");
        assert_eq!(id2, "ami-11112222");

        let (p3, r3, id3) = parse_artifact_metadata("azure-arm.web", "img-resource-id");
        assert_eq!(p3, "azure");
        assert_eq!(r3, "eastus");
        assert_eq!(id3, "img-resource-id");

        let (p4, r4, id4) = parse_artifact_metadata("googlecompute", "gcp-image-name");
        assert_eq!(p4, "gcp");
        assert_eq!(r4, "global");
        assert_eq!(id4, "gcp-image-name");

        let (p4b, r4b, id4b) = parse_artifact_metadata("gce.custom", "gcp-image-2");
        assert_eq!(p4b, "gcp");
        assert_eq!(r4b, "global");
        assert_eq!(id4b, "gcp-image-2");

        let (p5, r5, id5) = parse_artifact_metadata("docker.test", "sha256:abcd");
        assert_eq!(p5, "docker");
        assert_eq!(r5, "local");
        assert_eq!(id5, "sha256:abcd");

        let (p6, r6, id6) = parse_artifact_metadata("other", "custom-artifact");
        assert_eq!(p6, "unknown");
        assert_eq!(r6, "default");
        assert_eq!(id6, "custom-artifact");
    }

    /// Tests authentication flows including token cache, missing credentials, and HTTP errors.
    #[tokio::test]
    async fn test_hcp_auth_flow_and_errors() -> Result<(), StampError> {
        // Direct token cached
        let cached_cfg = HcpClientConfig {
            auth_token: Some("cached-token".to_string()),
            ..Default::default()
        };
        let cached_client = HcpRegistryClient::new(cached_cfg);
        let res_cached = cached_client.authenticate().await?;
        assert_eq!(res_cached, "cached-token");

        // Missing client_id
        let no_id_cfg = HcpClientConfig {
            client_id: None,
            client_secret: Some("sec".to_string()),
            auth_token: None,
            ..Default::default()
        };
        assert!(matches!(
            HcpRegistryClient::new(no_id_cfg).authenticate().await,
            Err(StampError::HcpApi(_))
        ));

        // Missing client_secret
        let no_sec_cfg = HcpClientConfig {
            client_id: Some("id".to_string()),
            client_secret: None,
            auth_token: None,
            ..Default::default()
        };
        assert!(matches!(
            HcpRegistryClient::new(no_sec_cfg).authenticate().await,
            Err(StampError::HcpApi(_))
        ));

        // Network connection error
        let net_err_cfg = HcpClientConfig {
            client_id: Some("id".to_string()),
            client_secret: Some("sec".to_string()),
            auth_token: None,
            auth_url: "http://127.0.0.1:1/oauth/token".to_string(),
            ..Default::default()
        };
        assert!(matches!(
            HcpRegistryClient::new(net_err_cfg).authenticate().await,
            Err(StampError::HcpApi(_))
        ));

        // Invalid Authorization header value
        let bad_token_cfg = HcpClientConfig {
            auth_token: Some("bad\ntoken\r".to_string()),
            ..Default::default()
        };
        let bad_client = HcpRegistryClient::new(bad_token_cfg);
        let bad_hdr_res = bad_client
            .create_iteration(&HcpBucketName("b".to_string()), "fp", None, &HashMap::new())
            .await;
        assert!(matches!(bad_hdr_res, Err(StampError::HcpApi(_))));

        // HTTP error, invalid JSON, missing access_token, and success
        let mut server = mockito::Server::new_async().await;

        // 1. HTTP 401
        let mock_401 = server
            .mock("POST", "/oauth/token")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let auth_url = format!("{}/oauth/token", server.url());
        let client_401 = HcpRegistryClient::new(HcpClientConfig {
            client_id: Some("id".to_string()),
            client_secret: Some("sec".to_string()),
            auth_token: None,
            auth_url: auth_url.clone(),
            ..Default::default()
        });
        assert!(matches!(
            client_401.authenticate().await,
            Err(StampError::HcpApi(_))
        ));
        mock_401.assert_async().await;

        // 2. Invalid JSON
        let mock_bad_json = server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body("not-json")
            .create_async()
            .await;
        assert!(matches!(
            client_401.authenticate().await,
            Err(StampError::HcpApi(_))
        ));
        mock_bad_json.assert_async().await;

        // 3. Missing access_token
        let mock_no_token = server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body(r#"{"expires_in": 3600}"#)
            .create_async()
            .await;
        assert!(matches!(
            client_401.authenticate().await,
            Err(StampError::HcpApi(_))
        ));
        mock_no_token.assert_async().await;

        // 4. Success
        let mock_ok = server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body(r#"{"access_token": "token-valid"}"#)
            .create_async()
            .await;
        let tok = client_401.authenticate().await?;
        assert_eq!(tok, "token-valid");
        mock_ok.assert_async().await;
        Ok(())
    }

    /// Tests create_iteration under success, fallbacks, and failure conditions.
    #[tokio::test]
    async fn test_create_iteration_lifecycle() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        // Success nested
        let mock_ok = server
            .mock(
                "POST",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations",
            )
            .with_status(200)
            .with_body(r#"{"iteration": {"id": "iter_nested"}}"#)
            .create_async()
            .await;

        let client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            organization_id: HcpOrganizationId("org".to_string()),
            project_id: HcpProjectId("proj".to_string()),
            api_url: server.url(),
            auth_url: format!("{}/oauth/token", server.url()),
            ..Default::default()
        });

        let bucket = HcpBucketName("b".to_string());
        let res_ok = client
            .create_iteration(&bucket, "fp1", Some("desc"), &HashMap::new())
            .await?;
        assert_eq!(res_ok.id.0, "iter_nested");
        mock_ok.assert_async().await;

        // Success top-level id
        let mock_top = server
            .mock(
                "POST",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations",
            )
            .with_status(200)
            .with_body(r#"{"id": "iter_top"}"#)
            .create_async()
            .await;
        let res_top = client
            .create_iteration(&bucket, "fp1", None, &HashMap::new())
            .await?;
        assert_eq!(res_top.id.0, "iter_top");
        mock_top.assert_async().await;

        // Success fallback unknown id
        let mock_unknown = server
            .mock(
                "POST",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations",
            )
            .with_status(200)
            .with_body(r#"{}"#)
            .create_async()
            .await;
        let res_unknown = client
            .create_iteration(&bucket, "fp1", None, &HashMap::new())
            .await?;
        assert_eq!(res_unknown.id.0, "iter_unknown");
        mock_unknown.assert_async().await;

        // HTTP 500
        let mock_500 = server
            .mock(
                "POST",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations",
            )
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;
        assert!(matches!(
            client
                .create_iteration(&bucket, "fp1", None, &HashMap::new())
                .await,
            Err(StampError::HcpApi(_))
        ));
        mock_500.assert_async().await;

        // Invalid JSON
        let mock_bad_json = server
            .mock(
                "POST",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations",
            )
            .with_status(200)
            .with_body("bad-json")
            .create_async()
            .await;
        assert!(matches!(
            client
                .create_iteration(&bucket, "fp1", None, &HashMap::new())
                .await,
            Err(StampError::HcpApi(_))
        ));
        mock_bad_json.assert_async().await;

        // Network error
        let net_client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            api_url: "http://127.0.0.1:1".to_string(),
            ..Default::default()
        });
        assert!(matches!(
            net_client
                .create_iteration(&bucket, "fp1", None, &HashMap::new())
                .await,
            Err(StampError::HcpApi(_))
        ));
        Ok(())
    }

    /// Tests register_image under success, fallbacks, and failure conditions.
    #[tokio::test]
    async fn test_register_image_lifecycle() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        // Success nested
        let mock_ok = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images")
            .with_status(200)
            .with_body(r#"{"image": {"id": "img_nested"}}"#)
            .create_async()
            .await;

        let client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            organization_id: HcpOrganizationId("org".to_string()),
            project_id: HcpProjectId("proj".to_string()),
            api_url: server.url(),
            auth_url: format!("{}/oauth/token", server.url()),
            ..Default::default()
        });

        let bucket = HcpBucketName("b".to_string());
        let iter_id = HcpIterationId("iter_1".to_string());
        let res_ok = client
            .register_image(
                &bucket,
                &iter_id,
                "comp",
                "aws",
                "us-east-1",
                "ami-123",
                &HashMap::new(),
            )
            .await?;
        assert_eq!(res_ok.id.0, "img_nested");
        mock_ok.assert_async().await;

        // Success top-level id
        let mock_top = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images")
            .with_status(200)
            .with_body(r#"{"id": "img_top"}"#)
            .create_async()
            .await;
        let res_top = client
            .register_image(
                &bucket,
                &iter_id,
                "comp",
                "aws",
                "us-east-1",
                "ami-123",
                &HashMap::new(),
            )
            .await?;
        assert_eq!(res_top.id.0, "img_top");
        mock_top.assert_async().await;

        // Success fallback unknown id
        let mock_unknown = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images")
            .with_status(200)
            .with_body(r#"{}"#)
            .create_async()
            .await;
        let res_unknown = client
            .register_image(
                &bucket,
                &iter_id,
                "comp",
                "aws",
                "us-east-1",
                "ami-123",
                &HashMap::new(),
            )
            .await?;
        assert_eq!(res_unknown.id.0, "img_unknown");
        mock_unknown.assert_async().await;

        // HTTP 500
        let mock_500 = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images")
            .with_status(500)
            .with_body("error")
            .create_async()
            .await;
        assert!(matches!(
            client
                .register_image(
                    &bucket,
                    &iter_id,
                    "comp",
                    "aws",
                    "us-east-1",
                    "ami-123",
                    &HashMap::new()
                )
                .await,
            Err(StampError::HcpApi(_))
        ));
        mock_500.assert_async().await;

        // Invalid JSON
        let mock_bad_json = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images")
            .with_status(200)
            .with_body("bad-json")
            .create_async()
            .await;
        assert!(matches!(
            client
                .register_image(
                    &bucket,
                    &iter_id,
                    "comp",
                    "aws",
                    "us-east-1",
                    "ami-123",
                    &HashMap::new()
                )
                .await,
            Err(StampError::HcpApi(_))
        ));
        mock_bad_json.assert_async().await;

        // Network error
        let net_client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            api_url: "http://127.0.0.1:1".to_string(),
            ..Default::default()
        });
        assert!(matches!(
            net_client
                .register_image(
                    &bucket,
                    &iter_id,
                    "comp",
                    "aws",
                    "us-east-1",
                    "ami-123",
                    &HashMap::new()
                )
                .await,
            Err(StampError::HcpApi(_))
        ));
        Ok(())
    }

    /// Tests assign_channel failure branches.
    #[tokio::test]
    async fn test_assign_channel_lifecycle() {
        let mut server = mockito::Server::new_async().await;

        let mock_500 = server
            .mock(
                "PATCH",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/channels/prod",
            )
            .with_status(500)
            .with_body("failed channel assign")
            .create_async()
            .await;

        let client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            organization_id: HcpOrganizationId("org".to_string()),
            project_id: HcpProjectId("proj".to_string()),
            api_url: server.url(),
            auth_url: format!("{}/oauth/token", server.url()),
            ..Default::default()
        });

        let bucket = HcpBucketName("b".to_string());
        let iter_id = HcpIterationId("iter_1".to_string());
        let chan = HcpChannelName("prod".to_string());

        assert!(matches!(
            client.assign_channel(&bucket, &iter_id, &chan).await,
            Err(StampError::HcpApi(_))
        ));
        mock_500.assert_async().await;

        // Network error
        let net_client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            api_url: "http://127.0.0.1:1".to_string(),
            ..Default::default()
        });
        assert!(matches!(
            net_client.assign_channel(&bucket, &iter_id, &chan).await,
            Err(StampError::HcpApi(_))
        ));
    }

    /// Tests get_channel_iteration under all revocation and error conditions.
    #[tokio::test]
    async fn test_get_channel_iteration_lifecycle() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            organization_id: HcpOrganizationId("org".to_string()),
            project_id: HcpProjectId("proj".to_string()),
            api_url: server.url(),
            auth_url: format!("{}/oauth/token", server.url()),
            ..Default::default()
        });

        let bucket = HcpBucketName("b".to_string());
        let chan = HcpChannelName("prod".to_string());

        // 1. Success without nested "iteration" key
        let mock_top = server
            .mock(
                "GET",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/channels/prod",
            )
            .with_status(200)
            .with_body(r#"{"id": "iter_flat", "status": "READY"}"#)
            .create_async()
            .await;
        let res_flat = client.get_channel_iteration(&bucket, &chan, false).await?;
        assert_eq!(res_flat.id.0, "iter_flat");
        mock_top.assert_async().await;

        // 2. Revoked via status "REVOKED" with no reason provided
        let mock_rev_status = server
            .mock(
                "GET",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/channels/prod",
            )
            .with_status(200)
            .with_body(r#"{"id": "iter_rev", "status": "REVOKED"}"#)
            .create_async()
            .await;
        assert!(matches!(
            client.get_channel_iteration(&bucket, &chan, false).await,
            Err(StampError::PolicyViolation { .. })
        ));
        mock_rev_status.assert_async().await;

        // 3. Revoked via revoked_at key
        let mock_rev_at = server
            .mock(
                "GET",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/channels/prod",
            )
            .with_status(200)
            .with_body(r#"{"id": "iter_rev2", "revoked_at": "2023-01-01T00:00:00Z"}"#)
            .create_async()
            .await;
        assert!(matches!(
            client.get_channel_iteration(&bucket, &chan, false).await,
            Err(StampError::PolicyViolation { .. })
        ));
        mock_rev_at.assert_async().await;

        // 4. HTTP 500
        let mock_500 = server
            .mock(
                "GET",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/channels/prod",
            )
            .with_status(500)
            .with_body("get channel failed")
            .create_async()
            .await;
        assert!(matches!(
            client.get_channel_iteration(&bucket, &chan, false).await,
            Err(StampError::HcpApi(_))
        ));
        mock_500.assert_async().await;

        // 5. Invalid JSON
        let mock_bad_json = server
            .mock(
                "GET",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/channels/prod",
            )
            .with_status(200)
            .with_body("bad json")
            .create_async()
            .await;
        assert!(matches!(
            client.get_channel_iteration(&bucket, &chan, false).await,
            Err(StampError::HcpApi(_))
        ));
        mock_bad_json.assert_async().await;

        // 6. Network error
        let net_client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            api_url: "http://127.0.0.1:1".to_string(),
            ..Default::default()
        });
        assert!(matches!(
            net_client
                .get_channel_iteration(&bucket, &chan, false)
                .await,
            Err(StampError::HcpApi(_))
        ));
        Ok(())
    }

    /// Tests get_image under all revocation, response shape, and error conditions.
    #[tokio::test]
    async fn test_get_image_lifecycle() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            organization_id: HcpOrganizationId("org".to_string()),
            project_id: HcpProjectId("proj".to_string()),
            api_url: server.url(),
            auth_url: format!("{}/oauth/token", server.url()),
            ..Default::default()
        });

        let bucket = HcpBucketName("b".to_string());
        let iter_id = HcpIterationId("iter_1".to_string());

        // 1. Success with empty images array (fallback to &json)
        let mock_empty_arr = server
            .mock("GET", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body(r#"{"images": [], "id": "fallback_id", "cloud_image_id": "ami-fallback"}"#)
            .create_async()
            .await;
        let res_empty = client
            .get_image(&bucket, &iter_id, "aws", "us-east-1", false)
            .await?;
        assert_eq!(res_empty.id.0, "fallback_id");
        mock_empty_arr.assert_async().await;

        // 2. Success with nested "image" object
        let mock_obj = server
            .mock("GET", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body(r#"{"image": {"id": "single_obj", "cloud_image_id": "ami-single"}}"#)
            .create_async()
            .await;
        let res_obj = client
            .get_image(&bucket, &iter_id, "aws", "us-east-1", false)
            .await?;
        assert_eq!(res_obj.id.0, "single_obj");
        mock_obj.assert_async().await;

        // 3. Success with top-level fields (no images array and no image object)
        let mock_flat = server
            .mock("GET", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body(r#"{"id": "flat_id", "cloud_image_id": "ami-flat"}"#)
            .create_async()
            .await;
        let res_flat = client
            .get_image(&bucket, &iter_id, "aws", "us-east-1", false)
            .await?;
        assert_eq!(res_flat.id.0, "flat_id");
        mock_flat.assert_async().await;

        // 4. Revoked via status: "REVOKED" without reason
        let mock_rev_status = server
            .mock("GET", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body(r#"{"id": "img_rev", "cloud_image_id": "ami-rev", "status": "REVOKED"}"#)
            .create_async()
            .await;
        assert!(matches!(
            client
                .get_image(&bucket, &iter_id, "aws", "us-east-1", false)
                .await,
            Err(StampError::PolicyViolation { .. })
        ));
        mock_rev_status.assert_async().await;

        // 5. Revoked via revoked_at key
        let mock_rev_at = server
            .mock("GET", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body(r#"{"id": "img_rev2", "cloud_image_id": "ami-rev2", "revoked_at": "2023-01-01"}"#)
            .create_async()
            .await;
        assert!(matches!(
            client
                .get_image(&bucket, &iter_id, "aws", "us-east-1", false)
                .await,
            Err(StampError::PolicyViolation { .. })
        ));
        mock_rev_at.assert_async().await;

        // 6. HTTP 500
        let mock_500 = server
            .mock("GET", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(500)
            .with_body("failed get image")
            .create_async()
            .await;
        assert!(matches!(
            client
                .get_image(&bucket, &iter_id, "aws", "us-east-1", false)
                .await,
            Err(StampError::HcpApi(_))
        ));
        mock_500.assert_async().await;

        // 7. Invalid JSON
        let mock_bad_json = server
            .mock("GET", "/packer/2021-04-30/organizations/org/projects/proj/buckets/b/iterations/iter_1/images?cloud_provider=aws&region=us-east-1")
            .with_status(200)
            .with_body("invalid json")
            .create_async()
            .await;
        assert!(matches!(
            client
                .get_image(&bucket, &iter_id, "aws", "us-east-1", false)
                .await,
            Err(StampError::HcpApi(_))
        ));
        mock_bad_json.assert_async().await;

        // 8. Network error
        let net_client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            api_url: "http://127.0.0.1:1".to_string(),
            ..Default::default()
        });
        assert!(matches!(
            net_client
                .get_image(&bucket, &iter_id, "aws", "us-east-1", false)
                .await,
            Err(StampError::HcpApi(_))
        ));
        Ok(())
    }

    /// Tests revoke_iteration and activate_iteration under success and error circumstances.
    #[tokio::test]
    async fn test_revoke_and_activate_iteration() {
        let mut server = mockito::Server::new_async().await;

        let _revoke_mock = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/ubuntu/iterations/iter_1/revoke")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{}"#)
            .create_async()
            .await;

        let _complete_mock = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/ubuntu/iterations/iter_1/complete")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{}"#)
            .create_async()
            .await;

        let client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("mock-token".to_string()),
            organization_id: HcpOrganizationId("org".to_string()),
            project_id: HcpProjectId("proj".to_string()),
            api_url: server.url(),
            auth_url: format!("{}/oauth/token", server.url()),
            ..Default::default()
        });

        let bucket = HcpBucketName("ubuntu".to_string());
        let iteration = HcpIterationId("iter_1".to_string());

        assert!(
            client
                .revoke_iteration(&bucket, &iteration, "CVE found")
                .await
                .is_ok()
        );
        assert!(client.activate_iteration(&bucket, &iteration).await.is_ok());

        // Revoke 500 error
        let mock_rev_500 = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/ubuntu/iterations/iter_1/revoke")
            .with_status(500)
            .with_body("revoke failed")
            .create_async()
            .await;
        assert!(matches!(
            client.revoke_iteration(&bucket, &iteration, "CVE").await,
            Err(StampError::HcpApi(_))
        ));
        mock_rev_500.assert_async().await;

        // Activate 500 error
        let mock_act_500 = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/ubuntu/iterations/iter_1/complete")
            .with_status(500)
            .with_body("complete failed")
            .create_async()
            .await;
        assert!(matches!(
            client.activate_iteration(&bucket, &iteration).await,
            Err(StampError::HcpApi(_))
        ));
        mock_act_500.assert_async().await;

        // Network errors
        let net_client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("token".to_string()),
            api_url: "http://127.0.0.1:1".to_string(),
            ..Default::default()
        });
        assert!(matches!(
            net_client
                .revoke_iteration(&bucket, &iteration, "cve")
                .await,
            Err(StampError::HcpApi(_))
        ));
        assert!(matches!(
            net_client.activate_iteration(&bucket, &iteration).await,
            Err(StampError::HcpApi(_))
        ));
    }

    /// Tests the push_build_artifacts pipeline across all builder types and error points.
    #[tokio::test]
    async fn test_push_build_artifacts_pipeline() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let _create_iter_mock = server
            .mock(
                "POST",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/prod-images/iterations",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"iteration": {"id": "iter_pipeline", "status": "READY"}}"#)
            .create_async()
            .await;

        let _reg_mock = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/prod-images/iterations/iter_pipeline/images")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"image": {"id": "img_pipeline"}}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        let _chan_mock = server
            .mock("PATCH", "/packer/2021-04-30/organizations/org/projects/proj/buckets/prod-images/channels/production")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{}"#)
            .create_async()
            .await;

        let client = HcpRegistryClient::new(HcpClientConfig {
            auth_token: Some("mock-token".to_string()),
            organization_id: HcpOrganizationId("org".to_string()),
            project_id: HcpProjectId("proj".to_string()),
            api_url: server.url(),
            auth_url: format!("{}/oauth/token", server.url()),
            ..Default::default()
        });

        let registry = HcpPackerRegistryConfig {
            bucket_name: crate::template::BucketName("prod-images".to_string()),
            description: Some("Production golden images".to_string()),
            labels: HashMap::from([("env".to_string(), "prod".to_string())]),
            bucket_labels: HashMap::from([("team".to_string(), "infra".to_string())]),
            build_labels: HashMap::from([("ci".to_string(), "true".to_string())]),
            channels: vec!["production".to_string()],
        };

        let artifacts: Vec<Box<dyn Artifact>> = vec![
            Box::new(crate::artifact::MockArtifact {
                builder_id: "amazon-ebs.web".to_string(),
                id: "us-east-1:ami-0123456789abcdef0".to_string(),
                files: vec![],
            }),
            Box::new(crate::artifact::MockArtifact {
                builder_id: "amazon-ebs".to_string(),
                id: "ami-99998888".to_string(),
                files: vec![],
            }),
            Box::new(crate::artifact::MockArtifact {
                builder_id: "azure-arm.app".to_string(),
                id: "azure-img-123".to_string(),
                files: vec![],
            }),
            Box::new(crate::artifact::MockArtifact {
                builder_id: "googlecompute.core".to_string(),
                id: "gcp-img-123".to_string(),
                files: vec![],
            }),
            Box::new(crate::artifact::MockArtifact {
                builder_id: "docker".to_string(),
                id: "sha256:1234".to_string(),
                files: vec![],
            }),
            Box::new(crate::artifact::MockArtifact {
                builder_id: "other".to_string(),
                id: "custom-file".to_string(),
                files: vec![],
            }),
        ];

        let iter = client.push_build_artifacts(&registry, &artifacts).await?;
        assert_eq!(iter.id.0, "iter_pipeline");

        // Failure at create_iteration step
        let mock_create_fail = server
            .mock(
                "POST",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/prod-images/iterations",
            )
            .with_status(500)
            .with_body("create iter failed")
            .create_async()
            .await;
        assert!(matches!(
            client.push_build_artifacts(&registry, &artifacts).await,
            Err(StampError::HcpApi(_))
        ));
        mock_create_fail.assert_async().await;

        // Failure at register_image step
        let _create_ok2 = server
            .mock(
                "POST",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/prod-images/iterations",
            )
            .with_status(200)
            .with_body(r#"{"iteration": {"id": "iter_pipeline2"}}"#)
            .create_async()
            .await;
        let mock_reg_fail = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/prod-images/iterations/iter_pipeline2/images")
            .with_status(500)
            .with_body("reg img failed")
            .create_async()
            .await;
        assert!(matches!(
            client.push_build_artifacts(&registry, &artifacts).await,
            Err(StampError::HcpApi(_))
        ));
        mock_reg_fail.assert_async().await;

        // Failure at assign_channel step
        let _create_ok3 = server
            .mock(
                "POST",
                "/packer/2021-04-30/organizations/org/projects/proj/buckets/prod-images/iterations",
            )
            .with_status(200)
            .with_body(r#"{"iteration": {"id": "iter_pipeline3"}}"#)
            .create_async()
            .await;
        let _reg_ok3 = server
            .mock("POST", "/packer/2021-04-30/organizations/org/projects/proj/buckets/prod-images/iterations/iter_pipeline3/images")
            .with_status(200)
            .with_body(r#"{"image": {"id": "img_ok"}}"#)
            .expect_at_least(1)
            .create_async()
            .await;
        let mock_chan_fail = server
            .mock("PATCH", "/packer/2021-04-30/organizations/org/projects/proj/buckets/prod-images/channels/production")
            .with_status(500)
            .with_body("chan assign failed")
            .create_async()
            .await;
        assert!(matches!(
            client.push_build_artifacts(&registry, &artifacts).await,
            Err(StampError::HcpApi(_))
        ));
        mock_chan_fail.assert_async().await;
        Ok(())
    }
}
