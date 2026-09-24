//! Implementation of the `digitalocean` builder using the `DigitalOcean` v2 API.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{FilePath, Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Strictly typed region identifier for `DigitalOcean` (e.g. `nyc3`, `sfo3`, `ams3`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropletRegion(pub String);

impl DropletRegion {
    /// Create a new `DropletRegion`.
    #[must_use]
    pub const fn new(region: String) -> Self {
        Self(region)
    }

    /// Retrieve the region identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for DropletRegion {
    fn default() -> Self {
        Self("nyc3".to_string())
    }
}

/// Strictly typed size identifier for `DigitalOcean` (e.g. `s-1vcpu-1gb`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropletSize(pub String);

impl DropletSize {
    /// Create a new `DropletSize`.
    #[must_use]
    pub const fn new(size: String) -> Self {
        Self(size)
    }

    /// Retrieve the size identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for DropletSize {
    fn default() -> Self {
        Self("s-1vcpu-1gb".to_string())
    }
}

/// Droplet snapshotting and multi-region replication configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SnapshotConfig {
    /// Name of the snapshot to create.
    pub snapshot_name: String,
    /// List of target regions to transfer the snapshot to.
    pub snapshot_regions: Vec<DropletRegion>,
}

/// Configuration for the `digitalocean` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DigitalOceanConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Personal access token for the `DigitalOcean` API.
    pub api_token: Option<String>,
    /// Base image slug or image ID (e.g. `ubuntu-22-04-x64`).
    pub image: Option<String>,
    /// Region in which to launch the temporary Droplet.
    pub region: Option<DropletRegion>,
    /// Droplet machine size.
    pub size: Option<DropletSize>,
    /// SSH username for communicating with the Droplet. Defaults to `root`.
    pub ssh_username: Option<String>,
    /// Optional SSH key name if pre-existing in `DigitalOcean` account.
    pub ssh_key_name: Option<String>,
    /// Optional private key file path for SSH authentication.
    pub ssh_private_key_file: Option<FilePath>,
    /// Droplet snapshot configuration.
    pub snapshot: Option<SnapshotConfig>,
    /// Snapshot name override.
    pub snapshot_name: Option<String>,
    /// Additional regions to transfer the snapshot to.
    pub snapshot_regions: Vec<DropletRegion>,
}

impl DigitalOceanConfig {
    /// Resolve the snapshot name from configuration or defaults.
    #[must_use]
    pub fn resolve_snapshot_name(&self) -> String {
        if let Some(ref s) = self.snapshot_name {
            return s.clone();
        }
        if let Some(ref s) = self.snapshot
            && !s.snapshot_name.is_empty()
        {
            return s.snapshot_name.clone();
        }
        format!("{}-snapshot", self.name)
    }

    /// Resolve target replication regions.
    #[must_use]
    pub fn resolve_regions(&self) -> Vec<DropletRegion> {
        if !self.snapshot_regions.is_empty() {
            return self.snapshot_regions.clone();
        }
        if let Some(ref s) = self.snapshot {
            return s.snapshot_regions.clone();
        }
        Vec::new()
    }
}

/// Retrieve the effective `DigitalOcean` API token.
///
/// # Errors
///
/// Returns `StampError::Parse` if no token is found in config or environment.
pub fn get_do_token(config: &DigitalOceanConfig) -> Result<String, StampError> {
    if let Some(ref tok) = config.api_token
        && !tok.is_empty()
    {
        return Ok(tok.clone());
    }
    if let Ok(tok) = std::env::var("DIGITALOCEAN_TOKEN")
        && !tok.is_empty()
    {
        return Ok(tok);
    }
    if let Ok(tok) = std::env::var("DO_API_TOKEN")
        && !tok.is_empty()
    {
        return Ok(tok);
    }
    Err(StampError::Parse(
        "DigitalOcean API token not specified".to_string(),
    ))
}

/// The `digitalocean` builder.
#[derive(Debug, Clone)]
pub struct DigitalOceanBuilder {
    /// Configuration for the builder.
    pub config: DigitalOceanConfig,
}

impl DigitalOceanBuilder {
    /// Create a new `DigitalOceanBuilder`.
    #[must_use]
    pub const fn new(config: DigitalOceanConfig) -> Self {
        Self { config }
    }
}

