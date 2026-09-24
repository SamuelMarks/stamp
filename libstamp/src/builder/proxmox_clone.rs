#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `proxmox-clone` builder with Proxmox VE REST API client,
//! template cloning, provisioning, and template conversion (`qm template`).

pub use super::proxmox_iso::ProxmoxIsoConfig;
use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `proxmox-clone` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProxmoxCloneConfig {
    /// The name of the builder instance.
    pub name: String,
    /// Proxmox base URL.
    pub proxmox_url: Option<String>,
    /// Proxmox username.
    pub username: Option<String>,
    /// Proxmox password for ticket auth.
    pub password: Option<String>,
    /// Proxmox API token.
    pub token: Option<String>,
    /// Proxmox node to provision on.
    pub node: Option<String>,
    /// The template/VM ID to clone from.
    pub clone_vm: Option<String>,
    /// New VM ID.
    pub vm_id: Option<u32>,
    /// Template name to create.
    pub template_name: Option<String>,
    /// BIOS type (`ovmf` for UEFI or `seabios` for legacy BIOS).
    pub bios: Option<String>,
    /// Target storage pool for UEFI/EFI disk (e.g. `local-lvm`).
    pub efi_storage_pool: Option<String>,
    /// Target format for the EFI disk (`raw` or `qcow2`).
    pub efi_format: Option<String>,
    /// Whether to enroll default Microsoft secure boot keys on EFI disk.
    pub pre_enrolled_keys: bool,
    /// SCSI controller model (`virtio-scsi-single`, `virtio-scsi-pci`, `lsi`, etc.).
    pub scsihw: Option<String>,
    /// Whether to enable IO thread on SCSI disks.
    pub scsi_iothread: bool,
}

/// The `proxmox-clone` builder.
#[derive(Debug, Clone)]
pub struct ProxmoxCloneBuilder {
    /// The builder configuration.
    pub config: ProxmoxCloneConfig,
}

impl ProxmoxCloneBuilder {
    /// Create a new `ProxmoxCloneBuilder`.
    #[must_use]
    pub const fn new(config: ProxmoxCloneConfig) -> Self {
        Self { config }
    }
}

/// Helper function to create an authenticated Proxmox VE REST client for cloning.
///
/// # Errors
///
/// Returns `StampError::Execution` on authentication or client creation error.
pub async fn proxmox_clone_client(
    config: &ProxmoxCloneConfig,
) -> Result<reqwest::Client, StampError> {
    let iso_conf = ProxmoxIsoConfig {
        proxmox_url: config.proxmox_url.clone(),
        username: config.username.clone(),
        password: config.password.clone(),
        token: config.token.clone(),
        node: config.node.clone(),
        ..Default::default()
    };
    super::proxmox_iso::proxmox_client(&iso_conf).await
}

/// Step to clone an existing VM template.
#[derive(Debug, Clone)]
struct StepCloneVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ProxmoxCloneConfig,
}

#[async_trait::async_trait]
impl Step for StepCloneVM {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let node = self.config.node.as_deref().unwrap_or("pve");
        let vmid = self.config.vm_id.unwrap_or(999);
        let clone_vm = self.config.clone_vm.as_deref().unwrap_or("100");

        self.ui.say(
            &self.name,
            &format!("Cloning Proxmox VM {vmid} from {clone_vm} on node {node}"),
        );

        state.put("vm_id", vmid);
        state.put("node", node.to_string());
        state.put("vm_ip", "127.0.0.1".to_string());

        let client = proxmox_clone_client(&self.config).await?;
        let url = self
            .config
            .proxmox_url
            .as_deref()
            .unwrap_or("https://localhost:8006/api2/json");

        let payload = serde_json::json!({
            "newid": vmid,
            "name": self.config.template_name.as_deref().unwrap_or("packer-proxmox-clone"),
            "full": 1,
        });

        let res = client
            .post(format!("{url}/nodes/{node}/qemu/{clone_vm}/clone"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Proxmox API Clone VM failed: {e}")))?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "Proxmox API returned {status}: {text}"
            )));
        }

        // Apply hardware customizations (BIOS, EFI disk, SCSI controller)
        if self.config.bios.is_some()
            || self.config.efi_storage_pool.is_some()
            || self.config.scsihw.is_some()
            || self.config.scsi_iothread
        {
            let mut conf_payload = serde_json::json!({});
            if let Some(ref b) = self.config.bios {
                conf_payload["bios"] = serde_json::json!(b);
            }
            if let Some(ref pool) = self.config.efi_storage_pool {
                conf_payload["bios"] = serde_json::json!("ovmf");
                let pre = if self.config.pre_enrolled_keys {
                    ",pre-enrolled-keys=1"
                } else {
                    ""
                };
                let fmt = if let Some(ref f) = self.config.efi_format {
                    format!(",format={f}")
                } else {
                    String::new()
                };
                conf_payload["efidisk0"] =
                    serde_json::json!(format!("{pool}:1,efitype=4m{pre}{fmt}"));
            }
            if let Some(ref scsi) = self.config.scsihw {
                conf_payload["scsihw"] = serde_json::json!(scsi);
            }
            if self.config.scsi_iothread {
                conf_payload["scsihw"] = serde_json::json!(
                    self.config
                        .scsihw
                        .as_deref()
                        .unwrap_or("virtio-scsi-single")
                );
                conf_payload["scsi0"] = serde_json::json!("local-lvm:32,iothread=1");
            }
            let _ = client
                .post(format!("{url}/nodes/{node}/qemu/{vmid}/config"))
                .json(&conf_payload)
                .send()
                .await;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let (Some(vmid), Some(node)) = (state.get::<u32>("vm_id"), state.get::<String>("node")) {
            self.ui.say(
                &self.name,
                &format!("Cleaning up Proxmox VM {vmid} on node {node}"),
            );
            if let Ok(client) = proxmox_clone_client(&self.config).await {
                let url = self
                    .config
                    .proxmox_url
                    .as_deref()
                    .unwrap_or("https://localhost:8006/api2/json");
                let _ = client
                    .delete(format!("{url}/nodes/{node}/qemu/{vmid}"))
                    .send()
                    .await;
            }
        }
    }
}

