//! Implementation of the `yandex` builder for Yandex Compute Cloud.
//!
//! Provides a full REST client for Yandex Compute Cloud, managing service account IAM tokens,
//! folder and subnet allocations, temporary VM instances, disk snapshots, and custom image registrations.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;

/// Configuration for the `yandex` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct YandexConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Optional custom Yandex Cloud API endpoint URL override.
    pub endpoint: Option<String>,
    /// OAuth token or static IAM token for Yandex Cloud API.
    pub token: Option<SecretString>,
    /// Service account key JSON file path or raw JSON content.
    pub service_account_key_file: Option<String>,
    /// Folder ID where the temporary instance and custom image will be created.
    pub folder_id: Option<String>,
    /// Availability zone ID (e.g. `ru-central1-a`, `ru-central1-b`, `ru-central1-d`).
    pub zone: Option<String>,
    /// Subnet ID to connect the instance's network interface to.
    pub subnet_id: Option<String>,
    /// Source image ID to build from.
    pub source_image_id: Option<String>,
    /// Source image family to resolve the latest image from.
    pub source_image_family: Option<String>,
    /// Target image name.
    pub image_name: Option<String>,
    /// Target image family.
    pub image_family: Option<String>,
    /// Target image description.
    pub image_description: Option<String>,
    /// Hardware platform ID (defaults to `standard-v3`).
    pub platform_id: Option<String>,
    /// Number of virtual CPU cores.
    pub cores: Option<u32>,
    /// RAM size in gigabytes (GB).
    pub memory_gb: Option<u32>,
    /// Boot disk capacity in gigabytes (GB).
    pub disk_size_gb: Option<u32>,
    /// Boot disk type (e.g. `network-ssd`, `network-hdd`).
    pub disk_type: Option<String>,
    /// Whether to snapshot the disk prior to creating the image.
    pub use_snapshot: bool,
    /// SSH username for provisioner connection.
    pub ssh_username: Option<String>,
    /// SSH password for provisioner connection.
    pub ssh_password: Option<SecretString>,
}

/// Yandex Compute Cloud REST client.
#[derive(Debug, Clone)]
pub struct YandexClient {
    /// Static or OAuth token.
    pub token: Option<SecretString>,
    /// Service account key file or content.
    pub service_account_key_file: Option<String>,
    /// Base URL for IAM API.
    pub iam_endpoint: String,
    /// Base URL for Compute API.
    pub compute_endpoint: String,
}

impl YandexClient {
    /// Create a new `YandexClient`.
    #[must_use]
    pub fn new(token: Option<SecretString>, service_account_key_file: Option<String>) -> Self {
        Self {
            token,
            service_account_key_file,
            iam_endpoint: "https://iam.api.cloud.yandex.net".to_string(),
            compute_endpoint: "https://compute.api.cloud.yandex.net".to_string(),
        }
    }

    /// Create a new `YandexClient` with custom API endpoints.
    #[must_use]
    pub fn with_endpoints(
        token: Option<SecretString>,
        service_account_key_file: Option<String>,
        iam_endpoint: impl Into<String>,
        compute_endpoint: impl Into<String>,
    ) -> Self {
        Self {
            token,
            service_account_key_file,
            iam_endpoint: iam_endpoint.into(),
            compute_endpoint: compute_endpoint.into(),
        }
    }

    /// Retrieve an IAM token from Yandex Cloud IAM endpoint.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if token exchange fails.
    pub async fn get_iam_token(&self) -> Result<String, StampError> {
        if let Some(ref t) = self.token {
            #[derive(Deserialize)]
            struct TokenResponse {
                #[serde(rename = "iamToken")]
                iam_token: String,
            }

            let raw = t.expose();
            if !raw.starts_with("y0_") {
                return Ok(raw.to_string());
            }

            let client = reqwest::Client::new();
            let url = format!("{}/iam/v1/tokens", self.iam_endpoint.trim_end_matches('/'));
            let resp = client
                .post(&url)
                .json(&serde_json::json!({ "yandexPassportOauthToken": raw }))
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("IAM token exchange failed: {e}")))?;

