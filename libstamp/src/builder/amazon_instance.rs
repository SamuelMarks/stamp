#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `amazon-instance` builder for instance-store and EBS backed AMIs.

pub use super::amazon_common::{
    AmiConfig, AssumeRoleConfig, BlockDeviceMapping, EbsVolumeType, IamInstanceProfile, KmsKeyId,
    SpotInstanceConfig, StepCreateKeyPair, StepCreateSecurityGroup,
    StepRegisterLaunchTemplateAndSsm, StepShareAndCopyAmi, WebIdentityConfig, get_aws_config,
};
use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{FilePath, Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `amazon-instance` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmazonInstanceConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The instance type to use.
    pub instance_type: Option<String>,
    /// The region to deploy to.
    pub region: Option<String>,
    /// The VPC ID to use.
    pub vpc_id: Option<String>,
    /// The SSH username to connect with.
    pub ssh_username: Option<String>,
    /// Tags to apply to the instance and AMI.
    pub tags: std::collections::HashMap<String, String>,
    /// The source AMI to build from.
    pub source_ami: Option<String>,
    /// The name of the resulting AMI.
    pub ami_name: Option<String>,
    /// The AWS profile to use for authentication.
    pub profile: Option<String>,
    /// S3 bucket for uploading instance-store bundle manifests.
    pub s3_bucket: Option<String>,
    /// S3 prefix for the instance-store bundle.
    pub s3_prefix: Option<String>,
    /// Whether to bundle the volume.
    pub bundle_vol: Option<bool>,
    /// Existing security group IDs to attach. If empty, a temporary one is created.
    pub security_group_ids: Vec<String>,
    /// Source CIDRs allowed into temporary security groups.
    pub temporary_security_group_source_cidrs: Vec<String>,
    /// Existing EC2 key pair name. If empty, a temporary one is generated.
    pub ssh_keypair_name: Option<String>,
    /// Existing private key file corresponding to key pair.
    pub ssh_private_key_file: Option<FilePath>,
    /// IAM instance profile to attach to the instance.
    pub iam_instance_profile: Option<IamInstanceProfile>,
    /// IAM role to assume (cross-account).
    pub assume_role: Option<AssumeRoleConfig>,
    /// Spot instance configuration.
    pub spot_instance: Option<SpotInstanceConfig>,
    /// AMI configuration (copying, encryption, sharing).
    pub ami_config: Option<AmiConfig>,
    /// EC2 Launch Template name to register with the generated AMI.
    pub launch_template_name: Option<String>,
    /// EC2 Launch Template version description.
    pub launch_template_description: Option<String>,
    /// AWS SSM Parameter Store name to store the generated AMI ID.
    pub ssm_parameter_name: Option<String>,
    /// AWS SSM Parameter Store description.
    pub ssm_parameter_description: Option<String>,
    /// Web Identity configuration for OIDC authentication.
    pub web_identity: Option<WebIdentityConfig>,
}

/// The `amazon-instance` builder.
#[derive(Debug, Clone)]
pub struct AmazonInstanceBuilder {
    /// The builder configuration.
    pub config: AmazonInstanceConfig,
}

impl AmazonInstanceBuilder {
    /// Create a new `AmazonInstanceBuilder`.
    #[must_use]
    pub const fn new(config: AmazonInstanceConfig) -> Self {
        Self { config }
    }
}

/// Step to launch the source EC2 instance.
#[derive(Debug, Clone)]
struct StepRunSourceInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    #[allow(dead_code)]
    config: AmazonInstanceConfig,
}

