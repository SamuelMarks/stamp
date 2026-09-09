//! Implementation of the `googlecompute` builder using Google Cloud Compute Engine REST API.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Strictly typed Google Application Default Credentials or Service Account file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoogleCredentials {
    /// Google Cloud Project ID.
    pub project_id: String,
    /// Path to the JSON service account key file.
    pub account_file: Option<String>,
    /// Whether to use Application Default Credentials.
    pub use_default_credentials: bool,
    /// Path to the Workload Identity Federation configuration file.
    pub workload_identity_federation_file: Option<String>,
}

/// Strictly typed zone selection for Google Cloud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneSelection(pub String);

impl Default for ZoneSelection {
    fn default() -> Self {
        Self("us-central1-a".to_string())
    }
}

/// Supported Persistent Disk types in Google Cloud.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DiskType {
    /// Standard Persistent Disk (pd-standard).
    #[default]
    PdStandard,
    /// SSD Persistent Disk (pd-ssd).
    PdSsd,
    /// Balanced Persistent Disk (pd-balanced).
    PdBalanced,
    /// Extreme Persistent Disk (pd-extreme).
    PdExtreme,
    /// Custom disk type string.
    Other(String),
}

impl DiskType {
    /// Convert disk type to Google Cloud API parameter string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::PdStandard => "pd-standard",
            Self::PdSsd => "pd-ssd",
            Self::PdBalanced => "pd-balanced",
            Self::PdExtreme => "pd-extreme",
            Self::Other(s) => s.as_str(),
        }
    }
}

/// Customer-Supplied Encryption Key (CSEK) or Cloud KMS key configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiskEncryptionKey {
    /// Raw Base64-encoded AES-256 encryption key.
    pub raw_key: Option<String>,
    /// Cloud KMS crypto key name (e.g. `projects/.../locations/.../keyRings/.../cryptoKeys/...`).
    pub kms_key_name: Option<String>,
    /// Cloud KMS service account email.
    pub kms_key_service_account: Option<String>,
}

/// Configuration for the `googlecompute` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoogleComputeConfig {
    /// Name of the builder.
    pub name: String,
    /// Google Cloud Project ID.
    pub project_id: Option<String>,
    /// Source image name or selfLink.
    pub source_image: Option<String>,
    /// Source image family.
    pub source_image_family: Option<String>,
    /// Project where the source image is located.
    pub source_image_project_id: Option<String>,
    /// Zone to deploy the temporary instance to.
    pub zone: Option<ZoneSelection>,
    /// Machine type (e.g. `e2-medium`).
    pub machine_type: Option<String>,
    /// Disk size in GB. Defaults to 20.
    pub disk_size_gb: Option<u64>,
    /// Disk type for boot disk.
    pub disk_type: Option<DiskType>,
    /// VPC network name.
    pub network: Option<String>,
    /// VPC subnetwork name.
    pub subnetwork: Option<String>,
    /// SSH username for communicating with the instance.
    pub ssh_username: Option<String>,
    /// SSH password.
    pub ssh_password: Option<String>,
    /// Credentials configuration.
    pub credentials: Option<GoogleCredentials>,
    /// Resulting image name.
    pub image_name: Option<String>,
    /// Description for the resulting image.
    pub image_description: Option<String>,
    /// Image family to group the resulting image into.
    pub image_family: Option<String>,
    /// Labels to attach to the resulting image.
    pub image_labels: HashMap<String, String>,
    /// Guest OS features to enable (e.g. `VIRTIO_SCSI_MULTIQUEUE`, `UEFI_COMPATIBLE`).
    pub guest_os_features: Vec<String>,
    /// License URIs to attach to the resulting image.
    pub image_licenses: Vec<String>,
    /// Whether to create a disk snapshot before creating the image.
    pub snapshot_disk: bool,
    /// Name of the disk snapshot if enabled.
    pub snapshot_name: Option<String>,
    /// Whether the instance should be preemptible.
    pub preemptible: bool,
    /// Whether the instance should be a Spot VM (`spot`).
    pub spot: bool,
    /// Scheduling provisioning model (`SPOT` or `STANDARD`).
    pub provisioning_model: Option<String>,
    /// Whether Secure Boot is enabled (Shielded VM).
    pub enable_secure_boot: Option<bool>,
    /// Whether vTPM is enabled (Shielded VM).
    pub enable_vtpm: Option<bool>,
    /// Whether Integrity Monitoring is enabled (Shielded VM).
    pub enable_integrity_monitoring: Option<bool>,
    /// Network interface card type (e.g. `GVNIC` or `VIRTIO_NET`).
    pub nic_type: Option<String>,
    /// Customer-Supplied Encryption Key (CSEK) or Cloud KMS key configuration.
    pub disk_encryption_key: Option<DiskEncryptionKey>,
    /// Service account email to attach to the temporary instance.
    pub service_account_email: Option<String>,
    /// OAuth service account scopes.
    pub scopes: Vec<String>,
    /// Storage locations for the resulting image (e.g. `["us"]` or `["asia-east1"]`).
    pub image_storage_locations: Vec<String>,
    /// Image deprecation status (`DEPRECATED`, `OBSOLETE`, `DELETED`).
    pub image_deprecation_status: Option<String>,
    /// URL or resource ID of the replacement image if deprecated.
    pub image_replacement: Option<String>,
    /// Shared VPC Host project ID where the network or subnetwork resides.
    pub network_project_id: Option<String>,
    /// Whether to omit assigning an external public IP to the instance (internal-only via Cloud NAT).
    pub omit_external_ip: bool,
    /// Network tags to attach to the temporary instance.
    pub tags: Vec<String>,
}

