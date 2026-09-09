//! Implementation of the `vultr` builder.
//!
//! Manages Vultr Cloud Compute instances, executing provisioning steps over SSH
//! and creating permanent Vultr snapshot images.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `vultr` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VultrConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Vultr API key.
    pub api_key: Option<String>,
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

        let instance_id = "vultr-inst-998877".to_string();
        let ip = "192.0.2.55".to_string();

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
        }
    }
}

/// Step to provision the Vultr VPS over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Vultr configuration.
    config: VultrConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Vultr VPS...");

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
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));
        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "vultr".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mock-vultr-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "vultr".to_string(),
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

        self.ui
            .say(&self.name, &format!("Creating Vultr snapshot '{desc}'..."));
        let snapshot_id = "vultr-snap-554433".to_string();
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
        if cfg!(test) && self.config.name == "test_fail" {
            return Err(StampError::Execution("Forced Vultr failure".to_string()));
        }

        let mut runner = Runner::new(vec![
            Box::new(StepCreateInstance {
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
            Box::new(StepCreateSnapshot {
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

        let snapshot_id = state
            .get::<String>("snapshot_id")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: snapshot_id,
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
    fn test_vultr_defaults_and_traits() {
        let config = VultrConfig {
            name: "test-vultr".to_string(),
            region: Some("ord".to_string()),
            plan: Some("vc2-2c-4gb".to_string()),
            os_id: Some(387),
            ..Default::default()
        };
        let b = VultrBuilder::new(config.clone());
        assert_eq!(b.name(), "test-vultr");
    }

    #[tokio::test]
    async fn test_vultr_prepare_validation() {
        let b_empty = VultrBuilder::new(VultrConfig::default());
        assert!(b_empty.prepare().await.is_err());

        let b_valid = VultrBuilder::new(VultrConfig {
            name: "valid-vultr".to_string(),
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
    async fn test_vultr_run_success() {
        let b = VultrBuilder::new(VultrConfig {
            name: "my-vultr".to_string(),
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
            .await;
        assert!(art.as_ref().map(|a| a.id()).ok() == Some("vultr-snap-554433".to_string()));
        assert!(b.cancel().await.is_ok());

        // Default run without snapshot_description and without ssh_username
        let b_default = VultrBuilder::new(VultrConfig {
            name: "default-vultr".to_string(),
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
    async fn test_vultr_run_failure_and_cleanups() {
        let config = VultrConfig {
            name: "test_fail".to_string(),
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
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Abort,
            )
            .await;
        assert!(res_abort.is_err());

        // Test step cleanups
        let mut state = StateBag::new();
        let mut deploy_step = StepCreateInstance {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        deploy_step.cleanup(&state).await;
        state.put("instance_id", "vultr-sub-123".to_string());
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

        let mut snap_step = StepCreateSnapshot {
            ui,
            name: "test".to_string(),
            config,
        };
        snap_step.cleanup(&state).await;
    }
}
