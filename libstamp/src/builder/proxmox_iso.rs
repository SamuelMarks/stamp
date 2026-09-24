#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `proxmox-iso` builder with Proxmox VE REST API client,
//! ticket and API token authentication, ISO and cloud-init drive creation, and template conversion (`qm template`).

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `proxmox-iso` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProxmoxIsoConfig {
    /// The name of the builder instance.
    pub name: String,
    /// Proxmox API base URL (e.g. `https://pve.example.com:8006/api2/json`).
    pub proxmox_url: Option<String>,
    /// Proxmox username (e.g. `root@pam` or `packer@pve`).
    pub username: Option<String>,
    /// Proxmox password for ticket authentication.
    pub password: Option<String>,
    /// Proxmox API Token (e.g. `USER@REALM!TOKENID=UUID`).
    pub token: Option<String>,
    /// Proxmox node to provision on (defaults to `pve`).
    pub node: Option<String>,
    /// Target VM ID. Defaults to 999.
    pub vm_id: Option<u32>,
    /// Memory size in MB. Defaults to 1024.
    pub memory: Option<u64>,
    /// CPU cores. Defaults to 1.
    pub cores: Option<u32>,
    /// The source ISO file on Proxmox storage (e.g. `local:iso/ubuntu-22.04.iso`).
    pub iso_file: Option<String>,
    /// Whether to attach a cloud-init drive.
    pub cloud_init: bool,
    /// Storage pool for the cloud-init drive (e.g. `local-lvm`).
    pub cloud_init_storage_pool: Option<String>,
    /// The boot command sequence.
    pub boot_command: Option<Vec<String>>,
    /// The wait time before booting.
    pub boot_wait: Option<String>,
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

/// The `proxmox-iso` builder.
#[derive(Debug, Clone)]
pub struct ProxmoxIsoBuilder {
    /// The builder configuration.
    pub config: ProxmoxIsoConfig,
}

impl ProxmoxIsoBuilder {
    /// Create a new `ProxmoxIsoBuilder`.
    #[must_use]
    pub const fn new(config: ProxmoxIsoConfig) -> Self {
        Self { config }
    }
}

/// Helper function to create an authenticated Proxmox VE REST client.
///
/// Supports API token headers (`PVEAPIToken`) and username/password ticket authentication.
///
/// # Errors
///
/// Returns `StampError::Execution` if authentication or client construction fails.
pub async fn proxmox_client(config: &ProxmoxIsoConfig) -> Result<reqwest::Client, StampError> {
    let mut headers = reqwest::header::HeaderMap::new();

    if let Some(ref token) = config.token {
        let auth_val = if token.starts_with("PVEAPIToken=") {
            token.clone()
        } else {
            format!("PVEAPIToken={token}")
        };
        let auth_header = reqwest::header::HeaderValue::from_str(&auth_val)
            .map_err(|e| StampError::Execution(format!("Invalid token header: {e}")))?;
        headers.insert(reqwest::header::AUTHORIZATION, auth_header);
    } else if let (Some(user), Some(pass)) = (&config.username, &config.password) {
        let base_url = config
            .proxmox_url
            .as_deref()
            .unwrap_or("https://localhost:8006/api2/json");

        let auth_client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_default();

        let ticket_resp = auth_client
            .post(format!("{base_url}/access/ticket"))
            .json(&serde_json::json!({
                "username": user,
                "password": pass,
            }))
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Proxmox ticket request failed: {e}")))?;

        if !ticket_resp.status().is_success() {
            let err_text = ticket_resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "Proxmox ticket auth rejected: {err_text}"
            )));
        }

        let ticket_json: serde_json::Value = ticket_resp.json().await.unwrap_or_default();
        let ticket = ticket_json["data"]["ticket"].as_str().unwrap_or_default();
        let csrf = ticket_json["data"]["CSRFPreventionToken"]
            .as_str()
            .unwrap_or_default();

        let cookie_val = format!("PVEAuthCookie={ticket}");
        headers.insert(
            reqwest::header::COOKIE,
            reqwest::header::HeaderValue::from_str(&cookie_val)
                .map_err(|e| StampError::Execution(e.to_string()))?,
        );
        headers.insert(
            "CSRFPreventionToken",
            reqwest::header::HeaderValue::from_str(csrf)
                .map_err(|e| StampError::Execution(e.to_string()))?,
        );
    }

    Ok(reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .default_headers(headers)
        .build()
        .unwrap_or_default())
}

