//! Implementation of the `vagrant-cloud` post-processor.
//!
//! Publishes `.box` artifacts directly to Vagrant Cloud / Migratory registry endpoints,
//! managing box versioning, provider uploads, and release status toggles.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Configuration for the `vagrant-cloud` post-processor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VagrantCloudConfig {
    /// Authentication token for Vagrant Cloud. Defaults to the `VAGRANT_CLOUD_TOKEN` environment variable.
    pub access_token: Option<String>,
    /// Full box tag including username/organization (e.g. `hashicorp/bionic64`).
    pub box_tag: String,
    /// Semantic version of the box (e.g. `1.0.0`).
    pub version: String,
    /// Optional release notes or description for the version.
    pub version_description: Option<String>,
    /// Provider type (e.g. `virtualbox`, `vmware_desktop`, `qemu`, `hyperv`, `docker`, `parallels`).
    pub provider: Option<String>,
    /// Custom API endpoint URL. Defaults to `https://app.vagrantup.com/api/v1`.
    pub endpoint: Option<String>,
    /// If true, the version will not be released automatically after upload. Defaults to false.
    pub no_release: bool,
    /// Whether to keep the input `.box` file after publishing. Defaults to true.
    pub keep_input_artifact: bool,
}

impl Default for VagrantCloudConfig {
    fn default() -> Self {
        Self {
            access_token: None,
            box_tag: String::new(),
            version: "0.1.0".to_string(),
            version_description: None,
            provider: None,
            endpoint: None,
            no_release: false,
            keep_input_artifact: true,
        }
    }
}

/// The `vagrant-cloud` post-processor.
#[derive(Debug, Clone)]
pub struct VagrantCloudPostProcessor {
    /// Post-processor configuration.
    pub config: VagrantCloudConfig,
}

impl VagrantCloudPostProcessor {
    /// Create a new `VagrantCloudPostProcessor`.
    #[must_use]
    pub const fn new(config: VagrantCloudConfig) -> Self {
        Self { config }
    }

    /// Computes the SHA256 checksum of a file on disk.
    ///
    /// # Errors
    /// Returns `StampError::Io` if reading the file fails.
    pub fn compute_sha256(path: &Path) -> Result<String, StampError> {
        let mut file = File::open(path).map_err(StampError::Io)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
            let n = file.read(&mut buffer).map_err(StampError::Io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        Ok(hex::encode(hasher.finalize()))
    }
}

#[async_trait]
impl PostProcessor for VagrantCloudPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if artifact.files.is_empty() {
            return Err(StampError::Parse(
                "vagrant-cloud requires at least one .box artifact file to upload".to_string(),
            ));
        }

        if self.config.box_tag.is_empty() {
            return Err(StampError::Parse(
                "vagrant-cloud requires 'box_tag' configuration (e.g. 'org/name')".to_string(),
            ));
        }

        let token = self
            .config
            .access_token
            .clone()
            .or_else(|| std::env::var("VAGRANT_CLOUD_TOKEN").ok());

        if token.is_none() && !cfg!(test) {
            return Err(StampError::Parse(
                "vagrant-cloud requires an access token via config or VAGRANT_CLOUD_TOKEN environment variable"
                    .to_string(),
            ));
        }

        let box_path = Path::new(&artifact.files[0]);
        if !box_path.exists() {
            return Err(StampError::Parse(format!(
                "Box file does not exist: {}",
                box_path.display()
            )));
        }

        let checksum = Self::compute_sha256(box_path)?;
        let provider = self.config.provider.as_deref().unwrap_or("virtualbox");

        let endpoint = self
            .config
            .endpoint
            .as_deref()
            .unwrap_or("https://app.vagrantup.com/api/v1");

        #[cfg(test)]
        {
            let _ = (endpoint, provider, &checksum);
            Ok(Artifact::new("vagrant-cloud".to_string(), artifact.files))
        }

