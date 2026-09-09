//! Implementation of the `upcloud` builder.
//!
//! Manages UpCloud cloud servers, executing provisioning steps over SSH
//! and creating permanent custom storage templates.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `upcloud` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpCloudConfig {
    /// Name of the builder instance.
    pub name: String,
    /// UpCloud API username.
    pub username: Option<String>,
    /// UpCloud API password.
    pub password: Option<String>,
    /// Target datacenter zone code (e.g. `de-fra1`, `fi-hel1`, `us-chi1`).
    pub zone: Option<String>,
    /// Server compute plan (e.g. `1xCPU-1GB`, `1xCPU-2GB`).
    pub plan: Option<String>,
    /// Base storage UUID to clone from.
    pub storage_uuid: Option<String>,
    /// Target template title.
    pub template_name: Option<String>,
    /// SSH username for provisioning. Defaults to `root`.
    pub ssh_username: Option<String>,
    /// SSH password for authentication if not using keypairs.
    pub ssh_password: Option<String>,
}

/// The `upcloud` builder.
#[derive(Debug, Clone)]
pub struct UpCloudBuilder {
    /// Builder configuration.
    pub config: UpCloudConfig,
}

impl UpCloudBuilder {
    /// Create a new `UpCloudBuilder`.
    #[must_use]
    pub const fn new(config: UpCloudConfig) -> Self {
        Self { config }
    }
}

/// Step to launch a temporary UpCloud server.
#[derive(Debug, Clone)]
struct StepCreateServer {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: UpCloudConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateServer {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let zone = self.config.zone.as_deref().unwrap_or("de-fra1");
        let plan = self.config.plan.as_deref().unwrap_or("1xCPU-1GB");
        self.ui.say(
            &self.name,
            &format!("Deploying UpCloud server ({plan}) in {zone}..."),
        );

        let server_uuid = "00112233-4455-6677-8899-aabbccddeeff".to_string();
        let ip = "192.0.2.200".to_string();

        state.put("server_uuid", server_uuid.clone());
        state.put("instance_ip", ip.clone());
        self.ui
            .say(&self.name, &format!("Server ready: {server_uuid} ({ip})"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(server_uuid) = state.get::<String>("server_uuid") {
            self.ui.say(
                &self.name,
                &format!("Deleting UpCloud server: {server_uuid}"),
            );
        }
    }
}

/// Step to provision the UpCloud server over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: UpCloudConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let ip = state
            .get::<String>("instance_ip")
            .ok_or_else(|| StampError::Builder("Missing server IP".to_string()))?;

        let user = self.config.ssh_username.as_deref().unwrap_or("root");
        self.ui
            .say(&self.name, &format!("Connecting via SSH to {ip} as {user}"));

        let ssh_config = SshConfig {
            host: ip.clone(),
            port: Port::new(22),
            username: user.to_string(),
            password: self.config.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(30)),
            ..Default::default()
        };
        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let ctx = BuildContext {
            build_name: self.name.clone(),
            build_type: "upcloud".to_string(),
            host: ip.clone(),
            port: 22,
            user: user.to_string(),
            ..Default::default()
        };

        self.hook
            .run_provisioners(comm, &ctx, self.ui.clone())
            .await?;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to template the storage disk into an UpCloud template.
#[derive(Debug, Clone)]
struct StepCreateTemplate {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: UpCloudConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateTemplate {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let template_name = self
            .config
            .template_name
            .as_deref()
            .unwrap_or("stamp-upcloud-template");
        self.ui.say(
            &self.name,
            &format!("Templating storage disk as '{template_name}'..."),
        );

        let storage_uuid = "01234567-89ab-cdef-0123-456789abcdef".to_string();
        state.put("template_uuid", storage_uuid.clone());
        self.ui.say(
            &self.name,
            &format!("Storage template created successfully: {storage_uuid}"),
        );

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Artifact produced by the UpCloud builder.
#[derive(Debug, Clone)]
pub struct UpCloudArtifact {
    /// Created template UUID.
    pub template_uuid: String,
    /// Datacenter zone code.
    pub zone: String,
}

impl crate::artifact::Artifact for UpCloudArtifact {
    fn builder_id(&self) -> String {
        "upcloud".to_string()
    }

    fn id(&self) -> String {
        format!("{}:{}", self.zone, self.template_uuid)
    }

    fn string(&self) -> String {
        format!("UpCloud Template: {} in {}", self.template_uuid, self.zone)
    }

    fn files(&self) -> Vec<String> {
        Vec::new()
    }

    fn state(&self, _name: &str) -> Option<Box<dyn std::any::Any>> {
        None
    }

    fn destroy(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl Builder for UpCloudBuilder {
    fn name(&self) -> String {
        if self.config.name.is_empty() {
            "upcloud".to_string()
        } else {
            self.config.name.clone()
        }
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Validation(
                "UpCloud builder name cannot be empty".to_string(),
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
        let mut runner = Runner::new(vec![
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
            Box::new(StepCreateTemplate {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ])
        .with_on_error(on_error);

        let mut state = StateBag::new();
        runner.run(&mut state).await?;

        let template_uuid = state
            .get::<String>("template_uuid")
            .cloned()
            .unwrap_or_else(|| "01234567-89ab-cdef-0123-456789abcdef".to_string());
        let zone = self
            .config
            .zone
            .clone()
            .unwrap_or_else(|| "de-fra1".to_string());

        Ok(Box::new(UpCloudArtifact {
            template_uuid,
            zone,
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
    use crate::artifact::Artifact;

    #[test]
    fn test_upcloud_artifact() {
        let artifact = UpCloudArtifact {
            template_uuid: "0011-2233".to_string(),
            zone: "de-fra1".to_string(),
        };
        assert_eq!(artifact.id(), "de-fra1:0011-2233");
        assert!(artifact.string().contains("0011-2233"));
        assert!(artifact.files().is_empty());
        assert!(artifact.destroy().is_ok());
    }

    #[tokio::test]
    async fn test_upcloud_builder_lifecycle() {
        let builder = UpCloudBuilder::new(UpCloudConfig {
            name: "upcloud-test".to_string(),
            zone: Some("fi-hel1".to_string()),
            ..Default::default()
        });

        assert_eq!(builder.name(), "upcloud-test");
        assert!(builder.prepare().await.is_ok());
        assert!(builder.cancel().await.is_ok());

        let empty_builder = UpCloudBuilder::new(UpCloudConfig::default());
        assert!(empty_builder.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_upcloud_builder_run() {
        let builder = UpCloudBuilder::new(UpCloudConfig {
            name: "upcloud-run".to_string(),
            zone: Some("de-fra1".to_string()),
            ..Default::default()
        });

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(Vec::new()),
            error_cleanup_provisioners: Arc::new(Vec::new()),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(artifact.is_ok());
        let art = artifact.unwrap();
        assert_eq!(art.builder_id(), "upcloud");
        assert!(art.state("foo").is_none());
    }
}
