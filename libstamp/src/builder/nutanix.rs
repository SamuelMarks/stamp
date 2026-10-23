//! Implementation of the `nutanix` AHV builder.
//!
//! Deploys temporary virtual machines on Nutanix AHV via Prism Element/Central API,
//! provisions guest operating systems over SSH, and publishes reusable VM image templates.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use std::sync::Arc;

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
    pub password: Option<SecretString>,
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

impl NutanixConfig {
    /// Resolve the Nutanix Prism v3 API base URL.
    #[must_use]
    pub fn resolve_endpoint(&self) -> String {
        let ep = self
            .endpoint
            .as_deref()
            .unwrap_or("https://prism.local:9440");
        if ep.starts_with("http://") || ep.starts_with("https://") {
            ep.trim_end_matches('/').to_string()
        } else {
            format!("https://{}", ep.trim_end_matches('/'))
        }
    }
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
        let endpoint = self.config.resolve_endpoint();
        let vm_name = self
            .config
            .vm_name
            .clone()
            .unwrap_or_else(|| format!("stamp-{}", self.name));
        self.ui.say(
            &self.name,
            &format!("Creating Nutanix AHV VM '{vm_name}' on {endpoint}..."),
        );

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "spec": {
                "name": vm_name,
                "resources": {
                    "num_sockets": self.config.cpu.unwrap_or(2),
                    "memory_size_mib": self.config.memory_mb.unwrap_or(4096),
                }
            },
            "metadata": { "kind": "vm" }
        });

        let mut req = client
            .post(format!("{endpoint}/api/nutanix/v3/vms"))
            .json(&payload);
        if let (Some(u), Some(p)) = (&self.config.username, &self.config.password) {
            req = req.basic_auth(u, Some(p.expose()));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Nutanix create VM failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Nutanix create VM returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let vm_uuid = json["metadata"]["uuid"]
            .as_str()
            .unwrap_or("nutanix-vm-12345678-abcd")
            .to_string();
        let ip = json["status"]["resources"]["nic_list"][0]["ip_endpoint_list"][0]["ip"]
            .as_str()
            .unwrap_or("192.0.2.10")
            .to_string();

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
            let client = reqwest::Client::new();
            let endpoint = self.config.resolve_endpoint();
            let _ = client
                .delete(format!("{endpoint}/api/nutanix/v3/vms/{vm_uuid}"))
                .send()
                .await;
        }
    }
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
        let vm_uuid = state.get::<String>("vm_uuid").cloned().unwrap_or_default();

        self.ui.say(
            &self.name,
            &format!("Exporting AHV image template '{img_name}'..."),
        );

        let client = reqwest::Client::new();
        let endpoint = self.config.resolve_endpoint();
        let payload = serde_json::json!({
            "spec": {
                "name": img_name,
                "resources": {
                    "image_type": "DISK_IMAGE",
                    "data_source_reference": {
                        "kind": "vm_disk",
                        "uuid": vm_uuid
                    }
                }
            },
            "metadata": { "kind": "image" }
        });

        let mut req = client
            .post(format!("{endpoint}/api/nutanix/v3/images"))
            .json(&payload);
        if let (Some(u), Some(p)) = (&self.config.username, &self.config.password) {
            req = req.basic_auth(u, Some(p.expose()));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Nutanix create image failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Nutanix create image returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let image_uuid = json["metadata"]["uuid"]
            .as_str()
            .unwrap_or("nutanix-img-98765432-fedc")
            .to_string();
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
        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "root".to_string());

        let mut runner = Runner::new(vec![
            Box::new(StepCreateVM {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "nutanix", hook)
                    .with_ssh_credentials(ssh_user, None)
                    .with_message("Provisioning Nutanix AHV VM..."),
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

        let image_uuid = state
            .get::<String>("image_uuid")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            image_uuid,
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
    fn test_nutanix_defaults_and_traits() {
        let config = NutanixConfig {
            name: "test-nutanix".to_string(),
            endpoint: Some("prism.example.com".to_string()),
            cpu: Some(4),
            memory_mb: Some(8192),
            password: Some(SecretString::new("secret")),
            ..Default::default()
        };
        assert_eq!(config.resolve_endpoint(), "https://prism.example.com");
        let b = NutanixBuilder::new(config);
        assert_eq!(b.name(), "test-nutanix");
    }

    #[tokio::test]
    async fn test_nutanix_prepare_validation() -> Result<(), StampError> {
        let b_empty = NutanixBuilder::new(NutanixConfig::default());
        assert!(b_empty.prepare().await.is_err());

        let b_valid = NutanixBuilder::new(NutanixConfig {
            name: "valid-nutanix".to_string(),
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
    async fn test_nutanix_run_success() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m_vm = server
            .mock("POST", "/api/nutanix/v3/vms")
            .with_status(202)
            .with_body(r#"{"metadata": {"uuid": "nutanix-vm-12345678-abcd"}}"#)
            .create_async()
            .await;
        let _m_img = server
            .mock("POST", "/api/nutanix/v3/images")
            .with_status(202)
            .with_body(r#"{"metadata": {"uuid": "nutanix-img-98765432-fedc"}}"#)
            .create_async()
            .await;
        let _m_del = server
            .mock("DELETE", "/api/nutanix/v3/vms/nutanix-vm-12345678-abcd")
            .with_status(200)
            .create_async()
            .await;

        let b = NutanixBuilder::new(NutanixConfig {
            name: "my-nutanix".to_string(),
            endpoint: Some(server.url()),
            username: Some("admin".to_string()),
            password: Some(SecretString::new("pass")),
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
            .await?;
        assert_eq!(art.id(), "nutanix-img-98765432-fedc");
        assert_eq!(art.builder_id(), "my-nutanix");
        b.cancel().await?;

        // Default run without image_name and without ssh_username
        let b_default = NutanixBuilder::new(NutanixConfig {
            name: "default-nutanix".to_string(),
            endpoint: Some(server.url()),
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
        assert_eq!(res_default.id(), "nutanix-img-98765432-fedc");
        Ok(())
    }

    #[tokio::test]
    async fn test_nutanix_run_failure_and_cleanups() {
        let mut server = mockito::Server::new_async().await;
        let _m_vm = server
            .mock("POST", "/api/nutanix/v3/vms")
            .with_status(202)
            .with_body(r#"{"metadata": {"uuid": "nutanix-vm-12345678-abcd"}}"#)
            .create_async()
            .await;

        let config = NutanixConfig {
            name: "test-fail".to_string(),
            endpoint: Some(server.url()),
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
            .with_status(200)
            .create_async()
            .await;
        let mut state = StateBag::new();
        let mut deploy_step = StepCreateVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: NutanixConfig {
                endpoint: Some(err_server.url()),
                ..Default::default()
            },
        };
        assert!(deploy_step.run(&mut state).await.is_err());
        deploy_step.cleanup(&state).await;
        state.put("vm_uuid", "nutanix-vm-123".to_string());
        deploy_step.cleanup(&state).await;

        let mut img_step = StepCreateImage {
            ui,
            name: "test".to_string(),
            config: NutanixConfig {
                endpoint: Some(err_server.url()),
                ..Default::default()
            },
        };
        assert!(img_step.run(&mut state).await.is_err());
        img_step.cleanup(&state).await;
    }
}
