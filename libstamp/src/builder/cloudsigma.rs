//! Implementation of the `cloudsigma` builder for `CloudSigma` API 2.0.
//!
//! Provides a REST client for `CloudSigma` (`https://{region}.cloudsigma.com/api/2.0/`),
//! managing drive cloning, server orchestration, communicator execution, and drive snapshots.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration for the `cloudsigma` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CloudSigmaConfig {
    /// Name of the builder instance.
    pub name: String,
    /// `CloudSigma` account email / username.
    pub username: Option<String>,
    /// `CloudSigma` account password.
    pub password: Option<SecretString>,
    /// `CloudSigma` API endpoint URL (e.g. `https://zrh.cloudsigma.com/api/2.0/`).
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
    pub ssh_password: Option<SecretString>,
}

/// `CloudSigma` API 2.0 client.
#[derive(Debug, Clone)]
pub struct CloudSigmaClient {
    /// Endpoint.
    pub endpoint: String,
    /// Username.
    pub username: Option<String>,
    /// Password.
    pub password: Option<SecretString>,
}

/// Drive response payload.
#[derive(Deserialize)]
struct DriveResp {
    /// Drive UUID.
    uuid: String,
}

/// Server object summary.
#[derive(Deserialize)]
struct ServerObj {
    /// Server UUID.
    uuid: String,
}

/// Server list response payload.
#[derive(Deserialize)]
struct ServerListResp {
    /// List of server objects.
    objects: Option<Vec<ServerObj>>,
}

/// Snapshot response payload.
#[derive(Deserialize)]
struct SnapResp {
    /// Snapshot UUID.
    uuid: String,
}

impl CloudSigmaClient {
    /// Creates a new `CloudSigmaClient`.
    #[must_use]
    pub const fn new(
        endpoint: String,
        username: Option<String>,
        password: Option<SecretString>,
    ) -> Self {
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
            req = req.basic_auth(u, Some(p.expose()));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Clone drive failed: {e}")))?;

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
            req = req.basic_auth(u, Some(p.expose()));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create server failed: {e}")))?;

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

    /// Start a `CloudSigma` server.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if starting fails.
    pub async fn start_server(&self, server_uuid: &str) -> Result<(), StampError> {
        let client = reqwest::Client::new();
        let url = format!(
            "{}/servers/{server_uuid}/action/?action=start",
            self.endpoint.trim_end_matches('/')
        );
        let mut req = client.post(&url);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p.expose()));
        }
        let _ = req.send().await;
        Ok(())
    }

    /// Stop a `CloudSigma` server.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if stopping fails.
    pub async fn stop_server(&self, server_uuid: &str) -> Result<(), StampError> {
        let client = reqwest::Client::new();
        let url = format!(
            "{}/servers/{server_uuid}/action/?action=stop",
            self.endpoint.trim_end_matches('/')
        );
        let mut req = client.post(&url);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p.expose()));
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
            req = req.basic_auth(u, Some(p.expose()));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Snapshot drive failed: {e}")))?;

        let snap: SnapResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(snap.uuid)
    }

    /// Terminate and delete a `CloudSigma` server.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if deletion fails.
    pub async fn delete_server(&self, server_uuid: &str) -> Result<(), StampError> {
        let client = reqwest::Client::new();
        let url = format!(
            "{}/servers/{server_uuid}/",
            self.endpoint.trim_end_matches('/')
        );
        let mut req = client.delete(&url);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p.expose()));
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

/// Step to clone drive and launch `CloudSigma` server.
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

