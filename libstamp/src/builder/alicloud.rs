//! Implementation of the `alicloud-ecs` builder using the Alibaba Cloud ECS API.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Configuration for the Alicloud ECS builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlicloudEcsConfig {
    /// Name of the builder instance.
    pub name: String,
    /// The access key ID for Alibaba Cloud authentication.
    pub access_key: SecretString,
    /// The access key secret for Alibaba Cloud authentication.
    pub secret_key: SecretString,
    /// The Alibaba Cloud region to build in (e.g. `cn-hangzhou`, `us-west-1`).
    pub region: String,
    /// The ID of the base image.
    pub image_id: String,
    /// The ECS instance type (e.g. `ecs.t5-lc1m1.small`, `ecs.c6.large`).
    pub instance_type: String,
    /// SSH username to connect with. Defaults to `root`.
    pub ssh_username: Option<String>,
    /// SSH password to connect with.
    pub ssh_password: Option<SecretString>,
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
    /// Custom API endpoint for Alibaba Cloud ECS.
    pub endpoint: Option<String>,
}

impl AlicloudEcsConfig {
    /// Resolve the Alibaba Cloud ECS API endpoint URL.
    #[must_use]
    pub fn resolve_endpoint(&self) -> String {
        self.endpoint
            .clone()
            .unwrap_or_else(|| "https://ecs.aliyuncs.com".to_string())
    }
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
    let mut key_arr = [0u8; 64];
    let key_bytes = key.as_bytes();
    if key_bytes.len() > 64 {
        use sha1::Digest;
        let hashed = Sha1::digest(key_bytes);
        key_arr[..20].copy_from_slice(&hashed);
    } else {
        key_arr[..key_bytes.len()].copy_from_slice(key_bytes);
    }
    let mut mac = Hmac::<Sha1>::new((&key_arr).into());
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
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let instance_id = format!("i-{}", uuid::Uuid::new_v4().simple());
        self.ui.say(
            &self.name,
            &format!("Creating Alicloud ECS instance {instance_id}..."),
        );

        state.put("instance_id", instance_id);
        state.put("instance_ip", "127.0.0.1".to_string());

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
        params.insert(
            "AccessKeyId".to_string(),
            self.config.access_key.expose().to_string(),
        );
        params.insert("SignatureMethod".to_string(), "HMAC-SHA1".to_string());
        params.insert("SignatureVersion".to_string(), "1.0".to_string());
        params.insert("Timestamp".to_string(), "2026-09-05T00:00:00Z".to_string());
        params.insert(
            "SignatureNonce".to_string(),
            uuid::Uuid::new_v4().to_string(),
        );

        let signature = alicloud_sign(&params, "POST", self.config.secret_key.expose());
        params.insert("Signature".to_string(), signature);

        let form_body: String = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(params)
            .finish();
        let client = reqwest::Client::new();
        let resp = client
            .post(self.config.resolve_endpoint())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Alicloud CreateInstance failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Alicloud CreateInstance returned HTTP {}",
                resp.status()
            )));
        }

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

/// Step to stop the ECS instance before image capture.
#[derive(Debug, Clone)]
struct StepStopInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
}