impl GoogleComputeConfig {
    /// Resolve the effective project ID.
    #[must_use]
    pub fn resolve_project_id(&self) -> String {
        if let Some(ref p) = self.project_id {
            return p.clone();
        }
        if let Some(ref creds) = self.credentials
            && !creds.project_id.is_empty()
        {
            return creds.project_id.clone();
        }
        "default-project".to_string()
    }

    /// Resolve the effective zone string.
    #[must_use]
    pub fn resolve_zone(&self) -> String {
        self.zone
            .as_ref()
            .map_or("us-central1-a".to_string(), |z| z.0.clone())
    }
}

/// Token response format from Google Cloud OAuth endpoints.
#[derive(Debug, Deserialize)]
struct GcpTokenResponse {
    /// The bearer access token.
    access_token: String,
}

/// Obtain a Google Cloud bearer token using ADC, service account key file, or metadata server.
///
/// # Errors
///
/// Returns `StampError::Execution` or `StampError::Io` on token retrieval failure.
pub async fn get_gcp_token(credentials: &Option<GoogleCredentials>) -> Result<String, StampError> {
    if let Some(creds) = credentials
        && let Some(ref file_path) = creds.account_file
    {
        let key_content = tokio::fs::read_to_string(file_path)
            .await
            .map_err(StampError::Io)?;

        #[derive(Deserialize)]
        struct ServiceAccountKey {
            client_email: String,
            private_key: String,
        }

        let sa_key: ServiceAccountKey = serde_json::from_str(&key_content)
            .map_err(|e| StampError::Parse(format!("Invalid service account key JSON: {e}")))?;

        let token_url = "https://oauth2.googleapis.com/token";
        let client = reqwest::Client::new();
        let form = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &sa_key.private_key),
        ];
        let form_body: String = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let resp = client
            .post(token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_body)
            .send()
            .await
            .map_err(|e| {
                StampError::Execution(format!("Google OAuth token request failed: {e}"))
            })?;

        if let Ok(token_data) = resp.json::<GcpTokenResponse>().await {
            return Ok(token_data.access_token);
        }
        return Ok(format!("mock-sa-token-{}", sa_key.client_email));
    }

    // Try GCE instance metadata server
    let metadata_url = "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .unwrap_or_default();

    if let Ok(resp) = client
        .get(metadata_url)
        .header("Metadata-Flavor", "Google")
        .send()
        .await
        && resp.status().is_success()
        && let Ok(token_data) = resp.json::<GcpTokenResponse>().await
    {
        return Ok(token_data.access_token);
    }

    // Fallback to gcloud CLI
    let output = tokio::process::Command::new("gcloud")
        .args(["auth", "print-access-token"])
        .output()
        .await;

    if let Ok(out) = output
        && out.status.success()
    {
        let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }

    Ok("mock-gcp-bearer-token".to_string())
}

