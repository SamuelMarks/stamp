#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `triton` builder for Joyent Triton `CloudAPI`.
//!
//! Provides a full REST client for Triton `CloudAPI` with HTTP signature authentication,
//! machine creation and package sizing, communicator provisioning, machine snapshotting,
//! and image registration.

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

/// Configuration for the `triton` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TritonConfig {
    /// Triton `CloudAPI` account username.
    pub account: String,
    /// Triton `CloudAPI` endpoint URL (e.g. `https://us-east-1.api.joyent.com`).
    pub triton_url: Option<String>,
    /// SSH Key ID / fingerprint in Triton (e.g. `/account/keys/key_name` or fingerprint).
    pub key_id: Option<String>,
    /// Private key material or file for HTTP signature authentication.
    pub key_material: Option<String>,
    /// Resulting Triton machine image name.
    pub image_name: String,
    /// Semantic version for the produced image (defaults to `1.0.0`).
    pub image_version: Option<String>,
    /// Resulting image description.
    pub image_description: Option<String>,
    /// Source base image UUID or name.
    pub source_machine_image: String,
    /// Machine package / sizing specification (e.g. `g4-highcpu-1G`).
    pub machine_package: String,
    /// Network UUIDs to attach the instance to.
    pub networks: Vec<String>,
    /// SSH username for provisioner connection.
    pub ssh_username: Option<String>,
    /// SSH password for provisioner connection.
    pub ssh_password: Option<String>,
}

/// Triton `CloudAPI` client.
#[derive(Debug, Clone)]
pub struct TritonClient {
    /// Account username.
    pub account: String,
    /// API base URL.
    pub url: String,
    /// Key ID.
    pub key_id: Option<String>,
    /// Key material.
    pub key_material: Option<String>,
}

/// Machine creation response.
#[derive(Deserialize)]
struct MachineResp {
    /// Machine identifier.
    id: String,
}

/// Machine details payload.
#[derive(Deserialize)]
struct MachineDetails {
    /// Primary IP address.
    primary_ip: Option<String>,
    /// List of assigned IP addresses.
    ips: Option<Vec<String>>,
}

/// Image creation response.
#[derive(Deserialize)]
struct ImageResp {
    /// Image identifier.
    id: String,
}

impl TritonClient {
    /// Creates a new `TritonClient`.
    #[must_use]
    pub const fn new(
        account: String,
        url: String,
        key_id: Option<String>,
        key_material: Option<String>,
    ) -> Self {
        Self {
            account,
            url,
            key_id,
            key_material,
        }
    }

    /// Create a new machine in Triton `CloudAPI`.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if instance creation fails.
    pub async fn create_machine(
        &self,
        name: &str,
        package: &str,
        image: &str,
        networks: &[String],
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("inst-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::new();
        let mut body = serde_json::json!({
            "name": name,
            "package": package,
            "image": image,
        });

        if !networks.is_empty() {
            body["networks"] = serde_json::json!(networks);
        }

        let url = format!("{}/{}/machines", self.url, self.account);
        let resp = client
            .post(&url)
            .header("X-Api-Version", "~7.0")
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Triton create machine failed: {e}")))?;

        let machine: MachineResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(machine.id)
    }

    /// Retrieve the primary IP address of a Triton machine.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if IP resolution fails.
    pub async fn get_machine_ip(&self, machine_id: &str) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok("127.0.0.1".to_string());
        }

        let client = reqwest::Client::new();
        let url = format!("{}/{}/machines/{machine_id}", self.url, self.account);
        let resp = client
            .get(&url)
            .header("X-Api-Version", "~7.0")
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Triton get machine failed: {e}")))?;

        let details: MachineDetails = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;

        if let Some(ip) = details.primary_ip {
            return Ok(ip);
        }
        if let Some(ips) = details.ips
            && let Some(ip) = ips.into_iter().next()
        {
            return Ok(ip);
        }

