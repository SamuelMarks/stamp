//! Strongly-typed identifiers and domain models for the `HashiCorp` Cloud Platform (HCP) Packer Registry.

use crate::types::SecretString;
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
    pub client_secret: Option<SecretString>,
    /// Direct Bearer token override, if already obtained.
    pub auth_token: Option<SecretString>,
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

/// Parses artifact identifier strings to infer cloud provider, region, and clean image ID.
#[must_use]
pub fn parse_artifact_metadata(builder_id: &str, artifact_id: &str) -> (String, String, String) {
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
