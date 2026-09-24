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
            .unwrap_or_default();

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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::pedantic,
    clippy::all,
    for_loops_over_fallibles
)]
mod tests {
    use super::*;

    struct FailingProvisioner;
    #[async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn crate::communicator::Communicator,
            _ui: Arc<crate::engine::ui::Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Execution("mock provision failure".to_string()))
        }
    }

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
    async fn test_opennebula_client_mocked() {
        let mut server = mockito::Server::new_async().await;

        let _m_inst = server
            .mock("POST", "/RPC2")
            .match_header("content-type", "text/xml")
            .match_body(mockito::Matcher::Regex(
                r"one\.template\.instantiate".to_string(),
            ))
            .with_status(200)
            .with_body(
                r#"<?xml version="1.0"?><methodResponse><params><param><value><array><data><value><boolean>1</boolean></value><value><int>1001</int></value></data></array></value></param></params></methodResponse>"#,
            )
            .create_async()
            .await;

        let _m_poweroff = server
            .mock("POST", "/RPC2")
            .match_header("content-type", "text/xml")
            .match_body(mockito::Matcher::Regex("poweroff".to_string()))
            .with_status(200)
            .with_body(
                r#"<?xml version="1.0"?><methodResponse><params><param><value><boolean>1</boolean></value></param></params></methodResponse>"#,
            )
            .create_async()
            .await;

        let _m_save = server
            .mock("POST", "/RPC2")
            .match_header("content-type", "text/xml")
            .match_body(mockito::Matcher::Regex(r"one\.vm\.disksaveas".to_string()))
            .with_status(200)
            .with_body(
                r#"<?xml version="1.0"?><methodResponse><params><param><value><array><data><value><boolean>1</boolean></value><value><int>2002</int></value></data></array></value></param></params></methodResponse>"#,
            )
            .create_async()
            .await;

        let _m_term = server
            .mock("POST", "/RPC2")
            .match_header("content-type", "text/xml")
            .match_body(mockito::Matcher::Regex("terminate-hard".to_string()))
            .with_status(200)
            .with_body(
                r#"<?xml version="1.0"?><methodResponse><params><param><value><boolean>1</boolean></value></param></params></methodResponse>"#,
            )
            .create_async()
            .await;

        let client = OpenNebulaClient::new(
            format!("{}/RPC2", server.url()),
            "oneadmin:pass".to_string(),
        );
        let vm_id = client.instantiate_template(1, "test-vm").await;
        assert!(vm_id.is_ok());
        for id in vm_id {
            assert_eq!(id, 1001);
            let po = client.poweroff_vm(id).await;
            assert!(po.is_ok());
            let img_id = client.disk_save_as(id, 0, "test-img").await;
            assert!(img_id.is_ok());
            for i_id in img_id {
                assert_eq!(i_id, 2002);
            }
            let term = client.terminate_vm(id).await;
            assert!(term.is_ok());
        }
    }

    #[tokio::test]
    async fn test_opennebula_client_fault_and_errors() {
        let mut server = mockito::Server::new_async().await;

        let _m_inst_fault = server
            .mock("POST", "/RPC2_fault")
            .match_body(mockito::Matcher::Regex(
                r"one\.template\.instantiate".to_string(),
            ))
            .with_status(200)
            .with_body(r#"<methodResponse><fault><value><boolean>0</boolean></value></fault></methodResponse>"#)
            .create_async()
            .await;

        let _m_save_fault = server
            .mock("POST", "/RPC2_fault")
            .match_body(mockito::Matcher::Regex(r"one\.vm\.disksaveas".to_string()))
            .with_status(200)
            .with_body(r#"<methodResponse><fault><value><boolean>0</boolean></value></fault></methodResponse>"#)
            .create_async()
            .await;

        let client = OpenNebulaClient::new(
            format!("{}/RPC2_fault", server.url()),
            "oneadmin:pass".to_string(),
        );
        let inst_fault = client.instantiate_template(1, "vm").await;
        assert!(inst_fault.is_err());

        let save_fault = client.disk_save_as(1001, 0, "img").await;
        assert!(save_fault.is_err());

        // Connection error with invalid endpoint
        let bad_client = OpenNebulaClient::new(
            "http://invalid.domain.that.does.not.exist:9999/RPC2".to_string(),
            "a".to_string(),
        );
        assert!(bad_client.instantiate_template(1, "vm").await.is_err());
        assert!(bad_client.disk_save_as(1, 0, "img").await.is_err());
    }

    #[tokio::test]
    async fn test_opennebula_builder_run() {
        let mut server = mockito::Server::new_async().await;

        let _m_inst = server
            .mock("POST", "/RPC2")
            .match_body(mockito::Matcher::Regex(
                r"one\.template\.instantiate".to_string(),
            ))
            .with_status(200)
            .with_body(r#"<methodResponse><params><param><value><boolean>1</boolean></value></param></params></methodResponse>"#)
            .create_async()
            .await;

        let _m_poweroff = server
            .mock("POST", "/RPC2")
            .match_body(mockito::Matcher::Regex("poweroff".to_string()))
            .with_status(200)
            .create_async()
            .await;

        let _m_save = server
            .mock("POST", "/RPC2")
            .match_body(mockito::Matcher::Regex(r"one\.vm\.disksaveas".to_string()))
            .with_status(200)
            .with_body(r#"<methodResponse><params><param><value><boolean>1</boolean></value></param></params></methodResponse>"#)
            .create_async()
            .await;

        let _m_term = server
            .mock("POST", "/RPC2")
            .match_body(mockito::Matcher::Regex("terminate-hard".to_string()))
            .with_status(200)
            .create_async()
            .await;

        let config = OpenNebulaConfig {
            name: "test-one".to_string(),
            endpoint: format!("{}/RPC2", server.url()),
            auth: "user:pass".to_string(),
            template_id: Some(5),
            template_name: Some("tpl-base".to_string()),
            vm_name: Some("custom-vm".to_string()),
            disk_save_as_name: "saved-image".to_string(),
            disk_id: Some(1),
            cpu: Some(2.0),
            vcpu: Some(2),
            memory_mb: Some(4096),
            ssh_username: Some("root".to_string()),
            ssh_password: Some("secret".to_string()),
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
            .await;
        assert!(artifact.is_ok());
        for art in artifact {
            assert_eq!(art.id(), "opennebula:2002");
        }
        assert!(b.cancel().await.is_ok());
    }

    #[tokio::test]
    async fn test_opennebula_builder_run_failures() {
        let config_fail = OpenNebulaConfig {
            name: "test-fail".to_string(),
            endpoint: "http://invalid.endpoint:9999/RPC2".to_string(),
            auth: "user:pass".to_string(),
            disk_save_as_name: "saved-image".to_string(),
            ..Default::default()
        };
        let b_fail = OpenNebulaBuilder::new(config_fail);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // Cleanup strategy on error
        let res_cleanup = b_fail
            .run(
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res_cleanup.is_err());

        // Abort strategy on error
        let res_abort = b_fail
            .run(
                hook,
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Abort,
            )
            .await;
        assert!(res_abort.is_err());

        // Provisioner failure
        let fail_hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut step_prov = StepProvisionOpenNebula {
            ui,
            name: "test-prov".to_string(),
            hook: fail_hook,
            ssh_username: None,
            ssh_password: None,
        };
        let mut prov_state = StateBag::new();
        // Missing vm_ip falls back to 127.0.0.1
        let prov_res = step_prov.run(&mut prov_state).await;
        assert!(prov_res.is_err());
    }

    #[tokio::test]
    async fn test_step_cleanups_and_edges() {
        let mut server = mockito::Server::new_async().await;
        let _m_term = server
            .mock("POST", "/RPC2")
            .with_status(200)
            .create_async()
            .await;

        let client = OpenNebulaClient::new(format!("{}/RPC2", server.url()), "auth".to_string());
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // StepInstantiateOpenNebulaVm cleanup with and without vm_id
        let mut step_inst = StepInstantiateOpenNebulaVm {
            ui: ui.clone(),
            name: "test".to_string(),
            client: client.clone(),
            config: OpenNebulaConfig::default(),
        };
        let mut state = StateBag::new();
        step_inst.cleanup(&state).await;
        state.put("vm_id", 1001u32);
        step_inst.cleanup(&state).await;

        // StepSaveOpenNebulaImage run with default disk_id (None) and default vm_id (None)
        let _m_poweroff = server
            .mock("POST", "/RPC2")
            .with_status(200)
            .create_async()
            .await;
        let _m_save = server
            .mock("POST", "/RPC2")
            .with_status(200)
            .with_body(r#"<methodResponse><params><param><value><boolean>1</boolean></value></param></params></methodResponse>"#)
            .create_async()
            .await;

        let mut step_save = StepSaveOpenNebulaImage {
            ui: ui.clone(),
            name: "test".to_string(),
            client,
            config: OpenNebulaConfig {
                disk_id: None,
                disk_save_as_name: "default-disk-save".to_string(),
                ..Default::default()
            },
        };
        let mut empty_state = StateBag::new();
        let save_res = step_save.run(&mut empty_state).await;
        assert!(save_res.is_ok());
        step_save.cleanup(&empty_state).await;

        // StepProvisionOpenNebula run with vm_ip in state
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut step_prov = StepProvisionOpenNebula {
            ui,
            name: "test".to_string(),
            hook,
            ssh_username: Some("root".to_string()),
            ssh_password: None,
        };
        let mut state_with_ip = StateBag::new();
        state_with_ip.put("vm_ip", "10.0.0.1".to_string());
        let prov_ok = step_prov.run(&mut state_with_ip).await;
        assert!(prov_ok.is_ok());
        step_prov.cleanup(&state_with_ip).await;
    }

    #[test]
    fn test_derived_traits() {
        let config = OpenNebulaConfig {
            name: "test".to_string(),
            endpoint: "http://one:2633/RPC2".to_string(),
            auth: "oneadmin:pass".to_string(),
            template_id: Some(1),
            template_name: Some("tpl".to_string()),
            vm_name: Some("vm".to_string()),
            cpu: Some(1.0),
            vcpu: Some(1),
            memory_mb: Some(1024),
            disk_save_as_name: "img".to_string(),
            disk_id: Some(0),
            ssh_username: Some("root".to_string()),
            ssh_password: Some("pass".to_string()),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let serialized = serde_json::to_string(&config);
        assert!(serialized.is_ok());
        for json in serialized {
            let deserialized: Result<OpenNebulaConfig, _> = serde_json::from_str(&json);
            assert!(deserialized.is_ok());
        }

        let builder = OpenNebulaBuilder::new(config);
        assert_eq!(format!("{builder:?}"), format!("{builder:?}"));

        let client = OpenNebulaClient::new("endpoint".to_string(), "auth".to_string());
        assert_eq!(format!("{client:?}"), format!("{client:?}"));
    }
}
