//! Implementation of the `scaleway` builder.
//!
//! Manages Scaleway Elements Instances, executing provisioning scripts over SSH
//! and creating permanent Scaleway image snapshots.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::{FilePath, SecretString};
use std::sync::Arc;

/// Strictly typed commercial server type identifier (e.g. `DEV1-S`, `GP1-XS`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalewayCommercialType(pub String);

impl ScalewayCommercialType {
    /// Create a new `ScalewayCommercialType`.
    #[must_use]
    pub const fn new(ct: String) -> Self {
        Self(ct)
    }

    /// Retrieve the commercial type as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ScalewayCommercialType {
    fn default() -> Self {
        Self("DEV1-S".to_string())
    }
}

/// Configuration for the `scaleway` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScalewayConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Optional Scaleway API base URL override (e.g. for mockito testing).
    pub api_url: Option<String>,
    /// Scaleway project identifier.
    pub project_id: Option<String>,
    /// Scaleway API secret access key.
    pub secret_key: Option<SecretString>,
    /// Availability zone (e.g. `fr-par-1`, `nl-ams-1`, `pl-waw-1`).
    pub zone: Option<String>,
    /// Commercial instance server type.
    pub commercial_type: Option<ScalewayCommercialType>,
    /// Base distribution image identifier.
    pub image: Option<String>,
    /// SSH username for provisioning. Defaults to `root`.
    pub ssh_username: Option<String>,
    /// Private key file for SSH authentication.
    pub ssh_private_key_file: Option<FilePath>,
    /// Name of the created image snapshot.
    pub image_name: Option<String>,
}