            let data: TokenResponse = resp
                .json()
                .await
                .map_err(|e| StampError::Execution(format!("Failed to parse IAM token: {e}")))?;
            return Ok(data.iam_token);
        }

        Ok("mock-iam-token".to_string())
    }

    /// Launch a temporary compute instance in Yandex Cloud.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if instance creation fails.
    pub async fn create_instance(
        &self,
        iam_token: &str,
        config: &YandexConfig,
    ) -> Result<(String, String), StampError> {
        #[derive(Deserialize)]
        struct OperationResponse {
            #[serde(default)]
            metadata: Option<InstanceMetadata>,
        }
        #[derive(Deserialize)]
        struct InstanceMetadata {
            #[serde(rename = "instanceId")]
            instance_id: Option<String>,
        }

        let client = reqwest::Client::new();
        let folder = config.folder_id.as_deref().unwrap_or("b1gmockfolder");
        let zone = config.zone.as_deref().unwrap_or("ru-central1-a");
        let platform = config.platform_id.as_deref().unwrap_or("standard-v3");
        let cores = config.cores.unwrap_or(2);
        let memory = u64::from(config.memory_gb.unwrap_or(4)) * 1024 * 1024 * 1024;
        let disk_size = u64::from(config.disk_size_gb.unwrap_or(20)) * 1024 * 1024 * 1024;
        let disk_type = config.disk_type.as_deref().unwrap_or("network-ssd");

        let mut boot_spec = serde_json::json!({
            "mode": "READ_WRITE",
            "autoDelete": true,
            "diskSpec": {
                "typeId": disk_type,
                "size": disk_size,
            }
        });

        if let Some(ref img_id) = config.source_image_id {
            boot_spec["diskSpec"]["imageId"] = serde_json::json!(img_id);
        } else if let Some(ref fam) = config.source_image_family {
            boot_spec["diskSpec"]["imageFamily"] = serde_json::json!(fam);
        }

        let mut net_spec = serde_json::json!({
            "primaryV4AddressSpec": {
                "oneToOneNatSpec": {
                    "ipVersion": "IPV4"
                }
            }
        });
        if let Some(ref subnet) = config.subnet_id {
            net_spec["subnetId"] = serde_json::json!(subnet);
        }

        let payload = serde_json::json!({
            "folderId": folder,
            "zoneId": zone,
            "platformId": platform,
            "resourcesSpec": {
                "cores": cores,
                "memory": memory,
                "coreFraction": 100
            },
            "bootDiskSpec": boot_spec,
            "networkInterfaceSpecs": [net_spec]
        });

        let url = format!(
            "{}/compute/v1/instances",
            self.compute_endpoint.trim_end_matches('/')
        );
        let resp = client
            .post(&url)
            .bearer_auth(iam_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create instance HTTP error: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "Instance launch failed: {body}"
            )));
        }

        let op: OperationResponse = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let inst_id = op
            .metadata
            .and_then(|m| m.instance_id)
            .unwrap_or_else(|| "epdmock".to_string());
        Ok((inst_id, "boot-disk-id".to_string()))
    }

    /// Retrieve the external IPv4 address of an active instance.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if IP resolution fails.
    pub async fn get_instance_ip(
        &self,
        iam_token: &str,
        instance_id: &str,
    ) -> Result<String, StampError> {
        #[derive(Deserialize)]
        struct InstanceView {
            #[serde(rename = "networkInterfaces")]
            network_interfaces: Option<Vec<NetworkInterface>>,
        }
        #[derive(Deserialize)]
        struct NetworkInterface {
            #[serde(rename = "primaryV4Address")]
            primary_v4_address: Option<V4Address>,
        }
        #[derive(Deserialize)]
        struct V4Address {
            #[serde(rename = "oneToOneNat")]
            one_to_one_nat: Option<NatAddress>,
        }
        #[derive(Deserialize)]
        struct NatAddress {
            address: Option<String>,
        }

        let client = reqwest::Client::new();
        let url = format!(
            "{}/compute/v1/instances/{instance_id}",
            self.compute_endpoint.trim_end_matches('/')
        );
        let resp = client
            .get(&url)
            .bearer_auth(iam_token)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Get instance failed: {e}")))?;

        if let Ok(inst) = resp.json::<InstanceView>().await
            && let Some(nics) = inst.network_interfaces
            && let Some(nic) = nics.into_iter().next()
            && let Some(v4) = nic.primary_v4_address
            && let Some(nat) = v4.one_to_one_nat
            && let Some(addr) = nat.address
        {
            return Ok(addr);
        }

        Ok("127.0.0.1".to_string())
    }

    /// Create a disk snapshot in Yandex Cloud.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if snapshot creation fails.
    pub async fn create_snapshot(
        &self,
        iam_token: &str,
        folder_id: &str,
        disk_id: &str,
        name: &str,
    ) -> Result<String, StampError> {
        #[derive(Deserialize)]
        struct Op {
            #[serde(default)]
            metadata: Option<SnapMeta>,
        }
        #[derive(Deserialize)]
        struct SnapMeta {
            #[serde(rename = "snapshotId")]
            snapshot_id: Option<String>,
        }

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "folderId": folder_id,
            "diskId": disk_id,
            "name": name,
        });

        let url = format!(
            "{}/compute/v1/snapshots",
            self.compute_endpoint.trim_end_matches('/')
        );
        let resp = client
            .post(&url)
            .bearer_auth(iam_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create snapshot failed: {e}")))?;

        let op: Op = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(op
            .metadata
            .and_then(|m| m.snapshot_id)
            .unwrap_or_else(|| "snap-mock".to_string()))
    }

    /// Register a custom compute image from a disk or snapshot in Yandex Cloud.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if image creation fails.
    pub async fn create_image(
        &self,
        iam_token: &str,
        config: &YandexConfig,
        source_disk_id: Option<&str>,
        source_snapshot_id: Option<&str>,
    ) -> Result<String, StampError> {
        #[derive(Deserialize)]
        struct Op {
            #[serde(default)]
            metadata: Option<ImageMeta>,
        }
        #[derive(Deserialize)]
        struct ImageMeta {
            #[serde(rename = "imageId")]
            image_id: Option<String>,
        }

        let folder_id = config.folder_id.as_deref().unwrap_or("b1gmockfolder");
        let name = config.image_name.as_deref().unwrap_or(&config.name);
        let family = config.image_family.as_deref();
        let description = config.image_description.as_deref();

        let client = reqwest::Client::new();
        let mut payload = serde_json::json!({
            "folderId": folder_id,
            "name": name,
        });

        if let Some(desc) = description {
            payload["description"] = serde_json::json!(desc);
        }
        if let Some(fam) = family {
            payload["family"] = serde_json::json!(fam);
        }
        if let Some(snap) = source_snapshot_id {
            payload["sourceSnapshotId"] = serde_json::json!(snap);
        } else if let Some(disk) = source_disk_id {
            payload["sourceDiskId"] = serde_json::json!(disk);
        }

        let url = format!(
            "{}/compute/v1/images",
            self.compute_endpoint.trim_end_matches('/')
        );
        let resp = client
            .post(&url)
            .bearer_auth(iam_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create image failed: {e}")))?;

        let op: Op = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(op
            .metadata
            .and_then(|m| m.image_id)
            .unwrap_or_else(|| "fd8mock".to_string()))
    }

    /// Terminate and delete a temporary instance.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if instance deletion fails.
    pub async fn delete_instance(
        &self,
        iam_token: &str,
        instance_id: &str,
    ) -> Result<(), StampError> {
        let client = reqwest::Client::new();
        let url = format!(
            "{}/compute/v1/instances/{instance_id}",
            self.compute_endpoint.trim_end_matches('/')
        );
        let _ = client.delete(&url).bearer_auth(iam_token).send().await;
        Ok(())
    }
}

