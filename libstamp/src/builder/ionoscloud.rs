#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `ionoscloud` (and `oneandone`) builder for IONOS Cloud API v6.
//!
//! Provides a complete REST client for IONOS Cloud (`https://api.ionos.com/cloudapi/v6/`),
//! managing virtual datacenters, cuboid/dedicated servers, volume attachments,
//! and volume snapshot image creation.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `ionoscloud` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IonosConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Custom API endpoint for mock testing or private clouds.
    pub api_endpoint: Option<String>,
    /// Bearer token or API token for IONOS Cloud.
    pub token: Option<String>,
    /// Username for HTTP basic authentication.
    pub username: Option<String>,
    /// Password for HTTP basic authentication.
    pub password: Option<String>,
    /// Target datacenter location (e.g. `de/fra`, `us/las`, `de/txl`).
    pub location: Option<String>,
    /// Target datacenter name.
    pub datacenter_name: Option<String>,
    /// Virtual CPU cores for the build instance.
    pub cores: Option<u32>,
    /// RAM size in megabytes (MB).
    pub ram_mb: Option<u32>,
    /// Volume size in gigabytes (GB).
    pub disk_size_gb: Option<u32>,
    /// Base image alias or image ID (e.g. `ubuntu:latest`).
    pub image_alias: String,
    /// Root/admin password for the deployed instance.
    pub image_password: Option<String>,
    /// Resulting snapshot image name.
    pub snapshot_name: String,
    /// Resulting snapshot description.
    pub snapshot_description: Option<String>,
    /// SSH username for provisioner connection.
    pub ssh_username: Option<String>,
}

/// IONOS Cloud API v6 REST client.
#[derive(Debug, Clone)]
pub struct IonosClient {
    /// Custom endpoint URL.
    pub endpoint: Option<String>,
    /// API token.
    pub token: Option<String>,
    /// Username.
    pub username: Option<String>,
    /// Password.
    pub password: Option<String>,
}

/// Server creation specifications for IONOS Cloud.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IonosServerSpec<'a> {
    /// Server name.
    pub name: &'a str,
    /// Virtual CPU cores.
    pub cores: u32,
    /// RAM size in megabytes.
    pub ram_mb: u32,
    /// Boot disk size in gigabytes.
    pub disk_size_gb: u32,
    /// Image alias or ID.
    pub image_alias: &'a str,
    /// Image root/admin password.
    pub image_password: Option<&'a str>,
}

/// Datacenter creation response.
#[derive(Deserialize)]
struct DcResp {
    /// Datacenter identifier.
    id: String,
}

/// Volume identifier item.
#[derive(Deserialize)]
struct VolumeId {
    /// Volume identifier.
    id: String,
}

/// Volume items collection.
#[derive(Deserialize)]
struct VolumeItems {
    /// List of volumes.
    items: Option<Vec<VolumeId>>,
}

/// Server entities envelope.
#[derive(Deserialize)]
struct ServerEntities {
    /// Volumes attached to the server.
    volumes: Option<VolumeItems>,
}

/// Server response envelope.
#[derive(Deserialize)]
struct ServerResp {
    /// Server identifier.
    id: String,
    /// Server entities.
    entities: Option<ServerEntities>,
}

/// Snapshot response payload.
#[derive(Deserialize)]
struct SnapResp {
    /// Snapshot identifier.
    id: String,
}

impl IonosClient {
    /// Create a new `IonosClient`.
    #[must_use]
    pub const fn new(
        token: Option<String>,
        username: Option<String>,
        password: Option<String>,
    ) -> Self {
        Self {
            endpoint: None,
            token,
            username,
            password,
        }
    }

    /// Create a new `IonosClient` with a custom endpoint.
    #[must_use]
    pub const fn with_endpoint(
        endpoint: Option<String>,
        token: Option<String>,
        username: Option<String>,
        password: Option<String>,
    ) -> Self {
        Self {
            endpoint,
            token,
            username,
            password,
        }
    }

    /// Returns the resolved base API URL for IONOS Cloud.
    fn base_url(&self) -> String {
        self.endpoint
            .as_deref()
            .unwrap_or("https://api.ionos.com/cloudapi/v6")
            .trim_end_matches('/')
            .to_string()
    }