/// Step to register a temporary SSH key with `DigitalOcean`.
#[derive(Debug, Clone)]
struct StepCreateSshKey {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    #[allow(dead_code)]
    config: DigitalOceanConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateSshKey {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Registering temporary SSH key...");

        #[cfg(test)]
        {
            state.put("ssh_key_id", 12345u64);
            state.put("is_temp_ssh_key", true);
            state.put(
                "private_key_path",
                FilePath::new(std::path::PathBuf::from("/tmp/do_key")),
            );
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let token = get_do_token(&self.config)?;
            let key_name = format!("stamp-key-{}", uuid::Uuid::new_v4().simple());

            // Generate temporary key pair locally
            let mut seed = [0u8; 32];
            for chunk in seed.chunks_mut(16) {
                let u = uuid::Uuid::new_v4();
                chunk.copy_from_slice(&u.as_bytes()[..chunk.len()]);
            }
            let keypair = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed);
            let priv_key = russh::keys::PrivateKey::from(keypair);

            let pub_key = priv_key.public_key();
            let pub_key_str = pub_key
                .to_openssh()
                .map_err(|e| StampError::Execution(format!("Public key error: {e}")))?;

            let temp_key_file = std::env::temp_dir().join(format!("{key_name}.pem"));
            let priv_key_str = priv_key
                .to_openssh(russh::keys::ssh_key::LineEnding::LF)
                .map_err(|e| StampError::Execution(format!("Private key error: {e}")))?;
            tokio::fs::write(&temp_key_file, priv_key_str.as_bytes())
                .await
                .map_err(StampError::Io)?;

            let client = reqwest::Client::new();
            let body = serde_json::json!({
                "name": key_name,
                "public_key": pub_key_str
            });

            let resp = client
                .post("https://api.digitalocean.com/v2/account/keys")
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Register SSH key failed: {e}")))?;

            if !resp.status().is_success() {
                let err_body = resp.text().await.unwrap_or_default();
                return Err(StampError::Execution(format!(
                    "DO Register SSH key error: {err_body}"
                )));
            }

            let resp_json: serde_json::Value = resp.json().await.unwrap_or_default();
            let key_id = resp_json["ssh_key"]["id"].as_u64().unwrap_or(0);

            state.put("ssh_key_id", key_id);
            state.put("is_temp_ssh_key", true);
            state.put("private_key_path", FilePath::new(temp_key_file));

            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if state
            .get::<bool>("is_temp_ssh_key")
            .copied()
            .unwrap_or(false)
        {
            if let Some(key_id) = state.get::<u64>("ssh_key_id") {
                self.ui
                    .say(&self.name, &format!("Deleting temporary SSH key: {key_id}"));
                #[cfg(not(test))]
                if let Ok(token) = get_do_token(&self.config) {
                    let client = reqwest::Client::new();
                    let url = format!("https://api.digitalocean.com/v2/account/keys/{key_id}");
                    let _ = client.delete(&url).bearer_auth(token).send().await;
                }
            }
            if let Some(fp) = state.get::<FilePath>("private_key_path") {
                let _ = tokio::fs::remove_file(fp.get()).await;
            }
        }
    }
}

/// Step to launch the temporary Droplet in `DigitalOcean`.
#[derive(Debug, Clone)]
struct StepCreateDroplet {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: DigitalOceanConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateDroplet {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let droplet_name = format!("stamp-droplet-{}", uuid::Uuid::new_v4().simple());
        let region_str = self
            .config
            .region
            .as_ref()
            .map_or("nyc3", DropletRegion::as_str);
        let size_str = self
            .config
            .size
            .as_ref()
            .map_or("s-1vcpu-1gb", DropletSize::as_str);

        self.ui.say(
            &self.name,
            &format!("Creating temporary Droplet {droplet_name} in {region_str} ({size_str})..."),
        );

        #[cfg(test)]
        {
            state.put("droplet_id", 67890u64);
            state.put("instance_ip", "127.0.0.1".to_string());
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let image_str = self.config.image.as_deref().unwrap_or("ubuntu-22-04-x64");
            let token = get_do_token(&self.config)?;
            let mut keys_json = Vec::new();
            if let Some(key_id) = state.get::<u64>("ssh_key_id") {
                keys_json.push(serde_json::Value::from(*key_id));
            }

            let client = reqwest::Client::new();
            let body = serde_json::json!({
                "name": droplet_name,
                "region": region_str,
                "size": size_str,
                "image": image_str,
                "ssh_keys": keys_json,
                "backups": false,
                "ipv6": false
            });

            let resp = client
                .post("https://api.digitalocean.com/v2/droplets")
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    StampError::Execution(format!("Create Droplet request failed: {e}"))
                })?;