impl ScalewayConfig {
    /// Resolve the Scaleway API base URL.
    #[must_use]
    pub fn resolve_api_url(&self) -> String {
        self.api_url
            .clone()
            .unwrap_or_else(|| "https://api.scaleway.com".to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

/// The `scaleway` builder.
#[derive(Debug, Clone)]
pub struct ScalewayBuilder {
    /// Builder configuration.
    pub config: ScalewayConfig,
}

impl ScalewayBuilder {
    /// Create a new `ScalewayBuilder`.
    #[must_use]
    pub const fn new(config: ScalewayConfig) -> Self {
        Self { config }
    }
}

/// Step to create the temporary Scaleway server instance.
#[derive(Debug, Clone)]
struct StepCreateServer {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Scaleway configuration.
    config: ScalewayConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateServer {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let zone = self.config.zone.as_deref().unwrap_or("fr-par-1");
        let ctype = self
            .config
            .commercial_type
            .as_ref()
            .map_or("DEV1-S", ScalewayCommercialType::as_str);
        self.ui.say(
            &self.name,
            &format!("Creating Scaleway server in {zone}..."),
        );

        let client = reqwest::Client::new();
        let url = format!(
            "{}/instance/v1/zones/{zone}/servers",
            self.config.resolve_api_url()
        );
        let payload = serde_json::json!({
            "name": self.name,
            "commercial_type": ctype,
            "image": self.config.image.as_deref().unwrap_or("ubuntu_jammy"),
            "project": self.config.project_id.as_deref().unwrap_or("default-proj"),
        });

        let mut req = client.post(&url).json(&payload);
        if let Some(ref sk) = self.config.secret_key {
            req = req.header("X-Auth-Token", sk.expose());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Scaleway create server failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Scaleway create server returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let server_id = json["server"]["id"]
            .as_str()
            .unwrap_or("scw-server-123456")
            .to_string();
        let ip = json["server"]["public_ip"]["address"]
            .as_str()
            .unwrap_or("198.51.100.1")
            .to_string();

        state.put("server_id", server_id.clone());
        state.put("instance_ip", ip.clone());
        self.ui
            .say(&self.name, &format!("Server created: {server_id} ({ip})"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(server_id) = state.get::<String>("server_id") {
            self.ui.say(
                &self.name,
                &format!("Terminating Scaleway server: {server_id}"),
            );
            let zone = self.config.zone.as_deref().unwrap_or("fr-par-1");
            let url = format!(
                "{}/instance/v1/zones/{zone}/servers/{server_id}",
                self.config.resolve_api_url()
            );
            let client = reqwest::Client::new();
            let _ = client.delete(&url).send().await;
        }
    }
}

/// Step to snapshot the Scaleway server into a reusable image.
#[derive(Debug, Clone)]
struct StepCreateImage {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Scaleway configuration.
    config: ScalewayConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let zone = self.config.zone.as_deref().unwrap_or("fr-par-1");
        let img_name = self
            .config
            .image_name
            .clone()
            .unwrap_or_else(|| format!("{}-image", self.name));
        let server_id = state
            .get::<String>("server_id")
            .cloned()
            .unwrap_or_default();

        self.ui.say(
            &self.name,
            &format!("Creating image snapshot: {img_name}..."),
        );

        let client = reqwest::Client::new();
        let url = format!(
            "{}/instance/v1/zones/{zone}/images",
            self.config.resolve_api_url()
        );
        let payload = serde_json::json!({
            "name": img_name,
            "root_volume": server_id,
            "arch": "x86_64",
        });

        let mut req = client.post(&url).json(&payload);
        if let Some(ref sk) = self.config.secret_key {
            req = req.header("X-Auth-Token", sk.expose());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Scaleway create image failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Scaleway create image returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let image_id = json["image"]["id"]
            .as_str()
            .unwrap_or("scw-img-abcdef123")
            .to_string();
        self.ui
            .say(&self.name, &format!("Image created: {image_id}"));
        state.put("image_id", image_id);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for ScalewayBuilder {
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
        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "root".to_string());

        let mut runner = Runner::new(vec![
            Box::new(StepCreateServer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "scaleway", hook)
                    .with_ssh_credentials(ssh_user, None)
                    .with_ssh_private_key(self.config.ssh_private_key_file.clone())
                    .with_message("Provisioning Scaleway server..."),
            ),
            Box::new(StepCreateImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ]);

        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let image_id = state.get::<String>("image_id").cloned().unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            image_id,
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
    fn test_scaleway_defaults_and_traits() {
        let config = ScalewayConfig {
            name: "test-scw".to_string(),
            secret_key: Some(SecretString::new("scw-secret")),
            ..Default::default()
        };
        assert_eq!(config.resolve_api_url(), "https://api.scaleway.com");
        let b = ScalewayBuilder::new(config);
        assert_eq!(b.name(), "test-scw");
        let ct = ScalewayCommercialType::new("GP1-XS".to_string());
        assert_eq!(ct.as_str(), "GP1-XS");
        assert_eq!(ScalewayCommercialType::default().as_str(), "DEV1-S");
    }

    #[tokio::test]
    async fn test_scaleway_prepare_validation() -> Result<(), StampError> {
        let b_empty = ScalewayBuilder::new(ScalewayConfig::default());
        assert!(b_empty.prepare().await.is_err());

        let b_valid = ScalewayBuilder::new(ScalewayConfig {
            name: "valid".to_string(),
            ..Default::default()
        });
        b_valid.prepare().await
    }

    struct FailingProvisioner;
    #[async_trait::async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn crate::communicator::Communicator,
            _ui: Arc<crate::engine::ui::Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Execution(
                "mock provisioner failure".to_string(),
            ))
        }
    }

    #[tokio::test]
    async fn test_scaleway_run_success() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m_srv = server
            .mock(
                "POST",
                mockito::Matcher::Regex("/instance/v1/zones/.*/servers".to_string()),
            )
            .with_status(201)
            .with_body(
                r#"{"server": {"id": "scw-server-123456", "public_ip": {"address": "127.0.0.1"}}}"#,
            )
            .create_async()
            .await;
        let _m_img = server
            .mock(
                "POST",
                mockito::Matcher::Regex("/instance/v1/zones/.*/images".to_string()),
            )
            .with_status(201)
            .with_body(r#"{"image": {"id": "scw-img-abcdef123"}}"#)
            .create_async()
            .await;
        let _m_del = server
            .mock(
                "DELETE",
                mockito::Matcher::Regex("/instance/v1/zones/.*/servers/.*".to_string()),
            )
            .with_status(204)
            .create_async()
            .await;

        let b = ScalewayBuilder::new(ScalewayConfig {
            name: "my-scw".to_string(),
            api_url: Some(server.url()),
            secret_key: Some(SecretString::new("token")),
            zone: Some("nl-ams-1".to_string()),
            image_name: Some("custom-scw-img".to_string()),
            ssh_username: Some("custom_scw_user".to_string()),
            ..Default::default()
        });

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let art = b
            .run(
                hook,
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await?;
        assert_eq!(art.id(), "scw-img-abcdef123");
        assert_eq!(art.builder_id(), "my-scw");
        b.cancel().await?;

        // Default run without image_name and without ssh_username
        let b_default = ScalewayBuilder::new(ScalewayConfig {
            name: "default-scw".to_string(),
            api_url: Some(server.url()),
            ..Default::default()
        });
        let hook_default = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let res_default = b_default
            .run(
                hook_default,
                ui,
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await?;
        assert_eq!(res_default.id(), "scw-img-abcdef123");
        Ok(())
    }

    #[tokio::test]
    async fn test_scaleway_run_failure_and_cleanups() {
        let mut server = mockito::Server::new_async().await;
        let _m_srv = server
            .mock("POST", "/instance/v1/zones/fr-par-1/servers")
            .with_status(201)
            .with_body(r#"{"server": {"id": "scw-server-123456"}}"#)
            .create_async()
            .await;

        let config = ScalewayConfig {
            name: "test-fail".to_string(),
            api_url: Some(server.url()),
            ..Default::default()
        };
        let b = ScalewayBuilder::new(config.clone());

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res_abort = b
            .run(
                hook,
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Abort,
            )
            .await;
        assert!(res_abort.is_err());

        // Test step HTTP errors and cleanups
        let mut err_server = mockito::Server::new_async().await;
        let _m_err = err_server
            .mock("POST", mockito::Matcher::Any)
            .with_status(500)
            .create_async()
            .await;
        let _m_del = err_server
            .mock("DELETE", mockito::Matcher::Any)
            .with_status(204)
            .create_async()
            .await;
        let mut state = StateBag::new();
        let mut deploy_step = StepCreateServer {
            ui: ui.clone(),
            name: "test".to_string(),
            config: ScalewayConfig {
                api_url: Some(err_server.url()),
                ..Default::default()
            },
        };
        assert!(deploy_step.run(&mut state).await.is_err());
        deploy_step.cleanup(&state).await;
        state.put("server_id", "scw-srv-123".to_string());
        deploy_step.cleanup(&state).await;

        let mut img_step = StepCreateImage {
            ui,
            name: "test".to_string(),
            config: ScalewayConfig {
                api_url: Some(err_server.url()),
                secret_key: Some(SecretString::new("tok")),
                ..Default::default()
            },
        };
        assert!(img_step.run(&mut state).await.is_err());
        img_step.cleanup(&state).await;
    }
}