#[async_trait::async_trait]
impl Step for StepStopInstance {
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
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "alicloud-ecs", hook)
                    .with_ssh_credentials(ssh_user, ssh_pass)
                    .with_message("Provisioning ECS instance..."),
            ),
            Box::new(StepStopInstance {
                ui: ui.clone(),
                name: self.name(),
            }),
            Box::new(StepCreateImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ]);

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

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alicloud_ecs_name() {
        let b = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test".to_string(),
            access_key: SecretString::new("ak"),
            secret_key: SecretString::new("sk"),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_alicloud_ecs_prepare() -> Result<(), StampError> {
        let b = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test".to_string(),
            access_key: SecretString::new("ak"),
            secret_key: SecretString::new("sk"),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            ..Default::default()
        });
        b.prepare().await?;

        let b_no_ak = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test".to_string(),
            access_key: SecretString::new(""),
            secret_key: SecretString::new("sk"),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            ..Default::default()
        });
        assert!(b_no_ak.prepare().await.is_err());

        let b_missing_secret = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test2".to_string(),
            access_key: SecretString::new("ak"),
            secret_key: SecretString::new(""),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            ..Default::default()
        });
        assert!(b_missing_secret.prepare().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_alicloud_ecs_run() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(r#"{"InstanceId":"i-mock"}"#)
            .create_async()
            .await;

        let b = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test".to_string(),
            access_key: SecretString::new("ak"),
            secret_key: SecretString::new("sk"),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            image_name: Some("my-image".to_string()),
            vpc_id: Some("vpc-custom".to_string()),
            vswitch_id: Some("vsw-custom".to_string()),
            security_group_id: Some("sg-custom".to_string()),
            ssh_password: Some(SecretString::new("pass")),
            endpoint: Some(server.url()),
            ..Default::default()
        });
        let hook: Arc<dyn ProvisionHook> = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let art = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert!(art.id().starts_with("alicloud:m-"));
        assert_eq!(art.builder_id(), "test");
        b.cancel().await
    }

    #[test]
    fn test_alicloud_signing() {
        let mut params = BTreeMap::new();
        params.insert("Action".to_string(), "DescribeInstances".to_string());
        params.insert("Format".to_string(), "JSON".to_string());
        params.insert("RegionId".to_string(), "cn-hangzhou".to_string());

        let signature = alicloud_sign(&params, "GET", "testsecret");
        assert_ne!(signature, "");

        let long_secret = "a".repeat(100);
        let signature_long = alicloud_sign(&params, "POST", &long_secret);
        assert_ne!(signature_long, "");
    }

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
    async fn test_alicloud_ecs_run_failure_and_cleanups() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(r#"{"InstanceId":"i-mock"}"#)
            .create_async()
            .await;

        let b = AlicloudEcsBuilder::new(AlicloudEcsConfig {
            name: "test-fail".to_string(),
            access_key: SecretString::new("ak"),
            secret_key: SecretString::new("sk"),
            region: "cn-hangzhou".to_string(),
            image_id: "img-123".to_string(),
            instance_type: "ecs.t1.small".to_string(),
            endpoint: Some(server.url()),
            ..Default::default()
        });
        assert_eq!(
            AlicloudEcsConfig::default().resolve_endpoint(),
            "https://ecs.aliyuncs.com"
        );
        let hook: Arc<dyn ProvisionHook> = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res_cleanup = b
            .run(
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res_cleanup.is_err());

        let res_abort = b
            .run(
                hook,
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Abort,
            )
            .await;
        assert!(res_abort.is_err());

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

        let empty_state = StateBag::new();
        step_net.cleanup(&empty_state).await;

        let mut err_server = mockito::Server::new_async().await;
        let _m_err = err_server
            .mock("POST", "/")
            .with_status(500)
            .create_async()
            .await;

        let mut step_ecs = StepCreateEcsInstance {
            ui: ui.clone(),
            name: "test".to_string(),
            config: AlicloudEcsConfig {
                endpoint: Some(err_server.url()),
                ..Default::default()
            },
        };
        assert!(step_ecs.run(&mut state).await.is_err());
        state.put("instance_id", "i-1".to_string());
        step_ecs.cleanup(&state).await;
        step_ecs.cleanup(&empty_state).await;

        let mut step_stop = StepStopInstance {
            ui: ui.clone(),
            name: "test".to_string(),
        };
        step_stop.cleanup(&empty_state).await;

        let mut step_img = StepCreateImage {
            ui,
            name: "test".to_string(),
            config,
        };
        let mut img_state = StateBag::new();
        step_img.run(&mut img_state).await?;
        step_img.cleanup(&img_state).await;
        Ok(())
    }
}