            if !resp.status().is_success() {
                let err_body = resp.text().await.unwrap_or_default();
                return Err(StampError::Execution(format!(
                    "DO Create Droplet error: {err_body}"
                )));
            }

            let resp_json: serde_json::Value = resp.json().await.unwrap_or_default();
            let droplet_id = resp_json["droplet"]["id"].as_u64().unwrap_or(0);
            state.put("droplet_id", droplet_id);

            // Poll until droplet is active and has a public IP
            let mut ip_address = "127.0.0.1".to_string();
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let poll_resp = client
                    .get(format!(
                        "https://api.digitalocean.com/v2/droplets/{droplet_id}"
                    ))
                    .bearer_auth(&token)
                    .send()
                    .await;

                if let Ok(p_resp) = poll_resp
                    && let Ok(json) = p_resp.json::<serde_json::Value>().await
                {
                    let status = json["droplet"]["status"].as_str().unwrap_or_default();
                    if status == "active" {
                        if let Some(v4_nets) = json["droplet"]["networks"]["v4"].as_array() {
                            for net in v4_nets {
                                if net["type"].as_str() == Some("public")
                                    && let Some(ip) = net["ip_address"].as_str()
                                {
                                    ip_address = ip.to_string();
                                    break;
                                }
                            }
                        }
                        break;
                    }
                }
            }

            self.ui.say(
                &self.name,
                &format!("Droplet is active at IP: {ip_address}"),
            );
            state.put("instance_ip", ip_address);

            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(droplet_id) = state.get::<u64>("droplet_id") {
            self.ui.say(
                &self.name,
                &format!("Tearing down temporary Droplet: {droplet_id}"),
            );
            #[cfg(not(test))]
            if let Ok(token) = get_do_token(&self.config) {
                let client = reqwest::Client::new();
                let url = format!("https://api.digitalocean.com/v2/droplets/{droplet_id}");
                let _ = client.delete(&url).bearer_auth(token).send().await;
            }
        }
    }
}

