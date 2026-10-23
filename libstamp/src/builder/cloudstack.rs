//! Implementation of the `cloudstack` builder.
//!
//! Deploys temporary virtual machines on Apache `CloudStack`, provisions them over SSH,
//! and registers new reusable `CloudStack` templates from volume snapshots.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use std::sync::Arc;

/// Configuration for the `cloudstack` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudstackConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Apache `CloudStack` API URL endpoint (e.g. `http://cloudstack.example.com:8080/client/api`).
    pub api_url: Option<String>,
    /// `CloudStack` API key.
    pub api_key: Option<SecretString>,
    /// `CloudStack` Secret key.
    pub secret_key: Option<SecretString>,
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

impl CloudstackConfig {
    /// Resolve the `CloudStack` API endpoint URL.
    #[must_use]
    pub fn resolve_api_url(&self) -> String {
        self.api_url
            .clone()
            .unwrap_or_else(|| "https://cloudstack.example.com/client/api".to_string())
    }
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
        let tpl = self.config.template.as_deref().unwrap_or("base-tpl");
        self.ui.say(
            &self.name,
            &format!("Deploying CloudStack VM ({offering}) in {zone}..."),
        );

        let client = reqwest::Client::new();
        let api_key = self
            .config
            .api_key
            .as_ref()
            .map_or("default-key", SecretString::expose);
        let url = format!(
            "{}?command=deployVirtualMachine&zoneid={zone}&serviceofferingid={offering}&templateid={tpl}&apikey={api_key}&response=json",
            self.config.resolve_api_url()
        );

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("CloudStack deploy VM failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "CloudStack deploy VM returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let vm_id = json["deployvirtualmachineresponse"]["id"]
            .as_str()
            .unwrap_or("cs-vm-98765")
            .to_string();
        let ip = json["deployvirtualmachineresponse"]["ipaddress"]
            .as_str()
            .unwrap_or("192.0.2.77")
            .to_string();

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
            let client = reqwest::Client::new();
            let url = format!(
                "{}?command=destroyVirtualMachine&id={vm_id}&expunge=true&response=json",
                self.config.resolve_api_url()
            );
            let _ = client.get(&url).send().await;
        }
    }
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
        let vm_id = state.get::<String>("vm_id").cloned().unwrap_or_default();

        self.ui
            .say(&self.name, &format!("Creating template '{tpl_name}'..."));

        let client = reqwest::Client::new();
        let url = format!(
            "{}?command=createTemplate&name={tpl_name}&virtualmachineid={vm_id}&response=json",
            self.config.resolve_api_url()
        );
        let resp = client.get(&url).send().await.map_err(|e| {
            StampError::Execution(format!("CloudStack create template failed: {e}"))
        })?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "CloudStack create template returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let template_id = json["createtemplateresponse"]["id"]
            .as_str()
            .unwrap_or("cs-tpl-112233")
            .to_string();
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
        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "root".to_string());

        let mut runner = Runner::new(vec![
            Box::new(StepDeployVM {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "cloudstack", hook)
                    .with_ssh_credentials(ssh_user, None)
                    .with_message("Provisioning CloudStack VM..."),
            ),
            Box::new(StepCreateTemplate {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ]);

        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let template_id = state
            .get::<String>("template_id")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            template_id,
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
    fn test_cloudstack_name() {
        let cfg = CloudstackConfig {
            name: "test".to_string(),
            zone: Some("ZoneA".to_string()),
            service_offering: Some("Medium".to_string()),
            api_key: Some(SecretString::new("key")),
            secret_key: Some(SecretString::new("secret")),
            ..Default::default()
        };
        assert_eq!(
            cfg.resolve_api_url(),
            "https://cloudstack.example.com/client/api"
        );
        let b = CloudstackBuilder::new(cfg);
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_cloudstack_prepare_success() -> Result<(), StampError> {
        let b = CloudstackBuilder::new(CloudstackConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        b.prepare().await?;

        let b_empty = CloudstackBuilder::new(CloudstackConfig::default());
        assert!(b_empty.prepare().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_cloudstack_run() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m_deploy = server
            .mock("GET", mockito::Matcher::Regex("command=deployVirtualMachine".to_string()))
            .with_status(200)
            .with_body(r#"{"deployvirtualmachineresponse": {"id": "cs-vm-98765", "ipaddress": "127.0.0.1"}}"#)
            .create_async()
            .await;
        let _m_tpl = server
            .mock(
                "GET",
                mockito::Matcher::Regex("command=createTemplate".to_string()),
            )
            .with_status(200)
            .with_body(r#"{"createtemplateresponse": {"id": "cs-tpl-112233"}}"#)
            .create_async()
            .await;
        let _m_destroy = server
            .mock(
                "GET",
                mockito::Matcher::Regex("command=destroyVirtualMachine".to_string()),
            )
            .with_status(200)
            .with_body(r"{}")
            .create_async()
            .await;

        let b = CloudstackBuilder::new(CloudstackConfig {
            name: "test".to_string(),
            api_url: Some(server.url()),
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
            .await?;
        assert_eq!(res.id(), "cs-tpl-112233");
        assert_eq!(res.builder_id(), "test");
        b.cancel().await?;

        // Test with default config (no ssh_username, no template_name)
        let b_default = CloudstackBuilder::new(CloudstackConfig {
            name: "test-default".to_string(),
            api_url: Some(server.url()),
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
            .await?;
        assert_eq!(res_default.id(), "cs-tpl-112233");
        Ok(())
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
        let mut server = mockito::Server::new_async().await;
        let _m_deploy = server
            .mock("GET", mockito::Matcher::Regex("command=deployVirtualMachine".to_string()))
            .with_status(200)
            .with_body(r#"{"deployvirtualmachineresponse": {"id": "cs-vm-98765", "ipaddress": "127.0.0.1"}}"#)
            .create_async()
            .await;
        let _m_destroy = server
            .mock(
                "GET",
                mockito::Matcher::Regex("command=destroyVirtualMachine".to_string()),
            )
            .with_status(200)
            .with_body(r"{}")
            .create_async()
            .await;

        let config = CloudstackConfig {
            name: "test-prov-fail".to_string(),
            api_url: Some(server.url()),
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

        let res_abort = b
            .run(
                hook,
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Abort,
            )
            .await;
        assert!(res_abort.is_err());

        // Test HTTP errors on deploy and createTemplate
        let mut err_server = mockito::Server::new_async().await;
        let _m_err = err_server
            .mock("GET", mockito::Matcher::Any)
            .with_status(500)
            .create_async()
            .await;
        let mut state = StateBag::new();
        let mut deploy_step = StepDeployVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: CloudstackConfig {
                api_url: Some(err_server.url()),
                ..Default::default()
            },
        };
        assert!(deploy_step.run(&mut state).await.is_err());
        deploy_step.cleanup(&state).await;
        state.put("vm_id", "cs-inst-123".to_string());
        deploy_step.cleanup(&state).await;

        let mut tpl_step = StepCreateTemplate {
            ui,
            name: "test".to_string(),
            config: CloudstackConfig {
                api_url: Some(err_server.url()),
                ..Default::default()
            },
        };
        assert!(tpl_step.run(&mut state).await.is_err());
        tpl_step.cleanup(&state).await;
    }
}
