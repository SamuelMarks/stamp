//! Implementation of the `hetzner-cloud` builder.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `hetzner-cloud` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HetznerCloudConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The Hetzner Cloud API token.
    pub api_token: Option<SecretString>,
    /// Optional custom Hetzner Cloud API base URL override (e.g. for mock servers).
    pub api_url: Option<String>,
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
    /// Optional polling interval in milliseconds for action completion checks.
    pub poll_interval_ms: Option<u64>,
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

/// Return the base Hetzner Cloud API endpoint URL.
fn hcloud_api_url_from_config(config: &HetznerCloudConfig) -> String {
    config
        .api_url
        .clone()
        .or_else(|| std::env::var("HCLOUD_API_URL").ok())
        .unwrap_or_else(|| "https://api.hetzner.cloud".to_string())
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

/// Poll a Hetzner Cloud asynchronous action until completion.
async fn wait_for_hcloud_action(
    client: &reqwest::Client,
    base_url: &str,
    _ui: &Arc<crate::engine::ui::Ui>,
    _name: &str,
    action_id: u64,
    poll_interval_ms: u64,
) -> Result<(), StampError> {
    let url = format!("{base_url}/v1/actions/{action_id}");
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(poll_interval_ms)).await;
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
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Creating Hetzner Cloud Server...");

        let token = self
            .config
            .api_token
            .as_ref()
            .map(SecretString::expose_secret)
            .unwrap_or_default();
        let client = hcloud_client(token)?;
        let base_url = hcloud_api_url_from_config(&self.config);
        let poll_ms = self.config.poll_interval_ms.unwrap_or(1);

        let payload = serde_json::json!({
            "name": self.config.server_name.as_deref().unwrap_or("packer-hcloud"),
            "server_type": self.config.server_type.as_deref().unwrap_or("cx11"),
            "image": self.config.image.as_deref().unwrap_or("ubuntu-20.04"),
            "location": self.config.location.as_deref().unwrap_or("nbg1"),
        });

        let res = client
            .post(format!("{base_url}/v1/servers"))
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
            wait_for_hcloud_action(&client, &base_url, &self.ui, &self.name, action_id, poll_ms)
                .await?;
        }

        self.ui
            .say(&self.name, &format!("Server created: {server_id}"));

        let mut server_addr = "127.0.0.1".to_string();
        let res = client
            .get(format!("{base_url}/v1/servers/{server_id}"))
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to fetch server: {e}")))?;
        if res.status().is_success() {
            let json: serde_json::Value = res.json().await.unwrap_or_default();
            if let Some(ip) = json["server"]["public_net"]["ipv4"]["ip"].as_str() {
                server_addr = ip.to_string();
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
            let token = self
                .config
                .api_token
                .as_ref()
                .map(SecretString::expose_secret)
                .unwrap_or_default();
            let base_url = hcloud_api_url_from_config(&self.config);
            if let Ok(client) = hcloud_client(token) {
                let _ = client
                    .delete(format!("{base_url}/v1/servers/{server_id}"))
                    .send()
                    .await;
            }
        }
    }
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
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let server_id = state.get::<u64>("server_id").copied().unwrap_or_default();
        self.ui
            .say(&self.name, &format!("Powering off Server: {server_id}"));
        let token = self
            .config
            .api_token
            .as_ref()
            .map(SecretString::expose_secret)
            .unwrap_or_default();
        let client = hcloud_client(token)?;
        let base_url = hcloud_api_url_from_config(&self.config);
        let poll_ms = self.config.poll_interval_ms.unwrap_or(1);

        let res = client
            .post(format!(
                "{base_url}/v1/servers/{server_id}/actions/poweroff"
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
            wait_for_hcloud_action(&client, &base_url, &self.ui, &self.name, action_id, poll_ms)
                .await?;
            self.ui.say(&self.name, "Server powered off.");
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
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let server_id = state.get::<u64>("server_id").copied().unwrap_or_default();
        self.ui.say(
            &self.name,
            &format!("Taking snapshot of Server: {server_id}"),
        );
        let token = self
            .config
            .api_token
            .as_ref()
            .map(SecretString::expose_secret)
            .unwrap_or_default();
        let client = hcloud_client(token)?;
        let base_url = hcloud_api_url_from_config(&self.config);
        let poll_ms = self.config.poll_interval_ms.unwrap_or(1);

        let payload = serde_json::json!({
            "description": self.config.snapshot_name.as_deref().unwrap_or("packer-snapshot"),
            "type": "snapshot",
            "labels": self.config.snapshot_labels,
        });

        let res = client
            .post(format!(
                "{base_url}/v1/servers/{server_id}/actions/create_image"
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
            wait_for_hcloud_action(&client, &base_url, &self.ui, &self.name, action_id, poll_ms)
                .await?;
            self.ui.say(&self.name, "Snapshot action completed.");
        }

        if let Some(image_id) = json["image"]["id"].as_u64() {
            state.put("snapshot_image_id", image_id);
            self.ui
                .say(&self.name, &format!("Found snapshot image ID: {image_id}"));
        }
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for HetznerCloudBuilder {
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
        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "root".to_string());
        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateServer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "hetzner-cloud", hook)
                    .with_ip_key("server_addr")
                    .with_ssh_credentials(ssh_user, None)
                    .with_message("Provisioning Server..."),
            ),
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

        let mut runner = Runner::new(steps)
            .with_ui(ui.clone())
            .with_on_error(on_error.clone());
        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let image_id = state.get::<u64>("snapshot_image_id").copied().unwrap_or(0);
        let server_id = state.get::<u64>("server_id").copied().unwrap_or(0);

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            format!("hetzner-cloud-image:{image_id}-server:{server_id}"),
            vec![],
        )))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }

    fn name(&self) -> String {
        self.config.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hcloud_helpers() {
        assert!(hcloud_client("valid-token").is_ok());
        assert!(hcloud_client("\n").is_err());

        let cfg_custom = HetznerCloudConfig {
            api_url: Some("http://custom-api".to_string()),
            ..Default::default()
        };
        assert_eq!(hcloud_api_url_from_config(&cfg_custom), "http://custom-api");

        let cfg_default = HetznerCloudConfig::default();
        assert_eq!(
            hcloud_api_url_from_config(&cfg_default),
            "https://api.hetzner.cloud"
        );
    }

    #[tokio::test]
    async fn test_hetznercloudbuilder_run_action_error() {
        use crate::engine::hook::DefaultProvisionHook;
        use crate::engine::packer::OnErrorStrategy;
        use crate::engine::ui::Ui;
        use std::sync::Arc;

        let config = HetznerCloudConfig {
            name: "test-builder".to_string(),
            ..Default::default()
        };
        let builder = HetznerCloudBuilder::new(config);
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
        assert!(format!("{config:?}").contains("HetznerCloudConfig"));

        let builder = HetznerCloudBuilder::new(config);
        assert!(format!("{builder:?}").contains("HetznerCloudBuilder"));
        assert_eq!(builder.clone().name(), "test");
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
    async fn test_hetzner_cloud_run() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m1 = server
            .mock("POST", "/v1/servers")
            .with_status(201)
            .with_body(r#"{"server": {"id": 123456789}}"#)
            .create_async()
            .await;
        let _m2 = server
            .mock("GET", "/v1/servers/123456789")
            .with_status(200)
            .with_body(r#"{"server": {"public_net": {"ipv4": {"ip": "127.0.0.1"}}}}"#)
            .create_async()
            .await;
        let _m3 = server
            .mock("POST", "/v1/servers/123456789/actions/poweroff")
            .with_status(201)
            .with_body(r"{}")
            .create_async()
            .await;
        let _m4 = server
            .mock("POST", "/v1/servers/123456789/actions/create_image")
            .with_status(201)
            .with_body(r#"{"image": {"id": 555}}"#)
            .create_async()
            .await;
        let _m5 = server
            .mock("DELETE", "/v1/servers/123456789")
            .with_status(200)
            .create_async()
            .await;

        let config = HetznerCloudConfig {
            name: "test".to_string(),
            api_url: Some(server.url()),
            ..Default::default()
        };
        let builder = HetznerCloudBuilder::new(config);
        let artifact = builder
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
            .await?;
        assert!(artifact.id().contains("server:123456789"));
        Ok(())
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
    async fn test_hetzner_cloud_run_mocked() -> Result<(), StampError> {
        use crate::engine::packer::OnErrorStrategy;
        let mut server = mockito::Server::new_async().await;

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
            api_token: Some(SecretString::from("secret")),
            api_url: Some(server.url()),
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

        builder.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_hetzner_cloud_run_mocked_action_error() {
        use crate::engine::packer::OnErrorStrategy;
        let mut server = mockito::Server::new_async().await;

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
            api_token: Some(SecretString::from("secret")),
            api_url: Some(server.url()),
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
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_wait_for_hcloud_action_timeout() {
        let mut server = mockito::Server::new_async().await;

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
        let res = wait_for_hcloud_action(&client, &server.url(), &ui, "test", 9999, 1).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_hetzner_cloud_create_server_api_error() {
        use crate::engine::packer::OnErrorStrategy;
        let mut server = mockito::Server::new_async().await;

        let _m1 = server
            .mock("POST", "/v1/servers")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let config = HetznerCloudConfig {
            name: "test".to_string(),
            api_token: Some(SecretString::from("secret")),
            api_url: Some(server.url()),
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
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_hetzner_cloud_poweroff_api_error() {
        use crate::engine::packer::OnErrorStrategy;
        let mut server = mockito::Server::new_async().await;

        let _m1 = server
            .mock("POST", "/v1/servers")
            .with_status(201)
            .with_body(r#"{"server": {"id": 999}, "action": {"id": 111}}"#)
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
            .with_status(500)
            .with_body("Poweroff Failed")
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
            api_token: Some(SecretString::from("secret")),
            api_url: Some(server.url()),
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
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_hetzner_cloud_create_image_api_error() {
        use crate::engine::packer::OnErrorStrategy;
        let mut server = mockito::Server::new_async().await;

        let _m1 = server
            .mock("POST", "/v1/servers")
            .with_status(201)
            .with_body(r#"{"server": {"id": 999}, "action": {"id": 111}}"#)
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
            .with_status(500)
            .with_body("Create Image Failed")
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
            api_token: Some(SecretString::from("secret")),
            api_url: Some(server.url()),
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
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_hetzner_cloud_wait_for_hcloud_action_mock() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/v1/actions/1")
            .with_status(200)
            .with_body(r#"{"action": {"status": "success"}}"#)
            .create_async()
            .await;

        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let client = reqwest::Client::new();
        assert!(
            wait_for_hcloud_action(&client, &server.url(), &ui, "test", 1, 1)
                .await
                .is_ok()
        );
    }
}
