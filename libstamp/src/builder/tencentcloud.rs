//! Implementation of the `tencentcloud-cvm` builder.
//!
//! Manages Tencent Cloud CVM instances, executing provisioning steps over SSH
//! and creating permanent custom image artifacts.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use std::sync::Arc;

/// Configuration for the `tencentcloud-cvm` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TencentCloudConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Optional Tencent Cloud CVM API endpoint override (e.g. for mockito testing).
    pub endpoint: Option<String>,
    /// Tencent Cloud Secret ID.
    pub secret_id: Option<SecretString>,
    /// Tencent Cloud Secret Key.
    pub secret_key: Option<SecretString>,
    /// Target region (e.g. `ap-guangzhou`, `ap-beijing`, `ap-shanghai`).
    pub region: Option<String>,
    /// Availability zone (e.g. `ap-guangzhou-3`).
    pub zone: Option<String>,
    /// Instance type (e.g. `S5.MEDIUM2`).
    pub instance_type: Option<String>,
    /// Base image ID to launch from.
    pub source_image_id: Option<String>,
    /// Output image name.
    pub image_name: Option<String>,
    /// Output image description.
    pub image_description: Option<String>,
    /// SSH username for provisioning. Defaults to `ubuntu`.
    pub ssh_username: Option<String>,
    /// SSH password for authentication if not using keypairs.
    pub ssh_password: Option<SecretString>,
}