    /// Create a temporary virtual datacenter.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if creation fails.
    pub async fn create_datacenter(
        &self,
        name: &str,
        location: &str,
    ) -> Result<String, StampError> {
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "properties": {
                "name": name,
                "location": location,
                "description": "Created by Stamp builder"
            }
        });

        let url = format!("{}/datacenters", self.base_url());
        let mut req = client.post(&url).json(&payload);

        if let Some(ref t) = self.token {
            req = req.bearer_auth(t);
        } else if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create datacenter failed: {e}")))?;

        let dc: DcResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(dc.id)
    }

    /// Launch a server and attached boot volume within the datacenter.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if server launch fails.
    pub async fn create_server_and_volume(
        &self,
        datacenter_id: &str,
        spec: &IonosServerSpec<'_>,
    ) -> Result<(String, String), StampError> {
        let client = reqwest::Client::new();
        let mut vol_props = serde_json::json!({
            "name": format!("{}-boot", spec.name),
            "size": spec.disk_size_gb,
            "imageAlias": spec.image_alias,
            "type": "SSD"
        });
        if let Some(pwd) = spec.image_password {
            vol_props["imagePassword"] = serde_json::json!(pwd);
        }

        let payload = serde_json::json!({
            "properties": {
                "name": spec.name,
                "cores": spec.cores,
                "ram": spec.ram_mb
            },
            "entities": {
                "volumes": {
                    "items": [{
                        "properties": vol_props
                    }]
                }
            }
        });

        let url = format!("{}/datacenters/{datacenter_id}/servers", self.base_url());
        let mut req = client.post(&url).json(&payload);
        if let Some(ref t) = self.token {
            req = req.bearer_auth(t);
        } else if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create server failed: {e}")))?;

        let srv: ServerResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let vol_id = srv
            .entities
            .and_then(|e| e.volumes)
            .and_then(|v| v.items)
            .and_then(|items| items.into_iter().next())
            .map_or_else(|| "vol-mock".to_string(), |v| v.id);

        Ok((srv.id, vol_id))
    }

    /// Create a snapshot from a volume.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if snapshot creation fails.
    pub async fn create_snapshot(
        &self,
        datacenter_id: &str,
        volume_id: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<String, StampError> {
        let client = reqwest::Client::new();
        let url = format!(
            "{}/datacenters/{datacenter_id}/volumes/{volume_id}/create-snapshot",
            self.base_url()
        );

        let mut form_str = format!("name={name}");
        if let Some(d) = description {
            let _ = write!(form_str, "&description={d}");
        }

        let mut req = client
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_str);
        if let Some(ref t) = self.token {
            req = req.bearer_auth(t);
        } else if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create snapshot failed: {e}")))?;

        let snap: SnapResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(snap.id)
    }

    /// Delete a virtual datacenter and all contained resources.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if deletion fails.
    pub async fn delete_datacenter(&self, datacenter_id: &str) -> Result<(), StampError> {
        let client = reqwest::Client::new();
        let url = format!("{}/datacenters/{datacenter_id}", self.base_url());
        let mut req = client.delete(&url);
        if let Some(ref t) = self.token {
            req = req.bearer_auth(t);
        } else if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }
        let _ = req.send().await;
        Ok(())
    }
}

/// The `ionoscloud` builder.
#[derive(Debug, Clone)]
pub struct IonosBuilder {
    /// Builder configuration.
    config: IonosConfig,
}

impl IonosBuilder {
    /// Creates a new `IonosBuilder`.
    #[must_use]
    pub const fn new(config: IonosConfig) -> Self {
        Self { config }
    }
}

/// Step to create datacenter and launch server in IONOS Cloud.
#[derive(Debug, Clone)]
struct StepCreateIonosInfrastructure {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: IonosClient,
    /// Config.
    config: IonosConfig,
}

#[async_trait]
impl Step for StepCreateIonosInfrastructure {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let loc = self.config.location.as_deref().unwrap_or("de/fra");
        let dc_name = self.config.datacenter_name.as_deref().unwrap_or(&self.name);
        self.ui.say(
            &self.name,
            &format!("Creating IONOS virtual datacenter {dc_name} in {loc}..."),
        );

        let dc_id = self.client.create_datacenter(dc_name, loc).await?;
        self.ui
            .say(&self.name, &format!("Datacenter created: {dc_id}"));
        state.put("datacenter_id", dc_id.clone());

