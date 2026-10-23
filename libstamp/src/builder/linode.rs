//! Implementation of the `linode` builder.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use std::sync::Arc;

/// Configuration for the `linode` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinodeConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The Linode API token.
    pub api_token: Option<SecretString>,
    /// Optional base URL override for the Linode API.
    pub api_url: Option<String>,
    /// The Linode instance type (e.g., "g6-nanode-1").
    pub instance_type: Option<String>,
    /// The base image (e.g., "linode/ubuntu20.04").
    pub image: Option<String>,
    /// The region (e.g., "us-east").
    pub region: Option<String>,
    /// The SSH username to connect with.
    pub ssh_username: Option<String>,
    /// The root password to set on creation.
    pub root_pass: Option<SecretString>,
    /// The name of the temporary Linode instance.
    pub instance_name: Option<String>,
    /// The name of the resulting image.
    pub image_name: Option<String>,
}

impl LinodeConfig {
    /// Resolve the Linode API base URL.
    #[must_use]
    pub fn resolve_api_url(&self) -> String {
        if let Some(ref url) = self.api_url {
            return url.trim_end_matches('/').to_string();
        }
        std::env::var("LINODE_API_URL").unwrap_or_else(|_| "https://api.linode.com/v4".to_string())
    }
}

/// The `linode` builder.
#[derive(Debug, Clone)]
pub struct LinodeBuilder {
    /// The builder configuration.
    pub config: LinodeConfig,
}

impl LinodeBuilder {
    /// Create a new `LinodeBuilder`.
    #[must_use]
    pub const fn new(config: LinodeConfig) -> Self {
        Self { config }
    }
}

/// Construct an authenticated HTTP client for Linode API calls.
fn linode_client(token: &str) -> Result<reqwest::Client, StampError> {
    let mut headers = reqwest::header::HeaderMap::new();
    let auth_value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|e| StampError::Execution(format!("Invalid token header: {e}")))?;
    headers.insert(reqwest::header::AUTHORIZATION, auth_value);

    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .map_err(StampError::Http)
}

/// Step to create a temporary Linode instance.
#[derive(Debug, Clone)]
struct StepCreateInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: LinodeConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateInstance {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Creating Linode Instance...");

        let token = self
            .config
            .api_token
            .as_ref()
            .map_or("", SecretString::expose);
        let client = linode_client(token)?;

        let payload = serde_json::json!({
            "label": self.config.instance_name.as_deref().unwrap_or("packer-linode"),
            "type": self.config.instance_type.as_deref().unwrap_or("g6-nanode-1"),
            "image": self.config.image.as_deref().unwrap_or("linode/ubuntu20.04"),
            "region": self.config.region.as_deref().unwrap_or("us-east"),
            "root_pass": self.config.root_pass.as_ref().map_or("packer-dummy-pass-123!", SecretString::expose),
        });

        let res = client
            .post(format!(
                "{}/linode/instances",
                self.config.resolve_api_url()
            ))
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                StampError::Execution(format!("Linode API Create Instance failed: {e}"))
            })?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "Linode API returned {status}: {text}"
            )));
        }

        let json: serde_json::Value = res.json().await.map_err(|e| {
            StampError::Execution(format!("Failed to parse Linode API response: {e}"))
        })?;

        let linode_id = json["id"]
            .as_u64()
            .ok_or_else(|| StampError::Execution("Linode ID missing in response".to_string()))?;

        self.ui
            .say(&self.name, &format!("Instance created: {linode_id}"));
        state.put("linode_id", linode_id);

        let ip = json["ipv4"][0].as_str().unwrap_or("127.0.0.1").to_string();
        state.put("linode_ip", ip);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(linode_id) = state.get::<u64>("linode_id") {
            self.ui
                .say(&self.name, &format!("Destroying Instance: {linode_id}"));
            let token = self
                .config
                .api_token
                .as_ref()
                .map_or("", SecretString::expose);
            if let Ok(client) = linode_client(token) {
                let _ = client
                    .delete(format!(
                        "{}/linode/instances/{linode_id}",
                        self.config.resolve_api_url()
                    ))
                    .send()
                    .await;
            }
        }
    }
}

/// Step to shut down the Linode instance before imaging.
#[derive(Debug, Clone)]
struct StepShutdownInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: LinodeConfig,
}

#[async_trait::async_trait]
impl Step for StepShutdownInstance {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let linode_id = state.get::<u64>("linode_id").copied().unwrap_or_default();
        self.ui
            .say(&self.name, &format!("Shutting down Instance: {linode_id}"));
        let token = self
            .config
            .api_token
            .as_ref()
            .map_or("", SecretString::expose);
        let client = linode_client(token)?;

        let res = client
            .post(format!(
                "{}/linode/instances/{linode_id}/shutdown",
                self.config.resolve_api_url()
            ))
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Shutdown failed: {e}")))?;

        if !res.status().is_success() {
            return Err(StampError::Execution(format!(
                "Shutdown returned HTTP {}",
                res.status()
            )));
        }
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to create a Linode custom image from the instance disk.
#[derive(Debug, Clone)]
struct StepCreateImage {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: LinodeConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let linode_id = state.get::<u64>("linode_id").copied().unwrap_or_default();
        self.ui.say(
            &self.name,
            &format!("Creating image from Instance: {linode_id}"),
        );
        let token = self
            .config
            .api_token
            .as_ref()
            .map_or("", SecretString::expose);
        let client = linode_client(token)?;

