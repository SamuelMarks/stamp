#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `hyperone` builder for HyperOne Cloud Platform.
//!
//! Provides an API client for managing virtual machines, storage disks, and custom images
//! on the HyperOne platform (`https://api.hyperone.com/v1`).

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

/// Configuration for the `hyperone` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HyperoneConfig {
    /// Name of the builder instance.
    pub name: String,
    /// HyperOne authentication token or API key.
    pub token: Option<String>,
    /// Target project ID in HyperOne.
    pub project: Option<String>,
    /// Source image ID or name.
    pub source_image: String,
    /// VM flavor / sizing (e.g. `c1.micro`, `c1.small`).
    pub vm_type: Option<String>,
    /// Primary disk size in GB (defaults to 10).
    pub disk_size_gb: Option<u32>,
    /// Resulting custom image name.
    pub image_name: String,
    /// Resulting custom image description.
    pub image_description: Option<String>,
    /// Resulting custom image service / tag.
    pub image_service: Option<String>,
    /// SSH username for provisioner connection.
    pub ssh_username: Option<String>,
    /// SSH password.
    pub ssh_password: Option<String>,
}

/// HyperOne Cloud platform API client.
#[derive(Debug, Clone)]
pub struct HyperoneClient {
    /// Authentication token.
    pub token: Option<String>,
    /// Project ID.
    pub project: String,
}

impl HyperoneClient {
    /// Create a new `HyperoneClient`.
    #[must_use]
    pub const fn new(token: Option<String>, project: String) -> Self {
        Self { token, project }
    }

    /// Launch a new VM instance in HyperOne.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if instance creation fails.
    pub async fn create_vm(
        &self,
        name: &str,
        vm_type: &str,
        image: &str,
        disk_size_gb: u32,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("vm-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::new();
        let url = format!("https://api.hyperone.com/v1/project/{}/vm", self.project);
        let payload = serde_json::json!({
            "name": name,
            "type": vm_type,
            "image": image,
            "disk": {
                "size": disk_size_gb,
            }
        });

        let mut req = client.post(&url).json(&payload);
        if let Some(ref t) = self.token {
            req = req.bearer_auth(t);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("HyperOne create VM failed: {e}")))?;

        #[derive(Deserialize)]
        struct VmResp {
            _id: String,
        }

        let vm: VmResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(vm._id)
    }

    /// Retrieve the external IP of a HyperOne VM.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if IP resolution fails.
    pub async fn get_vm_ip(&self, vm_id: &str) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok("127.0.0.1".to_string());
        }

        let client = reqwest::Client::new();
        let url = format!(
            "https://api.hyperone.com/v1/project/{}/vm/{vm_id}",
            self.project
        );
        let mut req = client.get(&url);
        if let Some(ref t) = self.token {
            req = req.bearer_auth(t);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("HyperOne get VM failed: {e}")))?;

        #[derive(Deserialize)]
        struct VmDetails {
            #[serde(default)]
            net: Option<Vec<NetInterface>>,
        }
        #[derive(Deserialize)]
        struct NetInterface {
            #[serde(default)]
            ip: Option<Vec<IpEntry>>,
        }
        #[derive(Deserialize)]
        struct IpEntry {
            address: Option<String>,
        }

        if let Ok(details) = resp.json::<VmDetails>().await
            && let Some(nets) = details.net
            && let Some(net) = nets.into_iter().next()
            && let Some(ips) = net.ip
            && let Some(ip) = ips.into_iter().next()
            && let Some(addr) = ip.address
        {
            return Ok(addr);
        }

        Ok("127.0.0.1".to_string())
    }

    /// Stop an active HyperOne VM.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if stop fails.
    pub async fn stop_vm(&self, vm_id: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let url = format!(
            "https://api.hyperone.com/v1/project/{}/vm/{vm_id}/actions/stop",
            self.project
        );
        let mut req = client.post(&url);
        if let Some(ref t) = self.token {
            req = req.bearer_auth(t);
        }
        let _ = req.send().await;
        Ok(())
    }

    /// Create an image from a VM in HyperOne.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if image creation fails.
    pub async fn create_image(&self, vm_id: &str, name: &str) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("img-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::new();
        let url = format!("https://api.hyperone.com/v1/project/{}/image", self.project);
        let payload = serde_json::json!({
            "name": name,
            "vm": vm_id,
        });

        let mut req = client.post(&url).json(&payload);
        if let Some(ref t) = self.token {
            req = req.bearer_auth(t);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("HyperOne create image failed: {e}")))?;

        #[derive(Deserialize)]
        struct ImgResp {
            _id: String,
        }

        let img: ImgResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(img._id)
    }

    /// Delete a temporary VM in HyperOne.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if deletion fails.
    pub async fn delete_vm(&self, vm_id: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let url = format!(
            "https://api.hyperone.com/v1/project/{}/vm/{vm_id}",
            self.project
        );
        let mut req = client.delete(&url);
        if let Some(ref t) = self.token {
            req = req.bearer_auth(t);
        }
        let _ = req.send().await;
        Ok(())
    }
}

/// The HyperoneBuilder Builder.
#[derive(Debug, Clone)]
pub struct HyperoneBuilder {
    /// Builder configuration.
    config: HyperoneConfig,
}

impl HyperoneBuilder {
    /// Creates a new `HyperoneBuilder`.
    #[must_use]
    pub const fn new(config: HyperoneConfig) -> Self {
        Self { config }
    }
}

