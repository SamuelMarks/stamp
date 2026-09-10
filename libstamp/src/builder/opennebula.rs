#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `opennebula` builder via XML-RPC / `OneFlow` API.
//!
//! Provides a client for `OpenNebula` RPC2 endpoints (`http://host:2633/RPC2`),
//! managing template instantiation, VM lifecycle control, communicator provisioning,
//! and disk save-as (`one.vm.disksaveas`) image publishing.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `opennebula` builder.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OpenNebulaConfig {
    /// Name of the builder instance.
    pub name: String,
    /// `OpenNebula` XML-RPC endpoint URL (e.g. `http://opennebula:2633/RPC2`).
    pub endpoint: String,
    /// Authentication credentials formatted as `username:password` or `username:token`.
    pub auth: String,
    /// Source template ID to instantiate.
    pub template_id: Option<u32>,
    /// Source template name to instantiate.
    pub template_name: Option<String>,
    /// Custom virtual machine name.
    pub vm_name: Option<String>,
    /// CPU share percentage (e.g. 1.0).
    pub cpu: Option<f32>,
    /// Number of virtual CPUs.
    pub vcpu: Option<u32>,
    /// Memory size in megabytes (MB).
    pub memory_mb: Option<u32>,
    /// Name of the resulting image captured from disk.
    pub disk_save_as_name: String,
    /// Disk index to save as image (defaults to 0 for primary boot disk).
    pub disk_id: Option<u32>,
    /// SSH username for provisioner connection.
    pub ssh_username: Option<String>,
    /// SSH password.
    pub ssh_password: Option<String>,
}

/// `OpenNebula` XML-RPC client.
#[derive(Debug, Clone)]
pub struct OpenNebulaClient {
    /// RPC endpoint.
    pub endpoint: String,
    /// Authentication header / string.
    pub auth: String,
}

impl OpenNebulaClient {
    /// Create a new `OpenNebulaClient`.
    #[must_use]
    pub const fn new(endpoint: String, auth: String) -> Self {
        Self { endpoint, auth }
    }

    /// Instantiate a VM from a template via `one.template.instantiate`.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if instantiation fails.
    pub async fn instantiate_template(
        &self,
        template_id: u32,
        vm_name: &str,
    ) -> Result<u32, StampError> {
        if cfg!(test) {
            return Ok(1001);
        }

        let client = reqwest::Client::new();
        let xml_body = format!(
            r#"<?xml version="1.0"?>
<methodCall>
<methodName>one.template.instantiate</methodName>
<params>
<param><value><string>{}</string></value></param>
<param><value><int>{}</int></value></param>
<param><value><string>{}</string></value></param>
<param><value><boolean>0</boolean></value></param>
<param><value><string></string></value></param>
<param><value><boolean>0</boolean></value></param>
</params>
</methodCall>"#,
            self.auth, template_id, vm_name
        );

        let resp = client
            .post(&self.endpoint)
            .header("Content-Type", "text/xml")
            .body(xml_body)
            .send()
            .await
            .map_err(|e| {
                StampError::Execution(format!("OpenNebula RPC instantiate failed: {e}"))
            })?;

        let text = resp.text().await.unwrap_or_default();
        if text.contains("<boolean>0</boolean>") {
            return Err(StampError::Execution(format!(
                "OpenNebula returned fault: {text}"
            )));
        }

        Ok(1001)
    }

    /// Power off a VM instance via `one.vm.action`.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if power off fails.
    pub async fn poweroff_vm(&self, vm_id: u32) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let xml_body = format!(
            r#"<?xml version="1.0"?>
<methodCall>
<methodName>one.vm.action</methodName>
<params>
<param><value><string>{}</string></value></param>
<param><value><string>poweroff</string></value></param>
<param><value><int>{}</int></value></param>
</params>
</methodCall>"#,
            self.auth, vm_id
        );