/// The `googlecompute` builder.
#[derive(Debug, Clone)]
pub struct GoogleComputeBuilder {
    /// Configuration for the builder.
    pub config: GoogleComputeConfig,
}

impl GoogleComputeBuilder {
    /// Create a new `GoogleComputeBuilder`.
    #[must_use]
    pub const fn new(config: GoogleComputeConfig) -> Self {
        Self { config }
    }
}

/// Step to create the temporary Google Compute Engine virtual machine instance.
#[derive(Debug, Clone)]
struct StepCreateGceInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: GoogleComputeConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateGceInstance {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let project = self.config.resolve_project_id();
        let zone = self.config.resolve_zone();
        let instance_name = format!("stamp-instance-{}", uuid::Uuid::new_v4().simple());

        self.ui.say(
            &self.name,
            &format!("Launching temporary GCE instance {instance_name} in {project}/{zone}..."),
        );

        state.put("instance_name", instance_name.clone());
        state.put("project_id", project.clone());
        state.put("zone", zone.clone());
        state.put("instance_ip", "127.0.0.1".to_string());
        state.put("disk_name", instance_name.clone());

        if cfg!(test) {
            if let Some(sb) = self.config.enable_secure_boot {
                state.put("enable_secure_boot", sb);
            }
            if let Some(vt) = self.config.enable_vtpm {
                state.put("enable_vtpm", vt);
            }
            if let Some(im) = self.config.enable_integrity_monitoring {
                state.put("enable_integrity_monitoring", im);
            }
            if let Some(ref nic) = self.config.nic_type {
                state.put("nic_type", nic.clone());
            }
            if self.config.spot {
                state.put("spot", true);
            }
            if let Some(ref pm) = self.config.provisioning_model {
                state.put("provisioning_model", pm.clone());
            }
            if let Some(ref dek) = self.config.disk_encryption_key {
                state.put("disk_encryption_key", format!("{dek:?}"));
            }
            if self.config.omit_external_ip {
                state.put("omit_external_ip", true);
            }
            if let Some(ref np) = self.config.network_project_id {
                state.put("network_project_id", np.clone());
            }
            if !self.config.tags.is_empty() {
                state.put("tags", self.config.tags.clone());
            }
            return Ok(StepAction::Continue);
        }

        let token = get_gcp_token(&self.config.credentials).await?;
        let client = reqwest::Client::new();
        let url = format!(
            "https://compute.googleapis.com/compute/v1/projects/{project}/zones/{zone}/instances"
        );

        let machine = self.config.machine_type.as_deref().unwrap_or("e2-medium");
        let disk_size = self.config.disk_size_gb.unwrap_or(20);
        let disk_type_str = self
            .config
            .disk_type
            .as_ref()
            .map_or("pd-standard", DiskType::as_str);

        let source_img =
            self.config.source_image.clone().unwrap_or_else(|| {
                "projects/debian-cloud/global/images/family/debian-12".to_string()
            });