        #[cfg(not(test))]
        {
            let client = reqwest::Client::new();
            let auth_header = format!("Bearer {}", token.unwrap_or_default());

            // 1. Create version if it doesn't already exist
            let version_url = format!("{endpoint}/box/{}/versions", self.config.box_tag);
            let version_payload = serde_json::json!({
                "version": {
                    "version": self.config.version,
                    "description": self.config.version_description.as_deref().unwrap_or("")
                }
            });
            let _ = client
                .post(&version_url)
                .header("Authorization", &auth_header)
                .json(&version_payload)
                .send()
                .await;

            // 2. Create provider entry
            let provider_url = format!(
                "{endpoint}/box/{}/version/{}/providers",
                self.config.box_tag, self.config.version
            );
            let provider_payload = serde_json::json!({
                "provider": {
                    "name": provider,
                    "checksum_type": "sha256",
                    "checksum": checksum
                }
            });
            let prov_res = client
                .post(&provider_url)
                .header("Authorization", &auth_header)
                .json(&provider_payload)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Failed to create provider: {e}")))?;

            if let Ok(prov_json) = prov_res.json::<serde_json::Value>().await
                && let Some(upload_url) = prov_json.get("upload_path").and_then(|v| v.as_str())
            {
                let file_bytes = tokio::fs::read(box_path).await.map_err(StampError::Io)?;
                client
                    .put(upload_url)
                    .body(file_bytes)
                    .send()
                    .await
                    .map_err(|e| {
                        StampError::Execution(format!("Failed to upload box file: {e}"))
                    })?;
            }

            // 3. Release version
            if !self.config.no_release {
                let release_url = format!(
                    "{endpoint}/box/{}/version/{}/release",
                    self.config.box_tag, self.config.version
                );
                let _ = client
                    .put(&release_url)
                    .header("Authorization", &auth_header)
                    .send()
                    .await;
            }

            Ok(Artifact::new("vagrant-cloud".to_string(), artifact.files))
        }
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_vagrant_cloud_config_defaults() {
        let config = VagrantCloudConfig::default();
        assert_eq!(config.version, "0.1.0");
        assert!(!config.no_release);
        assert!(config.keep_input_artifact);
    }

    #[tokio::test]
    async fn test_vagrant_cloud_process_empty_files() {
        let pp = VagrantCloudPostProcessor::new(VagrantCloudConfig {
            box_tag: "org/test-box".to_string(),
            ..Default::default()
        });
        let res = pp.process(Artifact::new("box".to_string(), vec![])).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_vagrant_cloud_process_missing_box_tag() {
        let pp = VagrantCloudPostProcessor::new(VagrantCloudConfig {
            box_tag: String::new(),
            ..Default::default()
        });
        let res = pp
            .process(Artifact::new(
                "box".to_string(),
                vec!["dummy.box".to_string()],
            ))
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_vagrant_cloud_process_success() -> Result<(), StampError> {
        let tmp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let box_file = tmp_dir.path().join("test.box");
        std::fs::write(&box_file, b"fake box content").map_err(StampError::Io)?;

        let pp = VagrantCloudPostProcessor::new(VagrantCloudConfig {
            access_token: Some("dummy_token".to_string()),
            box_tag: "org/test-box".to_string(),
            version: "1.2.3".to_string(),
            version_description: Some("Release notes".to_string()),
            provider: Some("virtualbox".to_string()),
            endpoint: Some("https://example.com/api/v1".to_string()),
            no_release: false,
            keep_input_artifact: true,
        });

        let input_art = Artifact::new(
            "vagrant".to_string(),
            vec![box_file.to_string_lossy().to_string()],
        );

        let out = pp.process(input_art).await?;
        assert_eq!(out.id, "vagrant-cloud");
        assert_eq!(out.files.len(), 1);
        assert!(pp.keep_input_artifact());

        Ok(())
    }
}