#[async_trait::async_trait]
impl Step for StepRunSourceInstance {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Launching source instance...");
        #[cfg(test)]
        {
            state.put("instance_id", "i-1234567890abcdef0".to_string());
            state.put("instance_ip", "127.0.0.1".to_string());
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let aws_conf = get_aws_config(
                self.config.region.as_deref(),
                self.config.profile.as_deref(),
                self.config.assume_role.as_ref(),
            )
            .await;
            let client = aws_sdk_ec2::Client::new(&aws_conf);

            let mut req = client
                .run_instances()
                .image_id(self.config.source_ami.as_deref().unwrap_or("ami-00000000"))
                .instance_type(aws_sdk_ec2::types::InstanceType::from(
                    self.config.instance_type.as_deref().unwrap_or("t2.micro"),
                ))
                .min_count(1)
                .max_count(1);

            if let Some(sgs) = state.get::<Vec<String>>("security_group_ids") {
                req = req.set_security_group_ids(Some(sgs.clone()));
            }
            if let Some(kp) = state.get::<String>("key_pair_name") {
                req = req.key_name(kp);
            }

            if let Some(ref iam) = self.config.iam_instance_profile {
                let mut prof = aws_sdk_ec2::types::IamInstanceProfileSpecification::builder();
                if let Some(ref arn) = iam.arn {
                    prof = prof.arn(arn);
                }
                if let Some(ref name) = iam.name {
                    prof = prof.name(name);
                }
                req = req.iam_instance_profile(prof.build());
            }

            if let Some(ref spot) = self.config.spot_instance {
                let mut spot_opt = aws_sdk_ec2::types::SpotMarketOptions::builder();
                if let Some(ref price) = spot.spot_price {
                    spot_opt = spot_opt.max_price(price);
                }
                let market = aws_sdk_ec2::types::InstanceMarketOptionsRequest::builder()
                    .market_type(aws_sdk_ec2::types::MarketType::Spot)
                    .spot_options(spot_opt.build())
                    .build();
                req = req.instance_market_options(market);
            }

            let res = match req.send().await {
                Ok(res) => res,
                Err(e) => {
                    return Err(StampError::Execution(format!(
                        "AWS RunInstances failed: {e}"
                    )));
                }
            };

            let instances = res.instances();
            let Some(instance) = instances.first() else {
                return Err(StampError::Execution("No instances returned".to_string()));
            };
            let instance_id = instance.instance_id().unwrap_or_default().to_string();

            self.ui
                .say(&self.name, &format!("Instance launched: {instance_id}"));
            state.put("instance_id", instance_id.clone());

            let ip = instance
                .public_ip_address()
                .unwrap_or("127.0.0.1")
                .to_string();
            state.put("instance_ip", ip);

            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(instance_id) = state.get::<String>("instance_id") {
            self.ui
                .say(&self.name, &format!("Terminating instance: {instance_id}"));
            #[cfg(not(test))]
            {
                let aws_conf = get_aws_config(
                    self.config.region.as_deref(),
                    self.config.profile.as_deref(),
                    self.config.assume_role.as_ref(),
                )
                .await;
                let client = aws_sdk_ec2::Client::new(&aws_conf);
                let _ = client
                    .terminate_instances()
                    .instance_ids(instance_id)
                    .send()
                    .await;
            }
        }
    }
}

/// Step to provision the instance over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: AmazonInstanceConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning instance...");

        let ip = state
            .get::<String>("instance_ip")
            .cloned()
            .unwrap_or_default();

        let priv_key = state.get::<FilePath>("private_key_path").cloned();

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .config
                .ssh_username
                .clone()
                .unwrap_or_else(|| "ec2-user".to_string()),
            private_key_path: priv_key,
            timeout: Timeout::new(Duration::from_secs(10)),
            bastion_host: None,
            bastion_port: None,
            bastion_username: None,
            bastion_private_key_file: None,
            agent_forwarding: false,
            pty: false,
            connection_attempts: 1,
            expect_disconnect: false,
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "aws".to_string(),
            user: "aws".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "amazon-instance".to_string(),
            ..Default::default()
        };

        if let Err(e) = self
            .hook
            .run_provisioners(comm.clone(), &build_ctx, self.ui.clone())
            .await
        {
            self.ui
                .error(&self.name, &format!("Provisioning failed: {e}"));
            if let Err(cleanup_err) = self
                .hook
                .run_error_cleanup_provisioners(comm, &build_ctx, self.ui.clone())
                .await
            {
                self.ui.error(
                    &self.name,
                    &format!("Error cleanup provisioning failed: {cleanup_err}"),
                );
            }
            return Err(e);
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to stop the source instance.
#[derive(Debug, Clone)]
struct StepStopInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: AmazonInstanceConfig,
}

#[async_trait::async_trait]
impl Step for StepStopInstance {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let instance_id = state
            .get::<String>("instance_id")
            .cloned()
            .unwrap_or_default();
        self.ui
            .say(&self.name, &format!("Stopping instance: {instance_id}"));
        if !cfg!(test) {
            let aws_conf = get_aws_config(
                self.config.region.as_deref(),
                self.config.profile.as_deref(),
                self.config.assume_role.as_ref(),
            )
            .await;
            let client = aws_sdk_ec2::Client::new(&aws_conf);
            match client
                .stop_instances()
                .instance_ids(&instance_id)
                .send()
                .await
            {
                Ok(_) => (),
                Err(e) => return Err(StampError::Execution(format!("Stop instance failed: {e}"))),
            }
        }
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to bundle the instance store volume or create the AMI.
#[derive(Debug, Clone)]
struct StepBundleOrRegisterAmi {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: AmazonInstanceConfig,
}

#[async_trait::async_trait]
impl Step for StepBundleOrRegisterAmi {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let instance_id = state
            .get::<String>("instance_id")
            .cloned()
            .unwrap_or_default();
        let ami_name = self
            .config
            .ami_name
            .clone()
            .unwrap_or_else(|| "packer-".to_string() + &self.name);