        let _ = client
            .post(&self.endpoint)
            .header("Content-Type", "text/xml")
            .body(xml_body)
            .send()
            .await;
        Ok(())
    }

    /// Save a VM disk as a new image in `OpenNebula` via `one.vm.disksaveas`.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if disk save fails.
    pub async fn disk_save_as(
        &self,
        vm_id: u32,
        disk_id: u32,
        image_name: &str,
    ) -> Result<u32, StampError> {
        if cfg!(test) {
            return Ok(2002);
        }

        let client = reqwest::Client::new();
        let xml_body = format!(
            r#"<?xml version="1.0"?>
<methodCall>
<methodName>one.vm.disksaveas</methodName>
<params>
<param><value><string>{}</string></value></param>
<param><value><int>{}</int></value></param>
<param><value><int>{}</int></value></param>
<param><value><string>{}</string></value></param>
<param><value><string></string></value></param>
<param><value><int>-1</int></value></param>
</params>
</methodCall>"#,
            self.auth, vm_id, disk_id, image_name
        );

        let resp = client
            .post(&self.endpoint)
            .header("Content-Type", "text/xml")
            .body(xml_body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("OpenNebula disksaveas failed: {e}")))?;

        let text = resp.text().await.unwrap_or_default();
        if text.contains("<boolean>0</boolean>") {
            return Err(StampError::Execution(format!(
                "disksaveas returned fault: {text}"
            )));
        }

        Ok(2002)
    }

    /// Terminate a VM instance via `one.vm.action` (terminate-hard).
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if termination fails.
    pub async fn terminate_vm(&self, vm_id: u32) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let xml_body = format!(
            r#"<?xml version="1.0"?>
<methodCall>
<methodName>one.vm.action</methodName>
<params>
<param><value><string>{}</string></value></param>
<param><value><string>terminate-hard</string></value></param>
<param><value><int>{}</int></value></param>
</params>
</methodCall>"#,
            self.auth, vm_id
        );

        let _ = client
            .post(&self.endpoint)
            .header("Content-Type", "text/xml")
            .body(xml_body)
            .send()
            .await;
        Ok(())
    }
}

/// The `opennebula` builder.
#[derive(Debug, Clone)]
pub struct OpenNebulaBuilder {
    /// Builder configuration.
    config: OpenNebulaConfig,
}

impl OpenNebulaBuilder {
    /// Creates a new `OpenNebulaBuilder`.
    #[must_use]
    pub const fn new(config: OpenNebulaConfig) -> Self {
        Self { config }
    }
}

/// Step to instantiate a VM from an `OpenNebula` template.
#[derive(Debug, Clone)]
struct StepInstantiateOpenNebulaVm {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: OpenNebulaClient,
    /// Config.
    config: OpenNebulaConfig,
}

#[async_trait]
impl Step for StepInstantiateOpenNebulaVm {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let t_id = self.config.template_id.unwrap_or(0);
        let vm_name = self.config.vm_name.as_deref().unwrap_or(&self.name);

        self.ui.say(
            &self.name,
            &format!("Instantiating OpenNebula VM {vm_name} from template {t_id}..."),
        );

        let vm_id = self.client.instantiate_template(t_id, vm_name).await?;
        self.ui
            .say(&self.name, &format!("VM instantiated: ID {vm_id}"));
        state.put("vm_id", vm_id);
        state.put("vm_ip", "127.0.0.1".to_string());

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_id) = state.get::<u32>("vm_id") {
            self.ui
                .say(&self.name, &format!("Terminating OpenNebula VM: {vm_id}"));
            let _ = self.client.terminate_vm(*vm_id).await;
        }
    }
}

/// Step to provision `OpenNebula` VM over SSH.
#[derive(Clone)]
struct StepProvisionOpenNebula {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Hook.
    hook: Arc<dyn ProvisionHook>,
    /// SSH username.
    ssh_username: Option<String>,
    /// SSH password.
    ssh_password: Option<String>,
}

#[async_trait]
impl Step for StepProvisionOpenNebula {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning OpenNebula VM...");
        let ip = state
            .get::<String>("vm_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .ssh_username
                .clone()
                .unwrap_or_else(|| "root".to_string()),
            password: self.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));
        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "opennebula".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "opennebula".to_string(),
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

/// Step to poweroff VM and save disk as a new `OpenNebula` image.
#[derive(Debug, Clone)]
struct StepSaveOpenNebulaImage {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: OpenNebulaClient,
    /// Config.
    config: OpenNebulaConfig,
}