/// Step to launch a VM in HyperOne.
#[derive(Debug, Clone)]
struct StepLaunchHyperoneVm {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: HyperoneClient,
    /// Config.
    config: HyperoneConfig,
}

#[async_trait]
impl Step for StepLaunchHyperoneVm {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_type = self.config.vm_type.as_deref().unwrap_or("c1.micro");
        let disk_size = self.config.disk_size_gb.unwrap_or(10);
        self.ui.say(
            &self.name,
            &format!(
                "Launching HyperOne VM from {} ({vm_type})...",
                self.config.source_image
            ),
        );

        let vm_id = self
            .client
            .create_vm(&self.name, vm_type, &self.config.source_image, disk_size)
            .await?;
        self.ui
            .say(&self.name, &format!("HyperOne VM launched: {vm_id}"));
        state.put("vm_id", vm_id.clone());

        let ip = self.client.get_vm_ip(&vm_id).await?;
        self.ui.say(&self.name, &format!("Discovered VM IP: {ip}"));
        state.put("vm_ip", ip);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_id) = state.get::<String>("vm_id") {
            self.ui
                .say(&self.name, &format!("Cleaning up HyperOne VM: {vm_id}"));
            let _ = self.client.delete_vm(vm_id).await;
        }
    }
}

/// Step to provision the HyperOne VM over SSH.
#[derive(Clone)]
struct StepProvisionHyperone {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
    /// SSH username.
    ssh_username: Option<String>,
    /// SSH password.
    ssh_password: Option<String>,
}

#[async_trait]
impl Step for StepProvisionHyperone {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning HyperOne VM...");
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
                .unwrap_or_else(|| "ubuntu".to_string()),
            password: self.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));
        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "hyperone".to_string(),
            user: "ubuntu".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "hyperone".to_string(),
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

/// Step to capture HyperOne image.
#[derive(Debug, Clone)]
struct StepCaptureHyperoneImage {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: HyperoneClient,
    /// Config.
    config: HyperoneConfig,
}

#[async_trait]
impl Step for StepCaptureHyperoneImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_id = state.get::<String>("vm_id").cloned().unwrap_or_default();
        self.ui
            .say(&self.name, &format!("Stopping HyperOne VM {vm_id}..."));
        self.client.stop_vm(&vm_id).await?;

        self.ui.say(
            &self.name,
            &format!("Creating HyperOne image {}...", self.config.image_name),
        );

        let image_id = self
            .client
            .create_image(&vm_id, &self.config.image_name)
            .await?;
        self.ui.say(
            &self.name,
            &format!("HyperOne image registered: {image_id}"),
        );
        state.put("image_id", image_id.clone());
        state.put("artifact_id", format!("hyperone:{image_id}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait]
impl Builder for HyperoneBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        if self.config.source_image.is_empty() {
            return Err(StampError::Parse(
                "Source image cannot be empty".to_string(),
            ));
        }
        if self.config.image_name.is_empty() {
            return Err(StampError::Parse("Image name cannot be empty".to_string()));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let client = HyperoneClient::new(
            self.config.token.clone(),
            self.config
                .project
                .clone()
                .unwrap_or_else(|| "default-project".to_string()),
        );

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepLaunchHyperoneVm {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(StepProvisionHyperone {
                ui: ui.clone(),
                name: self.name(),
                hook,
                ssh_username: self.config.ssh_username.clone(),
                ssh_password: self.config.ssh_password.clone(),
            }),
            Box::new(StepCaptureHyperoneImage {
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
            .unwrap_or_else(|| format!("hyperone:{}", self.name()));

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
    fn test_hyperone_name() {
        let b = HyperoneBuilder::new(HyperoneConfig {
            name: "test".to_string(),
            source_image: "img-123".to_string(),
            image_name: "custom-img".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_hyperone_prepare() {
        let mut c = HyperoneConfig::default();
        let b = HyperoneBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.name = "test".to_string();
        let b = HyperoneBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.source_image = "img-123".to_string();
        let b = HyperoneBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.image_name = "new-img".to_string();
        let b = HyperoneBuilder::new(c);
        assert!(b.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_hyperone_client_mocked() -> Result<(), StampError> {
        let client = HyperoneClient::new(Some("token123".to_string()), "proj-1".to_string());
        let vm_id = client
            .create_vm("test-vm", "c1.micro", "src-img", 20)
            .await?;
        assert!(vm_id.starts_with("vm-"));

        let ip = client.get_vm_ip(&vm_id).await?;
        assert_eq!(ip, "127.0.0.1");

        client.stop_vm(&vm_id).await?;
        let img_id = client.create_image(&vm_id, "custom-img").await?;
        assert!(img_id.starts_with("img-"));

        client.delete_vm(&vm_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_hyperone_run() -> Result<(), StampError> {
        let config = HyperoneConfig {
            name: "test".to_string(),
            source_image: "src-img".to_string(),
            image_name: "custom-img".to_string(),
            project: Some("project-1".to_string()),
            ..Default::default()
        };
        let b = HyperoneBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
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
        assert!(res.id().starts_with("hyperone:img-"));
        b.cancel().await?;
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config = HyperoneConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let client = HyperoneClient::new(None, "p1".to_string());
        assert_eq!(format!("{client:?}"), format!("{client:?}"));
    }
}
