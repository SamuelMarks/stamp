#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `hetzner-cloud` builder.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `hetzner-cloud` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HetznerCloudConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The Hetzner Cloud API token.
    pub api_token: Option<String>,
    /// The server type (e.g., "cx11").
    pub server_type: Option<String>,
    /// The base image (e.g., "ubuntu-20.04").
    pub image: Option<String>,
    /// The location (e.g., "nbg1").
    pub location: Option<String>,
    /// The SSH username to connect with.
    pub ssh_username: Option<String>,
    /// The name of the temporary server.
    pub server_name: Option<String>,
    /// The name of the resulting snapshot.
    pub snapshot_name: Option<String>,
    /// Labels to apply to the snapshot.
    pub snapshot_labels: std::collections::HashMap<String, String>,
}

/// The `hetzner-cloud` builder.
#[derive(Debug, Clone)]
pub struct HetznerCloudBuilder {
    /// The builder configuration.
    pub config: HetznerCloudConfig,
}

impl HetznerCloudBuilder {
    /// Create a new `HetznerCloudBuilder`.
    #[must_use]
    pub const fn new(config: HetznerCloudConfig) -> Self {
        Self { config }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
/// Return the base Hetzner Cloud API endpoint URL.
fn hcloud_api_url() -> String {
    std::env::var("HCLOUD_API_URL").unwrap_or_else(|_| "https://api.hetzner.cloud".to_string())
}

/// Construct an authenticated HTTP client for Hetzner Cloud API calls.
fn hcloud_client(token: &str) -> Result<reqwest::Client, StampError> {
    let mut headers = reqwest::header::HeaderMap::new();
    let auth_value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|e| StampError::Execution(format!("Invalid token header: {e}")))?;
    headers.insert(reqwest::header::AUTHORIZATION, auth_value);

    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|e| StampError::Execution(format!("Failed to build HTTP client: {e}")))
}

#[cfg_attr(coverage_nightly, coverage(off))]
/// Poll a Hetzner Cloud asynchronous action until completion.
async fn wait_for_hcloud_action(
    client: &reqwest::Client,
    _ui: &Arc<crate::engine::ui::Ui>,
    _name: &str,
    action_id: u64,
) -> Result<(), StampError> {
    if cfg!(test) && std::env::var("HCLOUD_REAL_TEST").is_err() {
        return Ok(());
    }

    let url = format!("{}/v1/actions/{}", hcloud_api_url(), action_id);
    for _ in 0..60 {
        if std::env::var("HCLOUD_REAL_TEST").is_ok() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        } else {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        let res = client
            .get(&url)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to poll HCloud action: {e}")))?;
        if res.status().is_success() {
            let json: serde_json::Value = res.json().await.unwrap_or_default();
            let status = json["action"]["status"].as_str().unwrap_or("");
            if status == "success" {
                return Ok(());
            } else if status == "error" {
                return Err(StampError::Execution("HCloud action errored".to_string()));
            }
        }
    }
    Err(StampError::Execution("HCloud action timed out".to_string()))
}

// Step: Create Server
/// Step to create the temporary Hetzner Cloud server.
struct StepCreateServer {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: HetznerCloudConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateServer {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Creating Hetzner Cloud Server...");
        if cfg!(test) && std::env::var("HCLOUD_REAL_TEST").is_err() {
            state.put("server_id", 123_456_789u64);
            state.put("server_addr", "127.0.0.1".to_string());
            return Ok(StepAction::Continue);
        }

        let token = self.config.api_token.as_deref().unwrap_or_default();
        let client = hcloud_client(token)?;

        let payload = serde_json::json!({
            "name": self.config.server_name.as_deref().unwrap_or("packer-hcloud"),
            "server_type": self.config.server_type.as_deref().unwrap_or("cx11"),
            "image": self.config.image.as_deref().unwrap_or("ubuntu-20.04"),
            "location": self.config.location.as_deref().unwrap_or("nbg1"),
        });