        let disk_res = client
            .get(format!(
                "{}/linode/instances/{linode_id}/disks",
                self.config.resolve_api_url()
            ))
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Disks fetch failed: {e}")))?;
        let disks_json: serde_json::Value = disk_res
            .json()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to parse disks JSON: {e}")))?;

        let disk_id = disks_json["data"][0]["id"]
            .as_u64()
            .ok_or_else(|| StampError::Execution("No disks found".to_string()))?;

        let payload = serde_json::json!({
            "disk_id": disk_id,
            "label": self.config.image_name.as_deref().unwrap_or("packer-linode-image"),
        });

        let res = client
            .post(format!("{}/images", self.config.resolve_api_url()))
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Image create failed: {e}")))?;

        if !res.status().is_success() {
            return Err(StampError::Execution(format!(
                "Image create returned HTTP {}",
                res.status()
            )));
        }

        let json: serde_json::Value = res
            .json()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to parse image response: {e}")))?;

        if let Some(image_id) = json["id"].as_str() {
            state.put("image_id", image_id.to_string());
        }
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for LinodeBuilder {
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
            Box::new(StepCreateInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "linode", hook)
                    .with_ip_key("linode_ip")
                    .with_ssh_credentials(ssh_user, None)
                    .with_message("Provisioning Instance..."),
            ),
            Box::new(StepShutdownInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepCreateImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let image_id = state.get::<String>("image_id").cloned().unwrap_or_default();
        let linode_id = state.get::<u64>("linode_id").copied().unwrap_or(0);

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            format!("linode-image:{image_id}-linode:{linode_id}"),
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
    use crate::engine::packer::OnErrorStrategy;

    #[test]
    fn test_linode_helpers() {
        assert!(linode_client("token").is_ok());
        assert!(
            linode_client(
                "
"
            )
            .is_err()
        );

        let cfg_custom = LinodeConfig {
            api_url: Some("http://custom-linode/".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg_custom.resolve_api_url(), "http://custom-linode");

        let cfg_default = LinodeConfig::default();
        {
            let _env = crate::utils::ScopedEnvVar::remove("LINODE_API_URL");
            assert_eq!(cfg_default.resolve_api_url(), "https://api.linode.com/v4");
        }
    }

    #[tokio::test]
    async fn test_linode_prepare() -> Result<(), StampError> {
        let config = LinodeConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        let builder = LinodeBuilder::new(config);
        builder.prepare().await?;
        assert_eq!(builder.name(), "test");
        builder.cancel().await?;

        let b_empty = LinodeBuilder::new(LinodeConfig::default());
        assert!(b_empty.prepare().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_linode_run_mocked() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let m1 = server
            .mock("POST", "/linode/instances")
            .with_status(200)
            .with_body(r#"{"id": 12345, "ipv4": ["127.0.0.1"]}"#)
            .create_async()
            .await;

        let m2 = server
            .mock("POST", "/linode/instances/12345/shutdown")
            .with_status(200)
            .create_async()
            .await;

        let m3 = server
            .mock("GET", "/linode/instances/12345/disks")
            .with_status(200)
            .with_body(r#"{"data":[{"id":999}]}"#)
            .create_async()
            .await;

        let m4 = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(r#"{"id": "img-999"}"#)
            .create_async()
            .await;

        let m5 = server
            .mock("DELETE", "/linode/instances/12345")
            .with_status(200)
            .create_async()
            .await;

        let config = LinodeConfig {
            name: "test_linode_mock".to_string(),
            api_token: Some(SecretString::new("fake")),
            api_url: Some(server.url()),
            root_pass: Some(SecretString::new("root-pass")),
            ..Default::default()
        };

        let builder = LinodeBuilder::new(config);
        let artifact = builder
            .run(
                Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: Arc::new(vec![]),
                    error_cleanup_provisioners: Arc::new(vec![]),
                }),
                Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                OnErrorStrategy::Cleanup,
            )
            .await?;

        assert_eq!(artifact.id(), "linode-image:img-999-linode:12345");
        assert_eq!(artifact.builder_id(), "test_linode_mock");
        m1.assert_async().await;
        m2.assert_async().await;
        m3.assert_async().await;
        m4.assert_async().await;
        m5.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_linode_error_cases() {
        let mut server = mockito::Server::new_async().await;

        let _m_500 = server
            .mock("POST", "/linode/instances")
            .with_status(500)
            .create_async()
            .await;

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let b_fail = LinodeBuilder::new(LinodeConfig {
            name: "test_linode_fail".to_string(),
            api_token: Some(SecretString::new("fake")),
            api_url: Some(server.url()),
            ..Default::default()
        });
        assert!(
            b_fail
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );

        // Transport errors
        let config_transport = LinodeConfig {
            name: "test_transport".to_string(),
            api_token: Some(SecretString::new("fake")),
            api_url: Some("http://127.0.0.1:1".to_string()),
            ..Default::default()
        };
        let mut state = StateBag::new();
        state.put("linode_id", 12345u64);

        let mut step_create = StepCreateInstance {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config_transport.clone(),
        };
        assert!(step_create.run(&mut state).await.is_err());

        let mut step_shutdown = StepShutdownInstance {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config_transport.clone(),
        };
        assert!(step_shutdown.run(&mut state).await.is_err());
        step_shutdown.cleanup(&state).await;

        let mut step_image = StepCreateImage {
            ui,
            name: "test".to_string(),
            config: config_transport,
        };
        assert!(step_image.run(&mut state).await.is_err());
        step_image.cleanup(&state).await;
    }
}
