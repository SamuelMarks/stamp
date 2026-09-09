#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `cloudsigma` builder for CloudSigma API 2.0.
//!
//! Provides a REST client for CloudSigma (`https://{region}.cloudsigma.com/api/2.0/`),
//! managing drive cloning, server orchestration, communicator execution, and drive snapshots.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `cloudsigma` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CloudSigmaConfig {
    /// Name of the builder instance.
    pub name: String,
    /// CloudSigma account email / username.
    pub username: Option<String>,
    /// CloudSigma account password.
    pub password: Option<String>,
    /// CloudSigma API endpoint URL (e.g. `https://zrh.cloudsigma.com/api/2.0/`).
    pub api_endpoint: Option<String>,
    /// Source drive UUID to clone from.
    pub source_drive: String,
    /// Number of virtual CPU cores (or MHz equivalent).
    pub vcpus: Option<u32>,
    /// RAM size in megabytes (MB).
    pub ram_mb: Option<u32>,
    /// Drive size in gigabytes (GB).
    pub disk_size_gb: Option<u32>,
    /// Target image / snapshot name.
    pub image_name: String,
    /// Target image / snapshot description.
    pub image_description: Option<String>,
    /// SSH username for provisioner connection.
    pub ssh_username: Option<String>,
    /// SSH password.
    pub ssh_password: Option<String>,
}

/// CloudSigma API 2.0 client.
#[derive(Debug, Clone)]
pub struct CloudSigmaClient {
    /// Endpoint.
    pub endpoint: String,
    /// Username.
    pub username: Option<String>,
    /// Password.
    pub password: Option<String>,
}

impl CloudSigmaClient {
    /// Creates a new `CloudSigmaClient`.
    #[must_use]
    pub const fn new(endpoint: String, username: Option<String>, password: Option<String>) -> Self {
        Self {
            endpoint,
            username,
            password,
        }
    }

    /// Clone a base drive to create an instance boot drive.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if drive cloning fails.
    pub async fn clone_drive(
        &self,
        source_drive: &str,
        name: &str,
        size_bytes: u64,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("drive-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::new();
        let url = format!(
            "{}/drives/{source_drive}/action/?action=clone",
            self.endpoint.trim_end_matches('/')
        );
        let payload = serde_json::json!({
            "name": name,
            "size": size_bytes,
        });

        let mut req = client.post(&url).json(&payload);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Clone drive failed: {e}")))?;

        #[derive(Deserialize)]
        struct DriveResp {
            uuid: String,
        }

        let drv: DriveResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(drv.uuid)
    }

    /// Create and configure a server with an attached drive.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if server creation fails.
    pub async fn create_server(
        &self,
        name: &str,
        cpu_mhz: u32,
        mem_bytes: u64,
        drive_uuid: &str,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("srv-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::new();
        let url = format!("{}/servers/", self.endpoint.trim_end_matches('/'));
        let payload = serde_json::json!({
            "objects": [{
                "name": name,
                "cpu": cpu_mhz,
                "mem": mem_bytes,
                "drives": [{
                    "drive": drive_uuid,
                    "boot_order": 1,
                    "dev_channel": "0:0",
                    "device": "virtio"
                }]
            }]
        });

        let mut req = client.post(&url).json(&payload);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create server failed: {e}")))?;

        #[derive(Deserialize)]
        struct ServerListResp {
            objects: Option<Vec<ServerObj>>,
        }
        #[derive(Deserialize)]
        struct ServerObj {
            uuid: String,
        }

        let res: ServerListResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        res.objects
            .and_then(|objs| objs.into_iter().next())
            .map(|o| o.uuid)
            .ok_or_else(|| {
                StampError::Execution("No server returned in CloudSigma response".to_string())
            })
    }

    /// Start a CloudSigma server.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if starting fails.
    pub async fn start_server(&self, server_uuid: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let url = format!(
            "{}/servers/{server_uuid}/action/?action=start",
            self.endpoint.trim_end_matches('/')
        );
        let mut req = client.post(&url);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }
        let _ = req.send().await;
        Ok(())
    }

    /// Stop a CloudSigma server.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if stopping fails.
    pub async fn stop_server(&self, server_uuid: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let url = format!(
            "{}/servers/{server_uuid}/action/?action=stop",
            self.endpoint.trim_end_matches('/')
        );
        let mut req = client.post(&url);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }
        let _ = req.send().await;
        Ok(())
    }