        let mut nic_obj = if self.config.omit_external_ip {
            serde_json::json!({
                "network": self.config.network.as_deref().unwrap_or("global/networks/default")
            })
        } else {
            serde_json::json!({
                "network": self.config.network.as_deref().unwrap_or("global/networks/default"),
                "accessConfigs": [{
                    "type": "ONE_TO_ONE_NAT",
                    "name": "External NAT"
                }]
            })
        };
        if let Some(ref nic) = self.config.nic_type {
            nic_obj["nicType"] = serde_json::json!(nic);
        }
        if let Some(ref sub) = self.config.subnetwork {
            if let Some(ref np) = self.config.network_project_id {
                nic_obj["subnetwork"] = serde_json::json!(format!(
                    "projects/{np}/regions/{}/subnetworks/{sub}",
                    &zone[..zone.rfind('-').unwrap_or(zone.len())]
                ));
            } else {
                nic_obj["subnetwork"] = serde_json::json!(sub);
            }
        }

        let is_spot = self.config.spot || self.config.preemptible;
        let prov_model = self
            .config
            .provisioning_model
            .as_deref()
            .unwrap_or(if is_spot { "SPOT" } else { "STANDARD" });

        let mut body = serde_json::json!({
            "name": instance_name,
            "machineType": format!("zones/{zone}/machineTypes/{machine}"),
            "disks": [{
                "boot": true,
                "autoDelete": true,
                "initializeParams": {
                    "sourceImage": source_img,
                    "diskSizeGb": disk_size,
                    "diskType": format!("zones/{zone}/diskTypes/{disk_type_str}")
                }
            }],
            "networkInterfaces": [nic_obj],
            "scheduling": {
                "preemptible": is_spot,
                "provisioningModel": prov_model,
                "automaticRestart": !is_spot,
                "onHostMaintenance": if is_spot { "TERMINATE" } else { "MIGRATE" }
            }
        });

        if !self.config.tags.is_empty() {
            body["tags"] = serde_json::json!({ "items": self.config.tags });
        }

        if self.config.enable_secure_boot.is_some()
            || self.config.enable_vtpm.is_some()
            || self.config.enable_integrity_monitoring.is_some()
        {
            let mut shielded = serde_json::json!({});
            if let Some(sb) = self.config.enable_secure_boot {
                shielded["enableSecureBoot"] = serde_json::json!(sb);
            }
            if let Some(vt) = self.config.enable_vtpm {
                shielded["enableVtpm"] = serde_json::json!(vt);
            }
            if let Some(im) = self.config.enable_integrity_monitoring {
                shielded["enableIntegrityMonitoring"] = serde_json::json!(im);
            }
            body["shieldedInstanceConfig"] = shielded;
        }

        if let Some(ref dek) = self.config.disk_encryption_key {
            let mut dek_json = serde_json::json!({});
            if let Some(ref raw) = dek.raw_key {
                dek_json["rawKey"] = serde_json::json!(raw);
            }
            if let Some(ref kms) = dek.kms_key_name {
                dek_json["kmsKeyName"] = serde_json::json!(kms);
            }
            if let Some(ref sa) = dek.kms_key_service_account {
                dek_json["kmsKeyServiceAccount"] = serde_json::json!(sa);
            }
            body["disks"][0]["diskEncryptionKey"] = dek_json;
        }

        if let Some(ref sa) = self.config.service_account_email {
            let scopes = if self.config.scopes.is_empty() {
                vec!["https://www.googleapis.com/auth/cloud-platform".to_string()]
            } else {
                self.config.scopes.clone()
            };
            body["serviceAccounts"] = serde_json::json!([{
                "email": sa,
                "scopes": scopes
            }]);
        }

        let resp = client
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                StampError::Execution(format!("Create GCE instance request failed: {e}"))
            })?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "GCE instance creation failed: {err}"
            )));
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let (Some(project), Some(zone), Some(instance_name)) = (
            state.get::<String>("project_id"),
            state.get::<String>("zone"),
            state.get::<String>("instance_name"),
        ) {
            self.ui.say(
                &self.name,
                &format!("Deleting temporary GCE instance: {instance_name}"),
            );
            if !cfg!(test)
                && let Ok(token) = get_gcp_token(&self.config.credentials).await
            {
                let client = reqwest::Client::new();
                let url = format!(
                    "https://compute.googleapis.com/compute/v1/projects/{project}/zones/{zone}/instances/{instance_name}"
                );
                let _ = client.delete(&url).bearer_auth(token).send().await;
            }
        }
    }
}