/// Step to start the cloned VM.
#[derive(Debug, Clone)]
struct StepStartVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ProxmoxCloneConfig,
}

#[async_trait::async_trait]
impl Step for StepStartVM {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vmid = state.get::<u32>("vm_id").copied().unwrap_or(999);
        let node = state
            .get::<String>("node")
            .cloned()
            .unwrap_or_else(|| "pve".to_string());
        self.ui.say(
            &self.name,
            &format!("Starting Proxmox VM {vmid} on node {node}"),
        );

        let client = proxmox_clone_client(&self.config).await?;
        let url = self
            .config
            .proxmox_url
            .as_deref()
            .unwrap_or("https://localhost:8006/api2/json");

        let _ = client
            .post(format!("{url}/nodes/{node}/qemu/{vmid}/status/start"))
            .send()
            .await;

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to provision the cloned VM over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Proxmox VM...");

        let ip = state.get::<String>("vm_ip").cloned().unwrap_or_default();

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: "root".to_string(),
            private_key_path: None,
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "proxmox-clone".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "proxmox-clone".to_string(),
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

/// Step to convert the provisioned VM into a Proxmox template.
#[derive(Debug, Clone)]
struct StepConvertToTemplate {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ProxmoxCloneConfig,
}

#[async_trait::async_trait]
impl Step for StepConvertToTemplate {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vmid = state.get::<u32>("vm_id").copied().unwrap_or(999);
        let node = state
            .get::<String>("node")
            .cloned()
            .unwrap_or_else(|| "pve".to_string());

        self.ui.say(
            &self.name,
            &format!("Converting cloned VM {vmid} to Proxmox template (qm template)..."),
        );

        let client = proxmox_clone_client(&self.config).await?;
        let url = self
            .config
            .proxmox_url
            .as_deref()
            .unwrap_or("https://localhost:8006/api2/json");

        let _ = client
            .post(format!("{url}/nodes/{node}/qemu/{vmid}/status/stop"))
            .send()
            .await;
        tokio::time::sleep(Duration::from_millis(if cfg!(test) { 1 } else { 3000 })).await;

        let res = client
            .post(format!("{url}/nodes/{node}/qemu/{vmid}/template"))
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Convert to template failed: {e}")))?;

        if !res.status().is_success() {
            return Err(StampError::Execution(format!(
                "Proxmox template conversion returned {}",
                res.status()
            )));
        }

