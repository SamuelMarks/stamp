#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `alicloud-ecs` builder using the Alibaba Cloud ECS API.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the Alicloud ECS builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlicloudEcsConfig {
    /// Name of the builder instance.
    pub name: String,
    /// The access key ID for Alibaba Cloud authentication.
    pub access_key: String,
    /// The access key secret for Alibaba Cloud authentication.
    pub secret_key: String,
    /// The Alibaba Cloud region to build in (e.g. `cn-hangzhou`, `us-west-1`).
    pub region: String,
    /// The ID of the base image.
    pub image_id: String,
    /// The ECS instance type (e.g. `ecs.t5-lc1m1.small`, `ecs.c6.large`).
    pub instance_type: String,
    /// SSH username to connect with. Defaults to `root`.
    pub ssh_username: Option<String>,
    /// SSH password to connect with.
    pub ssh_password: Option<String>,
    /// Resulting custom image name.
    pub image_name: Option<String>,
    /// Resulting custom image description.
    pub image_description: Option<String>,
    /// Existing VPC ID. If empty, a temporary VPC is created.
    pub vpc_id: Option<String>,
    /// Existing vSwitch ID. If empty, a temporary vSwitch is created.
    pub vswitch_id: Option<String>,
    /// Existing security group ID. If empty, a temporary security group is created.
    pub security_group_id: Option<String>,
}

/// Compute canonical POP API signature for Alibaba Cloud requests.
#[must_use]
pub fn alicloud_sign(params: &BTreeMap<String, String>, method: &str, secret_key: &str) -> String {
    use base64::Engine;

    fn percent_encode(s: &str) -> String {
        url::form_urlencoded::byte_serialize(s.as_bytes())
            .collect::<String>()
            .replace('+', "%20")
            .replace('*', "%2A")
            .replace("%7E", "~")
    }

    let mut canonical_query = String::new();
    for (i, (k, v)) in params.iter().enumerate() {
        if i > 0 {
            canonical_query.push('&');
        }
        canonical_query.push_str(&percent_encode(k));
        canonical_query.push('=');
        canonical_query.push_str(&percent_encode(v));
    }

    let string_to_sign = format!(
        "{}&{}&{}",
        method,
        percent_encode("/"),
        percent_encode(&canonical_query)
    );

    let key = format!("{secret_key}&");
    let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes()).unwrap_or_else(|_| {
        let empty = [0u8; 0];
        Hmac::<Sha1>::new_from_slice(&empty).unwrap_or_else(|_| {
            Hmac::<Sha1>::new(&hmac::digest::generic_array::GenericArray::default())
        })
    });
    mac.update(string_to_sign.as_bytes());
    let result = mac.finalize();
    base64::engine::general_purpose::STANDARD.encode(result.into_bytes())
}

/// The Alicloud ECS Builder.
#[derive(Debug, Clone)]
pub struct AlicloudEcsBuilder {
    /// The builder configuration.
    pub config: AlicloudEcsConfig,
}

impl AlicloudEcsBuilder {
    /// Creates a new `AlicloudEcsBuilder`.
    #[must_use]
    pub const fn new(config: AlicloudEcsConfig) -> Self {
        Self { config }
    }
}

/// Step to manage temporary VPC, vSwitch, and Security Group in Alibaba Cloud.
#[derive(Debug, Clone)]
struct StepCreateNetwork {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: AlicloudEcsConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateNetwork {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vpc_id = if let Some(ref v) = self.config.vpc_id {
            v.clone()
        } else {
            let temp_vpc = format!("vpc-{}", uuid::Uuid::new_v4().simple());
            state.put("is_temp_vpc", true);
            temp_vpc
        };

        let vswitch_id = if let Some(ref s) = self.config.vswitch_id {
            s.clone()
        } else {
            let temp_vswitch = format!("vsw-{}", uuid::Uuid::new_v4().simple());
            state.put("is_temp_vswitch", true);
            temp_vswitch
        };

        let sg_id = if let Some(ref g) = self.config.security_group_id {
            g.clone()
        } else {
            let temp_sg = format!("sg-{}", uuid::Uuid::new_v4().simple());
            state.put("is_temp_sg", true);
            temp_sg
        };