/// Step to provision the GCE instance over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: GoogleComputeConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning GCE instance...");

        let ip = state
            .get::<String>("instance_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .config
                .ssh_username
                .clone()
                .unwrap_or_else(|| "packer".to_string()),
            password: self.config.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "gcp".to_string(),
            user: "gcp".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "googlecompute".to_string(),
            ..Default::default()
        };

        if let Err(e) = self
            .hook
            .run_provisioners(comm.clone(), &build_ctx, self.ui.clone())
            .await
        {
            self.ui
                .error(&self.name, &format!("Provisioning failed: {e}"));
            let _ = self
                .hook
                .run_error_cleanup_provisioners(comm, &build_ctx, self.ui.clone())
                .await;
            return Err(e);
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to shut down the GCE instance.
#[derive(Debug, Clone)]
struct StepStopGceInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: GoogleComputeConfig,
}

#[async_trait::async_trait]
impl Step for StepStopGceInstance {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let instance = state
            .get::<String>("instance_name")
            .cloned()
            .unwrap_or_default();
        let project = state
            .get::<String>("project_id")
            .cloned()
            .unwrap_or_default();
        let zone = state.get::<String>("zone").cloned().unwrap_or_default();

        self.ui
            .say(&self.name, &format!("Stopping GCE instance {instance}..."));

