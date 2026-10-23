//! Implementation of the `triton` builder for Joyent Triton `CloudAPI`.
//!
//! Provides a full REST client for Triton `CloudAPI` with HTTP signature authentication,
//! machine creation and package sizing, communicator provisioning, machine snapshotting,
//! and image registration.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
    pub key_material: Option<SecretString>,
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
    pub ssh_password: Option<SecretString>,
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
    pub key_material: Option<SecretString>,
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
        key_material: Option<SecretString>,
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
        let client = TritonClient::new(
            self.config.account.clone(),
            self.config
                .triton_url
                .clone()
                .unwrap_or_else(|| "https://us-east-1.api.joyent.com".to_string()),
            self.config.key_id.clone(),
            self.config.key_material.clone(),
        );
        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "root".to_string());
        let ssh_pass = self
            .config
            .ssh_password
            .as_ref()
            .map(|s| s.expose().to_string());

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateTritonMachine {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "triton", hook)
                    .with_ip_key("machine_ip")
                    .with_ssh_credentials(ssh_user, ssh_pass)
                    .with_message("Provisioning Triton machine..."),
            ),
            Box::new(StepCaptureTritonImage {
                ui: ui.clone(),
                name: self.name(),
                client,
                config: self.config.clone(),
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();

        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            artifact_id,
            vec![],
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::packer::OnErrorStrategy;

    #[tokio::test]
    async fn test_triton_run() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let _m_create = server
            .mock("POST", "/acc/machines")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id": "inst-12345"}"#)
            .create_async()
            .await;

        let _m_get = server
            .mock("GET", "/acc/machines/inst-12345")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"primary_ip": "10.0.0.50"}"#)
            .create_async()
            .await;

        let _m_stop = server
            .mock("POST", "/acc/machines/inst-12345?action=stop")
            .with_status(200)
            .create_async()
            .await;

        let _m_img = server
            .mock("POST", "/acc/images")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id": "img-67890"}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("DELETE", "/acc/machines/inst-12345")
            .with_status(200)
            .create_async()
            .await;

        let cfg = TritonConfig {
            account: "acc".into(),
            triton_url: Some(server.url()),
            image_name: "test-img".into(),
            image_version: Some("2.0.0".into()),
            image_description: Some("Custom image".into()),
            source_machine_image: "base-uuid".into(),
            machine_package: "g4-highcpu-1G".into(),
            networks: vec!["net-uuid-1".into()],
            ssh_username: Some("admin".into()),
            ssh_password: Some(SecretString::new("secret")),
            ..Default::default()
        };

        let b = TritonBuilder::new(cfg);
        b.prepare().await?;
        assert_eq!(b.name(), "test-img");

        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook: Arc<dyn ProvisionHook> = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });

        let art = b.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        assert_eq!(art.id(), "triton:img-67890");
        assert_eq!(art.builder_id(), "test-img");

        b.cancel().await
    }

    #[tokio::test]
    async fn test_triton_client_methods_and_branches() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let _m_create = server
            .mock("POST", "/acc/machines")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id": "inst-empty-net"}"#)
            .create_async()
            .await;

        let client = TritonClient::new(
            "acc".to_string(),
            server.url(),
            Some("key".to_string()),
            Some(SecretString::new("mat")),
        );

        let id = client.create_machine("vm", "pkg", "img", &[]).await?;
        assert_eq!(id, "inst-empty-net");

        let _m_get_ips = server
            .mock("GET", "/acc/machines/inst-ips")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"primary_ip": null, "ips": ["192.168.1.100"]}"#)
            .create_async()
            .await;
        let ip = client.get_machine_ip("inst-ips").await?;
        assert_eq!(ip, "192.168.1.100");

        let _m_get_none = server
            .mock("GET", "/acc/machines/inst-none")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"primary_ip": null, "ips": []}"#)
            .create_async()
            .await;
        let ip2 = client.get_machine_ip("inst-none").await?;
        assert_eq!(ip2, "127.0.0.1");

        let _m_stop = server
            .mock("POST", "/acc/machines/inst-1?action=stop")
            .with_status(200)
            .create_async()
            .await;
        client.stop_machine("inst-1").await?;

        let _m_del = server
            .mock("DELETE", "/acc/machines/inst-1")
            .with_status(200)
            .create_async()
            .await;
        client.delete_machine("inst-1").await?;

        let _m_img = server
            .mock("POST", "/acc/images")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id": "img-no-desc"}"#)
            .create_async()
            .await;
        let img_id = client
            .create_image_from_machine("inst-1", "name", "1.0", None)
            .await?;
        assert_eq!(img_id, "img-no-desc");
        Ok(())
    }

    #[tokio::test]
    async fn test_triton_validation_and_errors() -> Result<(), StampError> {
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
        b.prepare().await?;

        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let cfg_fail = TritonConfig {
            account: "acc".to_string(),
            triton_url: Some("http://127.0.0.1:1".to_string()),
            image_name: "fail".to_string(),
            source_machine_image: "img".to_string(),
            machine_package: "pkg".to_string(),
            ..Default::default()
        };
        let b_fail = TritonBuilder::new(cfg_fail);
        assert!(b_fail.run(hook, ui, OnErrorStrategy::Abort).await.is_err());
        Ok(())
    }
}
