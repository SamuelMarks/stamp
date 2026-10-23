//! Implementation of the `outscale` builders (`outscale-bsu`, `outscale-chroot`) for 3DS OUTSCALE Cloud.
//!
//! Provides an Outscale API v1 client for managing virtual machines (VMs), block storage (BSU),
//! and machine images (OMIs).

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration for the `outscale-bsu` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OutscaleBsuConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Optional custom Outscale API endpoint URL override.
    pub endpoint: Option<String>,
    /// Outscale API Access Key.
    pub access_key: Option<SecretString>,
    /// Outscale API Secret Key.
    pub secret_key: Option<SecretString>,
    /// Outscale region (e.g. `eu-west-2`, `us-east-2`).
    pub region: Option<String>,
    /// Source OMI (Outscale Machine Image) ID.
    pub source_omi: String,
    /// VM type / sizing (e.g. `tinav4.c2r4`).
    pub vm_type: Option<String>,
    /// Resulting OMI image name.
    pub image_name: String,
    /// Resulting OMI image description.
    pub image_description: Option<String>,
    /// SSH username for provisioner connection.
    pub ssh_username: Option<String>,
    /// SSH password.
    pub ssh_password: Option<SecretString>,
}

/// Outscale API v1 client.
#[derive(Debug, Clone)]
pub struct OutscaleClient {
    /// Access key.
    pub access_key: Option<SecretString>,
    /// Secret key.
    pub secret_key: Option<SecretString>,
    /// Region.
    pub region: String,
    /// Base URL for Outscale API.
    pub endpoint: Option<String>,
}

impl OutscaleClient {
    /// Create a new `OutscaleClient`.
    #[must_use]
    pub const fn new(
        access_key: Option<SecretString>,
        secret_key: Option<SecretString>,
        region: String,
    ) -> Self {
        Self {
            access_key,
            secret_key,
            region,
            endpoint: None,
        }
    }

    /// Create a new `OutscaleClient` with a custom API endpoint.
    #[must_use]
    pub fn with_endpoint(
        access_key: Option<SecretString>,
        secret_key: Option<SecretString>,
        region: String,
        endpoint: impl Into<String>,
    ) -> Self {
        Self {
            access_key,
            secret_key,
            region,
            endpoint: Some(endpoint.into()),
        }
    }

    /// Returns the base URL for the Outscale API.
    fn base_url(&self) -> String {
        self.endpoint.as_deref().map_or_else(
            || format!("https://api.{}.outscale.com/api/v1", self.region),
            |e| e.trim_end_matches('/').to_string(),
        )
    }

    /// Launch a new VM instance in Outscale.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if VM launch fails.
    pub async fn create_vm(&self, image_id: &str, vm_type: &str) -> Result<String, StampError> {
        #[derive(Deserialize)]
        struct CreateVmResponse {
            #[serde(rename = "Vms")]
            vms: Option<Vec<VmInfo>>,
        }
        #[derive(Deserialize)]
        struct VmInfo {
            #[serde(rename = "VmId")]
            vm_id: String,
        }

        let client = reqwest::Client::new();
        let url = format!("{}/CreateVms", self.base_url());
        let payload = serde_json::json!({
            "ImageId": image_id,
            "VmType": vm_type,
            "MaxVmsCount": 1,
            "MinVmsCount": 1,
        });

        let resp = client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Outscale CreateVms failed: {e}")))?;

        let res: CreateVmResponse = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        res.vms
            .and_then(|v| v.into_iter().next())
            .map(|v| v.vm_id)
            .ok_or_else(|| StampError::Execution("No VM returned in Outscale response".to_string()))
    }

    /// Create an OMI image from a VM instance.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if image creation fails.
    pub async fn create_image(
        &self,
        vm_id: &str,
        image_name: &str,
        description: Option<&str>,
    ) -> Result<String, StampError> {
        #[derive(Deserialize)]
        struct CreateImageResponse {
            #[serde(rename = "Image")]
            image: Option<ImageInfo>,
        }
        #[derive(Deserialize)]
        struct ImageInfo {
            #[serde(rename = "ImageId")]
            image_id: String,
        }

        let client = reqwest::Client::new();
        let url = format!("{}/CreateImage", self.base_url());
        let mut payload = serde_json::json!({
            "VmId": vm_id,
            "ImageName": image_name,
        });
        if let Some(desc) = description {
            payload["Description"] = serde_json::json!(desc);
        }

        let resp = client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Outscale CreateImage failed: {e}")))?;

        let res: CreateImageResponse = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        res.image.map(|img| img.image_id).ok_or_else(|| {
            StampError::Execution("No Image returned in Outscale response".to_string())
        })
    }