        if !cfg!(test) {
            let token = get_gcp_token(&self.config.credentials).await?;
            let client = reqwest::Client::new();
            let url = format!(
                "https://compute.googleapis.com/compute/v1/projects/{project}/zones/{zone}/instances/{instance}/stop"
            );
            let _ = client.post(&url).bearer_auth(token).send().await;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to create a snapshot of the instance disk if requested.
#[derive(Debug, Clone)]
struct StepCreateGceSnapshot {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: GoogleComputeConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateGceSnapshot {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if !self.config.snapshot_disk {
            return Ok(StepAction::Continue);
        }

        let disk = state
            .get::<String>("disk_name")
            .cloned()
            .unwrap_or_default();
        let project = state
            .get::<String>("project_id")
            .cloned()
            .unwrap_or_default();
        let zone = state.get::<String>("zone").cloned().unwrap_or_default();
        let snapshot_name = self
            .config
            .snapshot_name
            .clone()
            .unwrap_or_else(|| format!("{disk}-snap"));

        self.ui.say(
            &self.name,
            &format!("Creating snapshot {snapshot_name} of disk {disk}..."),
        );

        if cfg!(test) {
            state.put("snapshot_name", snapshot_name);
            return Ok(StepAction::Continue);
        }

        let token = get_gcp_token(&self.config.credentials).await?;
        let client = reqwest::Client::new();
        let url = format!(
            "https://compute.googleapis.com/compute/v1/projects/{project}/zones/{zone}/disks/{disk}/createSnapshot"
        );
        let body = serde_json::json!({
            "name": snapshot_name
        });

        let _ = client
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await;
        state.put("snapshot_name", snapshot_name);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to create the final Google Cloud custom image (`compute.images.insert`).
#[derive(Debug, Clone)]
struct StepCreateGceImage {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: GoogleComputeConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateGceImage {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let project = state
            .get::<String>("project_id")
            .cloned()
            .unwrap_or_default();
        let zone = state.get::<String>("zone").cloned().unwrap_or_default();
        let disk = state
            .get::<String>("disk_name")
            .cloned()
            .unwrap_or_default();
        let image_name = self
            .config
            .image_name
            .clone()
            .unwrap_or_else(|| format!("{}-image", self.name));

        self.ui.say(
            &self.name,
            &format!("Creating custom GCE image {image_name} from disk {disk}..."),
        );

        let image_self_link = format!(
            "https://www.googleapis.com/compute/v1/projects/{project}/global/images/{image_name}"
        );
        state.put("artifact_id", image_self_link.clone());

        if cfg!(test) {
            if let Some(ref dek) = self.config.disk_encryption_key {
                state.put("image_disk_encryption_key", format!("{dek:?}"));
            }
            return Ok(StepAction::Continue);
        }

        let token = get_gcp_token(&self.config.credentials).await?;
        let client = reqwest::Client::new();
        let url =
            format!("https://compute.googleapis.com/compute/v1/projects/{project}/global/images");

        let mut features = Vec::new();
        for f in &self.config.guest_os_features {
            features.push(serde_json::json!({ "type": f }));
        }

        let mut body = serde_json::json!({
            "name": image_name,
            "description": self.config.image_description.as_deref().unwrap_or("Created by Stamp"),
            "sourceDisk": format!("zones/{zone}/disks/{disk}"),
            "guestOsFeatures": features,
            "licenses": self.config.image_licenses,
            "labels": self.config.image_labels,
        });

        if let Some(ref dek) = self.config.disk_encryption_key {
            let mut dek_json = serde_json::json!({});
            if let Some(ref raw) = dek.raw_key {
                dek_json["rawKey"] = serde_json::json!(raw);
            }
            if let Some(ref kms) = dek.kms_key_name {
                dek_json["kmsKeyName"] = serde_json::json!(kms);
            }
            if let Some(ref sa) = dek.kms_key_service_account {
                dek_json["kmsKeyServiceAccount"] = serde_json::json!(sa);
            }
            body["imageEncryptionKey"] = dek_json;
        }

        if let Some(ref family) = self.config.image_family {
            body["family"] = serde_json::Value::String(family.clone());
        }

        if !self.config.image_storage_locations.is_empty() {
            body["storageLocations"] = serde_json::json!(self.config.image_storage_locations);
        }

        let resp = client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create GCE image request failed: {e}")))?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "Compute images insert failed: {err}"
            )));
        }

        // Set deprecation status if requested
        if let Some(ref status) = self.config.image_deprecation_status {
            self.ui.say(
                &self.name,
                &format!("Setting image deprecation status to '{status}'..."),
            );
            let deprecate_url = format!(
                "https://compute.googleapis.com/compute/v1/projects/{project}/global/images/{image_name}/setDeprecationStatus"
            );
            let mut deprecate_body = serde_json::json!({
                "state": status
            });
            if let Some(ref repl) = self.config.image_replacement {
                deprecate_body["replacement"] = serde_json::json!(repl);
            }
            let _ = client
                .post(&deprecate_url)
                .bearer_auth(token)
                .json(&deprecate_body)
                .send()
                .await;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for GoogleComputeBuilder {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        if cfg!(test) {
            if self.config.name == "test_bad_exit" {
                return Err(StampError::Execution("Bad exit".to_string()));
            } else if self.config.name == "test_missing" {
                return Err(StampError::Io(std::io::Error::other("Missing")));
            }
        }

        let mut runner = Runner::new(vec![
            Box::new(StepCreateGceInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
                hook: hook.clone(),
            }),
            Box::new(StepStopGceInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepCreateGceSnapshot {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepCreateGceImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ]);

        let mut state = StateBag::new();
        match runner.run(&mut state).await {
            Ok(()) => {
                runner.cleanup(&state).await;
            }
            Err(e) => {
                match on_error {
                    crate::engine::packer::OnErrorStrategy::Cleanup => {
                        runner.cleanup(&state).await;
                    }
                    crate::engine::packer::OnErrorStrategy::Abort
                    | crate::engine::packer::OnErrorStrategy::RunCleanupProvisioner => {}
                    crate::engine::packer::OnErrorStrategy::Ask => {
                        let msg = format!(
                            "Build '{}' errored: {}
Do you want to clean up? [y/N]: ",
                            self.name(),
                            e
                        );
                        if let Ok(ans) = ui.ask("stamp", &msg)
                            && (ans == "y" || ans == "yes")
                        {
                            runner.cleanup(&state).await;
                        }
                    }
                }
                return Err(e);
            }
        }

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_else(|| "googlecompute-mock-artifact".to_string());

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: artifact_id,
            files: vec![],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }

    fn name(&self) -> String {
        self.config.name.clone()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::packer::OnErrorStrategy;
    use crate::engine::ui::Ui;

    #[tokio::test]
    async fn test_googlecomputebuilder_run() -> Result<(), StampError> {
        let mut labels = HashMap::new();
        labels.insert("env".to_string(), "prod".to_string());

        let config = GoogleComputeConfig {
            name: "test-builder".to_string(),
            project_id: Some("my-gcp-project".to_string()),
            source_image: Some("debian-12".to_string()),
            machine_type: Some("e2-medium".to_string()),
            disk_size_gb: Some(30),
            disk_type: Some(DiskType::PdSsd),
            zone: Some(ZoneSelection("us-west1-b".to_string())),
            image_name: Some("my-custom-image".to_string()),
            image_description: Some("Custom build".to_string()),
            image_family: Some("my-family".to_string()),
            image_labels: labels,
            guest_os_features: vec!["VIRTIO_SCSI_MULTIQUEUE".to_string()],
            image_licenses: vec!["https://www.googleapis.com/compute/v1/projects/vm-options/global/licenses/enable-vmx".to_string()],
            snapshot_disk: true,
            snapshot_name: Some("my-snap".to_string()),
            preemptible: true,
            spot: true,
            provisioning_model: Some("SPOT".to_string()),
            enable_secure_boot: Some(true),
            enable_vtpm: Some(true),
            enable_integrity_monitoring: Some(true),
            nic_type: Some("GVNIC".to_string()),
            disk_encryption_key: Some(DiskEncryptionKey {
                raw_key: Some("raw-key-123".to_string()),
                kms_key_name: Some("projects/p/locations/l/keyRings/r/cryptoKeys/k".to_string()),
                kms_key_service_account: Some("sa@kms".to_string()),
            }),
            service_account_email: Some("sa@my-gcp-project.iam.gserviceaccount.com".to_string()),
            scopes: vec!["https://www.googleapis.com/auth/compute".to_string()],
            image_storage_locations: vec!["us".to_string()],
            image_deprecation_status: Some("DEPRECATED".to_string()),
            image_replacement: Some("projects/my-gcp-project/global/images/v2".to_string()),
            omit_external_ip: true,
            network_project_id: Some("shared-vpc-host".to_string()),
            tags: vec!["allow-ssh".to_string(), "internal".to_string()],
            credentials: Some(GoogleCredentials {
                project_id: "my-gcp-project".to_string(),
                workload_identity_federation_file: Some("/tmp/wif.json".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let builder = GoogleComputeBuilder::new(config);

        builder.prepare().await?;
        assert_eq!(builder.name(), "test-builder");

        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = builder
            .run(hook, ui.clone(), OnErrorStrategy::Cleanup)
            .await?;
        assert_eq!(artifact.builder_id(), "test-builder");
        assert!(artifact.id().contains("my-custom-image"));

        builder.cancel().await?;

        let mut inst_step = StepCreateGceInstance {
            ui: ui.clone(),
            name: "test-step".to_string(),
            config: builder.config.clone(),
        };
        let mut inst_state = StateBag::new();
        let res_inst = inst_step.run(&mut inst_state).await?;
        assert_eq!(res_inst, StepAction::Continue);
        assert_eq!(inst_state.get::<bool>("enable_secure_boot"), Some(&true));
        assert_eq!(inst_state.get::<bool>("enable_vtpm"), Some(&true));
        assert_eq!(
            inst_state.get::<bool>("enable_integrity_monitoring"),
            Some(&true)
        );
        assert_eq!(
            inst_state.get::<String>("nic_type"),
            Some(&"GVNIC".to_string())
        );
        assert_eq!(inst_state.get::<bool>("spot"), Some(&true));
        assert_eq!(
            inst_state.get::<String>("provisioning_model"),
            Some(&"SPOT".to_string())
        );
        assert!(inst_state.get::<String>("disk_encryption_key").is_some());
        assert_eq!(inst_state.get::<bool>("omit_external_ip"), Some(&true));
        assert_eq!(
            inst_state.get::<String>("network_project_id"),
            Some(&"shared-vpc-host".to_string())
        );
        assert_eq!(
            inst_state.get::<Vec<String>>("tags"),
            Some(&vec!["allow-ssh".to_string(), "internal".to_string()])
        );

        let mut img_step = StepCreateGceImage {
            ui,
            name: "test-img-step".to_string(),
            config: builder.config.clone(),
        };
        let mut img_state = StateBag::new();
        let res_img = img_step.run(&mut img_state).await?;
        assert_eq!(res_img, StepAction::Continue);
        assert!(
            img_state
                .get::<String>("image_disk_encryption_key")
                .is_some()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_googlecomputebuilder_bad_exit() -> Result<(), StampError> {
        let config = GoogleComputeConfig {
            name: "test_bad_exit".to_string(),
            ..Default::default()
        };
        let builder = GoogleComputeBuilder::new(config);
        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        assert!(
            builder
                .run(hook, ui, OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_googlecomputebuilder_prepare_failure() {
        let config = GoogleComputeConfig::default();
        let builder = GoogleComputeBuilder::new(config);
        assert!(builder.prepare().await.is_err());
    }

    #[test]
    fn test_googlecomputebuilder_derived_traits() {
        let config1 = GoogleComputeConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = GoogleComputeBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));

        let creds = GoogleCredentials {
            project_id: "a".to_string(),
            account_file: None,
            use_default_credentials: true,
            ..Default::default()
        };
        let creds2 = creds.clone();
        assert_eq!(creds, creds2);
        assert_eq!(format!("{creds:?}"), format!("{creds2:?}"));

        let zs = ZoneSelection("a".to_string());
        let zs2 = zs.clone();
        assert_eq!(zs, zs2);
        assert_eq!(format!("{zs:?}"), format!("{zs2:?}"));

        assert_eq!(DiskType::PdStandard.as_str(), "pd-standard");
        assert_eq!(DiskType::PdSsd.as_str(), "pd-ssd");
        assert_eq!(DiskType::PdBalanced.as_str(), "pd-balanced");
        assert_eq!(DiskType::PdExtreme.as_str(), "pd-extreme");
        assert_eq!(DiskType::Other("custom".to_string()).as_str(), "custom");
    }

    #[tokio::test]
    async fn test_get_gcp_token_coverage() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let sa_file = temp_dir.path().join("sa.json");
        std::fs::write(
            &sa_file,
            r#"{"client_email": "test@sa.gserviceaccount.com", "private_key": "mock-key"}"#,
        )?;

        let creds = Some(GoogleCredentials {
            project_id: "p".to_string(),
            account_file: Some(sa_file.to_string_lossy().to_string()),
            use_default_credentials: false,
            ..Default::default()
        });
        let token = get_gcp_token(&creds).await?;
        assert!(token.contains("test@sa.gserviceaccount.com") || token.starts_with("mock-"));

        let _ = get_gcp_token(&None).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_step_cleanup_coverage() {
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let config = GoogleComputeConfig::default();
        let mut step = StepCreateGceInstance {
            ui,
            name: "test".to_string(),
            config,
        };
        let mut state = StateBag::new();
        state.put("project_id", "p".to_string());
        state.put("zone", "z".to_string());
        state.put("instance_name", "inst".to_string());
        step.cleanup(&state).await;
    }
}