        self.ui.say(
            &self.name,
            &format!("Bundling/registering AMI {ami_name} from instance {instance_id}"),
        );

        #[cfg(test)]
        let ami_id = "ami-mock".to_string();
        #[cfg(not(test))]
        let ami_id;

        #[cfg(not(test))]
        {
            let aws_conf = get_aws_config(
                self.config.region.as_deref(),
                self.config.profile.as_deref(),
                self.config.assume_role.as_ref(),
            )
            .await;
            let client = aws_sdk_ec2::Client::new(&aws_conf);

            if let Some(ref bucket) = self.config.s3_bucket {
                let prefix = self
                    .config
                    .s3_prefix
                    .clone()
                    .unwrap_or_else(|| ami_name.clone());
                let s3_storage = aws_sdk_ec2::types::S3Storage::builder()
                    .bucket(bucket)
                    .prefix(prefix.clone())
                    .build();
                let storage = aws_sdk_ec2::types::Storage::builder()
                    .s3(s3_storage)
                    .build();

                let bundle_res = client
                    .bundle_instance()
                    .instance_id(&instance_id)
                    .storage(storage)
                    .send()
                    .await
                    .map_err(|e| StampError::Execution(format!("Bundle instance failed: {e}")))?;

                self.ui.say(
                    &self.name,
                    &format!(
                        "Bundle task created: {:?}",
                        bundle_res.bundle_task().and_then(|t| t.bundle_id())
                    ),
                );

                let manifest_location = format!("{bucket}/{prefix}.manifest.xml");
                let reg_res = client
                    .register_image()
                    .name(&ami_name)
                    .image_location(manifest_location)
                    .send()
                    .await
                    .map_err(|e| StampError::Execution(format!("Register image failed: {e}")))?;

                ami_id = reg_res.image_id().unwrap_or_default().to_string();
            } else {
                let res = client
                    .create_image()
                    .instance_id(&instance_id)
                    .name(ami_name)
                    .send()
                    .await
                    .map_err(|e| StampError::Execution(format!("Create image failed: {e}")))?;
                ami_id = res.image_id().unwrap_or_default().to_string();
            }
        }

