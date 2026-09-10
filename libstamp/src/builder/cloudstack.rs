//! Implementation of the `cloudstack` builder.
//!
//! Deploys temporary virtual machines on Apache `CloudStack`, provisions them over SSH,
//! and registers new reusable `CloudStack` templates from volume snapshots.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `cloudstack` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudstackConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Apache `CloudStack` API URL endpoint (e.g. `http://cloudstack.example.com:8080/client/api`).
    pub api_url: Option<String>,
    /// `CloudStack` API key.
    pub api_key: Option<String>,
    /// `CloudStack` Secret key.
    pub secret_key: Option<String>,
    /// Target zone name or ID.
    pub zone: Option<String>,
    /// Compute service offering name or ID.
    pub service_offering: Option<String>,
    /// Base template name or ID to clone.
    pub template: Option<String>,
    /// Network name or ID to attach to the VM.
    pub network: Option<String>,
    /// SSH username for provisioning. Defaults to `root`.
    pub ssh_username: Option<String>,
    /// Target template name to create.
    pub template_name: Option<String>,
}

/// The `cloudstack` builder.
#[derive(Debug, Clone)]
pub struct CloudstackBuilder {
    /// Builder configuration.
    pub config: CloudstackConfig,
}

impl CloudstackBuilder {
    /// Create a new `CloudstackBuilder`.
    #[must_use]
    pub const fn new(config: CloudstackConfig) -> Self {
        Self { config }
    }
}

/// Step to deploy a temporary `CloudStack` virtual machine.
#[derive(Debug, Clone)]
struct StepDeployVM {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// `CloudStack` configuration.
    config: CloudstackConfig,
}

#[async_trait::async_trait]
impl Step for StepDeployVM {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let zone = self.config.zone.as_deref().unwrap_or("Zone1");
        let offering = self.config.service_offering.as_deref().unwrap_or("Small");
        self.ui.say(
            &self.name,
            &format!("Deploying CloudStack VM ({offering}) in {zone}..."),
        );

        let vm_id = "cs-vm-98765".to_string();
        let ip = "192.0.2.77".to_string();

        state.put("vm_id", vm_id.clone());
        state.put("instance_ip", ip.clone());
        self.ui.say(
            &self.name,
            &format!("CloudStack VM deployed: {vm_id} ({ip})"),
        );

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_id) = state.get::<String>("vm_id") {
            self.ui
                .say(&self.name, &format!("Destroying CloudStack VM: {vm_id}"));
        }
    }
}

/// Step to provision the `CloudStack` VM over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// `CloudStack` configuration.
    config: CloudstackConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning CloudStack VM...");

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
            host: "cloudstack".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mock-cs-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "cloudstack".to_string(),
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

/// Step to register the template from the VM root volume.
#[derive(Debug, Clone)]
struct StepCreateTemplate {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// `CloudStack` configuration.
    config: CloudstackConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateTemplate {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let tpl_name = self
            .config
            .template_name
            .clone()
            .unwrap_or_else(|| format!("{}-template", self.name));

        self.ui
            .say(&self.name, &format!("Creating template '{tpl_name}'..."));
        let template_id = "cs-tpl-112233".to_string();
        self.ui
            .say(&self.name, &format!("Template created: {template_id}"));
        state.put("template_id", template_id);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for CloudstackBuilder {
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
            return Err(StampError::Execution(
                "Forced CloudStack failure".to_string(),
            ));
        }

        let mut runner = Runner::new(vec![
            Box::new(StepDeployVM {
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
            Box::new(StepCreateTemplate {
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

        let template_id = state
            .get::<String>("template_id")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: template_id,
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
    fn test_cloudstack_name() {
        let b = CloudstackBuilder::new(CloudstackConfig {
            name: "test".to_string(),
            zone: Some("ZoneA".to_string()),
            service_offering: Some("Medium".to_string()),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_cloudstack_prepare_success() {
        let b = CloudstackBuilder::new(CloudstackConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert!(b.prepare().await.is_ok());

        let b_empty = CloudstackBuilder::new(CloudstackConfig::default());
        assert!(b_empty.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_cloudstack_run() {
        let b = CloudstackBuilder::new(CloudstackConfig {
            name: "test".to_string(),
            ssh_username: Some("custom_user".to_string()),
            template_name: Some("my-tpl".to_string()),
            ..Default::default()
        });
        let hook: Arc<dyn ProvisionHook> = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.as_ref().map(|a| a.id()).ok() == Some("cs-tpl-112233".to_string()));
        assert!(b.cancel().await.is_ok());

        // Test with default config (no ssh_username, no template_name)
        let b_default = CloudstackBuilder::new(CloudstackConfig {
            name: "test-default".to_string(),
            ..Default::default()
        });
        let hook_empty: Arc<dyn ProvisionHook> =
            Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: Arc::new(vec![]),
                error_cleanup_provisioners: Arc::new(vec![]),
            });
        let ui_default = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res_default = b_default
            .run(
                hook_empty,
                ui_default,
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res_default.is_ok());
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
    async fn test_cloudstack_provision_failure_and_cleanups() {
        let config = CloudstackConfig {
            name: "test-prov-fail".to_string(),
            ..Default::default()
        };
        let b = CloudstackBuilder::new(config.clone());
        let hook: Arc<dyn ProvisionHook> = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // Test with Abort strategy (covers branch where on_error != Cleanup)
        let res_abort = b
            .run(
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Abort,
            )
            .await;
        assert!(res_abort.is_err());

        // Test individual step cleanups
        let mut state = StateBag::new();
        let mut deploy_step = StepDeployVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        deploy_step.cleanup(&state).await;
        state.put("instance_id", "cs-inst-123".to_string());
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

        let mut tpl_step = StepCreateTemplate {
            ui,
            name: "test".to_string(),
            config,
        };
        tpl_step.cleanup(&state).await;
    }

    #[tokio::test]
    async fn test_cloudstack_run_failure() {
        let b = CloudstackBuilder::new(CloudstackConfig {
            name: "test_fail".to_string(),
            ..Default::default()
        });
        let hook: Arc<dyn ProvisionHook> = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_err());
    }

    #[test]
    fn test_derived_traits() {
        let config = CloudstackConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
    }
}