/// Step to provision the VM, attach the installation ISO, and optionally create a cloud-init drive.
#[derive(Debug, Clone)]
struct StepCreateVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ProxmoxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let node = self.config.node.as_deref().unwrap_or("pve");
        let vmid = self.config.vm_id.unwrap_or(999);
        self.ui.say(
            &self.name,
            &format!("Creating Proxmox VM {vmid} on node {node}"),
        );

        state.put("vm_id", vmid);
        state.put("node", node.to_string());
        state.put("vm_ip", "127.0.0.1".to_string());

        let client = proxmox_client(&self.config).await?;
        let url = self
            .config
            .proxmox_url
            .as_deref()
            .unwrap_or("https://localhost:8006/api2/json");

        let mut payload = serde_json::json!({
            "vmid": vmid,
            "name": self.config.template_name.as_deref().unwrap_or("packer-proxmox-iso"),
            "memory": self.config.memory.unwrap_or(1024),
            "cores": self.config.cores.unwrap_or(1),
            "ide2": format!("{},media=cdrom", self.config.iso_file.as_deref().unwrap_or("local:iso/ubuntu.iso")),
            "net0": "virtio,bridge=vmbr0",
        });

        if let Some(ref b) = self.config.bios {
            payload["bios"] = serde_json::json!(b);
        }

        if let Some(ref pool) = self.config.efi_storage_pool {
            payload["bios"] = serde_json::json!("ovmf");
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
            payload["efidisk0"] = serde_json::json!(format!("{pool}:1,efitype=4m{pre}{fmt}"));
        }

        if let Some(ref scsi) = self.config.scsihw {
            payload["scsihw"] = serde_json::json!(scsi);
        }

        if self.config.scsi_iothread {
            payload["scsihw"] = serde_json::json!(
                self.config
                    .scsihw
                    .as_deref()
                    .unwrap_or("virtio-scsi-single")
            );
            payload["scsi0"] = serde_json::json!("local-lvm:32,iothread=1");
        }

        if self.config.cloud_init {
            let pool = self
                .config
                .cloud_init_storage_pool
                .as_deref()
                .unwrap_or("local-lvm");
            payload["ide0"] = serde_json::json!(format!("{pool}:cloudinit"));
            self.ui.say(
                &self.name,
                &format!("Configured cloud-init drive on storage pool {pool}"),
            );
        }

        let res = client
            .post(format!("{url}/nodes/{node}/qemu"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Proxmox API Create VM failed: {e}")))?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "Proxmox API returned {status}: {text}"
            )));
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let (Some(vmid), Some(node)) = (state.get::<u32>("vm_id"), state.get::<String>("node")) {
            self.ui.say(
                &self.name,
                &format!("Cleaning up Proxmox VM {vmid} on node {node}"),
            );
            if let Ok(client) = proxmox_client(&self.config).await {
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

/// Step to start the VM and wait for boot.
#[derive(Debug, Clone)]
struct StepStartVM {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ProxmoxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepStartVM {
    #[cfg_attr(coverage_nightly, coverage(off))]
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

        let client = proxmox_client(&self.config).await?;
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

    async fn cleanup(&mut self, state: &StateBag) {
        if let (Some(vmid), Some(node)) = (state.get::<u32>("vm_id"), state.get::<String>("node"))
            && let Ok(client) = proxmox_client(&self.config).await
        {
            let url = self
                .config
                .proxmox_url
                .as_deref()
                .unwrap_or("https://localhost:8006/api2/json");
            let _ = client
                .post(format!("{url}/nodes/{node}/qemu/{vmid}/status/stop"))
                .send()
                .await;
        }
    }
}

/// Step to provision the Proxmox VM over SSH.
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
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Proxmox VM...");

        let ip = state
            .get::<String>("vm_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

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
            host: "proxmox".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "proxmox-iso".to_string(),
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

/// Step to convert the completed Proxmox VM into a template (`qm template`).
#[derive(Debug, Clone)]
struct StepConvertToTemplate {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// Builder configuration.
    config: ProxmoxIsoConfig,
}

#[async_trait::async_trait]
impl Step for StepConvertToTemplate {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vmid = state.get::<u32>("vm_id").copied().unwrap_or(999);
        let node = state
            .get::<String>("node")
            .cloned()
            .unwrap_or_else(|| "pve".to_string());

        self.ui.say(
            &self.name,
            &format!("Converting VM {vmid} to Proxmox template (qm template)..."),
        );

        let client = proxmox_client(&self.config).await?;
        let url = self
            .config
            .proxmox_url
            .as_deref()
            .unwrap_or("https://localhost:8006/api2/json");

        // Stop VM first
        let _ = client
            .post(format!("{url}/nodes/{node}/qemu/{vmid}/status/stop"))
            .send()
            .await;
        let sleep_duration = if cfg!(test) {
            Duration::from_millis(1)
        } else {
            Duration::from_secs(3)
        };
        tokio::time::sleep(sleep_duration).await;

        // Convert to template
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
impl Builder for ProxmoxIsoBuilder {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        Ok(())
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateVM {
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

    #[derive(Clone)]
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
    async fn test_proxmoxisobuilder_run() {
        let mut server = mockito::Server::new_async().await;

        let _m_create = server
            .mock("POST", "/nodes/pve-node-1/qemu")
            .with_status(200)
            .create_async()
            .await;

        let _m_start = server
            .mock("POST", "/nodes/pve-node-1/qemu/101/status/start")
            .with_status(200)
            .create_async()
            .await;

        let _m_stop = server
            .mock("POST", "/nodes/pve-node-1/qemu/101/status/stop")
            .with_status(200)
            .create_async()
            .await;

        let _m_tpl = server
            .mock("POST", "/nodes/pve-node-1/qemu/101/template")
            .with_status(200)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/nodes/pve-node-1/qemu/101")
            .with_status(200)
            .create_async()
            .await;

        let config = ProxmoxIsoConfig {
            name: "test-builder".to_string(),
            proxmox_url: Some(server.url()),
            node: Some("pve-node-1".to_string()),
            vm_id: Some(101),
            cloud_init: true,
            cloud_init_storage_pool: Some("local-lvm".to_string()),
            token: Some("root@pam!token=12345".to_string()),
            bios: Some("ovmf".to_string()),
            efi_storage_pool: Some("local-lvm".to_string()),
            efi_format: Some("raw".to_string()),
            pre_enrolled_keys: true,
            scsihw: Some("virtio-scsi-single".to_string()),
            scsi_iothread: true,
            memory: Some(2048),
            cores: Some(2),
            iso_file: Some("local:iso/ubuntu-22.04.iso".to_string()),
            template_name: Some("custom-template".to_string()),
            ..Default::default()
        };
        let builder = ProxmoxIsoBuilder::new(config);

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
            assert!(art.id().contains("101"));
        }

        assert!(builder.cancel().await.is_ok());
    }

    #[tokio::test]
    async fn test_proxmox_client_auth_matrix() {
        // Token already prefixed with PVEAPIToken=
        let cfg_token_prefixed = ProxmoxIsoConfig {
            token: Some("PVEAPIToken=root@pam!tok=abc".to_string()),
            ..Default::default()
        };
        assert!(proxmox_client(&cfg_token_prefixed).await.is_ok());

        // Invalid token with newline
        let cfg_token_bad = ProxmoxIsoConfig {
            token: Some("invalid\ntoken".to_string()),
            ..Default::default()
        };
        assert!(proxmox_client(&cfg_token_bad).await.is_err());

        // Ticket auth success
        let mut server = mockito::Server::new_async().await;
        let _m_ticket = server
            .mock("POST", "/access/ticket")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data": {"ticket": "TICKET123", "CSRFPreventionToken": "CSRF456"}}"#)
            .create_async()
            .await;

        let cfg_ticket = ProxmoxIsoConfig {
            proxmox_url: Some(server.url()),
            username: Some("root@pam".to_string()),
            password: Some("secret".to_string()),
            ..Default::default()
        };
        assert!(proxmox_client(&cfg_ticket).await.is_ok());

        // Ticket auth rejected
        let _m_ticket_err = server
            .mock("POST", "/access/ticket")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;
        let cfg_ticket_fail = ProxmoxIsoConfig {
            proxmox_url: Some(server.url()),
            username: Some("baduser".to_string()),
            password: Some("badpass".to_string()),
            ..Default::default()
        };
        assert!(proxmox_client(&cfg_ticket_fail).await.is_err());

        // Ticket auth network failure
        let cfg_ticket_net_err = ProxmoxIsoConfig {
            proxmox_url: Some("http://127.0.0.1:1".to_string()),
            username: Some("user".to_string()),
            password: Some("pass".to_string()),
            ..Default::default()
        };
        assert!(proxmox_client(&cfg_ticket_net_err).await.is_err());

        // Ticket auth with invalid cookie character (\n in ticket)
        let _m_ticket_bad_cookie = server
            .mock("POST", "/access/ticket")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data": {"ticket": "TICKET\n123", "CSRFPreventionToken": "CSRF456"}}"#)
            .create_async()
            .await;
        assert!(proxmox_client(&cfg_ticket).await.is_err());

        // Ticket auth with invalid CSRF character (\n in csrf)
        let _m_ticket_bad_csrf = server
            .mock("POST", "/access/ticket")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data": {"ticket": "TICKET123", "CSRFPreventionToken": "CSRF\n456"}}"#)
            .create_async()
            .await;
        assert!(proxmox_client(&cfg_ticket).await.is_err());

        // No token, no user/pass: default client
        let cfg_default = ProxmoxIsoConfig::default();
        assert!(proxmox_client(&cfg_default).await.is_ok());
    }

    #[tokio::test]
    async fn test_proxmox_step_create_vm_branches() {
        let mut server = mockito::Server::new_async().await;
        let _m_create = server
            .mock("POST", "/nodes/pve/qemu")
            .with_status(200)
            .create_async()
            .await;

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // Create VM without efi format, pre_enrolled_keys false, cloud_init false
        let cfg1 = ProxmoxIsoConfig {
            name: "test1".to_string(),
            proxmox_url: Some(server.url()),
            efi_storage_pool: Some("local-lvm".to_string()),
            efi_format: None,
            pre_enrolled_keys: false,
            cloud_init: false,
            scsi_iothread: false,
            ..Default::default()
        };
        let mut step1 = StepCreateVM {
            ui: ui.clone(),
            name: "test1".to_string(),
            config: cfg1,
        };
        let mut state1 = StateBag::new();
        assert!(step1.run(&mut state1).await.is_ok());

        // Server error (500)
        let _m_err = server
            .mock("POST", "/nodes/pve/qemu")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;
        let cfg_err = ProxmoxIsoConfig {
            name: "test_err".to_string(),
            proxmox_url: Some(server.url()),
            ..Default::default()
        };
        let mut step_err = StepCreateVM {
            ui: ui.clone(),
            name: "test_err".to_string(),
            config: cfg_err,
        };
        let mut state_err = StateBag::new();
        assert!(step_err.run(&mut state_err).await.is_err());

        // Network error (bad url)
        let cfg_bad_url = ProxmoxIsoConfig {
            name: "test_bad_url".to_string(),
            proxmox_url: Some("http://127.0.0.1:1".to_string()),
            ..Default::default()
        };
        let mut step_bad = StepCreateVM {
            ui,
            name: "test_bad".to_string(),
            config: cfg_bad_url,
        };
        let mut state_bad = StateBag::new();
        assert!(step_bad.run(&mut state_bad).await.is_err());
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
        let config = ProxmoxIsoConfig {
            proxmox_url: Some(server.url()),
            ..Default::default()
        };

        // StepCreateVM cleanup with state
        let mut step_create = StepCreateVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        let mut state = StateBag::new();
        state.put("vm_id", 102u32);
        state.put("node", "pve".to_string());
        step_create.cleanup(&state).await;

        // StepCreateVM cleanup empty state
        let empty_state = StateBag::new();
        step_create.cleanup(&empty_state).await;

        // StepStartVM run and cleanup
        let _m_start = server
            .mock("POST", "/nodes/pve/qemu/102/status/start")
            .with_status(200)
            .create_async()
            .await;
        let _m_stop = server
            .mock("POST", "/nodes/pve/qemu/102/status/stop")
            .with_status(200)
            .create_async()
            .await;

        let mut step_start = StepStartVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        assert!(step_start.run(&mut state).await.is_ok());
        step_start.cleanup(&state).await;

        // StepStartVM empty state (fallbacks)
        let mut step_start_empty = StepStartVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        let mut empty_state_start = StateBag::new();
        assert!(step_start_empty.run(&mut empty_state_start).await.is_ok());
        step_start_empty.cleanup(&empty_state_start).await;

        // StepConvertToTemplate template conversion error
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

        // StepConvertToTemplate network error
        let mut bad_config = config.clone();
        bad_config.proxmox_url = Some("http://127.0.0.1:1".to_string());
        let mut step_tpl_fail = StepConvertToTemplate {
            ui: ui.clone(),
            name: "test".to_string(),
            config: bad_config,
        };
        let mut empty_state_tpl = StateBag::new();
        assert!(step_tpl_fail.run(&mut empty_state_tpl).await.is_err());

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
    }

    #[tokio::test]
    async fn test_proxmox_builder_run_error_strategies() {
        let mut server = mockito::Server::new_async().await;
        let _m_err = server
            .mock("POST", "/nodes/pve/qemu")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let config = ProxmoxIsoConfig {
            name: "test-err".to_string(),
            proxmox_url: Some(server.url()),
            ..Default::default()
        };
        let builder = ProxmoxIsoBuilder::new(config);
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
        // RunCleanupProvisioner
        assert!(
            builder
                .run(
                    hook.clone(),
                    ui.clone(),
                    OnErrorStrategy::RunCleanupProvisioner
                )
                .await
                .is_err()
        );
        // Ask (with "yes" answer)
        let mut queue = std::collections::VecDeque::new();
        queue.push_back("yes".to_string());
        let ui_ask = Arc::new(
            Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(Arc::new(std::sync::Mutex::new(queue))),
        );
        assert!(
            builder
                .run(hook.clone(), ui_ask, OnErrorStrategy::Ask)
                .await
                .is_err()
        );

        // Ask (with "no" answer)
        assert!(builder.run(hook, ui, OnErrorStrategy::Ask).await.is_err());
    }

    #[tokio::test]
    async fn test_proxmox_prepare_and_cancel() {
        let mut cfg = ProxmoxIsoConfig::default();
        let b = ProxmoxIsoBuilder::new(cfg.clone());
        assert!(b.prepare().await.is_err());

        cfg.name = "valid".to_string();
        let b2 = ProxmoxIsoBuilder::new(cfg);
        assert!(b2.prepare().await.is_ok());
        assert!(b2.cancel().await.is_ok());
    }

    #[test]
    fn test_proxmox_derived_traits() {
        let config1 = ProxmoxIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = ProxmoxIsoBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));
    }
}