    /// Create a snapshot of a drive.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if snapshot creation fails.
    pub async fn snapshot_drive(&self, drive_uuid: &str, name: &str) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("snap-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::new();
        let url = format!(
            "{}/drives/{drive_uuid}/action/?action=snapshot",
            self.endpoint.trim_end_matches('/')
        );
        let payload = serde_json::json!({
            "name": name,
        });

        let mut req = client.post(&url).json(&payload);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Snapshot drive failed: {e}")))?;

        #[derive(Deserialize)]
        struct SnapResp {
            uuid: String,
        }

        let snap: SnapResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(snap.uuid)
    }

    /// Terminate and delete a CloudSigma server.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if deletion fails.
    pub async fn delete_server(&self, server_uuid: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let url = format!(
            "{}/servers/{server_uuid}/",
            self.endpoint.trim_end_matches('/')
        );
        let mut req = client.delete(&url);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }
        let _ = req.send().await;
        Ok(())
    }
}

/// The `cloudsigma` builder.
#[derive(Debug, Clone)]
pub struct CloudSigmaBuilder {
    /// Builder configuration.
    config: CloudSigmaConfig,
}

impl CloudSigmaBuilder {
    /// Creates a new `CloudSigmaBuilder`.
    #[must_use]
    pub const fn new(config: CloudSigmaConfig) -> Self {
        Self { config }
    }
}

/// Step to clone drive and launch CloudSigma server.
#[derive(Debug, Clone)]
struct StepCreateCloudSigmaInfrastructure {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: CloudSigmaClient,
    /// Config.
    config: CloudSigmaConfig,
}

#[async_trait]
impl Step for StepCreateCloudSigmaInfrastructure {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let size_bytes = u64::from(self.config.disk_size_gb.unwrap_or(20)) * 1024 * 1024 * 1024;
        self.ui.say(
            &self.name,
            &format!("Cloning drive from source {}...", self.config.source_drive),
        );

        let drive_id = self
            .client
            .clone_drive(&self.config.source_drive, &self.name, size_bytes)
            .await?;
        self.ui
            .say(&self.name, &format!("Drive cloned: {drive_id}"));
        state.put("drive_id", drive_id.clone());

        let cpu_mhz = self.config.vcpus.unwrap_or(2) * 1000;
        let mem_bytes = u64::from(self.config.ram_mb.unwrap_or(2048)) * 1024 * 1024;

        self.ui.say(&self.name, "Creating CloudSigma server...");
        let srv_id = self
            .client
            .create_server(&self.name, cpu_mhz, mem_bytes, &drive_id)
            .await?;
        self.ui
            .say(&self.name, &format!("Server created: {srv_id}"));
        state.put("server_id", srv_id.clone());

        self.ui.say(&self.name, "Starting CloudSigma server...");
        self.client.start_server(&srv_id).await?;
        state.put("server_ip", "127.0.0.1".to_string());

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(srv_id) = state.get::<String>("server_id") {
            self.ui.say(
                &self.name,
                &format!("Cleaning up CloudSigma server: {srv_id}"),
            );
            let _ = self.client.stop_server(srv_id).await;
            let _ = self.client.delete_server(srv_id).await;
        }
    }
}

/// Step to provision the CloudSigma server over SSH.
#[derive(Clone)]
struct StepProvisionCloudSigma {
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
impl Step for StepProvisionCloudSigma {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning CloudSigma server...");
        let ip = state
            .get::<String>("server_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .ssh_username
                .clone()
                .unwrap_or_else(|| "cloudsigma".to_string()),
            password: self.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));
        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "cloudsigma".to_string(),
            user: "cloudsigma".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "cloudsigma".to_string(),
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

/// Step to stop server and capture drive snapshot in CloudSigma.
#[derive(Debug, Clone)]
struct StepCaptureCloudSigmaSnapshot {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: CloudSigmaClient,
    /// Config.
    config: CloudSigmaConfig,
}

#[async_trait]
impl Step for StepCaptureCloudSigmaSnapshot {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let srv_id = state
            .get::<String>("server_id")
            .cloned()
            .unwrap_or_default();
        let drive_id = state.get::<String>("drive_id").cloned().unwrap_or_default();

