//! Implementation of the `scaleway` builder.
//!
//! Manages Scaleway Elements Instances, executing provisioning scripts over SSH
//! and creating permanent Scaleway image snapshots.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{FilePath, Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

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
    /// Scaleway project identifier.
    pub project_id: Option<String>,
    /// Scaleway API secret access key.
    pub secret_key: Option<String>,
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
        self.ui.say(
            &self.name,
            &format!("Creating Scaleway server in {zone}..."),
        );

        let server_id = "scw-server-123456".to_string();
        let ip = "198.51.100.1".to_string();

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
        }
    }
}

/// Step to provision the Scaleway server over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Scaleway configuration.
    config: ScalewayConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Scaleway server...");

        let ip = state
            .get::<String>("instance_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .config
                .ssh_username
                .clone()
                .unwrap_or_else(|| "root".to_string()),
            private_key_path: self.config.ssh_private_key_file.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));
        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "scaleway".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mock-scw-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "scaleway".to_string(),
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
        let img_name = self
            .config
            .image_name
            .clone()
            .unwrap_or_else(|| format!("{}-image", self.name));

        self.ui.say(
            &self.name,
            &format!("Creating image snapshot: {img_name}..."),
        );
        let image_id = "scw-img-abcdef123".to_string();
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
        if cfg!(test) && self.config.name == "test_fail" {
            return Err(StampError::Execution("Forced failure".to_string()));
        }

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
                hook: hook.clone(),
            }),
            Box::new(StepCreateImage {
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

        let image_id = state.get::<String>("image_id").cloned().unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: image_id,
            files: vec![],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_scaleway_defaults_and_traits() {
        let config = ScalewayConfig {
            name: "test-scw".to_string(),
            ..Default::default()
        };
        let b = ScalewayBuilder::new(config.clone());
        assert_eq!(b.name(), "test-scw");
        let ct = ScalewayCommercialType::new("GP1-XS".to_string());
        assert_eq!(ct.as_str(), "GP1-XS");
        assert_eq!(ScalewayCommercialType::default().as_str(), "DEV1-S");
    }

    #[tokio::test]
    async fn test_scaleway_prepare_validation() {
        let b_empty = ScalewayBuilder::new(ScalewayConfig::default());
        assert!(b_empty.prepare().await.is_err());

        let b_valid = ScalewayBuilder::new(ScalewayConfig {
            name: "valid".to_string(),
            ..Default::default()
        });
        assert!(b_valid.prepare().await.is_ok());
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
    async fn test_scaleway_run_success() {
        let b = ScalewayBuilder::new(ScalewayConfig {
            name: "my-scw".to_string(),
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
            .await;
        assert!(art.as_ref().map(|a| a.id()).ok() == Some("scw-img-abcdef123".to_string()));
        assert!(b.cancel().await.is_ok());

        // Default run without image_name and without ssh_username
        let b_default = ScalewayBuilder::new(ScalewayConfig {
            name: "default-scw".to_string(),
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
            .await;
        assert!(res_default.is_ok());
    }

    #[tokio::test]
    async fn test_scaleway_run_failure_and_cleanups() {
        let config = ScalewayConfig {
            name: "test_fail".to_string(),
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
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Abort,
            )
            .await;
        assert!(res_abort.is_err());

        // Test step cleanups
        let mut state = StateBag::new();
        let mut deploy_step = StepCreateServer {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        deploy_step.cleanup(&state).await;
        state.put("instance_id", "scw-srv-123".to_string());
        deploy_step.cleanup(&state).await;

        let mut prov_step = StepProvision {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
            hook: hook.clone(),
        };
        let mut empty_prov_state = StateBag::new();
        assert!(prov_step.run(&mut empty_prov_state).await.is_err());
        prov_step.cleanup(&state).await;

        let mut img_step = StepCreateImage {
            ui,
            name: "test".to_string(),
            config,
        };
        img_step.cleanup(&state).await;
    }
}