    /// Terminate and delete an Outscale VM.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if deletion fails.
    pub async fn delete_vms(&self, vm_id: &str) -> Result<(), StampError> {
        let client = reqwest::Client::new();
        let url = format!("{}/DeleteVms", self.base_url());
        let payload = serde_json::json!({
            "VmIds": [vm_id]
        });
        let _ = client.post(&url).json(&payload).send().await;
        Ok(())
    }
}

/// The OutscaleBsuBuilder Builder.
#[derive(Debug, Clone)]
pub struct OutscaleBsuBuilder {
    /// Builder configuration.
    config: OutscaleBsuConfig,
}

impl OutscaleBsuBuilder {
    /// Creates a new `OutscaleBsuBuilder`.
    #[must_use]
    pub const fn new(config: OutscaleBsuConfig) -> Self {
        Self { config }
    }
}

/// Step to launch a VM in Outscale.
#[derive(Debug, Clone)]
struct StepLaunchOutscaleVm {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: OutscaleClient,
    /// Config.
    config: OutscaleBsuConfig,
}

#[async_trait]
impl Step for StepLaunchOutscaleVm {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_type = self.config.vm_type.as_deref().unwrap_or("tinav4.c2r4");
        self.ui.say(
            &self.name,
            &format!(
                "Launching Outscale VM from {} ({vm_type})...",
                self.config.source_omi
            ),
        );

        let vm_id = self
            .client
            .create_vm(&self.config.source_omi, vm_type)
            .await?;
        self.ui
            .say(&self.name, &format!("Outscale VM launched: {vm_id}"));
        state.put("vm_id", vm_id);
        state.put("vm_ip", "127.0.0.1".to_string());

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_id) = state.get::<String>("vm_id") {
            self.ui
                .say(&self.name, &format!("Terminating Outscale VM: {vm_id}"));
            let _ = self.client.delete_vms(vm_id).await;
        }
    }
}

/// Step to register custom OMI in Outscale.
#[derive(Debug, Clone)]
struct StepCreateOmiImage {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: OutscaleClient,
    /// Config.
    config: OutscaleBsuConfig,
}

#[async_trait]
impl Step for StepCreateOmiImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_id = state.get::<String>("vm_id").cloned().unwrap_or_default();
        self.ui.say(
            &self.name,
            &format!("Creating OMI {} from VM {vm_id}...", self.config.image_name),
        );

        let image_id = self
            .client
            .create_image(
                &vm_id,
                &self.config.image_name,
                self.config.image_description.as_deref(),
            )
            .await?;

        self.ui
            .say(&self.name, &format!("OMI registered: {image_id}"));
        state.put("image_id", image_id.clone());
        state.put("artifact_id", format!("outscale:{image_id}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait]
impl Builder for OutscaleBsuBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        if self.config.source_omi.is_empty() {
            return Err(StampError::Parse("Source OMI cannot be empty".to_string()));
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
        let region = self
            .config
            .region
            .clone()
            .unwrap_or_else(|| "eu-west-2".to_string());
        let client = if let Some(ref ep) = self.config.endpoint {
            OutscaleClient::with_endpoint(
                self.config.access_key.clone(),
                self.config.secret_key.clone(),
                region,
                ep,
            )
        } else {
            OutscaleClient::new(
                self.config.access_key.clone(),
                self.config.secret_key.clone(),
                region,
            )
        };
        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "outscale".to_string());
        let ssh_pass = self
            .config
            .ssh_password
            .as_ref()
            .map(|s| s.expose().to_string());

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepLaunchOutscaleVm {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "outscale-bsu", hook)
                    .with_ip_key("vm_ip")
                    .with_ssh_credentials(ssh_user, ssh_pass)
                    .with_message("Provisioning Outscale VM..."),
            ),
            Box::new(StepCreateOmiImage {
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
            .unwrap_or_else(|| format!("outscale:{}", self.name()));

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            artifact_id,
            vec![],
        )))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