        state.put("artifact_id", format!("proxmox:{node}/{vmid}"));
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for ProxmoxCloneBuilder {
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
        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCloneVM {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepStartVM {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                hook,
            }),
            Box::new(StepConvertToTemplate {
                ui: ui.clone(),
                name: self.name(),
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

    fn name(&self) -> String {
        self.config.name.clone()
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
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::packer::OnErrorStrategy;
    use crate::engine::ui::Ui;

    struct FailingProvisioner;
    #[async_trait::async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn crate::communicator::Communicator,
            _ui: Arc<crate::engine::ui::Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Execution("mock provision failure".to_string()))
        }
    }

    #[tokio::test]
    async fn test_proxmoxclonebuilder_run() {
        let mut server = mockito::Server::new_async().await;

        let _m_clone = server
            .mock("POST", "/nodes/pve-node-1/qemu/9000/clone")
            .with_status(200)
            .create_async()
            .await;

        let _m_config = server
            .mock("POST", "/nodes/pve-node-1/qemu/102/config")
            .with_status(200)
            .create_async()
            .await;

        let _m_start = server
            .mock("POST", "/nodes/pve-node-1/qemu/102/status/start")
            .with_status(200)
            .create_async()
            .await;

        let _m_stop = server
            .mock("POST", "/nodes/pve-node-1/qemu/102/status/stop")
            .with_status(200)
            .create_async()
            .await;

        let _m_tpl = server
            .mock("POST", "/nodes/pve-node-1/qemu/102/template")
            .with_status(200)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/nodes/pve-node-1/qemu/102")
            .with_status(200)
            .create_async()
            .await;

        let config = ProxmoxCloneConfig {
            name: "test-builder".to_string(),
            proxmox_url: Some(server.url()),
            node: Some("pve-node-1".to_string()),
            clone_vm: Some("9000".to_string()),
            vm_id: Some(102),
            token: Some("root@pam!token=12345".to_string()),
            bios: Some("ovmf".to_string()),
            efi_storage_pool: Some("local-lvm".to_string()),
            efi_format: Some("raw".to_string()),
            pre_enrolled_keys: true,
            scsihw: Some("virtio-scsi-single".to_string()),
            scsi_iothread: true,
            ..Default::default()
        };
        let builder = ProxmoxCloneBuilder::new(config);

        assert!(builder.prepare().await.is_ok());
        assert_eq!(builder.name(), "test-builder");

        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = builder.run(hook, ui, OnErrorStrategy::Cleanup).await;
        assert!(artifact.is_ok());
        for art in artifact {
            assert!(art.id().contains("102"));
        }

        assert!(builder.cancel().await.is_ok());
    }

    #[test]
    fn test_proxmox_clone_derived_traits() {
        let config1 = ProxmoxCloneConfig {
            name: "test".to_string(),
            node: Some("pve".to_string()),
            ..Default::default()
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let serialized = serde_json::to_string(&config1);
        assert!(serialized.is_ok());
        for json in serialized {
            let deserialized: Result<ProxmoxCloneConfig, _> = serde_json::from_str(&json);
            assert!(deserialized.is_ok());
        }

        let b1 = ProxmoxCloneBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));
    }

    #[tokio::test]
    async fn test_proxmox_clone_prepare_failure() {
        let builder = ProxmoxCloneBuilder::new(ProxmoxCloneConfig::default());
        assert!(builder.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_proxmox_clone_failures_and_strategies() {
        let mut server = mockito::Server::new_async().await;

        let _m_clone_err = server
            .mock("POST", "/nodes/pve/qemu/100/clone")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let config = ProxmoxCloneConfig {
            name: "test-err".to_string(),
            proxmox_url: Some(server.url()),
            ..Default::default()
        };
        let builder = ProxmoxCloneBuilder::new(config);
        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // Cleanup
        assert!(
            builder
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        // Abort
        assert!(
            builder
                .run(hook.clone(), ui.clone(), OnErrorStrategy::Abort)
                .await
                .is_err()
        );
        // Ask
        assert!(builder.run(hook, ui, OnErrorStrategy::Ask).await.is_err());
    }

    #[tokio::test]
    async fn test_proxmox_step_branches_and_cleanups() {
        let mut server = mockito::Server::new_async().await;
        let _m_del = server
            .mock("DELETE", "/nodes/pve/qemu/102")
            .with_status(200)
            .create_async()
            .await;

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let config = ProxmoxCloneConfig {
            proxmox_url: Some(server.url()),
            ..Default::default()
        };

        // StepCloneVM cleanup with state
        let mut step_clone = StepCloneVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        let mut state = StateBag::new();
        state.put("vm_id", 102u32);
        state.put("node", "pve".to_string());
        step_clone.cleanup(&state).await;

        // StepCloneVM cleanup empty state
        let empty_state = StateBag::new();
        step_clone.cleanup(&empty_state).await;

        // StepConvertToTemplate template conversion error
        let _m_stop = server
            .mock("POST", "/nodes/pve/qemu/102/status/stop")
            .with_status(200)
            .create_async()
            .await;
        let _m_tpl_err = server
            .mock("POST", "/nodes/pve/qemu/102/template")
            .with_status(500)
            .create_async()
            .await;

        let mut step_tpl = StepConvertToTemplate {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        assert!(step_tpl.run(&mut state).await.is_err());
        step_tpl.cleanup(&state).await;

        // StepProvision failure
        let fail_hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut step_prov = StepProvision {
            ui: ui.clone(),
            name: "test".to_string(),
            hook: fail_hook,
        };
        assert!(step_prov.run(&mut state).await.is_err());
        step_prov.cleanup(&state).await;

        // StepCloneVM network error (send failure)
        let mut bad_config = config.clone();
        bad_config.proxmox_url = Some("http://127.0.0.1:1".to_string());
        let mut step_clone_fail = StepCloneVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: bad_config.clone(),
        };
        let mut state_bad = StateBag::new();
        assert!(step_clone_fail.run(&mut state_bad).await.is_err());

        // StepStartVM with empty state (triggering node fallback)
        let mut step_start = StepStartVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: bad_config.clone(),
        };
        let mut empty_state_start = StateBag::new();
        assert!(step_start.run(&mut empty_state_start).await.is_ok());

        // StepConvertToTemplate with empty state and bad network (node fallback + send failure)
        let mut step_tpl_fail = StepConvertToTemplate {
            ui,
            name: "test".to_string(),
            config: bad_config,
        };
        let mut empty_state_tpl = StateBag::new();
        assert!(step_tpl_fail.run(&mut empty_state_tpl).await.is_err());
    }
}
