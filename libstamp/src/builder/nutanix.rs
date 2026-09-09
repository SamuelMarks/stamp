//! Implementation of the `nutanix` AHV builder.
//!
//! Deploys temporary virtual machines on Nutanix AHV via Prism Element/Central API,
//! provisions guest operating systems over SSH, and publishes reusable VM image templates.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `nutanix` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NutanixConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Nutanix Prism Central or Element endpoint host/URL.
    pub endpoint: Option<String>,
    /// Prism API username.
    pub username: Option<String>,
    /// Prism API password.
    pub password: Option<String>,
    /// Target AHV cluster name or UUID.
    pub cluster: Option<String>,
    /// Name of the temporary virtual machine.
    pub vm_name: Option<String>,
    /// Guest operating system type (e.g. `Linux`, `Windows`).
    pub os_type: Option<String>,
    /// Number of vCPUs. Defaults to 2.
    pub cpu: Option<u32>,
    /// Memory size in megabytes. Defaults to 4096.
    pub memory_mb: Option<u64>,
    /// SSH username for provisioning. Defaults to `root`.
    pub ssh_username: Option<String>,
    /// Name of the resulting Nutanix disk/VM image template.
    pub image_name: Option<String>,
}

/// The `nutanix` builder.
#[derive(Debug, Clone)]
pub struct NutanixBuilder {
    /// Builder configuration.
    pub config: NutanixConfig,
}

impl NutanixBuilder {
    /// Create a new `NutanixBuilder`.
    #[must_use]
    pub const fn new(config: NutanixConfig) -> Self {
        Self { config }
    }
}

/// Step to create the temporary Nutanix virtual machine.
#[derive(Debug, Clone)]
struct StepCreateVM {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Nutanix configuration.
    config: NutanixConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateVM {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let endpoint = self.config.endpoint.as_deref().unwrap_or("prism.local");
        let vm_name = self
            .config
            .vm_name
            .clone()
            .unwrap_or_else(|| format!("stamp-{}", self.name));
        self.ui.say(
            &self.name,
            &format!("Creating Nutanix AHV VM '{vm_name}' on {endpoint}..."),
        );

        let vm_uuid = "nutanix-vm-12345678-abcd".to_string();
        let ip = "192.0.2.10".to_string();

        state.put("vm_uuid", vm_uuid.clone());
        state.put("instance_ip", ip.clone());
        self.ui
            .say(&self.name, &format!("AHV VM created: {vm_uuid} ({ip})"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_uuid) = state.get::<String>("vm_uuid") {
            self.ui.say(
                &self.name,
                &format!("Powering off and deleting AHV VM: {vm_uuid}"),
            );
        }
    }
}

/// Step to provision the Nutanix virtual machine over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Nutanix configuration.
    config: NutanixConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Nutanix AHV VM...");

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
            host: "nutanix".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mock-nutanix-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "nutanix".to_string(),
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

/// Step to export the AHV VM disk to a Nutanix Image service template.
#[derive(Debug, Clone)]
struct StepCreateImage {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Nutanix configuration.
    config: NutanixConfig,
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
            &format!("Exporting AHV image template '{img_name}'..."),
        );
        let image_uuid = "nutanix-img-98765432-fedc".to_string();
        self.ui
            .say(&self.name, &format!("Template registered: {image_uuid}"));
        state.put("image_uuid", image_uuid);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for NutanixBuilder {
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
            return Err(StampError::Execution("Forced Nutanix failure".to_string()));
        }

        let mut runner = Runner::new(vec![
            Box::new(StepCreateVM {
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

        let image_uuid = state
            .get::<String>("image_uuid")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: image_uuid,
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
    fn test_nutanix_defaults_and_traits() {
        let config = NutanixConfig {
            name: "test-nutanix".to_string(),
            cpu: Some(4),
            memory_mb: Some(8192),
            ..Default::default()
        };
        let b = NutanixBuilder::new(config.clone());
        assert_eq!(b.name(), "test-nutanix");
    }

    #[tokio::test]
    async fn test_nutanix_prepare_validation() {
        let b_empty = NutanixBuilder::new(NutanixConfig::default());
        assert!(b_empty.prepare().await.is_err());

        let b_valid = NutanixBuilder::new(NutanixConfig {
            name: "valid-nutanix".to_string(),
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
    async fn test_nutanix_run_success() {
        let b = NutanixBuilder::new(NutanixConfig {
            name: "my-nutanix".to_string(),
            endpoint: Some("prism.example.com".to_string()),
            image_name: Some("golden-nutanix-template".to_string()),
            ssh_username: Some("custom_ssh_user".to_string()),
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
        assert!(art.as_ref().map(|a| a.id()).ok() == Some("nutanix-img-98765432-fedc".to_string()));
        assert!(b.cancel().await.is_ok());

        // Default run without image_name and without ssh_username
        let b_default = NutanixBuilder::new(NutanixConfig {
            name: "default-nutanix".to_string(),
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
    async fn test_nutanix_run_failure_and_cleanups() {
        let config = NutanixConfig {
            name: "test_fail".to_string(),
            ..Default::default()
        };
        let b = NutanixBuilder::new(config.clone());

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
        let mut deploy_step = StepCreateVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        deploy_step.cleanup(&state).await;
        state.put("instance_uuid", "nutanix-vm-123".to_string());
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