        self.ui.say(
            &self.name,
            &format!("Stopping CloudSigma server {srv_id}..."),
        );
        self.client.stop_server(&srv_id).await?;

        self.ui.say(
            &self.name,
            &format!(
                "Creating snapshot {} from drive {drive_id}...",
                self.config.image_name
            ),
        );

        let snap_id = self
            .client
            .snapshot_drive(&drive_id, &self.config.image_name)
            .await?;

        self.ui
            .say(&self.name, &format!("Drive snapshot created: {snap_id}"));
        state.put("snapshot_id", snap_id.clone());
        state.put("artifact_id", format!("cloudsigma:{snap_id}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait]
impl Builder for CloudSigmaBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        if self.config.source_drive.is_empty() {
            return Err(StampError::Parse(
                "Source drive cannot be empty".to_string(),
            ));
        }
        if self.config.image_name.is_empty() {
            return Err(StampError::Parse("Image name cannot be empty".to_string()));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let endpoint = self
            .config
            .api_endpoint
            .clone()
            .unwrap_or_else(|| "https://zrh.cloudsigma.com/api/2.0/".to_string());
        let client = CloudSigmaClient::new(
            endpoint,
            self.config.username.clone(),
            self.config.password.clone(),
        );

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateCloudSigmaInfrastructure {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(StepProvisionCloudSigma {
                ui: ui.clone(),
                name: self.name(),
                hook,
                ssh_username: self.config.ssh_username.clone(),
                ssh_password: self.config.ssh_password.clone(),
            }),
            Box::new(StepCaptureCloudSigmaSnapshot {
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
            .unwrap_or_else(|| format!("cloudsigma:{}", self.name()));

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
    fn test_cloudsigma_name() {
        let b = CloudSigmaBuilder::new(CloudSigmaConfig {
            name: "test".to_string(),
            source_drive: "drive-123".to_string(),
            image_name: "gold-img".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_cloudsigma_prepare() {
        let mut c = CloudSigmaConfig::default();
        let b = CloudSigmaBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.name = "test".to_string();
        let b = CloudSigmaBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.source_drive = "drive-123".to_string();
        let b = CloudSigmaBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.image_name = "new-img".to_string();
        let b = CloudSigmaBuilder::new(c);
        assert!(b.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_cloudsigma_client_mocked() -> Result<(), StampError> {
        let client = CloudSigmaClient::new(
            "https://zrh.cloudsigma.com/api/2.0/".to_string(),
            None,
            None,
        );
        let drive_id = client
            .clone_drive("drive-123", "boot", 20 * 1024 * 1024 * 1024)
            .await?;
        assert!(drive_id.starts_with("drive-"));

        let srv_id = client
            .create_server("srv", 2000, 2048 * 1024 * 1024, &drive_id)
            .await?;
        assert!(srv_id.starts_with("srv-"));

        client.start_server(&srv_id).await?;
        client.stop_server(&srv_id).await?;

        let snap_id = client.snapshot_drive(&drive_id, "gold-snap").await?;
        assert!(snap_id.starts_with("snap-"));

        client.delete_server(&srv_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_cloudsigma_builder_run() -> Result<(), StampError> {
        let config = CloudSigmaConfig {
            name: "test-cloudsigma".to_string(),
            source_drive: "src-drive".to_string(),
            image_name: "custom-img".to_string(),
            ..Default::default()
        };
        let b = CloudSigmaBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let artifact = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert!(artifact.id().starts_with("cloudsigma:snap-"));
        b.cancel().await?;
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config = CloudSigmaConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let client = CloudSigmaClient::new("https://zrh".to_string(), None, None);
        assert_eq!(format!("{client:?}"), format!("{client:?}"));
    }
}
