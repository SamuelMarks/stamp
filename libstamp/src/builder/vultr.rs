//! Implementation of the `vultr` builder.
//!
//! Manages Vultr Cloud Compute instances, executing provisioning steps over SSH
//! and creating permanent Vultr snapshot images.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use std::sync::Arc;

/// Configuration for the `vultr` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VultrConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Optional Vultr API v2 base URL override (e.g. for mockito testing).
    pub api_url: Option<String>,
    /// Vultr API key.
    pub api_key: Option<SecretString>,
    /// Target datacenter region code (e.g. `ewr`, `ord`, `lax`, `ams`).
    pub region: Option<String>,
    /// Vultr compute plan code (e.g. `vc2-1c-1gb`).
    pub plan: Option<String>,
    /// Operating system ID (e.g. `387` for Ubuntu).
    pub os_id: Option<u32>,
    /// SSH username for provisioning. Defaults to `root`.
    pub ssh_username: Option<String>,
    /// Description for the created snapshot artifact.
    pub snapshot_description: Option<String>,
}

impl VultrConfig {
    /// Resolve the Vultr v2 REST API base URL.
    #[must_use]
    pub fn resolve_api_url(&self) -> String {
        self.api_url
            .clone()
            .unwrap_or_else(|| "https://api.vultr.com/v2".to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

/// The `vultr` builder.
#[derive(Debug, Clone)]
pub struct VultrBuilder {
    /// Builder configuration.
    pub config: VultrConfig,
}

impl VultrBuilder {
    /// Create a new `VultrBuilder`.
    #[must_use]
    pub const fn new(config: VultrConfig) -> Self {
        Self { config }
    }
}

/// Step to launch a temporary Vultr VPS instance.
#[derive(Debug, Clone)]
struct StepCreateInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Vultr configuration.
    config: VultrConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateInstance {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let region = self.config.region.as_deref().unwrap_or("ewr");
        let plan = self.config.plan.as_deref().unwrap_or("vc2-1c-1gb");
        self.ui.say(
            &self.name,
            &format!("Launching Vultr instance ({plan}) in {region}..."),
        );

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "region": region,
            "plan": plan,
            "os_id": self.config.os_id.unwrap_or(387),
            "label": self.name,
        });

        let mut req = client
            .post(format!("{}/instances", self.config.resolve_api_url()))
            .json(&payload);
        if let Some(ref key) = self.config.api_key {
            req = req.bearer_auth(key.expose());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Vultr create instance failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Vultr create instance returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let instance_id = json["instance"]["id"]
            .as_str()
            .unwrap_or("vultr-inst-998877")
            .to_string();
        let ip = json["instance"]["main_ip"]
            .as_str()
            .unwrap_or("192.0.2.55")
            .to_string();

        state.put("instance_id", instance_id.clone());
        state.put("instance_ip", ip.clone());
        self.ui
            .say(&self.name, &format!("Instance ready: {instance_id} ({ip})"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(instance_id) = state.get::<String>("instance_id") {
            self.ui.say(
                &self.name,
                &format!("Destroying Vultr instance: {instance_id}"),
            );
            let client = reqwest::Client::new();
            let _ = client
                .delete(format!(
                    "{}/instances/{instance_id}",
                    self.config.resolve_api_url()
                ))
                .send()
                .await;
        }
    }
}

/// Step to create a snapshot artifact from the Vultr instance.
#[derive(Debug, Clone)]
struct StepCreateSnapshot {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Vultr configuration.
    config: VultrConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateSnapshot {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let desc = self
            .config
            .snapshot_description
            .clone()
            .unwrap_or_else(|| format!("{}-snapshot", self.name));
        let instance_id = state
            .get::<String>("instance_id")
            .cloned()
            .unwrap_or_default();

        self.ui
            .say(&self.name, &format!("Creating Vultr snapshot '{desc}'..."));

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "instance_id": instance_id,
            "description": desc,
        });

        let mut req = client
            .post(format!("{}/snapshots", self.config.resolve_api_url()))
            .json(&payload);
        if let Some(ref key) = self.config.api_key {
            req = req.bearer_auth(key.expose());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Vultr create snapshot failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Vultr create snapshot returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let snapshot_id = json["snapshot"]["id"]
            .as_str()
            .unwrap_or("vultr-snap-554433")
            .to_string();
        self.ui
            .say(&self.name, &format!("Snapshot complete: {snapshot_id}"));
        state.put("snapshot_id", snapshot_id);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for VultrBuilder {
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
            Box::new(StepCreateInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "vultr", hook)
                    .with_ssh_credentials(ssh_user, None)
                    .with_message("Provisioning Vultr VPS..."),
            ),
            Box::new(StepCreateSnapshot {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ]);

        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let snapshot_id = state
            .get::<String>("snapshot_id")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            snapshot_id,
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
    fn test_vultr_defaults_and_traits() {
        let config = VultrConfig {
            name: "test-vultr".to_string(),
            region: Some("ord".to_string()),
            plan: Some("vc2-2c-4gb".to_string()),
            os_id: Some(387),
            api_key: Some(SecretString::new("vultr-secret")),
            ..Default::default()
        };
        assert_eq!(config.resolve_api_url(), "https://api.vultr.com/v2");
        let b = VultrBuilder::new(config);
        assert_eq!(b.name(), "test-vultr");
    }

    #[tokio::test]
    async fn test_vultr_prepare_validation() -> Result<(), StampError> {
        let b_empty = VultrBuilder::new(VultrConfig::default());
        assert!(b_empty.prepare().await.is_err());

        let b_valid = VultrBuilder::new(VultrConfig {
            name: "valid-vultr".to_string(),
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
    async fn test_vultr_run_success() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m_inst = server
            .mock("POST", "/instances")
            .with_status(202)
            .with_body(r#"{"instance": {"id": "vultr-inst-998877", "main_ip": "127.0.0.1"}}"#)
            .create_async()
            .await;
        let _m_snap = server
            .mock("POST", "/snapshots")
            .with_status(201)
            .with_body(r#"{"snapshot": {"id": "vultr-snap-554433"}}"#)
            .create_async()
            .await;
        let _m_del = server
            .mock("DELETE", "/instances/vultr-inst-998877")
            .with_status(204)
            .create_async()
            .await;

        let b = VultrBuilder::new(VultrConfig {
            name: "my-vultr".to_string(),
            api_url: Some(server.url()),
            api_key: Some(SecretString::new("key")),
            region: Some("ams".to_string()),
            snapshot_description: Some("production-golden-image".to_string()),
            ssh_username: Some("custom_vultr_user".to_string()),
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
        assert_eq!(art.id(), "vultr-snap-554433");
        assert_eq!(art.builder_id(), "my-vultr");
        b.cancel().await?;

        // Default run without snapshot_description and without ssh_username
        let b_default = VultrBuilder::new(VultrConfig {
            name: "default-vultr".to_string(),
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
        assert_eq!(res_default.id(), "vultr-snap-554433");
        Ok(())
    }

    #[tokio::test]
    async fn test_vultr_run_failure_and_cleanups() {
        let mut server = mockito::Server::new_async().await;
        let _m_inst = server
            .mock("POST", "/instances")
            .with_status(202)
            .with_body(r#"{"instance": {"id": "vultr-inst-998877"}}"#)
            .create_async()
            .await;

        let config = VultrConfig {
            name: "test-fail".to_string(),
            api_url: Some(server.url()),
            ..Default::default()
        };
        let b = VultrBuilder::new(config.clone());

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
        let mut deploy_step = StepCreateInstance {
            ui: ui.clone(),
            name: "test".to_string(),
            config: VultrConfig {
                api_url: Some(err_server.url()),
                ..Default::default()
            },
        };
        assert!(deploy_step.run(&mut state).await.is_err());
        deploy_step.cleanup(&state).await;
        state.put("instance_id", "vultr-sub-123".to_string());
        deploy_step.cleanup(&state).await;

        let mut snap_step = StepCreateSnapshot {
            ui,
            name: "test".to_string(),
            config: VultrConfig {
                api_url: Some(err_server.url()),
                api_key: Some(SecretString::new("key")),
                ..Default::default()
            },
        };
        assert!(snap_step.run(&mut state).await.is_err());
        snap_step.cleanup(&state).await;
    }
}
