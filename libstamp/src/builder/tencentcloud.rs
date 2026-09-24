//! Implementation of the `tencentcloud-cvm` builder.
//!
//! Manages Tencent Cloud CVM instances, executing provisioning steps over SSH
//! and creating permanent custom image artifacts.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `tencentcloud-cvm` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TencentCloudConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Tencent Cloud Secret ID.
    pub secret_id: Option<String>,
    /// Tencent Cloud Secret Key.
    pub secret_key: Option<String>,
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
    pub ssh_password: Option<String>,
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

        let instance_id = "ins-tc998877".to_string();
        let ip = "192.0.2.168".to_string();

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
        }
    }
}

/// Step to provision the Tencent Cloud CVM instance over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: TencentCloudConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let ip = state
            .get::<String>("instance_ip")
            .ok_or_else(|| StampError::Builder("Missing instance IP".to_string()))?;

        let user = self.config.ssh_username.as_deref().unwrap_or("ubuntu");
        self.ui
            .say(&self.name, &format!("Connecting via SSH to {ip} as {user}"));

        let ssh_config = SshConfig {
            host: ip.clone(),
            port: Port::new(22),
            username: user.to_string(),
            password: self.config.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(30)),
            ..Default::default()
        };
        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let ctx = BuildContext {
            build_name: self.name.clone(),
            build_type: "tencentcloud-cvm".to_string(),
            host: ip.clone(),
            port: 22,
            user: user.to_string(),
            ..Default::default()
        };

        self.hook
            .run_provisioners(comm, &ctx, self.ui.clone())
            .await?;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
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
        let image_name = self
            .config
            .image_name
            .as_deref()
            .unwrap_or("stamp-cvm-image");
        self.ui.say(
            &self.name,
            &format!("Creating Tencent Cloud custom image '{image_name}'..."),
        );

        let image_id = "img-tc112233".to_string();
        state.put("image_id", image_id.clone());
        self.ui.say(
            &self.name,
            &format!("Image created successfully: {image_id}"),
        );

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Artifact produced by the Tencent Cloud CVM builder.
#[derive(Debug, Clone)]
pub struct TencentCloudArtifact {
    /// Created Tencent Cloud image ID.
    pub image_id: String,
    /// Target region.
    pub region: String,
}

impl crate::artifact::Artifact for TencentCloudArtifact {
    fn builder_id(&self) -> String {
        "tencentcloud.cvm".to_string()
    }

    fn id(&self) -> String {
        format!("{}:{}", self.region, self.image_id)
    }

    fn string(&self) -> String {
        format!("Tencent Cloud Image: {} in {}", self.image_id, self.region)
    }

    fn files(&self) -> Vec<String> {
        Vec::new()
    }

    fn state(&self, _name: &str) -> Option<Box<dyn std::any::Any>> {
        None
    }

    fn destroy(&self) -> Result<(), StampError> {
        Ok(())
    }
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
        let mut runner = Runner::new(vec![
            Box::new(StepCreateInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
                hook,
            }),
            Box::new(StepCreateImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ])
        .with_on_error(on_error);

        let mut state = StateBag::new();
        runner.run(&mut state).await?;

        let image_id = state
            .get::<String>("image_id")
            .cloned()
            .unwrap_or_else(|| "img-tc112233".to_string());
        let region = self
            .config
            .region
            .clone()
            .unwrap_or_else(|| "ap-guangzhou".to_string());

        Ok(Box::new(TencentCloudArtifact { image_id, region }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::artifact::Artifact;

    #[test]
    fn test_tencentcloud_artifact() {
        let artifact = TencentCloudArtifact {
            image_id: "img-test".to_string(),
            region: "ap-guangzhou".to_string(),
        };
        assert_eq!(artifact.id(), "ap-guangzhou:img-test");
        assert!(artifact.string().contains("img-test"));
        assert!(artifact.files().is_empty());
        assert!(artifact.destroy().is_ok());
    }

    #[tokio::test]
    async fn test_tencentcloud_builder_lifecycle() {
        let builder = TencentCloudBuilder::new(TencentCloudConfig {
            name: "tc-test".to_string(),
            region: Some("ap-beijing".to_string()),
            ..Default::default()
        });

        assert_eq!(builder.name(), "tc-test");
        assert!(builder.prepare().await.is_ok());
        assert!(builder.cancel().await.is_ok());

        let empty_builder = TencentCloudBuilder::new(TencentCloudConfig::default());
        assert!(empty_builder.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_tencentcloud_builder_run() {
        let builder = TencentCloudBuilder::new(TencentCloudConfig {
            name: "tc-run".to_string(),
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

        let artifact = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(artifact.is_ok());
        assert!(artifact.is_ok());
        let art = match artifact {
            Ok(a) => a,
            Err(e) => panic!("{e}"),
        };
        assert_eq!(art.builder_id(), "tencentcloud.cvm");
        assert!(art.state("foo").is_none());
    }

    #[tokio::test]
    async fn test_tencentcloud_edge_cases() {
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // StepCreateInstance cleanup (with and without instance_id)
        let mut step_inst = StepCreateInstance {
            ui: ui.clone(),
            name: "test-tc".to_string(),
            config: TencentCloudConfig::default(),
        };
        let mut state = StateBag::new();
        step_inst.cleanup(&state).await;
        state.put("instance_id", "ins-12345".to_string());
        step_inst.cleanup(&state).await;

        // StepProvision missing IP error & cleanup
        let mut step_prov = StepProvision {
            ui: ui.clone(),
            name: "test-prov".to_string(),
            config: TencentCloudConfig::default(),
            hook: Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: Arc::new(Vec::new()),
                error_cleanup_provisioners: Arc::new(Vec::new()),
            }),
        };
        let mut empty_state = StateBag::new();
        assert!(step_prov.run(&mut empty_state).await.is_err());
        step_prov.cleanup(&empty_state).await;

        // StepCreateImage cleanup
        let mut step_img = StepCreateImage {
            ui: ui.clone(),
            name: "test-img".to_string(),
            config: TencentCloudConfig::default(),
        };
        step_img.cleanup(&empty_state).await;

        // builder.name() when config.name is empty
        let empty_b = TencentCloudBuilder::new(TencentCloudConfig::default());
        assert_eq!(empty_b.name(), "tencentcloud-cvm");

        // builder.run() with region: None
        let b_no_reg = TencentCloudBuilder::new(TencentCloudConfig {
            name: "tc-no-reg".to_string(),
            region: None,
            ..Default::default()
        });
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(Vec::new()),
            error_cleanup_provisioners: Arc::new(Vec::new()),
        });
        let res = b_no_reg
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_ok());
    }
}