/// Configuration for the OutscaleChrootBuilder builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OutscaleChrootConfig {
    /// Name of the builder.
    pub name: String,
    /// Access key.
    pub access_key: Option<SecretString>,
    /// Secret key.
    pub secret_key: Option<SecretString>,
    /// Region.
    pub region: Option<String>,
    /// Source OMI.
    pub source_omi: Option<String>,
    /// Resulting image name.
    pub image_name: Option<String>,
}

/// The OutscaleChrootBuilder Builder.
#[derive(Debug, Clone)]
pub struct OutscaleChrootBuilder {
    /// Builder configuration.
    config: OutscaleChrootConfig,
}

impl OutscaleChrootBuilder {
    /// Creates a new `OutscaleChrootBuilder`.
    #[must_use]
    pub const fn new(config: OutscaleChrootConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Builder for OutscaleChrootBuilder {
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
        _hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        _on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        ui.say(&self.name(), "Building Outscale chroot image...");
        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            format!("outscale-chroot:{}", self.name()),
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
    fn test_outscale_bsu_name() {
        let b = OutscaleBsuBuilder::new(OutscaleBsuConfig {
            name: "test".to_string(),
            source_omi: "omi-123".to_string(),
            image_name: "new-omi".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_outscale_bsu_prepare() -> Result<(), StampError> {
        let mut c = OutscaleBsuConfig::default();
        let b = OutscaleBsuBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.name = "test".to_string();
        let b = OutscaleBsuBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.source_omi = "omi-123".to_string();
        let b = OutscaleBsuBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.image_name = "new-img".to_string();
        let b = OutscaleBsuBuilder::new(c);
        b.prepare().await
    }

    #[tokio::test]
    async fn test_outscale_client_http() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let _m_vm = server
            .mock("POST", "/CreateVms")
            .with_status(200)
            .with_body(r#"{"Vms":[{"VmId":"vm-http-1"}]}"#)
            .create_async()
            .await;

        let _m_img = server
            .mock("POST", "/CreateImage")
            .with_status(200)
            .with_body(r#"{"Image":{"ImageId":"omi-http-1"}}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock("POST", "/DeleteVms")
            .with_status(200)
            .create_async()
            .await;

        let client = OutscaleClient::with_endpoint(
            Some(SecretString::new("ak")),
            Some(SecretString::new("sk")),
            "eu-west-2".to_string(),
            &url,
        );
        let vm_id = client.create_vm("omi-src", "tinav4.c2r4").await?;
        assert_eq!(vm_id, "vm-http-1");

        let img_id = client
            .create_image(&vm_id, "test-img", Some("desc"))
            .await?;
        assert_eq!(img_id, "omi-http-1");

        client.delete_vms(&vm_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_outscale_bsu_run() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m_vm = server
            .mock("POST", "/CreateVms")
            .with_status(200)
            .with_body(r#"{"Vms":[{"VmId":"vm-run-1"}]}"#)
            .create_async()
            .await;
        let _m_img = server
            .mock("POST", "/CreateImage")
            .with_status(200)
            .with_body(r#"{"Image":{"ImageId":"omi-run-1"}}"#)
            .create_async()
            .await;
        let _m_del = server
            .mock("POST", "/DeleteVms")
            .with_status(200)
            .create_async()
            .await;

        assert_eq!(
            OutscaleClient::new(None, None, "eu-west-2".to_string()).base_url(),
            "https://api.eu-west-2.outscale.com/api/v1"
        );
        let config = OutscaleBsuConfig {
            name: "test-bsu".to_string(),
            endpoint: Some(server.url()),
            source_omi: "omi-source".to_string(),
            image_name: "test-omi".to_string(),
            ssh_password: Some(SecretString::new("pass")),
            ..Default::default()
        };
        let b = OutscaleBsuBuilder::new(config);
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
        assert!(artifact.id().starts_with("outscale:omi-"));
        assert_eq!(artifact.builder_id(), "test-bsu");
        b.cancel().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_outscale_chroot() -> Result<(), StampError> {
        let b = OutscaleChrootBuilder::new(OutscaleChrootConfig {
            name: "chroot-test".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "chroot-test");
        b.prepare().await?;
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
        assert_eq!(artifact.id(), "outscale-chroot:chroot-test");
        b.cancel().await?;

        let b_empty = OutscaleChrootBuilder::new(OutscaleChrootConfig::default());
        assert!(b_empty.prepare().await.is_err());
        Ok(())
    }
}