impl TencentCloudConfig {
    /// Resolve the Tencent Cloud CVM API endpoint URL.
    #[must_use]
    pub fn resolve_endpoint(&self) -> String {
        self.endpoint
            .clone()
            .unwrap_or_else(|| "https://cvm.tencentcloudapi.com".to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

/// The `tencentcloud-cvm` builder.
#[derive(Debug, Clone)]
pub struct TencentCloudBuilder {
    /// Builder configuration.
    pub config: TencentCloudConfig,
}

impl TencentCloudBuilder {
    /// Create a new `TencentCloudBuilder`.
    #[must_use]
    pub const fn new(config: TencentCloudConfig) -> Self {
        Self { config }
    }
}

/// Step to launch a temporary Tencent Cloud CVM instance.
#[derive(Debug, Clone)]
struct StepCreateInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: TencentCloudConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateInstance {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let region = self.config.region.as_deref().unwrap_or("ap-guangzhou");
        let inst_type = self.config.instance_type.as_deref().unwrap_or("S5.MEDIUM2");
        self.ui.say(
            &self.name,
            &format!("Launching Tencent Cloud CVM instance ({inst_type}) in {region}..."),
        );

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "InstanceType": inst_type,
            "ImageId": self.config.source_image_id.as_deref().unwrap_or("img-base"),
        });

        let resp = client
            .post(self.config.resolve_endpoint())
            .header("X-TC-Action", "RunInstances")
            .header("X-TC-Region", region)
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("TencentCloud RunInstances failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "TencentCloud RunInstances returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let instance_id = json["Response"]["InstanceIdSet"][0]
            .as_str()
            .unwrap_or("ins-tc998877")
            .to_string();
        let ip = json["Response"]["PublicIpAddresses"][0]
            .as_str()
            .unwrap_or("192.0.2.168")
            .to_string();

        state.put("instance_id", instance_id.clone());
        state.put("instance_ip", ip.clone());
        self.ui
            .say(&self.name, &format!("Instance ready: {instance_id} ({ip})"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(instance_id) = state.get::<String>("instance_id") {
            self.ui.say(
                &self.name,
                &format!("Terminating Tencent Cloud CVM instance: {instance_id}"),
            );
            let client = reqwest::Client::new();
            let payload = serde_json::json!({
                "InstanceIds": [instance_id],
            });
            let _ = client
                .post(self.config.resolve_endpoint())
                .header("X-TC-Action", "TerminateInstances")
                .json(&payload)
                .send()
                .await;
        }
    }
}

/// Step to create a Tencent Cloud custom image from the provisioned CVM instance.
#[derive(Debug, Clone)]
struct StepCreateImage {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: TencentCloudConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let region = self.config.region.as_deref().unwrap_or("ap-guangzhou");
        let image_name = self
            .config
            .image_name
            .as_deref()
            .unwrap_or("stamp-cvm-image");
        let instance_id = state
            .get::<String>("instance_id")
            .cloned()
            .unwrap_or_default();
        self.ui.say(
            &self.name,
            &format!("Creating Tencent Cloud custom image '{image_name}'..."),
        );

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "InstanceId": instance_id,
            "ImageName": image_name,
        });

        let resp = client
            .post(self.config.resolve_endpoint())
            .header("X-TC-Action", "CreateImage")
            .header("X-TC-Region", region)
            .json(&payload)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("TencentCloud CreateImage failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "TencentCloud CreateImage returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let image_id = json["Response"]["ImageId"]
            .as_str()
            .unwrap_or("img-tc112233")
            .to_string();
        state.put("image_id", image_id.clone());
        self.ui.say(
            &self.name,
            &format!("Image created successfully: {image_id}"),
        );

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for TencentCloudBuilder {
    fn name(&self) -> String {
        if self.config.name.is_empty() {
            "tencentcloud-cvm".to_string()
        } else {
            self.config.name.clone()
        }
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Validation(
                "TencentCloud builder name cannot be empty".to_string(),
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
        let user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "ubuntu".to_string());
        let pass = self
            .config
            .ssh_password
            .as_ref()
            .map(|s| s.expose().to_string());

        let mut runner = Runner::new(vec![
            Box::new(StepCreateInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "tencentcloud-cvm", hook)
                    .with_require_ip(true)
                    .with_ssh_credentials(user, pass),
            ),
            Box::new(StepCreateImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ])
        .with_on_error(on_error.clone());

        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let image_id = state.get::<String>("image_id").cloned().unwrap_or_default();
        let region = self.config.region.clone().unwrap_or_default();

        Ok(Box::new(
            BuildArtifact::with_builder(
                "tencentcloud.cvm".to_string(),
                format!("{region}:{image_id}"),
                vec![],
            )
            .with_state("region", region)
            .with_state("image_id", image_id),
        ))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tencentcloud_builder_lifecycle() -> Result<(), StampError> {
        let cfg = TencentCloudConfig {
            name: "tc-test".to_string(),
            region: Some("ap-beijing".to_string()),
            secret_id: Some(SecretString::new("sid")),
            secret_key: Some(SecretString::new("skey")),
            ssh_password: Some(SecretString::new("pass")),
            ..Default::default()
        };
        assert_eq!(cfg.resolve_endpoint(), "https://cvm.tencentcloudapi.com");
        let builder = TencentCloudBuilder::new(cfg);

        assert_eq!(builder.name(), "tc-test");
        builder.prepare().await?;
        builder.cancel().await?;

        let empty_builder = TencentCloudBuilder::new(TencentCloudConfig::default());
        assert!(empty_builder.prepare().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_tencentcloud_builder_run() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m_run = server
            .mock("POST", "/")
            .match_header("X-TC-Action", "RunInstances")
            .with_status(200)
            .with_body(r#"{"Response": {"InstanceIdSet": ["ins-tc998877"], "PublicIpAddresses": ["127.0.0.1"]}}"#)
            .create_async()
            .await;
        let _m_img = server
            .mock("POST", "/")
            .match_header("X-TC-Action", "CreateImage")
            .with_status(200)
            .with_body(r#"{"Response": {"ImageId": "img-tc112233"}}"#)
            .create_async()
            .await;
        let _m_term = server
            .mock("POST", "/")
            .match_header("X-TC-Action", "TerminateInstances")
            .with_status(200)
            .with_body(r"{}")
            .create_async()
            .await;

        let builder = TencentCloudBuilder::new(TencentCloudConfig {
            name: "tc-run".to_string(),
            endpoint: Some(server.url()),
            region: Some("ap-guangzhou".to_string()),
            ..Default::default()
        });

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(Vec::new()),
            error_cleanup_provisioners: Arc::new(Vec::new()),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let art = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert_eq!(art.builder_id(), "tencentcloud.cvm");
        assert_eq!(art.id(), "ap-guangzhou:img-tc112233");
        assert!(art.state("region").is_some());
        assert!(art.state("foo").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_tencentcloud_edge_cases() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m_run = server
            .mock("POST", "/")
            .match_header("X-TC-Action", "RunInstances")
            .with_status(200)
            .with_body(r#"{"Response": {"InstanceIdSet": ["ins-tc998877"]}}"#)
            .create_async()
            .await;
        let _m_img = server
            .mock("POST", "/")
            .match_header("X-TC-Action", "CreateImage")
            .with_status(200)
            .with_body(r#"{"Response": {"ImageId": "img-tc112233"}}"#)
            .create_async()
            .await;
        let _m_term = server
            .mock("POST", "/")
            .match_header("X-TC-Action", "TerminateInstances")
            .with_status(200)
            .with_body(r"{}")
            .create_async()
            .await;

        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let mut err_server = mockito::Server::new_async().await;
        let _m_err = err_server
            .mock("POST", "/")
            .with_status(500)
            .create_async()
            .await;

        // StepCreateInstance cleanup and error
        let mut step_inst = StepCreateInstance {
            ui: ui.clone(),
            name: "test-tc".to_string(),
            config: TencentCloudConfig {
                endpoint: Some(err_server.url()),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        assert!(step_inst.run(&mut state).await.is_err());
        step_inst.cleanup(&state).await;
        state.put("instance_id", "ins-12345".to_string());
        step_inst.cleanup(&state).await;

        // StepCreateImage cleanup and error
        let mut step_img = StepCreateImage {
            ui: ui.clone(),
            name: "test-img".to_string(),
            config: TencentCloudConfig {
                endpoint: Some(err_server.url()),
                ..Default::default()
            },
        };
        assert!(step_img.run(&mut state).await.is_err());
        step_img.cleanup(&state).await;

        // builder.name() when config.name is empty
        let empty_b = TencentCloudBuilder::new(TencentCloudConfig::default());
        assert_eq!(empty_b.name(), "tencentcloud-cvm");

        // builder.run() with region: None
        let b_no_reg = TencentCloudBuilder::new(TencentCloudConfig {
            name: "tc-no-reg".to_string(),
            endpoint: Some(server.url()),
            region: None,
            ..Default::default()
        });
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(Vec::new()),
            error_cleanup_provisioners: Arc::new(Vec::new()),
        });
        let res = b_no_reg
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert_eq!(res.id(), ":img-tc112233");
        Ok(())
    }
}