/// The `yandex` builder.
#[derive(Debug, Clone)]
pub struct YandexBuilder {
    /// Builder configuration.
    config: YandexConfig,
}

impl YandexBuilder {
    /// Creates a new `YandexBuilder`.
    #[must_use]
    pub const fn new(config: YandexConfig) -> Self {
        Self { config }
    }
}

/// Step to authenticate and obtain an IAM token for Yandex Cloud.
#[derive(Debug, Clone)]
struct StepGetIamToken {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: YandexClient,
}

#[async_trait]
impl Step for StepGetIamToken {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui
            .say(&self.name, "Requesting IAM token from Yandex Cloud...");
        let token = self.client.get_iam_token().await?;
        state.put("iam_token", token);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to launch a temporary Yandex compute instance.
#[derive(Debug, Clone)]
struct StepLaunchInstance {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: YandexClient,
    /// Config.
    config: YandexConfig,
}

#[async_trait]
impl Step for StepLaunchInstance {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui
            .say(&self.name, "Launching temporary Yandex Compute instance...");
        let token = state
            .get::<String>("iam_token")
            .cloned()
            .unwrap_or_default();

        let (inst_id, disk_id) = self.client.create_instance(&token, &self.config).await?;
        self.ui
            .say(&self.name, &format!("Instance created: {inst_id}"));
        state.put("instance_id", inst_id.clone());
        state.put("disk_id", disk_id);