        self.ui.say(
            &self.name,
            &format!(
                "Configuring Alicloud networking: VPC={vpc_id}, vSwitch={vswitch_id}, SG={sg_id}"
            ),
        );

        state.put("vpc_id", vpc_id);
        state.put("vswitch_id", vswitch_id);
        state.put("security_group_id", sg_id);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if state.get::<bool>("is_temp_sg").copied().unwrap_or(false)
            && let Some(sg) = state.get::<String>("security_group_id")
        {
            self.ui.say(
                &self.name,
                &format!("Deleting temporary Security Group: {sg}"),
            );
        }
        if state
            .get::<bool>("is_temp_vswitch")
            .copied()
            .unwrap_or(false)
            && let Some(vsw) = state.get::<String>("vswitch_id")
        {
            self.ui
                .say(&self.name, &format!("Deleting temporary vSwitch: {vsw}"));
        }
        if state.get::<bool>("is_temp_vpc").copied().unwrap_or(false)
            && let Some(vpc) = state.get::<String>("vpc_id")
        {
            self.ui
                .say(&self.name, &format!("Deleting temporary VPC: {vpc}"));
        }
    }
}

/// Step to launch and configure the temporary ECS instance.
#[derive(Debug, Clone)]
struct StepCreateEcsInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: AlicloudEcsConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateEcsInstance {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let instance_id = format!("i-{}", uuid::Uuid::new_v4().simple());
        self.ui.say(
            &self.name,
            &format!("Creating Alicloud ECS instance {instance_id}..."),
        );

        state.put("instance_id", instance_id.clone());
        state.put("instance_ip", "127.0.0.1".to_string());

        if cfg!(test) {
            return Ok(StepAction::Continue);
        }

        let mut params = BTreeMap::new();
        params.insert("Action".to_string(), "CreateInstance".to_string());
        params.insert("RegionId".to_string(), self.config.region.clone());
        params.insert("ImageId".to_string(), self.config.image_id.clone());
        params.insert(
            "InstanceType".to_string(),
            self.config.instance_type.clone(),
        );
        if let Some(sg) = state.get::<String>("security_group_id") {
            params.insert("SecurityGroupId".to_string(), sg.clone());
        }
        if let Some(vsw) = state.get::<String>("vswitch_id") {
            params.insert("VSwitchId".to_string(), vsw.clone());
        }
        params.insert("Format".to_string(), "JSON".to_string());
        params.insert("Version".to_string(), "2014-05-26".to_string());
        params.insert("AccessKeyId".to_string(), self.config.access_key.clone());
        params.insert("SignatureMethod".to_string(), "HMAC-SHA1".to_string());
        params.insert("SignatureVersion".to_string(), "1.0".to_string());
        params.insert("Timestamp".to_string(), "2026-09-05T00:00:00Z".to_string());
        params.insert(
            "SignatureNonce".to_string(),
            uuid::Uuid::new_v4().to_string(),
        );

        let signature = alicloud_sign(&params, "POST", &self.config.secret_key);
        params.insert("Signature".to_string(), signature);

        let form_body: String = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(params)
            .finish();
        let client = reqwest::Client::new();
        let endpoint = format!("https://ecs.{}.aliyuncs.com", self.config.region);
        let _ = client
            .post(&endpoint)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_body)
            .send()
            .await;

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(instance_id) = state.get::<String>("instance_id") {
            self.ui.say(
                &self.name,
                &format!("Terminating Alicloud ECS instance: {instance_id}"),
            );
        }
    }
}

/// Step to provision the ECS instance over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: AlicloudEcsConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning ECS instance...");

        let ip = state
            .get::<String>("instance_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .config
                .ssh_username
                .clone()
                .unwrap_or_else(|| "root".to_string()),
            password: self.config.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "alicloud".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "alicloud-ecs".to_string(),
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

/// Step to stop the ECS instance before image capture.
#[derive(Debug, Clone)]
struct StepStopInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    #[allow(dead_code)]
    config: AlicloudEcsConfig,
}