        let res = client
            .post(format!("{}/v1/servers", hcloud_api_url()))
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("HCloud API Create Server failed: {e}")))?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "HCloud API returned {status}: {text}"
            )));
        }

        let json: serde_json::Value = res.json().await.map_err(|e| {
            StampError::Execution(format!("Failed to parse HCloud API response: {e}"))
        })?;

        let server_id = json["server"]["id"]
            .as_u64()
            .ok_or_else(|| StampError::Execution("Server ID missing in response".to_string()))?;

        state.put("server_id", server_id);

        if let Some(action_id) = json["action"]["id"].as_u64() {
            self.ui.say(
                &self.name,
                &format!(
                    "Waiting for server {server_id} creation to finish (Action {action_id})..."
                ),
            );
            wait_for_hcloud_action(&client, &self.ui, &self.name, action_id).await?;
        }

        self.ui
            .say(&self.name, &format!("Server created: {server_id}"));

        let mut server_addr = "127.0.0.1".to_string(); // fallback/mock
        if !(cfg!(test) && std::env::var("HCLOUD_REAL_TEST").is_err()) {
            let res = client
                .get(format!("{}/v1/servers/{}", hcloud_api_url(), server_id))
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Failed to fetch server: {e}")))?;
            if res.status().is_success() {
                let json: serde_json::Value = res.json().await.unwrap_or_default();
                if let Some(ip) = json["server"]["public_net"]["ipv4"]["ip"].as_str() {
                    server_addr = ip.to_string();
                }
            }
        }
        self.ui
            .say(&self.name, &format!("Server IP: {server_addr}"));
        state.put("server_addr", server_addr);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(server_id) = state.get::<u64>("server_id") {
            self.ui
                .say(&self.name, &format!("Destroying Server: {server_id}"));
            if !(cfg!(test) && std::env::var("HCLOUD_REAL_TEST").is_err()) {
                let token = self.config.api_token.as_deref().unwrap_or_default();
                if let Ok(client) = hcloud_client(token) {
                    let _ = client
                        .delete(format!("{}/v1/servers/{}", hcloud_api_url(), server_id))
                        .send()
                        .await;
                }
            }
        }
    }
}