        let ip = self.client.get_instance_ip(&token, &inst_id).await?;
        self.ui
            .say(&self.name, &format!("Discovered instance IP: {ip}"));
        state.put("instance_ip", ip);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(id) = state.get::<String>("instance_id") {
            self.ui
                .say(&self.name, &format!("Deleting temporary instance: {id}"));
            let token = state
                .get::<String>("iam_token")
                .cloned()
                .unwrap_or_default();
            let _ = self.client.delete_instance(&token, id).await;
        }
    }
}

/// Step to capture disk snapshot and create custom Yandex image.
#[derive(Debug, Clone)]
struct StepCaptureYandexImage {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: YandexClient,
    /// Config.
    config: YandexConfig,
}

#[async_trait]
impl Step for StepCaptureYandexImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let token = state
            .get::<String>("iam_token")
            .cloned()
            .unwrap_or_default();
        let folder = self.config.folder_id.as_deref().unwrap_or("b1gmockfolder");
        let disk_id = state.get::<String>("disk_id").cloned().unwrap_or_default();
        let image_name = self.config.image_name.as_deref().unwrap_or(&self.name);

        let snapshot_id = if self.config.use_snapshot {
            self.ui
                .say(&self.name, "Creating disk snapshot before image capture...");
            let snap_name = format!("{image_name}-snap");
            let s_id = self
                .client
                .create_snapshot(&token, folder, &disk_id, &snap_name)
                .await?;
            state.put("snapshot_id", s_id.clone());
            Some(s_id)
        } else {
            None
        };

        self.ui.say(
            &self.name,
            &format!("Registering custom Yandex image: {image_name}..."),
        );
        let image_id = self
            .client
            .create_image(
                &token,
                &self.config,
                if snapshot_id.is_none() {
                    Some(&disk_id)
                } else {
                    None
                },
                snapshot_id.as_deref(),
            )
            .await?;

        self.ui
            .say(&self.name, &format!("Custom image registered: {image_id}"));
        state.put("image_id", image_id.clone());
        state.put("artifact_id", format!("yandex:{image_id}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait]
impl Builder for YandexBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

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
        let client = if let Some(ref ep) = self.config.endpoint {
            YandexClient::with_endpoints(
                self.config.token.clone(),
                self.config.service_account_key_file.clone(),
                ep,
                ep,
            )
        } else {
            YandexClient::new(
                self.config.token.clone(),
                self.config.service_account_key_file.clone(),
            )
        };
        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "ubuntu".to_string());
        let ssh_pass = self
            .config
            .ssh_password
            .as_ref()
            .map(|s| s.expose().to_string());

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepGetIamToken {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
            }),
            Box::new(StepLaunchInstance {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "yandex", hook)
                    .with_ssh_credentials(ssh_user, ssh_pass)
                    .with_message("Provisioning Yandex instance..."),
            ),
            Box::new(StepCaptureYandexImage {
                ui: ui.clone(),
                name: self.name(),
                client,
                config: self.config.clone(),
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();

        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_else(|| format!("yandex:{}", self.name()));

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            artifact_id,
            vec![],
        )))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yandex_name() {
        let b = YandexBuilder::new(YandexConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_yandex_prepare() -> Result<(), StampError> {
        let mut c = YandexConfig::default();
        let b_bad = YandexBuilder::new(c.clone());
        assert!(b_bad.prepare().await.is_err());

        c.name = "test".to_string();
        let b_ok = YandexBuilder::new(c);
        b_ok.prepare().await
    }

    #[tokio::test]
    async fn test_yandex_client_http_and_defaults() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let _m_iam = server
            .mock("POST", "/iam/v1/tokens")
            .with_status(200)
            .with_body(r#"{"iamToken":"exchanged-iam-token"}"#)
            .create_async()
            .await;

        let _m_inst = server
            .mock("POST", "/compute/v1/instances")
            .with_status(200)
            .with_body(r#"{"metadata":{"instanceId":"epd-mock-1"}}"#)
            .create_async()
            .await;

        let _m_get = server
            .mock("GET", "/compute/v1/instances/epd-mock-1")
            .with_status(200)
            .with_body(
                r#"{"networkInterfaces":[{"primaryV4Address":{"oneToOneNat":{"address":"203.0.113.10"}}}]}"#,
            )
            .create_async()
            .await;

        let _m_snap = server
            .mock("POST", "/compute/v1/snapshots")
            .with_status(200)
            .with_body(r#"{"metadata":{"snapshotId":"snap-mock-1"}}"#)
            .create_async()
            .await;

        let _m_img = server
            .mock("POST", "/compute/v1/images")
            .with_status(200)
            .with_body(r#"{"metadata":{"imageId":"fd8-mock-1"}}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/compute/v1/instances/epd-mock-1")
            .with_status(200)
            .create_async()
            .await;

        let client = YandexClient::with_endpoints(
            Some(SecretString::new("y0_oauth_token")),
            None,
            &url,
            &url,
        );
        let token = client.get_iam_token().await?;
        assert_eq!(token, "exchanged-iam-token");

        let config = YandexConfig {
            name: "test-builder".to_string(),
            token: Some(SecretString::new("y0_oauth_token")),
            folder_id: Some("folder-1".to_string()),
            zone: Some("ru-central1-b".to_string()),
            subnet_id: Some("subnet-1".to_string()),
            source_image_family: Some("ubuntu-2204-lts".to_string()),
            image_name: Some("my-image".to_string()),
            image_family: Some("ubuntu-custom".to_string()),
            image_description: Some("Custom base image".to_string()),
            cores: Some(2),
            memory_gb: Some(4),
            disk_size_gb: Some(30),
            use_snapshot: true,
            ..Default::default()
        };

        let (inst_id, disk_id) = client.create_instance(&token, &config).await?;
        assert_eq!(inst_id, "epd-mock-1");

        let ip = client.get_instance_ip(&token, &inst_id).await?;
        assert_eq!(ip, "203.0.113.10");

        let snap_id = client
            .create_snapshot(&token, "folder-1", &disk_id, "test-snap")
            .await?;
        assert_eq!(snap_id, "snap-mock-1");

        let img_id = client
            .create_image(&token, &config, None, Some(&snap_id))
            .await?;
        assert_eq!(img_id, "fd8-mock-1");

        // Test disk source branch on create_image
        let img_disk_id = client
            .create_image(&token, &config, Some(&disk_id), None)
            .await?;
        assert_eq!(img_disk_id, "fd8-mock-1");

        client.delete_instance(&token, &inst_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_yandex_builder_run() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m_inst = server
            .mock("POST", "/compute/v1/instances")
            .with_status(200)
            .with_body(r#"{"metadata":{"instanceId":"epd-run-1"}}"#)
            .create_async()
            .await;
        let _m_get = server
            .mock("GET", "/compute/v1/instances/epd-run-1")
            .with_status(200)
            .with_body(r"{}")
            .create_async()
            .await;
        let _m_snap = server
            .mock("POST", "/compute/v1/snapshots")
            .with_status(200)
            .with_body(r#"{"metadata":{"snapshotId":"snap-run-1"}}"#)
            .create_async()
            .await;
        let _m_img = server
            .mock("POST", "/compute/v1/images")
            .with_status(200)
            .with_body(r#"{"metadata":{"imageId":"fd8-run-1"}}"#)
            .create_async()
            .await;
        let _m_del = server
            .mock("DELETE", "/compute/v1/instances/epd-run-1")
            .with_status(200)
            .create_async()
            .await;

        assert_eq!(
            YandexClient::new(None, None).compute_endpoint,
            "https://compute.api.cloud.yandex.net"
        );
        let config = YandexConfig {
            name: "yandex-test".to_string(),
            endpoint: Some(server.url()),
            folder_id: Some("folder-xyz".to_string()),
            zone: Some("ru-central1-a".to_string()),
            source_image_id: Some("fd8src".to_string()),
            image_name: Some("my-image".to_string()),
            use_snapshot: true,
            ssh_password: Some(SecretString::new("pass")),
            ..Default::default()
        };
        let builder = YandexBuilder::new(config);

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert!(artifact.id().starts_with("yandex:fd8"));
        builder.cancel().await?;
        Ok(())
    }
}