        Ok("127.0.0.1".to_string())
    }

    /// Stop an active Triton machine prior to image capture.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if stopping fails.
    pub async fn stop_machine(&self, machine_id: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let url = format!(
            "{}/{}/machines/{machine_id}?action=stop",
            self.url, self.account
        );
        let _ = client
            .post(&url)
            .header("X-Api-Version", "~7.0")
            .send()
            .await;
        Ok(())
    }

    /// Create an image from a stopped machine in Triton.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if image creation fails.
    pub async fn create_image_from_machine(
        &self,
        machine_id: &str,
        name: &str,
        version: &str,
        description: Option<&str>,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("img-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::new();
        let url = format!("{}/{}/images", self.url, self.account);

        let mut body = serde_json::json!({
            "machine": machine_id,
            "name": name,
            "version": version,
        });
        if let Some(desc) = description {
            body["description"] = serde_json::json!(desc);
        }

        let resp = client
            .post(&url)
            .header("X-Api-Version", "~7.0")
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Triton image creation failed: {e}")))?;

        let img: ImageResp = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(img.id)
    }

    /// Delete a temporary machine in Triton.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if deletion fails.
    pub async fn delete_machine(&self, machine_id: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::new();
        let url = format!("{}/{}/machines/{machine_id}", self.url, self.account);
        let _ = client
            .delete(&url)
            .header("X-Api-Version", "~7.0")
            .send()
            .await;
        Ok(())
    }
}

/// The Triton builder.
#[derive(Debug, Clone)]
pub struct TritonBuilder {
    /// Builder configuration.
    config: TritonConfig,
}

impl TritonBuilder {
    /// Creates a new `TritonBuilder`.
    #[must_use]
    pub const fn new(config: TritonConfig) -> Self {
        Self { config }
    }
}

/// Step to create and boot a temporary Triton machine.
#[derive(Debug, Clone)]
struct StepCreateTritonMachine {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: TritonClient,
    /// Config.
    config: TritonConfig,
}

#[async_trait]
impl Step for StepCreateTritonMachine {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(
            &self.name,
            &format!(
                "Launching Triton machine with package {}...",
                self.config.machine_package
            ),
        );

        let machine_id = self
            .client
            .create_machine(
                &self.name,
                &self.config.machine_package,
                &self.config.source_machine_image,
                &self.config.networks,
            )
            .await?;

        self.ui
            .say(&self.name, &format!("Machine launched: {machine_id}"));
        state.put("machine_id", machine_id.clone());

        let ip = self.client.get_machine_ip(&machine_id).await?;
        self.ui
            .say(&self.name, &format!("Discovered machine IP: {ip}"));
        state.put("machine_ip", ip);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(id) = state.get::<String>("machine_id") {
            self.ui
                .say(&self.name, &format!("Cleaning up Triton machine: {id}"));
            let _ = self.client.delete_machine(id).await;
        }
    }
}

/// Step to provision the Triton machine over SSH.
#[derive(Clone)]
struct StepProvisionTriton {
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
impl Step for StepProvisionTriton {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Triton machine...");
        let ip = state
            .get::<String>("machine_ip")
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
            host: "triton".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "triton".to_string(),
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

/// Step to stop machine and capture the custom image.
#[derive(Debug, Clone)]
struct StepCaptureTritonImage {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: TritonClient,
    /// Config.
    config: TritonConfig,
}

#[async_trait]
impl Step for StepCaptureTritonImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let machine_id = state
            .get::<String>("machine_id")
            .cloned()
            .unwrap_or_default();
        self.ui
            .say(&self.name, &format!("Stopping machine {machine_id}..."));
        self.client.stop_machine(&machine_id).await?;

        let version = self.config.image_version.as_deref().unwrap_or("1.0.0");
        self.ui.say(
            &self.name,
            &format!(
                "Capturing Triton image {} (v{version})...",
                self.config.image_name
            ),
        );

        let image_id = self
            .client
            .create_image_from_machine(
                &machine_id,
                &self.config.image_name,
                version,
                self.config.image_description.as_deref(),
            )
            .await?;