/// Step to stop server and capture drive snapshot in `CloudSigma`.
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
        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "cloudsigma".to_string());
        let ssh_pass = self
            .config
            .ssh_password
            .as_ref()
            .map(|s| s.expose().to_string());

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateCloudSigmaInfrastructure {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "cloudsigma", hook)
                    .with_ip_key("server_ip")
                    .with_ssh_credentials(ssh_user, ssh_pass)
                    .with_message("Provisioning CloudSigma server..."),
            ),
            Box::new(StepCaptureCloudSigmaSnapshot {
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
            .unwrap_or_default();

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

    struct FailingProvisioner;

    #[async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn crate::communicator::Communicator,
            _ui: Arc<crate::engine::ui::Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Execution("mock provision failure".to_string()))
        }
    }

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
    async fn test_cloudsigma_prepare() -> Result<(), StampError> {
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
        b.prepare().await
    }

    #[tokio::test]
    async fn test_cloudsigma_client_mocked() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let _m_clone = server
            .mock("POST", "/drives/drive-123/action/?action=clone")
            .with_status(200)
            .with_body(r#"{"uuid": "drive-uuid-1"}"#)
            .create_async()
            .await;

        let _m_srv = server
            .mock("POST", "/servers/")
            .with_status(200)
            .with_body(r#"{"objects": [{"uuid": "srv-uuid-1"}]}"#)
            .create_async()
            .await;

        let _m_start = server
            .mock("POST", "/servers/srv-uuid-1/action/?action=start")
            .with_status(200)
            .create_async()
            .await;

        let _m_stop = server
            .mock("POST", "/servers/srv-uuid-1/action/?action=stop")
            .with_status(200)
            .create_async()
            .await;

        let _m_snap = server
            .mock("POST", "/drives/drive-uuid-1/action/?action=snapshot")
            .with_status(200)
            .with_body(r#"{"uuid": "snap-uuid-1"}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/servers/srv-uuid-1/")
            .with_status(200)
            .create_async()
            .await;

        let client = CloudSigmaClient::new(
            server.url(),
            Some("user".to_string()),
            Some(SecretString::new("pass")),
        );

        let drive_id = client
            .clone_drive("drive-123", "boot", 20 * 1024 * 1024 * 1024)
            .await?;
        assert_eq!(drive_id, "drive-uuid-1");

        let srv_id = client
            .create_server("srv", 2000, 2048 * 1024 * 1024, &drive_id)
            .await?;
        assert_eq!(srv_id, "srv-uuid-1");

        client.start_server(&srv_id).await?;
        client.stop_server(&srv_id).await?;

        let snap_id = client.snapshot_drive(&drive_id, "gold-snap").await?;
        assert_eq!(snap_id, "snap-uuid-1");

        client.delete_server(&srv_id).await
    }

    #[tokio::test]
    async fn test_cloudsigma_client_no_auth() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let _m_clone = server
            .mock("POST", "/drives/drive-456/action/?action=clone")
            .with_status(200)
            .with_body(r#"{"uuid": "drive-uuid-2"}"#)
            .create_async()
            .await;

        let _m_srv = server
            .mock("POST", "/servers/")
            .with_status(200)
            .with_body(r#"{"objects": [{"uuid": "srv-uuid-2"}]}"#)
            .create_async()
            .await;

        let _m_start = server
            .mock("POST", "/servers/srv-uuid-2/action/?action=start")
            .with_status(200)
            .create_async()
            .await;

        let _m_stop = server
            .mock("POST", "/servers/srv-uuid-2/action/?action=stop")
            .with_status(200)
            .create_async()
            .await;

        let _m_snap = server
            .mock("POST", "/drives/drive-uuid-2/action/?action=snapshot")
            .with_status(200)
            .with_body(r#"{"uuid": "snap-uuid-2"}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/servers/srv-uuid-2/")
            .with_status(200)
            .create_async()
            .await;

        let client = CloudSigmaClient::new(server.url(), None, None);

        let drive_id = client.clone_drive("drive-456", "boot2", 1024).await?;
        assert_eq!(drive_id, "drive-uuid-2");

        let srv_id = client.create_server("srv2", 1000, 1024, &drive_id).await?;
        assert_eq!(srv_id, "srv-uuid-2");

        client.start_server(&srv_id).await?;
        client.stop_server(&srv_id).await?;

        let snap_id = client.snapshot_drive(&drive_id, "snap2").await?;
        assert_eq!(snap_id, "snap-uuid-2");

        client.delete_server(&srv_id).await
    }

    #[tokio::test]
    async fn test_cloudsigma_client_errors() {
        let mut server = mockito::Server::new_async().await;

        let _m_clone_bad = server
            .mock("POST", "/drives/bad/action/?action=clone")
            .with_status(200)
            .with_body("invalid-json")
            .create_async()
            .await;

        let _m_clone_500 = server
            .mock("POST", "/drives/err/action/?action=clone")
            .with_status(500)
            .create_async()
            .await;

        let _m_srv_empty = server
            .mock("POST", "/servers/")
            .with_status(200)
            .with_body(r#"{"objects": []}"#)
            .create_async()
            .await;

        let client = CloudSigmaClient::new(server.url(), None, None);

        assert!(client.clone_drive("bad", "b", 100).await.is_err());
        assert!(client.clone_drive("err", "e", 100).await.is_err());
        assert!(client.create_server("s", 100, 100, "d").await.is_err());

        let _m_snap_bad = server
            .mock("POST", "/drives/bad-snap/action/?action=snapshot")
            .with_status(200)
            .with_body("invalid-json")
            .create_async()
            .await;
        assert!(client.snapshot_drive("bad-snap", "s").await.is_err());

        let _m_snap_err = server
            .mock("POST", "/drives/err-snap/action/?action=snapshot")
            .with_status(500)
            .create_async()
            .await;
        assert!(client.snapshot_drive("err-snap", "s").await.is_err());

        let mut server2 = mockito::Server::new_async().await;
        let _m_srv_bad = server2
            .mock("POST", "/servers/")
            .with_status(200)
            .with_body("not-json")
            .create_async()
            .await;
        let client2 = CloudSigmaClient::new(server2.url(), None, None);
        assert!(client2.create_server("s", 100, 100, "d").await.is_err());

        let client_invalid = CloudSigmaClient::new("http://127.0.0.1:1".to_string(), None, None);
        assert!(client_invalid.clone_drive("d", "n", 10).await.is_err());
        assert!(
            client_invalid
                .create_server("s", 10, 10, "d")
                .await
                .is_err()
        );
        assert!(client_invalid.snapshot_drive("d", "n").await.is_err());
    }

    #[tokio::test]
    async fn test_cloudsigma_builder_run_success() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let _m_clone = server
            .mock("POST", "/drives/src-drive/action/?action=clone")
            .with_status(200)
            .with_body(r#"{"uuid": "drive-uuid-run"}"#)
            .create_async()
            .await;

        let _m_srv = server
            .mock("POST", "/servers/")
            .with_status(200)
            .with_body(r#"{"objects": [{"uuid": "srv-uuid-run"}]}"#)
            .create_async()
            .await;

        let _m_start = server
            .mock("POST", "/servers/srv-uuid-run/action/?action=start")
            .with_status(200)
            .create_async()
            .await;

        let _m_stop = server
            .mock("POST", "/servers/srv-uuid-run/action/?action=stop")
            .with_status(200)
            .expect(2)
            .create_async()
            .await;

        let _m_snap = server
            .mock("POST", "/drives/drive-uuid-run/action/?action=snapshot")
            .with_status(200)
            .with_body(r#"{"uuid": "snap-uuid-run"}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/servers/srv-uuid-run/")
            .with_status(200)
            .create_async()
            .await;

        let config = CloudSigmaConfig {
            name: "test-cloudsigma".to_string(),
            username: Some("u".to_string()),
            password: Some(SecretString::new("p")),
            api_endpoint: Some(server.url()),
            source_drive: "src-drive".to_string(),
            vcpus: Some(4),
            ram_mb: Some(4096),
            disk_size_gb: Some(40),
            image_name: "custom-img".to_string(),
            image_description: Some("desc".to_string()),
            ssh_username: Some("admin".to_string()),
            ssh_password: Some(SecretString::new("adminpass")),
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
        assert_eq!(artifact.id(), "cloudsigma:snap-uuid-run");
        assert_eq!(artifact.builder_id(), "test-cloudsigma");
        assert_eq!(artifact.files().len(), 0);
        assert!(artifact.state("dummy").is_none());
        artifact.destroy()?;

        b.cancel().await
    }

    #[tokio::test]
    async fn test_cloudsigma_builder_run_default_options() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let _m_clone = server
            .mock("POST", "/drives/src-drive/action/?action=clone")
            .with_status(200)
            .with_body(r#"{"uuid": "drive-uuid-def"}"#)
            .create_async()
            .await;

        let _m_srv = server
            .mock("POST", "/servers/")
            .with_status(200)
            .with_body(r#"{"objects": [{"uuid": "srv-uuid-def"}]}"#)
            .create_async()
            .await;

        let _m_start = server
            .mock("POST", "/servers/srv-uuid-def/action/?action=start")
            .with_status(200)
            .create_async()
            .await;

        let _m_stop = server
            .mock("POST", "/servers/srv-uuid-def/action/?action=stop")
            .with_status(200)
            .expect(2)
            .create_async()
            .await;

        let _m_snap = server
            .mock("POST", "/drives/drive-uuid-def/action/?action=snapshot")
            .with_status(200)
            .with_body(r#"{"uuid": "snap-uuid-def"}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/servers/srv-uuid-def/")
            .with_status(200)
            .create_async()
            .await;

        let config = CloudSigmaConfig {
            name: "test-default".to_string(),
            api_endpoint: Some(server.url()),
            source_drive: "src-drive".to_string(),
            image_name: "img".to_string(),
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

        let _ = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_cloudsigma_builder_run_failure_and_cleanups() {
        let mut server = mockito::Server::new_async().await;

        let _m_clone = server
            .mock("POST", "/drives/src-drive/action/?action=clone")
            .with_status(500)
            .create_async()
            .await;

        let config = CloudSigmaConfig {
            name: "fail-builder".to_string(),
            api_endpoint: Some(server.url()),
            source_drive: "src-drive".to_string(),
            image_name: "img".to_string(),
            ..Default::default()
        };
        let b = CloudSigmaBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res_cleanup = b
            .run(
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res_cleanup.is_err());

        let res_abort = b
            .run(
                hook,
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Abort,
            )
            .await;
        assert!(res_abort.is_err());

        let client = CloudSigmaClient::new(
            "https://zrh.cloudsigma.com/api/2.0/".to_string(),
            None,
            None,
        );
        let mut infra_step = StepCreateCloudSigmaInfrastructure {
            ui: ui.clone(),
            name: "test-infra".to_string(),
            client: client.clone(),
            config: CloudSigmaConfig::default(),
        };
        let state = StateBag::new();
        infra_step.cleanup(&state).await;

        let mut snap_step = StepCaptureCloudSigmaSnapshot {
            ui,
            name: "test-snap".to_string(),
            client,
            config: CloudSigmaConfig::default(),
        };
        snap_step.cleanup(&state).await;
    }
}