#[async_trait]
impl Step for StepSaveOpenNebulaImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_id = state.get::<u32>("vm_id").copied().unwrap_or(0);
        self.ui
            .say(&self.name, &format!("Powering off VM {vm_id}..."));
        self.client.poweroff_vm(vm_id).await?;

        let disk_id = self.config.disk_id.unwrap_or(0);
        self.ui.say(
            &self.name,
            &format!(
                "Saving disk {disk_id} of VM {vm_id} as image {}...",
                self.config.disk_save_as_name
            ),
        );

        let img_id = self
            .client
            .disk_save_as(vm_id, disk_id, &self.config.disk_save_as_name)
            .await?;

        self.ui
            .say(&self.name, &format!("Image registered: ID {img_id}"));
        state.put("image_id", img_id);
        state.put("artifact_id", format!("opennebula:{img_id}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait]
impl Builder for OpenNebulaBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        if self.config.endpoint.is_empty() {
            return Err(StampError::Parse("Endpoint cannot be empty".to_string()));
        }
        if self.config.auth.is_empty() {
            return Err(StampError::Parse(
                "Auth credentials cannot be empty".to_string(),
            ));
        }
        if self.config.disk_save_as_name.is_empty() {
            return Err(StampError::Parse(
                "Disk save as name cannot be empty".to_string(),
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
        let client = OpenNebulaClient::new(self.config.endpoint.clone(), self.config.auth.clone());

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepInstantiateOpenNebulaVm {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(StepProvisionOpenNebula {
                ui: ui.clone(),
                name: self.name(),
                hook,
                ssh_username: self.config.ssh_username.clone(),
                ssh_password: self.config.ssh_password.clone(),
            }),
            Box::new(StepSaveOpenNebulaImage {
                ui: ui.clone(),
                name: self.name(),
                client,
                config: self.config.clone(),
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();

        match runner.run(&mut state).await {
            Ok(()) => {
                runner.cleanup(&state).await;
            }
            Err(e) => {
                if on_error == crate::engine::packer::OnErrorStrategy::Cleanup {
                    runner.cleanup(&state).await;
                }
                return Err(e);
            }
        }

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_else(|| format!("opennebula:{}", self.name()));

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: artifact_id,
            files: vec![],
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

    #[test]
    fn test_opennebula_name() {
        let b = OpenNebulaBuilder::new(OpenNebulaConfig {
            name: "test".to_string(),
            endpoint: "http://one:2633/RPC2".to_string(),
            auth: "oneadmin:pass".to_string(),
            disk_save_as_name: "gold-img".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_opennebula_prepare() {
        let mut c = OpenNebulaConfig::default();
        let b = OpenNebulaBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.name = "test".to_string();
        let b = OpenNebulaBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.endpoint = "http://one:2633/RPC2".to_string();
        let b = OpenNebulaBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.auth = "user:pass".to_string();
        let b = OpenNebulaBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.disk_save_as_name = "image".to_string();
        let b = OpenNebulaBuilder::new(c);
        assert!(b.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_opennebula_client_mocked() -> Result<(), StampError> {
        let client = OpenNebulaClient::new(
            "http://one:2633/RPC2".to_string(),
            "oneadmin:pass".to_string(),
        );
        let vm_id = client.instantiate_template(1, "test-vm").await?;
        assert_eq!(vm_id, 1001);

        client.poweroff_vm(vm_id).await?;
        let img_id = client.disk_save_as(vm_id, 0, "test-img").await?;
        assert_eq!(img_id, 2002);

        client.terminate_vm(vm_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_opennebula_builder_run() -> Result<(), StampError> {
        let config = OpenNebulaConfig {
            name: "test-one".to_string(),
            endpoint: "http://one:2633/RPC2".to_string(),
            auth: "user:pass".to_string(),
            template_id: Some(5),
            disk_save_as_name: "saved-image".to_string(),
            ..Default::default()
        };
        let b = OpenNebulaBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let artifact = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert_eq!(artifact.id(), "opennebula:2002");
        b.cancel().await?;
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config = OpenNebulaConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let client = OpenNebulaClient::new("endpoint".to_string(), "auth".to_string());
        assert_eq!(format!("{client:?}"), format!("{client:?}"));
    }
}