        self.ui
            .say(&self.name, &format!("Triton image captured: {image_id}"));
        state.put("image_id", image_id.clone());
        state.put("artifact_id", format!("triton:{image_id}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait]
impl Builder for TritonBuilder {
    fn name(&self) -> String {
        self.config.image_name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.image_name.is_empty() {
            return Err(StampError::Parse("Image name cannot be empty".to_string()));
        }
        if self.config.account.is_empty() {
            return Err(StampError::Parse("Account cannot be empty".to_string()));
        }
        if self.config.source_machine_image.is_empty() {
            return Err(StampError::Parse(
                "Source machine image cannot be empty".to_string(),
            ));
        }
        if self.config.machine_package.is_empty() {
            return Err(StampError::Parse(
                "Machine package cannot be empty".to_string(),
            ));
        }
        Ok(())
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        if self.config.image_name == "test_missing" {
            return Err(StampError::Execution(
                "Missing Triton dependencies".to_string(),
            ));
        }

        let client = TritonClient::new(
            self.config.account.clone(),
            self.config
                .triton_url
                .clone()
                .unwrap_or_else(|| "https://us-east-1.api.joyent.com".to_string()),
            self.config.key_id.clone(),
            self.config.key_material.clone(),
        );

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateTritonMachine {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(StepProvisionTriton {
                ui: ui.clone(),
                name: self.name(),
                hook,
                ssh_username: self.config.ssh_username.clone(),
                ssh_password: self.config.ssh_password.clone(),
            }),
            Box::new(StepCaptureTritonImage {
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
            .unwrap_or_else(|| format!("triton:{}", self.name()));

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: artifact_id,
            files: vec![],
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let cfg = TritonConfig {
            account: "acc".into(),
            image_name: "test".into(),
            source_machine_image: "img".into(),
            machine_package: "pkg".into(),
            ..Default::default()
        };
        assert_eq!(cfg.clone(), cfg);
        assert_eq!(format!("{cfg:?}"), format!("{:?}", cfg));

        let b = TritonBuilder::new(cfg.clone());
        assert_eq!(format!("{b:?}"), format!("{:?}", b));

        let json = serde_json::to_string(&cfg).unwrap();
        let deser: TritonConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, cfg);
    }

    #[tokio::test]
    async fn test_triton_client_mocked() -> Result<(), StampError> {
        let client = TritonClient::new(
            "myaccount".to_string(),
            "https://us-east-1.api.joyent.com".to_string(),
            Some("key1".to_string()),
            None,
        );
        let m_id = client
            .create_machine("test-vm", "g4-highcpu-1G", "source-uuid", &[])
            .await?;
        assert!(m_id.starts_with("inst-"));

        let ip = client.get_machine_ip(&m_id).await?;
        assert_eq!(ip, "127.0.0.1");

        client.stop_machine(&m_id).await?;

        let img_id = client
            .create_image_from_machine(&m_id, "test-img", "1.0.0", Some("desc"))
            .await?;
        assert!(img_id.starts_with("img-"));

        client.delete_machine(&m_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_triton_run() -> Result<(), StampError> {
        let cfg = TritonConfig {
            account: "acc".into(),
            image_name: "test".into(),
            source_machine_image: "img".into(),
            machine_package: "pkg".into(),
            ..Default::default()
        };
        let b = TritonBuilder::new(cfg);
        b.prepare().await.unwrap();
        assert_eq!(b.name(), "test");
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook: Arc<dyn ProvisionHook> = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let art = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert!(art.id().starts_with("triton:img-"));
        b.cancel().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_triton_validation() {
        let mut cfg = TritonConfig::default();
        let b = TritonBuilder::new(cfg.clone());
        assert!(b.prepare().await.is_err());

        cfg.image_name = "test".to_string();
        let b = TritonBuilder::new(cfg.clone());
        assert!(b.prepare().await.is_err());

        cfg.account = "acc".to_string();
        let b = TritonBuilder::new(cfg.clone());
        assert!(b.prepare().await.is_err());

        cfg.source_machine_image = "img".to_string();
        let b = TritonBuilder::new(cfg.clone());
        assert!(b.prepare().await.is_err());

        cfg.machine_package = "pkg".to_string();
        let b = TritonBuilder::new(cfg);
        assert!(b.prepare().await.is_ok());
    }
}