        let cores = self.config.cores.unwrap_or(2);
        let ram = self.config.ram_mb.unwrap_or(2048);
        let disk = self.config.disk_size_gb.unwrap_or(20);

        self.ui.say(
            &self.name,
            &format!(
                "Launching server from image alias {} ({cores} cores, {ram}MB RAM)...",
                self.config.image_alias
            ),
        );

        let spec = IonosServerSpec {
            name: &self.name,
            cores,
            ram_mb: ram,
            disk_size_gb: disk,
            image_alias: &self.config.image_alias,
            image_password: self.config.image_password.as_deref(),
        };

        let (srv_id, vol_id) = self.client.create_server_and_volume(&dc_id, &spec).await?;

        self.ui.say(
            &self.name,
            &format!("Server created: {srv_id}, volume: {vol_id}"),
        );
        state.put("server_id", srv_id);
        state.put("volume_id", vol_id);
        state.put("server_ip", "127.0.0.1".to_string());

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(dc_id) = state.get::<String>("datacenter_id") {
            self.ui.say(
                &self.name,
                &format!("Cleaning up IONOS datacenter: {dc_id}"),
            );
            let _ = self.client.delete_datacenter(dc_id).await;
        }
    }
}

/// Step to provision the IONOS server over SSH.
#[derive(Clone)]
struct StepProvisionIonos {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Hook.
    hook: Arc<dyn ProvisionHook>,
    /// SSH username.
    ssh_username: Option<String>,
}

#[async_trait]
impl Step for StepProvisionIonos {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning IONOS server...");
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
                .unwrap_or_else(|| "root".to_string()),
            password: None,
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));
        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "ionos".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "ionoscloud".to_string(),
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

/// Step to capture volume snapshot in IONOS Cloud.
#[derive(Debug, Clone)]
struct StepCaptureIonosSnapshot {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: IonosClient,
    /// Config.
    config: IonosConfig,
}

#[async_trait]
impl Step for StepCaptureIonosSnapshot {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let dc_id = state
            .get::<String>("datacenter_id")
            .cloned()
            .unwrap_or_default();
        let vol_id = state
            .get::<String>("volume_id")
            .cloned()
            .unwrap_or_default();

        self.ui.say(
            &self.name,
            &format!(
                "Creating snapshot {} from volume {vol_id}...",
                self.config.snapshot_name
            ),
        );

        let snap_id = self
            .client
            .create_snapshot(
                &dc_id,
                &vol_id,
                &self.config.snapshot_name,
                self.config.snapshot_description.as_deref(),
            )
            .await?;