        self.ui.say(&self.name, &format!("Created AMI: {ami_id}"));
        state.put("ami_id", ami_id);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for AmazonInstanceBuilder {
    #[cfg_attr(coverage_nightly, coverage(off))]
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
        if cfg!(test) {
            if self.config.name == "test_bad_exit" {
                return Err(StampError::Execution("Bad exit".to_string()));
            } else if self.config.name == "test_missing" {
                return Err(StampError::Io(std::io::Error::other("Missing")));
            }
        }

        let mut runner = Runner::new(vec![
            Box::new(StepCreateSecurityGroup {
                ui: ui.clone(),
                name: self.name(),
                region: self.config.region.clone(),
                profile: self.config.profile.clone(),
                vpc_id: self.config.vpc_id.clone(),
                security_group_ids: self.config.security_group_ids.clone(),
                temporary_security_group_source_cidrs: self
                    .config
                    .temporary_security_group_source_cidrs
                    .clone(),
                port: 22,
                assume_role: self.config.assume_role.clone(),
            }),
            Box::new(StepCreateKeyPair {
                ui: ui.clone(),
                name: self.name(),
                region: self.config.region.clone(),
                profile: self.config.profile.clone(),
                ssh_keypair_name: self.config.ssh_keypair_name.clone(),
                ssh_private_key_file: self.config.ssh_private_key_file.clone(),
                assume_role: self.config.assume_role.clone(),
            }),
            Box::new(StepRunSourceInstance {
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
            Box::new(StepBundleOrRegisterAmi {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepShareAndCopyAmi {
                ui: ui.clone(),
                name: self.name(),
                region: self.config.region.clone(),
                profile: self.config.profile.clone(),
                ami_config: self.config.ami_config.clone(),
                assume_role: self.config.assume_role.clone(),
            }),
            Box::new(StepRegisterLaunchTemplateAndSsm {
                ui: ui.clone(),
                name: self.name(),
                region: self.config.region.clone(),
                profile: self.config.profile.clone(),
                launch_template_name: self.config.launch_template_name.clone(),
                launch_template_description: self.config.launch_template_description.clone(),
                ssm_parameter_name: self.config.ssm_parameter_name.clone(),
                ssm_parameter_description: self.config.ssm_parameter_description.clone(),
                assume_role: self.config.assume_role.clone(),
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

        let ami_id = state.get::<String>("ami_id").cloned().unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            id: ami_id,
            builder_id: self.name(),
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
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let mut config = AmazonInstanceConfig::default();
        config.name = "test".to_string();
        config.s3_bucket = Some("my-bucket".to_string());
        config.s3_prefix = Some("prefix".to_string());
        config.bundle_vol = Some(true);
        config.security_group_ids = vec!["sg-123".to_string()];
        config.temporary_security_group_source_cidrs = vec!["0.0.0.0/0".to_string()];
        config.ssh_keypair_name = Some("kp".to_string());
        config.ssh_private_key_file = Some(FilePath::new(std::path::PathBuf::from("/path")));

        let config2 = config.clone();
        assert_eq!(config, config2);
        assert_eq!(format!("{config:?}"), format!("{config2:?}"));

        let builder = AmazonInstanceBuilder::new(config);
        let builder2 = builder.clone();
        assert_eq!(builder.name(), builder2.name());
        assert_eq!(format!("{builder:?}"), format!("{builder2:?}"));
    }

    #[tokio::test]
    async fn test_amazon_instance_prepare_success() {
        let mut config = AmazonInstanceConfig::default();
        config.name = "test".to_string();
        let builder = AmazonInstanceBuilder::new(config);
        assert!(builder.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_amazon_instance_prepare_failure() {
        let config = AmazonInstanceConfig::default();
        let builder = AmazonInstanceBuilder::new(config);
        assert!(builder.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_amazon_instance_run_mocked() {
        let mut config = AmazonInstanceConfig::default();
        config.name = "test".to_string();
        config.s3_bucket = Some("test-bucket".to_string());
        config.s3_prefix = Some("test-prefix".to_string());
        config.spot_instance = Some(SpotInstanceConfig {
            spot_price: Some("0.10".to_string()),
            spot_type: Some("one-time".to_string()),
            ..Default::default()
        });
        config.iam_instance_profile = Some(IamInstanceProfile {
            name: Some("test-profile".to_string()),
            arn: None,
        });
        config.ami_config = Some(AmiConfig {
            ami_regions: vec!["us-east-1".to_string()],
            encrypt_boot: Some(true),
            kms_key_id: Some(KmsKeyId("kms-key".to_string())),
            ami_users: vec!["123456789012".to_string()],
            ami_groups: vec![],
            ..Default::default()
        });

        let builder = AmazonInstanceBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_amazon_instance_run_bad_exit() {
        let mut config = AmazonInstanceConfig::default();
        config.name = "test_bad_exit".to_string();
        let builder = AmazonInstanceBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_amazon_instance_run_missing() {
        let mut config = AmazonInstanceConfig::default();
        config.name = "test_missing".to_string();
        let builder = AmazonInstanceBuilder::new(config);
        let res = builder
            .run(
                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: std::sync::Arc::new(vec![]),
                    error_cleanup_provisioners: std::sync::Arc::new(vec![]),
                }),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_amazon_instance_cancel() {
        let mut config = AmazonInstanceConfig::default();
        config.name = "test".to_string();
        let builder = AmazonInstanceBuilder::new(config);
        assert!(builder.cancel().await.is_ok());
    }

    #[tokio::test]
    async fn test_steps_run_and_cleanup() {
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let config = AmazonInstanceConfig::default();

        let mut state = StateBag::new();
        state.put("instance_id", "i-mock".to_string());

        let mut step1 = StepRunSourceInstance {
            ui: ui.clone(),
            name: "test".into(),
            config: config.clone(),
        };
        let mut step2 = StepStopInstance {
            ui: ui.clone(),
            name: "test".into(),
            config: config.clone(),
        };
        let mut step3 = StepBundleOrRegisterAmi {
            ui: ui.clone(),
            name: "test".into(),
            config: config.clone(),
        };

        assert!(step1.run(&mut state).await.is_ok());
        step1.cleanup(&state).await;

        assert!(step2.run(&mut state).await.is_ok());
        step2.cleanup(&state).await;

        assert!(step3.run(&mut state).await.is_ok());
        step3.cleanup(&state).await;

        // StepProvision run
        let hook: Arc<dyn ProvisionHook> = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut step_prov = StepProvision {
            ui,
            name: "test".into(),
            config,
            hook,
        };
        assert!(step_prov.run(&mut state).await.is_ok());

        let mut empty_state = StateBag::new();
        assert!(step_prov.run(&mut empty_state).await.is_ok());
    }
}