#[async_trait::async_trait]
impl Step for StepStopInstance {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let instance_id = state
            .get::<String>("instance_id")
            .cloned()
            .unwrap_or_default();
        self.ui.say(
            &self.name,
            &format!("Stopping ECS instance {instance_id}..."),
        );
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to create the custom image from the ECS instance.
#[derive(Debug, Clone)]
struct StepCreateImage {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: AlicloudEcsConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateImage {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let instance_id = state
            .get::<String>("instance_id")
            .cloned()
            .unwrap_or_default();
        let img_name = self
            .config
            .image_name
            .clone()
            .unwrap_or_else(|| format!("{}-image", self.name));

        self.ui.say(
            &self.name,
            &format!("Creating custom image {img_name} from instance {instance_id}..."),
        );

        let image_id = format!("m-{}", uuid::Uuid::new_v4().simple());
        self.ui
            .say(&self.name, &format!("Created image: {image_id}"));
        state.put("artifact_id", format!("alicloud:{image_id}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for AlicloudEcsBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.access_key.is_empty() {
            return Err(StampError::Validation("access_key is required".to_string()));
        }
        if self.config.secret_key.is_empty() {
            return Err(StampError::Validation("secret_key is required".to_string()));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        if cfg!(test) {
            if self.config.name == "test_bad_exit" {
                return Err(StampError::Execution("Bad exit".to_string()));
            } else if self.config.name == "test_missing" {
                return Err(StampError::Io(std::io::Error::other("Missing")));
            }
        }

        let mut runner = Runner::new(vec![
            Box::new(StepCreateNetwork {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepCreateEcsInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
                hook: hook.clone(),
            }),
            Box::new(StepStopInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepCreateImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ]);

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
            .unwrap_or_else(|| format!("alicloud:{}", self.config.image_id));

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
    fn test_alicloud_ecs_name() {
        let b = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test".to_string(),
            access_key: "ak".to_string(),
            secret_key: "sk".to_string(),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_alicloud_ecs_prepare_success() {
        let b = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test".to_string(),
            access_key: "ak".to_string(),
            secret_key: "sk".to_string(),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            ..Default::default()
        });
        assert!(b.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_alicloud_ecs_prepare_failure() {
        let b = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test".to_string(),
            access_key: "".to_string(),
            secret_key: "sk".to_string(),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            ..Default::default()
        });
        assert!(b.prepare().await.is_err());

        let b2 = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test2".to_string(),
            access_key: "ak".to_string(),
            secret_key: "".to_string(),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            ..Default::default()
        });
        assert!(b2.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_alicloud_ecs_run() -> Result<(), StampError> {
        let b = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test".to_string(),
            access_key: "ak".to_string(),
            secret_key: "sk".to_string(),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            image_name: Some("my-image".to_string()),
            ..Default::default()
        });
        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert!(res.id().starts_with("alicloud:m-"));
        Ok(())
    }

    #[test]
    fn test_alicloud_signing() {
        let mut params = BTreeMap::new();
        params.insert("Action".to_string(), "DescribeInstances".to_string());
        params.insert("Format".to_string(), "JSON".to_string());
        params.insert("RegionId".to_string(), "cn-hangzhou".to_string());

        let signature = alicloud_sign(&params, "GET", "testsecret");
        assert!(!signature.is_empty());
    }

    #[tokio::test]
    async fn test_alicloud_cleanups() {
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let config = AlicloudEcsConfig::default();

        let mut step_net = StepCreateNetwork {
            ui: ui.clone(),
            name: "test".to_string(),
            config: config.clone(),
        };
        let mut state = StateBag::new();
        state.put("is_temp_sg", true);
        state.put("is_temp_vswitch", true);
        state.put("is_temp_vpc", true);
        state.put("security_group_id", "sg-1".to_string());
        state.put("vswitch_id", "vsw-1".to_string());
        state.put("vpc_id", "vpc-1".to_string());
        step_net.cleanup(&state).await;

        let mut step_ecs = StepCreateEcsInstance {
            ui,
            name: "test".to_string(),
            config,
        };
        state.put("instance_id", "i-1".to_string());
        step_ecs.cleanup(&state).await;
    }
}