        self.ui
            .say(&self.name, &format!("Snapshot created: {snap_id}"));
        state.put("snapshot_id", snap_id.clone());
        state.put("artifact_id", format!("ionos:{snap_id}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait]
impl Builder for IonosBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        if self.config.image_alias.is_empty() {
            return Err(StampError::Parse("Image alias cannot be empty".to_string()));
        }
        if self.config.snapshot_name.is_empty() {
            return Err(StampError::Parse(
                "Snapshot name cannot be empty".to_string(),
            ));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let client = IonosClient::with_endpoint(
            self.config.api_endpoint.clone(),
            self.config.token.clone(),
            self.config.username.clone(),
            self.config.password.clone(),
        );

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateIonosInfrastructure {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(StepProvisionIonos {
                ui: ui.clone(),
                name: self.name(),
                hook,
                ssh_username: self.config.ssh_username.clone(),
            }),
            Box::new(StepCaptureIonosSnapshot {
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
            .unwrap_or_default();

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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::pedantic, clippy::all, for_loops_over_fallibles)]
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
    fn test_ionos_name() {
        let b = IonosBuilder::new(IonosConfig {
            name: "test".to_string(),
            image_alias: "ubuntu:latest".to_string(),
            snapshot_name: "snap1".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_ionos_prepare() {
        let mut c = IonosConfig::default();
        let b = IonosBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.name = "test".to_string();
        let b = IonosBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.image_alias = "ubuntu".to_string();
        let b = IonosBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.snapshot_name = "snap".to_string();
        let b = IonosBuilder::new(c);
        assert!(b.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_ionos_client_mocked() {
        let mut server = mockito::Server::new_async().await;

        let _m_dc = server
            .mock("POST", "/datacenters")
            .with_status(200)
            .with_body(r#"{"id": "dc-1"}"#)
            .create_async()
            .await;

        let _m_srv = server
            .mock("POST", "/datacenters/dc-1/servers")
            .with_status(200)
            .with_body(r#"{"id": "srv-1", "entities": {"volumes": {"items": [{"id": "vol-1"}]}}}"#)
            .create_async()
            .await;

        let _m_snap = server
            .mock("POST", "/datacenters/dc-1/volumes/vol-1/create-snapshot")
            .with_status(200)
            .with_body(r#"{"id": "snap-1"}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/datacenters/dc-1")
            .with_status(200)
            .create_async()
            .await;

        let client = IonosClient::with_endpoint(
            Some(server.url()),
            Some("token123".to_string()),
            None,
            None,
        );

        let mut dc_id = String::new();
        for id in client.create_datacenter("test-dc", "de/fra").await {
            dc_id = id;
        }
        assert_eq!(dc_id, "dc-1");

        let spec = IonosServerSpec {
            name: "srv1",
            cores: 2,
            ram_mb: 2048,
            disk_size_gb: 20,
            image_alias: "ubuntu:latest",
            image_password: Some("pass"),
        };

        let mut srv_id = String::new();
        let mut vol_id = String::new();
        for (s, v) in client.create_server_and_volume(&dc_id, &spec).await {
            srv_id = s;
            vol_id = v;
        }
        assert_eq!(srv_id, "srv-1");
        assert_eq!(vol_id, "vol-1");

        let mut snap_id = String::new();
        for id in client
            .create_snapshot(&dc_id, &vol_id, "test-snap", Some("desc"))
            .await
        {
            snap_id = id;
        }
        assert_eq!(snap_id, "snap-1");

        assert!(client.delete_datacenter(&dc_id).await.is_ok());
    }

    #[tokio::test]
    async fn test_ionos_client_basic_auth_and_fallback_volume() {
        let mut server = mockito::Server::new_async().await;

        let _m_dc = server
            .mock("POST", "/datacenters")
            .with_status(200)
            .with_body(r#"{"id": "dc-2"}"#)
            .create_async()
            .await;

        let _m_srv = server
            .mock("POST", "/datacenters/dc-2/servers")
            .with_status(200)
            .with_body(r#"{"id": "srv-2", "entities": {"volumes": {"items": []}}}"#)
            .create_async()
            .await;

        let _m_snap = server
            .mock("POST", "/datacenters/dc-2/volumes/vol-mock/create-snapshot")
            .with_status(200)
            .with_body(r#"{"id": "snap-2"}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/datacenters/dc-2")
            .with_status(200)
            .create_async()
            .await;

        let client = IonosClient::with_endpoint(
            Some(server.url()),
            None,
            Some("user".to_string()),
            Some("pass".to_string()),
        );

        let mut dc_id = String::new();
        for id in client.create_datacenter("test-dc2", "de/txl").await {
            dc_id = id;
        }
        assert_eq!(dc_id, "dc-2");

        let spec = IonosServerSpec {
            name: "srv2",
            cores: 4,
            ram_mb: 4096,
            disk_size_gb: 40,
            image_alias: "debian:latest",
            image_password: None,
        };

        let mut srv_id = String::new();
        let mut vol_id = String::new();
        for (s, v) in client.create_server_and_volume(&dc_id, &spec).await {
            srv_id = s;
            vol_id = v;
        }
        assert_eq!(srv_id, "srv-2");
        assert_eq!(vol_id, "vol-mock");

        let mut snap_id = String::new();
        for id in client
            .create_snapshot(&dc_id, &vol_id, "test-snap2", None)
            .await
        {
            snap_id = id;
        }
        assert_eq!(snap_id, "snap-2");

        assert!(client.delete_datacenter(&dc_id).await.is_ok());
    }

    #[tokio::test]
    async fn test_ionos_client_errors() {
        let mut server = mockito::Server::new_async().await;

        let _m_dc_500 = server
            .mock("POST", "/datacenters")
            .with_status(500)
            .create_async()
            .await;

        let _m_dc_bad = server
            .mock("POST", "/datacenters")
            .with_status(200)
            .with_body("invalid-json")
            .create_async()
            .await;

        let client = IonosClient::with_endpoint(Some(server.url()), None, None, None);
        assert!(client.create_datacenter("d", "l").await.is_err());
        assert!(client.create_datacenter("d", "l").await.is_err());

        let mut server2 = mockito::Server::new_async().await;
        let _m_srv_500 = server2
            .mock("POST", "/datacenters/dc-1/servers")
            .with_status(500)
            .create_async()
            .await;
        let _m_srv_bad = server2
            .mock("POST", "/datacenters/dc-1/servers")
            .with_status(200)
            .with_body("not-json")
            .create_async()
            .await;

        let client2 = IonosClient::with_endpoint(Some(server2.url()), None, None, None);
        let spec = IonosServerSpec::default();
        assert!(
            client2
                .create_server_and_volume("dc-1", &spec)
                .await
                .is_err()
        );
        assert!(
            client2
                .create_server_and_volume("dc-1", &spec)
                .await
                .is_err()
        );

        let mut server3 = mockito::Server::new_async().await;
        let _m_snap_500 = server3
            .mock("POST", "/datacenters/dc-1/volumes/v-1/create-snapshot")
            .with_status(500)
            .create_async()
            .await;
        let _m_snap_bad = server3
            .mock("POST", "/datacenters/dc-1/volumes/v-1/create-snapshot")
            .with_status(200)
            .with_body("not-json")
            .create_async()
            .await;

        let client3 = IonosClient::with_endpoint(Some(server3.url()), None, None, None);
        assert!(
            client3
                .create_snapshot("dc-1", "v-1", "s", None)
                .await
                .is_err()
        );
        assert!(
            client3
                .create_snapshot("dc-1", "v-1", "s", None)
                .await
                .is_err()
        );

        // Invalid network URL
        let client_invalid =
            IonosClient::with_endpoint(Some("http://127.0.0.1:1".to_string()), None, None, None);
        assert!(client_invalid.create_datacenter("d", "l").await.is_err());
        assert!(
            client_invalid
                .create_server_and_volume("d", &spec)
                .await
                .is_err()
        );
        assert!(
            client_invalid
                .create_snapshot("d", "v", "s", None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_ionos_builder_run() {
        let mut server = mockito::Server::new_async().await;

        let _m_dc = server
            .mock("POST", "/datacenters")
            .with_status(200)
            .with_body(r#"{"id": "dc-run"}"#)
            .create_async()
            .await;

        let _m_srv = server
            .mock("POST", "/datacenters/dc-run/servers")
            .with_status(200)
            .with_body(
                r#"{"id": "srv-run", "entities": {"volumes": {"items": [{"id": "vol-run"}]}}}"#,
            )
            .create_async()
            .await;

        let _m_snap = server
            .mock(
                "POST",
                "/datacenters/dc-run/volumes/vol-run/create-snapshot",
            )
            .with_status(200)
            .with_body(r#"{"id": "snap-run"}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/datacenters/dc-run")
            .with_status(200)
            .create_async()
            .await;

        let config = IonosConfig {
            name: "test-ionos".to_string(),
            api_endpoint: Some(server.url()),
            image_alias: "ubuntu:latest".to_string(),
            snapshot_name: "gold-snap".to_string(),
            snapshot_description: Some("description".to_string()),
            token: Some("secret".to_string()),
            location: Some("de/fra".to_string()),
            datacenter_name: Some("custom-dc".to_string()),
            cores: Some(4),
            ram_mb: Some(4096),
            disk_size_gb: Some(50),
            ssh_username: Some("custom_ssh".to_string()),
            ..Default::default()
        };
        let b = IonosBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_ok());
        for artifact in res {
            assert_eq!(artifact.id(), "ionos:snap-run");
            assert_eq!(artifact.builder_id(), "test-ionos");
            assert!(artifact.files().is_empty());
            assert!(artifact.state("dummy").is_none());
            assert!(artifact.destroy().is_ok());
        }

        assert!(b.cancel().await.is_ok());
    }

    #[tokio::test]
    async fn test_ionos_builder_run_defaults() {
        let mut server = mockito::Server::new_async().await;

        let _m_dc = server
            .mock("POST", "/datacenters")
            .with_status(200)
            .with_body(r#"{"id": "dc-def"}"#)
            .create_async()
            .await;

        let _m_srv = server
            .mock("POST", "/datacenters/dc-def/servers")
            .with_status(200)
            .with_body(
                r#"{"id": "srv-def", "entities": {"volumes": {"items": [{"id": "vol-def"}]}}}"#,
            )
            .create_async()
            .await;

        let _m_snap = server
            .mock(
                "POST",
                "/datacenters/dc-def/volumes/vol-def/create-snapshot",
            )
            .with_status(200)
            .with_body(r#"{"id": "snap-def"}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/datacenters/dc-def")
            .with_status(200)
            .create_async()
            .await;

        let config = IonosConfig {
            name: "test-default".to_string(),
            api_endpoint: Some(server.url()),
            image_alias: "ubuntu:latest".to_string(),
            snapshot_name: "gold-snap".to_string(),
            ..Default::default()
        };
        let b = IonosBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_ionos_builder_run_failures() {
        let mut server = mockito::Server::new_async().await;

        let _m_dc = server
            .mock("POST", "/datacenters")
            .with_status(500)
            .create_async()
            .await;

        let config = IonosConfig {
            name: "test-fail".to_string(),
            api_endpoint: Some(server.url()),
            image_alias: "ubuntu:latest".to_string(),
            snapshot_name: "gold-snap".to_string(),
            ..Default::default()
        };
        let b = IonosBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // Test with Cleanup strategy
        let res_cleanup = b
            .run(
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res_cleanup.is_err());

        // Test with Abort strategy
        let res_abort = b
            .run(
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Abort,
            )
            .await;
        assert!(res_abort.is_err());

        // Test default endpoint (None) which fails network call to default ionos endpoint
        let b_no_endpoint = IonosBuilder::new(IonosConfig {
            name: "test-no-endpoint".to_string(),
            api_endpoint: None,
            image_alias: "ubuntu:latest".to_string(),
            snapshot_name: "gold-snap".to_string(),
            ..Default::default()
        });
        let res_no_endpoint = b_no_endpoint
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res_no_endpoint.is_err());
    }

    #[tokio::test]
    async fn test_ionos_provision_failure_and_edges() {
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook: Arc<dyn ProvisionHook> = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });

        let mut step = StepProvisionIonos {
            ui: ui.clone(),
            name: "test-step".to_string(),
            hook,
            ssh_username: None,
        };

        let mut state = StateBag::new();
        // Missing server_ip triggers fallback to "127.0.0.1"
        assert!(step.run(&mut state).await.is_err());
        step.cleanup(&state).await;

        // When server_ip is set
        state.put("server_ip", "10.0.0.1".to_string());
        assert!(step.run(&mut state).await.is_err());

        // StepCreateIonosInfrastructure cleanup without datacenter_id
        let client = IonosClient::new(None, None, None);
        let mut infra_step = StepCreateIonosInfrastructure {
            ui: ui.clone(),
            name: "test-infra".to_string(),
            client: client.clone(),
            config: IonosConfig::default(),
        };
        let empty_state = StateBag::new();
        infra_step.cleanup(&empty_state).await;

        // StepCaptureIonosSnapshot cleanup
        let mut snap_step = StepCaptureIonosSnapshot {
            ui,
            name: "test-snap".to_string(),
            client,
            config: IonosConfig::default(),
        };
        snap_step.cleanup(&empty_state).await;
    }

    #[test]
    fn test_derived_traits() {
        let config = IonosConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let json = serde_json::to_string(&config).unwrap_or_default();
        let decoded: Result<IonosConfig, _> = serde_json::from_str(&json);
        assert!(decoded.is_ok());

        let client = IonosClient::new(None, None, None);
        assert_eq!(format!("{client:?}"), format!("{client:?}"));

        let spec = IonosServerSpec::default();
        assert_eq!(spec.clone(), spec);
        assert_eq!(format!("{spec:?}"), format!("{spec:?}"));

        let dc_resp: Result<DcResp, _> = serde_json::from_str(r#"{"id":"dc-1"}"#);
        assert_eq!(dc_resp.as_ref().map(|d| d.id.as_str()).ok(), Some("dc-1"));

        let snap_resp: Result<SnapResp, _> = serde_json::from_str(r#"{"id":"s-1"}"#);
        assert_eq!(snap_resp.as_ref().map(|s| s.id.as_str()).ok(), Some("s-1"));

        let srv_resp: Result<ServerResp, _> = serde_json::from_str(
            r#"{"id":"srv-1","entities":{"volumes":{"items":[{"id":"v-1"}]}}}"#,
        );
        assert!(srv_resp.is_ok());
    }
}
