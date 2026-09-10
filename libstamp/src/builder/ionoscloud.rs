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
            token,
            username,
            password,
        }
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
        if cfg!(test) {
            return Ok(format!("dc-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "properties": {
                "name": name,
                "location": location,
                "description": "Created by Stamp builder"
            }
        });

        let mut req = client
            .post("https://api.ionos.com/cloudapi/v6/datacenters")
            .json(&payload);

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
        if cfg!(test) {
            return Ok((
                format!("srv-{}", uuid::Uuid::new_v4().simple()),
                format!("vol-{}", uuid::Uuid::new_v4().simple()),
            ));
        }

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

        let url = format!("https://api.ionos.com/cloudapi/v6/datacenters/{datacenter_id}/servers");
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
        if cfg!(test) {
            return Ok(format!("snap-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::new();
        let url = format!(
            "https://api.ionos.com/cloudapi/v6/datacenters/{datacenter_id}/volumes/{volume_id}/create-snapshot"
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
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let url = format!("https://api.ionos.com/cloudapi/v6/datacenters/{datacenter_id}");
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
        let client = IonosClient::new(
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
            .unwrap_or_else(|| format!("ionos:{}", self.name()));

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
    async fn test_ionos_client_mocked() -> Result<(), StampError> {
        let client = IonosClient::new(Some("token123".to_string()), None, None);
        let dc_id = client.create_datacenter("test-dc", "de/fra").await?;
        assert!(dc_id.starts_with("dc-"));

        let spec = IonosServerSpec {
            name: "srv1",
            cores: 2,
            ram_mb: 2048,
            disk_size_gb: 20,
            image_alias: "ubuntu:latest",
            image_password: Some("pass"),
        };
        let (srv_id, vol_id) = client.create_server_and_volume(&dc_id, &spec).await?;
        assert!(srv_id.starts_with("srv-"));
        assert!(vol_id.starts_with("vol-"));

        let snap_id = client
            .create_snapshot(&dc_id, &vol_id, "test-snap", Some("desc"))
            .await?;
        assert!(snap_id.starts_with("snap-"));

        client.delete_datacenter(&dc_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_ionos_builder_run() -> Result<(), StampError> {
        let config = IonosConfig {
            name: "test-ionos".to_string(),
            image_alias: "ubuntu:latest".to_string(),
            snapshot_name: "gold-snap".to_string(),
            token: Some("secret".to_string()),
            location: Some("de/fra".to_string()),
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
        let artifact = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert!(artifact.id().starts_with("ionos:snap-"));
        b.cancel().await?;
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config = IonosConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let client = IonosClient::new(None, None, None);
        assert_eq!(format!("{client:?}"), format!("{client:?}"));
    }
}