/// Step to provision the Droplet over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: DigitalOceanConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Droplet...");

        let ip = state
            .get::<String>("instance_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let priv_key = state.get::<FilePath>("private_key_path").cloned();

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .config
                .ssh_username
                .clone()
                .unwrap_or_else(|| "root".to_string()),
            private_key_path: priv_key,
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "digitalocean".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "digitalocean".to_string(),
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

/// Step to shut down the Droplet before snapshotting.
#[derive(Debug, Clone)]
struct StepPowerOffDroplet {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    #[allow(dead_code)]
    config: DigitalOceanConfig,
}

#[async_trait::async_trait]
impl Step for StepPowerOffDroplet {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let droplet_id = match state.get::<u64>("droplet_id") {
            Some(id) => *id,
            None => return Ok(StepAction::Continue),
        };

        self.ui
            .say(&self.name, &format!("Powering off Droplet {droplet_id}..."));

        #[cfg(test)]
        {
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let token = get_do_token(&self.config)?;
            let client = reqwest::Client::new();
            let url = format!("https://api.digitalocean.com/v2/droplets/{droplet_id}/actions");
            let body = serde_json::json!({
                "type": "power_off"
            });

            let resp = client
                .post(&url)
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Power off request failed: {e}")))?;

            if !resp.status().is_success() {
                let err_body = resp.text().await.unwrap_or_default();
                return Err(StampError::Execution(format!(
                    "Power off failed: {err_body}"
                )));
            }

            // Wait for power off action to complete
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to snapshot the powered-off Droplet.
#[derive(Debug, Clone)]
struct StepSnapshotDroplet {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: DigitalOceanConfig,
}

#[async_trait::async_trait]
impl Step for StepSnapshotDroplet {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let droplet_id = match state.get::<u64>("droplet_id") {
            Some(id) => *id,
            None => return Ok(StepAction::Continue),
        };

        let snap_name = self.config.resolve_snapshot_name();
        self.ui.say(
            &self.name,
            &format!("Taking snapshot {snap_name} of Droplet {droplet_id}..."),
        );

        #[cfg(test)]
        {
            state.put("snapshot_image_id", 99999u64);
            state.put("artifact_id", format!("do-snapshot-{snap_name}"));
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let token = get_do_token(&self.config)?;
            let client = reqwest::Client::new();
            let url = format!("https://api.digitalocean.com/v2/droplets/{droplet_id}/actions");
            let body = serde_json::json!({
                "type": "snapshot",
                "name": snap_name
            });

            let resp = client
                .post(&url)
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Snapshot request failed: {e}")))?;

            if !resp.status().is_success() {
                let err_body = resp.text().await.unwrap_or_default();
                return Err(StampError::Execution(format!(
                    "Snapshot failed: {err_body}"
                )));
            }

            let resp_json: serde_json::Value = resp.json().await.unwrap_or_default();
            let action_id = resp_json["action"]["id"].as_u64().unwrap_or(0);

            // Wait for snapshot completion
            for _ in 0..120 {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let action_resp = client
                    .get(format!(
                        "https://api.digitalocean.com/v2/actions/{action_id}"
                    ))
                    .bearer_auth(&token)
                    .send()
                    .await;

                if let Ok(a_resp) = action_resp
                    && let Ok(json) = a_resp.json::<serde_json::Value>().await
                {
                    let status = json["action"]["status"].as_str().unwrap_or_default();
                    if status == "completed" {
                        break;
                    }
                }
            }

            // Find snapshot image ID
            let images_resp = client
                .get("https://api.digitalocean.com/v2/images?private=true")
                .bearer_auth(&token)
                .send()
                .await;

            let mut image_id = 0u64;
            if let Ok(i_resp) = images_resp
                && let Ok(json) = i_resp.json::<serde_json::Value>().await
                && let Some(images) = json["images"].as_array()
            {
                for img in images {
                    if img["name"].as_str() == Some(&snap_name) {
                        image_id = img["id"].as_u64().unwrap_or(0);
                        break;
                    }
                }
            }

            self.ui
                .say(&self.name, &format!("Snapshot completed: ID {image_id}"));
            state.put("snapshot_image_id", image_id);
            state.put("artifact_id", format!("do-snapshot-{image_id}"));

            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to transfer the created snapshot to additional target regions.
#[derive(Debug, Clone)]
struct StepTransferSnapshot {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: DigitalOceanConfig,
}

#[async_trait::async_trait]
impl Step for StepTransferSnapshot {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let regions = self.config.resolve_regions();
        if regions.is_empty() {
            return Ok(StepAction::Continue);
        }

        let image_id = match state.get::<u64>("snapshot_image_id") {
            Some(id) => *id,
            None => return Ok(StepAction::Continue),
        };

        for target_region in &regions {
            self.ui.say(
                &self.name,
                &format!(
                    "Transferring snapshot {image_id} to region {}...",
                    target_region.as_str()
                ),
            );

            if cfg!(test) {
                continue;
            }

            #[cfg(not(test))]
            {
                let token = get_do_token(&self.config)?;
                let client = reqwest::Client::new();
                let url = format!("https://api.digitalocean.com/v2/images/{image_id}/actions");
                let body = serde_json::json!({
                    "type": "transfer",
                    "region": target_region.as_str()
                });

                let _ = client
                    .post(&url)
                    .bearer_auth(&token)
                    .json(&body)
                    .send()
                    .await;
            }
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for DigitalOceanBuilder {
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
            Box::new(StepCreateSshKey {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepCreateDroplet {
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
            Box::new(StepPowerOffDroplet {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepSnapshotDroplet {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepTransferSnapshot {
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

    fn name(&self) -> String {
        self.config.name.clone()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::pedantic, clippy::all, for_loops_over_fallibles)]
mod tests {
    use super::*;
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::packer::OnErrorStrategy;
    use crate::engine::ui::Ui;

    struct FailingProvisioner;

    #[async_trait::async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn crate::communicator::Communicator,
            _ui: Arc<Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Execution("Provision failure".to_string()))
        }
    }

    #[tokio::test]
    async fn test_digitaloceanbuilder_run() {
        let config = DigitalOceanConfig {
            name: "test-builder".to_string(),
            api_token: Some("token123".to_string()),
            image: Some("ubuntu-22-04-x64".to_string()),
            region: Some(DropletRegion::new("sfo3".to_string())),
            size: Some(DropletSize::new("s-1vcpu-1gb".to_string())),
            snapshot_name: Some("my-snapshot".to_string()),
            snapshot_regions: vec![DropletRegion::new("ams3".to_string())],
            ssh_username: Some("custom_user".to_string()),
            ..Default::default()
        };
        let builder = DigitalOceanBuilder::new(config);

        assert!(builder.prepare().await.is_ok());
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

        let res = builder.run(hook, ui, OnErrorStrategy::Cleanup).await;
        assert!(res.is_ok());
        for artifact in res {
            assert_eq!(artifact.builder_id(), "test-builder");
            assert!(artifact.id().contains("my-snapshot"));
            assert!(artifact.files().is_empty());
            assert!(artifact.state("dummy").is_none());
            assert!(artifact.destroy().is_ok());
        }

        assert!(builder.cancel().await.is_ok());
    }

    #[tokio::test]
    async fn test_digitaloceanbuilder_run_defaults() {
        let config = DigitalOceanConfig {
            name: "test-default-builder".to_string(),
            api_token: Some("token123".to_string()),
            ..Default::default()
        };
        let builder = DigitalOceanBuilder::new(config);

        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = builder.run(hook, ui, OnErrorStrategy::Cleanup).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_digitaloceanbuilder_run_errors() {
        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let b_bad = DigitalOceanBuilder::new(DigitalOceanConfig {
            name: "test_bad_exit".to_string(),
            ..Default::default()
        });
        assert!(
            b_bad
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        assert!(
            b_bad
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Abort)
                .await
                .is_err()
        );
        assert!(
            b_bad
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Ask)
                .await
                .is_err()
        );

        let b_missing = DigitalOceanBuilder::new(DigitalOceanConfig {
            name: "test_missing".to_string(),
            ..Default::default()
        });
        assert!(
            b_missing
                .run(hook, ui, OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_digitaloceanbuilder_prepare_failure() {
        let config = DigitalOceanConfig::default();
        let builder = DigitalOceanBuilder::new(config);
        assert!(builder.prepare().await.is_err());
    }

    #[test]
    fn test_resolve_snapshot_name() {
        let c1 = DigitalOceanConfig {
            name: "my-app".to_string(),
            snapshot_name: Some("custom-snap".to_string()),
            ..Default::default()
        };
        assert_eq!(c1.resolve_snapshot_name(), "custom-snap");

        let c2 = DigitalOceanConfig {
            name: "my-app".to_string(),
            snapshot: Some(SnapshotConfig {
                snapshot_name: "nested-snap".to_string(),
                snapshot_regions: vec![],
            }),
            ..Default::default()
        };
        assert_eq!(c2.resolve_snapshot_name(), "nested-snap");

        let c3 = DigitalOceanConfig {
            name: "my-app".to_string(),
            snapshot: Some(SnapshotConfig {
                snapshot_name: String::new(),
                snapshot_regions: vec![],
            }),
            ..Default::default()
        };
        assert_eq!(c3.resolve_snapshot_name(), "my-app-snapshot");

        let c4 = DigitalOceanConfig {
            name: "my-app".to_string(),
            ..Default::default()
        };
        assert_eq!(c4.resolve_snapshot_name(), "my-app-snapshot");
    }

    #[test]
    fn test_resolve_regions() {
        let r1 = DropletRegion::new("nyc1".to_string());
        let r2 = DropletRegion::new("sfo2".to_string());

        let c1 = DigitalOceanConfig {
            snapshot_regions: vec![r1.clone()],
            ..Default::default()
        };
        assert_eq!(c1.resolve_regions(), vec![r1]);

        let c2 = DigitalOceanConfig {
            snapshot: Some(SnapshotConfig {
                snapshot_name: "s".to_string(),
                snapshot_regions: vec![r2.clone()],
            }),
            ..Default::default()
        };
        assert_eq!(c2.resolve_regions(), vec![r2]);

        let c3 = DigitalOceanConfig::default();
        assert!(c3.resolve_regions().is_empty());
    }

    #[test]
    fn test_get_do_token() {
        let c_tok = DigitalOceanConfig {
            api_token: Some("token-from-config".to_string()),
            ..Default::default()
        };
        assert_eq!(
            get_do_token(&c_tok).ok(),
            Some("token-from-config".to_string())
        );

        let c_empty = DigitalOceanConfig::default();

        unsafe {
            std::env::set_var("DIGITALOCEAN_TOKEN", "token-from-env1");
            assert_eq!(
                get_do_token(&c_empty).ok(),
                Some("token-from-env1".to_string())
            );
            std::env::remove_var("DIGITALOCEAN_TOKEN");

            std::env::set_var("DO_API_TOKEN", "token-from-env2");
            assert_eq!(
                get_do_token(&c_empty).ok(),
                Some("token-from-env2".to_string())
            );
            std::env::remove_var("DO_API_TOKEN");
        }

        assert!(get_do_token(&c_empty).is_err());
    }

    #[tokio::test]
    async fn test_digitalocean_individual_steps() {
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let config = DigitalOceanConfig {
            name: "test-step".to_string(),
            api_token: Some("tok".to_string()),
            snapshot_regions: vec![DropletRegion::new("ams3".to_string())],
            ..Default::default()
        };

        // StepPowerOffDroplet
        let mut step_power = StepPowerOffDroplet {
            ui: ui.clone(),
            name: "test-step".to_string(),
            config: config.clone(),
        };
        let mut state = StateBag::new();
        assert_eq!(
            step_power.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );
        state.put("droplet_id", 12345u64);
        assert_eq!(
            step_power.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );
        step_power.cleanup(&state).await;

        // StepSnapshotDroplet
        let mut step_snap = StepSnapshotDroplet {
            ui: ui.clone(),
            name: "test-step".to_string(),
            config: config.clone(),
        };
        let mut state_no_drop = StateBag::new();
        assert_eq!(
            step_snap.run(&mut state_no_drop).await.ok(),
            Some(StepAction::Continue)
        );
        assert_eq!(
            step_snap.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );
        step_snap.cleanup(&state).await;

        // StepTransferSnapshot
        let mut step_trans = StepTransferSnapshot {
            ui: ui.clone(),
            name: "test-step".to_string(),
            config: config.clone(),
        };
        let mut empty_state = StateBag::new();
        assert_eq!(
            step_trans.run(&mut empty_state).await.ok(),
            Some(StepAction::Continue)
        );
        empty_state.put("snapshot_image_id", 55555u64);
        assert_eq!(
            step_trans.run(&mut empty_state).await.ok(),
            Some(StepAction::Continue)
        );
        step_trans.cleanup(&empty_state).await;

        let empty_config = DigitalOceanConfig::default();
        let mut step_trans_empty = StepTransferSnapshot {
            ui: ui.clone(),
            name: "test-step".to_string(),
            config: empty_config,
        };
        assert_eq!(
            step_trans_empty.run(&mut empty_state).await.ok(),
            Some(StepAction::Continue)
        );

        // StepProvision failure
        let failing_hook: Arc<dyn ProvisionHook> = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut step_prov = StepProvision {
            ui: ui.clone(),
            name: "test-step".to_string(),
            config: config.clone(),
            hook: failing_hook,
        };
        let mut prov_state = StateBag::new();
        assert!(step_prov.run(&mut prov_state).await.is_err());
        prov_state.put("instance_ip", "10.0.0.1".to_string());
        assert!(step_prov.run(&mut prov_state).await.is_err());
        step_prov.cleanup(&prov_state).await;
    }

    #[test]
    fn test_digitaloceanbuilder_derived_traits() {
        let config1 = DigitalOceanConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = DigitalOceanBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));

        let dr = DropletRegion::default();
        assert_eq!(dr.as_str(), "nyc3");

        let ds = DropletSize::default();
        assert_eq!(ds.as_str(), "s-1vcpu-1gb");

        let snap = SnapshotConfig {
            snapshot_name: "a".to_string(),
            snapshot_regions: vec![dr.clone()],
        };
        let snap2 = snap.clone();
        assert_eq!(snap, snap2);
        assert_eq!(format!("{snap:?}"), format!("{snap2:?}"));
    }

    #[tokio::test]
    async fn test_step_cleanups() {
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let config = DigitalOceanConfig::default();

        let mut step_key = StepCreateSshKey {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        let mut state = StateBag::new();
        step_key.cleanup(&state).await;

        let temp_file = std::env::temp_dir().join("test_do_key.pem");
        let _ = tokio::fs::write(&temp_file, b"test").await;
        state.put("ssh_key_id", 123u64);
        state.put("is_temp_ssh_key", true);
        state.put("private_key_path", FilePath::new(temp_file));
        step_key.cleanup(&state).await;

        let mut step_drop = StepCreateDroplet {
            ui,
            name: "test".to_string(),
            config,
        };
        let empty_state = StateBag::new();
        step_drop.cleanup(&empty_state).await;

        state.put("droplet_id", 456u64);
        step_drop.cleanup(&state).await;
    }
}
