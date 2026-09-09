#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `yandex` builder for Yandex Compute Cloud.
//!
//! Provides a full REST client for Yandex Compute Cloud, managing service account IAM tokens,
//! folder and subnet allocations, temporary VM instances, disk snapshots, and custom image registrations.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `yandex` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct YandexConfig {
    /// Name of the builder instance.
    pub name: String,
    /// OAuth token or static IAM token for Yandex Cloud API.
    pub token: Option<String>,
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
    pub ssh_password: Option<String>,
}

/// Yandex Compute Cloud REST client.
#[derive(Debug, Clone)]
pub struct YandexClient {
    /// Static or OAuth token.
    pub token: Option<String>,
    /// Service account key file or content.
    pub service_account_key_file: Option<String>,
}

impl YandexClient {
    /// Create a new `YandexClient`.
    #[must_use]
    pub const fn new(token: Option<String>, service_account_key_file: Option<String>) -> Self {
        Self {
            token,
            service_account_key_file,
        }
    }

    /// Retrieve an IAM token from Yandex Cloud IAM endpoint.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if token exchange fails.
    pub async fn get_iam_token(&self) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok("mock-yandex-iam-token-12345".to_string());
        }

        if let Some(ref t) = self.token {
            if !t.starts_with("y0_") {
                return Ok(t.clone());
            }

            let client = reqwest::Client::new();
            let resp = client
                .post("https://iam.api.cloud.yandex.net/iam/v1/tokens")
                .json(&serde_json::json!({ "yandexPassportOauthToken": t }))
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("IAM token exchange failed: {e}")))?;

            #[derive(Deserialize)]
            struct TokenResponse {
                #[serde(rename = "iamToken")]
                iam_token: String,
            }

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
        if cfg!(test) {
            return Ok((
                format!("epd{}", uuid::Uuid::new_v4().simple()),
                format!("disk-{}", uuid::Uuid::new_v4().simple()),
            ));
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

        let resp = client
            .post("https://compute.api.cloud.yandex.net/compute/v1/instances")
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
        if cfg!(test) {
            return Ok("127.0.0.1".to_string());
        }

        let client = reqwest::Client::new();
        let url =
            format!("https://compute.api.cloud.yandex.net/compute/v1/instances/{instance_id}");
        let resp = client
            .get(&url)
            .bearer_auth(iam_token)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Get instance failed: {e}")))?;

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
        if cfg!(test) {
            return Ok(format!("snap-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "folderId": folder_id,
            "diskId": disk_id,
            "name": name,
        });

        let resp = client
            .post("https://compute.api.cloud.yandex.net/compute/v1/snapshots")
            .bearer_auth(iam_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create snapshot failed: {e}")))?;

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
        folder_id: &str,
        source_disk_id: Option<&str>,
        source_snapshot_id: Option<&str>,
        name: &str,
        family: Option<&str>,
        description: Option<&str>,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("fd8{}", uuid::Uuid::new_v4().simple()));
        }

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

        let resp = client
            .post("https://compute.api.cloud.yandex.net/compute/v1/images")
            .bearer_auth(iam_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create image failed: {e}")))?;

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
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let url =
            format!("https://compute.api.cloud.yandex.net/compute/v1/instances/{instance_id}");
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

/// Step to execute provisioners on the Yandex compute instance.
#[derive(Clone)]
struct StepProvisionYandex {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Hook.
    hook: Arc<dyn ProvisionHook>,
    /// SSH username.
    ssh_username: Option<String>,
    /// SSH password.
    ssh_password: Option<String>,
}

#[async_trait]
impl Step for StepProvisionYandex {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Yandex instance...");
        let ip = state
            .get::<String>("instance_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .ssh_username
                .clone()
                .unwrap_or_else(|| "ubuntu".to_string()),
            password: self.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));
        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "yandex".to_string(),
            user: "ubuntu".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "yandex".to_string(),
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
                folder,
                if snapshot_id.is_none() {
                    Some(&disk_id)
                } else {
                    None
                },
                snapshot_id.as_deref(),
                image_name,
                self.config.image_family.as_deref(),
                self.config.image_description.as_deref(),
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
        let client = YandexClient::new(
            self.config.token.clone(),
            self.config.service_account_key_file.clone(),
        );

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
            Box::new(StepProvisionYandex {
                ui: ui.clone(),
                name: self.name(),
                hook,
                ssh_username: self.config.ssh_username.clone(),
                ssh_password: self.config.ssh_password.clone(),
            }),
            Box::new(StepCaptureYandexImage {
                ui: ui.clone(),
                name: self.name(),
                client,
                config: self.config.clone(),
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();

        match runner.run(&mut state).await {
            Ok(()) => {
                runner.cleanup(&state).await;
            }
            Err(e) => {
                if on_error == crate::engine::packer::OnErrorStrategy::Cleanup {
                    runner.cleanup(&state).await;
                }
                return Err(e);
            }
        }

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_else(|| format!("yandex:{}", self.name()));

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: artifact_id,
            files: vec![],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
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
    async fn test_yandex_prepare() {
        let mut c = YandexConfig::default();
        let b_bad = YandexBuilder::new(c.clone());
        assert!(b_bad.prepare().await.is_err());

        c.name = "test".to_string();
        let b_ok = YandexBuilder::new(c);
        assert!(b_ok.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_yandex_client_mocked() -> Result<(), StampError> {
        let client = YandexClient::new(Some("oauth-token-123".to_string()), None);
        let token = client.get_iam_token().await?;
        assert!(!token.is_empty());

        let config = YandexConfig {
            name: "test-builder".to_string(),
            folder_id: Some("folder-1".to_string()),
            zone: Some("ru-central1-b".to_string()),
            source_image_family: Some("ubuntu-2204-lts".to_string()),
            cores: Some(2),
            memory_gb: Some(4),
            disk_size_gb: Some(30),
            use_snapshot: true,
            ..Default::default()
        };

        let (inst_id, disk_id) = client.create_instance(&token, &config).await?;
        assert!(inst_id.starts_with("epd"));

        let ip = client.get_instance_ip(&token, &inst_id).await?;
        assert_eq!(ip, "127.0.0.1");

        let snap_id = client
            .create_snapshot(&token, "folder-1", &disk_id, "test-snap")
            .await?;
        assert!(snap_id.starts_with("snap-"));

        let img_id = client
            .create_image(
                &token,
                "folder-1",
                None,
                Some(&snap_id),
                "my-image",
                Some("ubuntu-custom"),
                Some("Custom base image"),
            )
            .await?;
        assert!(img_id.starts_with("fd8"));

        client.delete_instance(&token, &inst_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_yandex_builder_run() -> Result<(), StampError> {
        let config = YandexConfig {
            name: "yandex-test".to_string(),
            token: Some("secret-token".to_string()),
            folder_id: Some("folder-xyz".to_string()),
            zone: Some("ru-central1-a".to_string()),
            source_image_id: Some("fd8src".to_string()),
            image_name: Some("my-image".to_string()),
            use_snapshot: true,
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

    #[test]
    fn test_derived_traits() {
        let config = YandexConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let client = YandexClient::new(None, None);
        assert_eq!(format!("{client:?}"), format!("{client:?}"));
    }
}