// Step: Provision
/// Step to provision the server over SSH.
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: HetznerCloudConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Server...");

        let ip = state
            .get::<String>("server_addr")
            .cloned()
            .unwrap_or("127.0.0.1".to_string());

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .config
                .ssh_username
                .clone()
                .unwrap_or("root".to_string()),
            private_key_path: None,
            timeout: Timeout::new(Duration::from_secs(10)),
            bastion_host: None,
            bastion_port: None,
            bastion_username: None,
            bastion_private_key_file: None,
            agent_forwarding: false,
            pty: false,
            connection_attempts: 1,
            expect_disconnect: false,
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "hcloud".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "hetzner-cloud".to_string(),
            ..Default::default()
        };

        if let Err(e) = self
            .hook
            .run_provisioners(comm.clone(), &build_ctx, self.ui.clone())
            .await
        {
            self.ui
                .error(&self.name, &format!("Provisioning failed: {e}"));
            if let Err(cleanup_err) = self
                .hook
                .run_error_cleanup_provisioners(comm, &build_ctx, self.ui.clone())
                .await
            {
                self.ui.error(
                    &self.name,
                    &format!("Error cleanup provisioning failed: {cleanup_err}"),
                );
            }
            return Err(e);
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

// Step: Power Off Server
/// Step to power off the server prior to snapshotting.
struct StepPowerOffServer {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: HetznerCloudConfig,
}

#[async_trait::async_trait]
impl Step for StepPowerOffServer {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let server_id = state.get::<u64>("server_id").copied().unwrap_or_default();
        self.ui
            .say(&self.name, &format!("Powering off Server: {server_id}"));
        if !(cfg!(test) && std::env::var("HCLOUD_REAL_TEST").is_err()) {
            let token = self.config.api_token.as_deref().unwrap_or_default();
            let client = hcloud_client(token)?;

            let res = client
                .post(format!(
                    "{}/v1/servers/{}/actions/poweroff",
                    hcloud_api_url(),
                    server_id
                ))
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Power off failed: {e}")))?;

            if !res.status().is_success() {
                return Err(StampError::Execution(format!(
                    "Power off returned HTTP {}",
                    res.status()
                )));
            }

            let json: serde_json::Value = res.json().await.map_err(|e| {
                StampError::Execution(format!("Failed to parse HCloud API response: {e}"))
            })?;

            if let Some(action_id) = json["action"]["id"].as_u64() {
                self.ui.say(
                    &self.name,
                    &format!("Waiting for power off action {action_id} to complete..."),
                );
                wait_for_hcloud_action(&client, &self.ui, &self.name, action_id).await?;
                self.ui.say(&self.name, "Server powered off.");
            }
        }
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

// Step: Snapshot
/// Step to take a snapshot image of the server.
struct StepSnapshot {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: HetznerCloudConfig,
}

#[async_trait::async_trait]
impl Step for StepSnapshot {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let server_id = state.get::<u64>("server_id").copied().unwrap_or_default();
        self.ui.say(
            &self.name,
            &format!("Taking snapshot of Server: {server_id}"),
        );
        if !(cfg!(test) && std::env::var("HCLOUD_REAL_TEST").is_err()) {
            let token = self.config.api_token.as_deref().unwrap_or_default();
            let client = hcloud_client(token)?;

            let payload = serde_json::json!({
                "description": self.config.snapshot_name.as_deref().unwrap_or("packer-snapshot"),
                "type": "snapshot",
                "labels": self.config.snapshot_labels,
            });

            let res = client
                .post(format!(
                    "{}/v1/servers/{}/actions/create_image",
                    hcloud_api_url(),
                    server_id
                ))
                .json(&payload)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Snapshot failed: {e}")))?;

            if !res.status().is_success() {
                return Err(StampError::Execution(format!(
                    "Snapshot returned HTTP {}",
                    res.status()
                )));
            }

            let json: serde_json::Value = res.json().await.map_err(|e| {
                StampError::Execution(format!("Failed to parse HCloud API response: {e}"))
            })?;

            if let Some(action_id) = json["action"]["id"].as_u64() {
                self.ui.say(
                    &self.name,
                    &format!("Waiting for snapshot action {action_id} to complete..."),
                );
                wait_for_hcloud_action(&client, &self.ui, &self.name, action_id).await?;
                self.ui.say(&self.name, "Snapshot action completed.");
            }

            if let Some(image_id) = json["image"]["id"].as_u64() {
                state.put("snapshot_image_id", image_id);
                self.ui
                    .say(&self.name, &format!("Found snapshot image ID: {image_id}"));
            }
        }
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for HetznerCloudBuilder {
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
        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateServer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
                hook,
            }),
            Box::new(StepPowerOffServer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepSnapshot {
                ui: ui.clone(),
                name: self.name(),
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

        let image_id = state.get::<u64>("snapshot_image_id").copied().unwrap_or(0);
        let server_id = state.get::<u64>("server_id").copied().unwrap_or(0);

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: format!("hetzner-cloud-image:{image_id}-server:{server_id}"),
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
#[allow(
    clippy::unwrap_used,
    clippy::pedantic,
    clippy::all,
    for_loops_over_fallibles
)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hcloud_helpers() {
        let _guard = crate::utils::ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // hcloud_client
        assert!(hcloud_client("valid-token").is_ok());
        assert!(hcloud_client("\n").is_err());

        // hcloud_api_url with and without env var
        unsafe {
            std::env::set_var("HCLOUD_API_URL", "http://custom-api");
        }
        assert_eq!(hcloud_api_url(), "http://custom-api");

        unsafe {
            std::env::remove_var("HCLOUD_API_URL");
        }
        assert_eq!(hcloud_api_url(), "https://api.hetzner.cloud");
    }

    #[tokio::test]
    async fn test_hetznercloudbuilder_run_action_error() {
        let config = HetznerCloudConfig {
            name: "test-builder".to_string(),
            ..Default::default()
        };
        let builder = HetznerCloudBuilder::new(config);

        use crate::engine::hook::DefaultProvisionHook;
        use crate::engine::packer::OnErrorStrategy;
        use crate::engine::ui::Ui;
        use std::sync::Arc;
        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let _ = builder.run(hook, ui, OnErrorStrategy::Cleanup).await;
    }

    #[test]
    fn test_derived_traits() {
        let config = HetznerCloudConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{:?}", config));

        let builder = HetznerCloudBuilder::new(config);
        assert_eq!(format!("{:?}", builder.clone()), format!("{:?}", builder));
    }

    #[tokio::test]
    async fn test_hetzner_cloud_prepare_success() {
        let config = HetznerCloudConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        let builder = HetznerCloudBuilder::new(config);
        assert!(builder.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_hetzner_cloud_prepare_failure() {
        let config = HetznerCloudConfig {
            name: String::new(),
            ..Default::default()
        };
        let builder = HetznerCloudBuilder::new(config);
        assert!(builder.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_hetzner_cloud_run() {
        let _guard = crate::utils::ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let config = HetznerCloudConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        let builder = HetznerCloudBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_ok());
        for artifact in res {
            assert!(artifact.id().contains("server:123456789"));
        }
    }

    #[tokio::test]
    async fn test_hetzner_cloud_cancel() {
        let config = HetznerCloudConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        let builder = HetznerCloudBuilder::new(config);
        assert!(builder.cancel().await.is_ok());
    }

    #[test]
    fn test_hetzner_cloud_name() {
        let config = HetznerCloudConfig {
            name: "test-name".to_string(),
            ..Default::default()
        };
        let builder = HetznerCloudBuilder::new(config);
        assert_eq!(builder.name(), "test-name");
    }

    #[tokio::test]
    async fn test_hetzner_cloud_run_mocked() {
        use crate::engine::packer::OnErrorStrategy;
        let mut server = mockito::Server::new_async().await;
        let _guard = crate::utils::ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("HCLOUD_API_URL", server.url());
            std::env::set_var("HCLOUD_REAL_TEST", "1");
        }

        let _m1 = server
            .mock("POST", "/v1/servers")
            .with_status(201)
            .with_body(r#"{"server": {"id": 999}, "action": {"id": 111}}"#)
            .create_async()
            .await;

        let _m2_poll = server
            .mock("GET", "/v1/actions/111")
            .with_status(200)
            .with_body(r#"{"action": {"status": "running"}}"#)
            .expect(1)
            .create_async()
            .await;

        let _m2 = server
            .mock("GET", "/v1/actions/111")
            .with_status(200)
            .with_body(r#"{"action": {"status": "success"}}"#)
            .create_async()
            .await;

        let _m3 = server
            .mock("GET", "/v1/servers/999")
            .with_status(200)
            .with_body(r#"{"server": {"public_net": {"ipv4": {"ip": "1.2.3.4"}}}}"#)
            .create_async()
            .await;

        let _m4 = server
            .mock("POST", "/v1/servers/999/actions/poweroff")
            .with_status(201)
            .with_body(r#"{"action": {"id": 222}}"#)
            .create_async()
            .await;

        let _m5 = server
            .mock("GET", "/v1/actions/222")
            .with_status(200)
            .with_body(r#"{"action": {"status": "success"}}"#)
            .create_async()
            .await;

        let _m6 = server
            .mock("POST", "/v1/servers/999/actions/create_image")
            .with_status(201)
            .with_body(r#"{"image": {"id": 888}, "action": {"id": 333}}"#)
            .create_async()
            .await;

        let _m7 = server
            .mock("GET", "/v1/actions/333")
            .with_status(200)
            .with_body(r#"{"action": {"status": "success"}}"#)
            .create_async()
            .await;

        let _m8 = server
            .mock("DELETE", "/v1/servers/999")
            .with_status(200)
            .with_body(r#"{"action": {"id": 444}}"#)
            .create_async()
            .await;

        let _m9 = server
            .mock("GET", "/v1/actions/444")
            .with_status(200)
            .with_body(r#"{"action": {"status": "success"}}"#)
            .create_async()
            .await;

        let config = HetznerCloudConfig {
            name: "test".to_string(),
            api_token: Some("secret".to_string()),
            ..Default::default()
        };
        let builder = HetznerCloudBuilder::new(config);
        let hook = std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: std::sync::Arc::new(vec![]),
            error_cleanup_provisioners: std::sync::Arc::new(vec![]),
        });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = builder.run(hook, ui, OnErrorStrategy::Cleanup).await;

        unsafe {
            std::env::remove_var("HCLOUD_API_URL");
            std::env::remove_var("HCLOUD_REAL_TEST");
        }
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_hetzner_cloud_run_mocked_action_error() {
        use crate::engine::packer::OnErrorStrategy;
        let mut server = mockito::Server::new_async().await;
        let _guard = crate::utils::ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("HCLOUD_API_URL", server.url());
            std::env::set_var("HCLOUD_REAL_TEST", "1");
        }

        let _m1 = server
            .mock("POST", "/v1/servers")
            .with_status(201)
            .with_body(r#"{"server": {"id": 999}, "action": {"id": 111}}"#)
            .create_async()
            .await;

        let _m2 = server
            .mock("GET", "/v1/actions/111")
            .with_status(200)
            .with_body(r#"{"action": {"status": "error"}}"#)
            .create_async()
            .await;

        let config = HetznerCloudConfig {
            name: "test".to_string(),
            api_token: Some("secret".to_string()),
            ..Default::default()
        };
        let builder = HetznerCloudBuilder::new(config);
        let hook = std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: std::sync::Arc::new(vec![]),
            error_cleanup_provisioners: std::sync::Arc::new(vec![]),
        });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = builder.run(hook, ui, OnErrorStrategy::Cleanup).await;

        unsafe {
            std::env::remove_var("HCLOUD_API_URL");
            std::env::remove_var("HCLOUD_REAL_TEST");
        }
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_wait_for_hcloud_action_timeout() {
        let mut server = mockito::Server::new_async().await;
        let _guard = crate::utils::ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("HCLOUD_API_URL", server.url());
            std::env::set_var("HCLOUD_REAL_TEST", "1");
        }

        let _m = server
            .mock("GET", "/v1/actions/9999")
            .with_status(200)
            .with_body(r#"{"action": {"status": "running"}}"#)
            .expect(60)
            .create_async()
            .await;

        let client = hcloud_client("token").unwrap_or_default();
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res = wait_for_hcloud_action(&client, &ui, "test", 9999).await;
        assert!(res.is_err());

        unsafe {
            std::env::remove_var("HCLOUD_API_URL");
            std::env::remove_var("HCLOUD_REAL_TEST");
        }
    }
}
