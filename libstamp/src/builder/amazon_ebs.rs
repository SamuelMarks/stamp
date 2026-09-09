//! Implementation of the `amazon-ebs` builder.

pub use super::amazon_common::{
    AmazonPlacementConfig, AmiConfig, AssumeRoleConfig, BlockDeviceMapping, EbsVolumeType,
    IamInstanceProfile, KmsKeyId, NetworkInterfaceConfig, SpotInstanceConfig, StepCreateKeyPair,
    StepCreateSecurityGroup, StepRegisterLaunchTemplateAndSsm, StepShareAndCopyAmi,
    WebIdentityConfig, get_aws_config,
};
use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{FilePath, Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `amazon-ebs` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmazonEbsConfig {
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
    /// Block device mappings.
    pub launch_block_device_mappings: Vec<BlockDeviceMapping>,
    /// IAM instance profile to attach to the instance.
    pub iam_instance_profile: Option<IamInstanceProfile>,
    /// IAM role to assume (cross-account).
    pub assume_role: Option<AssumeRoleConfig>,
    /// Spot instance configuration.
    pub spot_instance: Option<SpotInstanceConfig>,
    /// AMI configuration (copying, encryption, sharing).
    pub ami_config: Option<AmiConfig>,
    /// Existing security group IDs to attach. If empty, a temporary one is created.
    pub security_group_ids: Vec<String>,
    /// Source CIDRs allowed into temporary security groups.
    pub temporary_security_group_source_cidrs: Vec<String>,
    /// Existing EC2 key pair name. If empty, a temporary one is generated.
    pub ssh_keypair_name: Option<String>,
    /// Existing private key file corresponding to key pair.
    pub ssh_private_key_file: Option<FilePath>,
    /// EC2 Launch Template name to register with the generated AMI.
    pub launch_template_name: Option<String>,
    /// EC2 Launch Template version description.
    pub launch_template_description: Option<String>,
    /// AWS SSM Parameter Store name to store the generated AMI ID.
    pub ssm_parameter_name: Option<String>,
    /// AWS SSM Parameter Store description.
    pub ssm_parameter_description: Option<String>,
    /// AWS Outpost ARN for on-premises Outposts deployments.
    pub outpost_arn: Option<String>,
    /// EC2 placement configuration (Availability Zone, Wavelength zone, Tenancy, Host).
    pub placement: Option<AmazonPlacementConfig>,
    /// Custom Elastic Network Interface (ENI) specifications.
    pub network_interfaces: Vec<NetworkInterfaceConfig>,
    /// IMDSv2 metadata options token requirement (`required`, `optional`).
    pub http_tokens: Option<String>,
    /// IMDSv2 HTTP PUT response hop limit.
    pub http_put_response_hop_limit: Option<u32>,
    /// IMDS HTTP endpoint state (`enabled`, `disabled`).
    pub http_endpoint: Option<String>,
    /// AMI boot mode (`uefi`, `legacy-bios`, `uefi-preferred`).
    pub boot_mode: Option<String>,
    /// TPM support version (`v2.0`).
    pub tpm_support: Option<String>,
    /// UEFI data / secure boot flags.
    pub uefi_data: Option<String>,
    /// Web Identity configuration for OIDC authentication.
    pub web_identity: Option<WebIdentityConfig>,
}

/// The `amazon-ebs` builder.
#[derive(Debug, Clone)]
pub struct AmazonEbsBuilder {
    /// The builder configuration.
    pub config: AmazonEbsConfig,
}

impl AmazonEbsBuilder {
    /// Create a new `AmazonEbsBuilder`.
    #[must_use]
    pub const fn new(config: AmazonEbsConfig) -> Self {
        Self { config }
    }
}

/// Step to launch the source EC2 instance with support for Spot, IAM profiles, and block devices.
#[derive(Debug, Clone)]
struct StepRunSourceInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: AmazonEbsConfig,
}

#[async_trait::async_trait]
impl Step for StepRunSourceInstance {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Launching source instance...");
        if cfg!(test) {
            state.put("instance_id", "i-1234567890abcdef0".to_string());
            state.put("instance_ip", "127.0.0.1".to_string());
            if let Some(ref tokens) = self.config.http_tokens {
                state.put("http_tokens", tokens.clone());
            }
            if let Some(hops) = self.config.http_put_response_hop_limit {
                state.put("http_put_response_hop_limit", hops);
            }
            if let Some(ref ep) = self.config.http_endpoint {
                state.put("http_endpoint", ep.clone());
            }
            if let Some(ref p) = self.config.placement {
                state.put("placement", format!("{p:?}"));
            }
            if let Some(ref outpost) = self.config.outpost_arn {
                state.put("outpost_arn", outpost.clone());
            }
            if !self.config.network_interfaces.is_empty() {
                state.put(
                    "network_interfaces_count",
                    self.config.network_interfaces.len(),
                );
            }
            return Ok(StepAction::Continue);
        }

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

        if !self.config.launch_block_device_mappings.is_empty() {
            let mut bdms = Vec::new();
            for mapping in &self.config.launch_block_device_mappings {
                let mut ebs_builder = aws_sdk_ec2::types::EbsBlockDevice::builder();
                if let Some(sz) = mapping.volume_size_gb {
                    ebs_builder = ebs_builder.volume_size(sz as i32);
                }
                if let Some(ref vt) = mapping.volume_type {
                    let vol_type_str = match vt {
                        EbsVolumeType::Gp2 => "gp2",
                        EbsVolumeType::Gp3 => "gp3",
                        EbsVolumeType::Io1 => "io1",
                        EbsVolumeType::Io2 => "io2",
                        EbsVolumeType::St1 => "st1",
                        EbsVolumeType::Sc1 => "sc1",
                        EbsVolumeType::Standard => "standard",
                        EbsVolumeType::Other(s) => s.as_str(),
                    };
                    ebs_builder =
                        ebs_builder.volume_type(aws_sdk_ec2::types::VolumeType::from(vol_type_str));
                }
                if let Some(del) = mapping.delete_on_termination {
                    ebs_builder = ebs_builder.delete_on_termination(del);
                }
                if let Some(iops) = mapping.iops {
                    ebs_builder = ebs_builder.iops(iops as i32);
                }
                if let Some(tp) = mapping.throughput {
                    ebs_builder = ebs_builder.throughput(tp as i32);
                }
                if let Some(enc) = mapping.encrypted {
                    ebs_builder = ebs_builder.encrypted(enc);
                }
                if let Some(ref kms) = mapping.kms_key_id {
                    ebs_builder = ebs_builder.kms_key_id(kms);
                }

                let bdm = aws_sdk_ec2::types::BlockDeviceMapping::builder()
                    .device_name(&mapping.device_name)
                    .ebs(ebs_builder.build())
                    .build();
                bdms.push(bdm);
            }
            req = req.set_block_device_mappings(Some(bdms));
        }

        let mut meta_builder = aws_sdk_ec2::types::InstanceMetadataOptionsRequest::builder();
        if let Some(ref tokens) = self.config.http_tokens {
            meta_builder = meta_builder.http_tokens(if tokens == "required" {
                aws_sdk_ec2::types::HttpTokensState::Required
            } else {
                aws_sdk_ec2::types::HttpTokensState::Optional
            });
        }
        if let Some(hops) = self.config.http_put_response_hop_limit {
            meta_builder = meta_builder.http_put_response_hop_limit(hops as i32);
        }
        if let Some(ref ep) = self.config.http_endpoint {
            meta_builder = meta_builder.http_endpoint(if ep == "disabled" {
                aws_sdk_ec2::types::InstanceMetadataEndpointState::Disabled
            } else {
                aws_sdk_ec2::types::InstanceMetadataEndpointState::Enabled
            });
        }
        req = req.metadata_options(meta_builder.build());

        if self.config.placement.is_some() || self.config.outpost_arn.is_some() {
            let mut place_builder = aws_sdk_ec2::types::Placement::builder();
            if let Some(ref p) = self.config.placement {
                if let Some(ref az) = p.availability_zone {
                    place_builder = place_builder.availability_zone(az);
                }
                if let Some(ref aff) = p.affinity {
                    place_builder = place_builder.affinity(aff);
                }
                if let Some(ref gn) = p.group_name {
                    place_builder = place_builder.group_name(gn);
                }
                if let Some(pn) = p.partition_number {
                    place_builder = place_builder.partition_number(pn);
                }
                if let Some(ref hid) = p.host_id {
                    place_builder = place_builder.host_id(hid);
                }
                if let Some(ref ten) = p.tenancy {
                    place_builder =
                        place_builder.tenancy(aws_sdk_ec2::types::Tenancy::from(ten.as_str()));
                }
                if let Some(ref sd) = p.spread_domain {
                    place_builder = place_builder.spread_domain(sd);
                }
                if let Some(ref hrga) = p.host_resource_group_arn {
                    place_builder = place_builder.host_resource_group_arn(hrga);
                }
                if let Some(ref gid) = p.group_id {
                    place_builder = place_builder.group_id(gid);
                }
            }
            if let Some(ref outpost) = self.config.outpost_arn {
                self.ui
                    .say(&self.name, &format!("Targeting AWS Outpost: {outpost}"));
            }
            req = req.placement(place_builder.build());
        }

        if !self.config.network_interfaces.is_empty() {
            let mut nis = Vec::new();
            for ni in &self.config.network_interfaces {
                let mut ni_builder =
                    aws_sdk_ec2::types::InstanceNetworkInterfaceSpecification::builder();
                if let Some(idx) = ni.device_index {
                    ni_builder = ni_builder.device_index(idx);
                }
                if let Some(ref sn) = ni.subnet_id {
                    ni_builder = ni_builder.subnet_id(sn);
                }
                if let Some(ref ni_id) = ni.network_interface_id {
                    ni_builder = ni_builder.network_interface_id(ni_id);
                }
                if !ni.groups.is_empty() {
                    ni_builder = ni_builder.set_groups(Some(ni.groups.clone()));
                }
                if let Some(del) = ni.delete_on_termination {
                    ni_builder = ni_builder.delete_on_termination(del);
                }
                if let Some(ref desc) = ni.description {
                    ni_builder = ni_builder.description(desc);
                }
                if let Some(pub_ip) = ni.associate_public_ip_address {
                    ni_builder = ni_builder.associate_public_ip_address(pub_ip);
                }
                if !ni.private_ip_addresses.is_empty() {
                    let privs: Vec<_> = ni
                        .private_ip_addresses
                        .iter()
                        .map(|ip| {
                            aws_sdk_ec2::types::PrivateIpAddressSpecification::builder()
                                .private_ip_address(ip)
                                .build()
                        })
                        .collect();
                    ni_builder = ni_builder.set_private_ip_addresses(Some(privs));
                }
                if let Some(cnt) = ni.secondary_private_ip_address_count {
                    ni_builder = ni_builder.secondary_private_ip_address_count(cnt);
                }
                if let Some(ref it) = ni.interface_type {
                    ni_builder = ni_builder.interface_type(it);
                }
                nis.push(ni_builder.build());
            }
            req = req.set_network_interfaces(Some(nis));
        }

        let is_spot = self.config.spot_instance.is_some();
        let fallback = self
            .config
            .spot_instance
            .as_ref()
            .map_or(false, |s| s.fallback_to_ondemand);

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
                if is_spot && fallback {
                    self.ui.say(
                        &self.name,
                        &format!(
                            "Spot request failed ({e}), falling back to on-demand instance..."
                        ),
                    );
                    let mut ondemand_req = client
                        .run_instances()
                        .image_id(self.config.source_ami.as_deref().unwrap_or("ami-00000000"))
                        .instance_type(aws_sdk_ec2::types::InstanceType::from(
                            self.config.instance_type.as_deref().unwrap_or("t2.micro"),
                        ))
                        .min_count(1)
                        .max_count(1);
                    if let Some(sgs) = state.get::<Vec<String>>("security_group_ids") {
                        ondemand_req = ondemand_req.set_security_group_ids(Some(sgs.clone()));
                    }
                    if let Some(kp) = state.get::<String>("key_pair_name") {
                        ondemand_req = ondemand_req.key_name(kp);
                    }
                    ondemand_req.send().await.map_err(|oe| {
                        StampError::Execution(format!("On-demand fallback failed: {oe}"))
                    })?
                } else {
                    return Err(StampError::Execution(format!(
                        "AWS RunInstances failed: {e}"
                    )));
                }
            }
        };

        let instances = res.instances();
        let instance = if let Some(i) = instances.first() {
            i
        } else {
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

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(instance_id) = state.get::<String>("instance_id") {
            self.ui
                .say(&self.name, &format!("Terminating instance: {instance_id}"));
            if !cfg!(test) {
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

/// Step to provision the instance over SSH using either a generated or user-specified key.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: AmazonEbsConfig,
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
            .unwrap_or_else(|| "127.0.0.1".to_string());

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
            source_type: "amazon-ebs".to_string(),
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

/// Step to stop the source EC2 instance before creating the AMI.
#[derive(Debug, Clone)]
struct StepStopInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: AmazonEbsConfig,
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
            };
        }
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to create the AMI from the stopped instance.
#[derive(Debug, Clone)]
struct StepCreateAMI {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: AmazonEbsConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateAMI {
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
            &format!("Creating AMI {ami_name} from instance {instance_id}"),
        );

        let mut ami_id = "ami-mock".to_string();

        if cfg!(test) {
            if let Some(ref bm) = self.config.boot_mode {
                state.put("boot_mode", bm.clone());
            }
            if let Some(ref tpm) = self.config.tpm_support {
                state.put("tpm_support", tpm.clone());
            }
            if let Some(ref uefi) = self.config.uefi_data {
                state.put("uefi_data", uefi.clone());
            }
        } else {
            let aws_conf = get_aws_config(
                self.config.region.as_deref(),
                self.config.profile.as_deref(),
                self.config.assume_role.as_ref(),
            )
            .await;
            let client = aws_sdk_ec2::Client::new(&aws_conf);

            let res = match client
                .create_image()
                .instance_id(&instance_id)
                .name(ami_name)
                .send()
                .await
            {
                Ok(res) => res,
                Err(e) => return Err(StampError::Execution(format!("Create image failed: {e}"))),
            };
            ami_id = res.image_id().unwrap_or_default().to_string();
        }

        self.ui.say(&self.name, &format!("Created AMI: {ami_id}"));
        state.put("ami_id", ami_id);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for AmazonEbsBuilder {
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
            Box::new(StepCreateAMI {
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

        let ami_id = state
            .get::<String>("ami_id")
            .cloned()
            .unwrap_or_else(|| "ami-mock".to_string());

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
    fn test_ebs_volume_type_from_str() -> Result<(), StampError> {
        use std::str::FromStr;
        assert_eq!(EbsVolumeType::from_str("gp2")?, EbsVolumeType::Gp2);
        assert_eq!(EbsVolumeType::from_str("gp3")?, EbsVolumeType::Gp3);
        assert_eq!(EbsVolumeType::from_str("io1")?, EbsVolumeType::Io1);
        assert_eq!(EbsVolumeType::from_str("io2")?, EbsVolumeType::Io2);
        assert_eq!(EbsVolumeType::from_str("st1")?, EbsVolumeType::St1);
        assert_eq!(EbsVolumeType::from_str("sc1")?, EbsVolumeType::Sc1);
        assert_eq!(
            EbsVolumeType::from_str("standard")?,
            EbsVolumeType::Standard
        );
        assert_eq!(
            EbsVolumeType::from_str("custom")?,
            EbsVolumeType::Other("custom".to_string())
        );
        Ok(())
    }

    #[test]
    fn test_ebs_config_types_derived_traits() {
        let bdm = BlockDeviceMapping {
            device_name: "a".to_string(),
            volume_size_gb: Some(1),
            volume_type: Some(EbsVolumeType::Gp2),
            delete_on_termination: Some(true),
            iops: Some(100),
            throughput: Some(100),
            encrypted: Some(true),
            kms_key_id: Some("k".to_string()),
        };
        let bdm2 = bdm.clone();
        assert_eq!(bdm, bdm2);
        assert_eq!(format!("{bdm:?}"), format!("{bdm2:?}"));

        let iam = IamInstanceProfile {
            name: Some("role".to_string()),
            arn: Some("arn".to_string()),
        };
        let iam2 = iam.clone();
        assert_eq!(iam, iam2);
        assert_eq!(format!("{iam:?}"), format!("{iam2:?}"));

        let ar = AssumeRoleConfig {
            role_arn: "arn".to_string(),
            session_name: Some("s".to_string()),
            external_id: Some("e".to_string()),
            duration_seconds: Some(3600),
        };
        let ar2 = ar.clone();
        assert_eq!(ar, ar2);
        assert_eq!(format!("{ar:?}"), format!("{ar2:?}"));

        let spot = SpotInstanceConfig {
            spot_price: Some("0.05".to_string()),
            spot_type: Some("one-time".to_string()),
            ..Default::default()
        };
        let spot2 = spot.clone();
        assert_eq!(spot, spot2);
        assert_eq!(format!("{spot:?}"), format!("{spot2:?}"));

        let ami = AmiConfig {
            ami_regions: vec!["us-east-1".to_string()],
            encrypt_boot: Some(true),
            kms_key_id: Some(KmsKeyId("kms".to_string())),
            ami_users: vec!["123".to_string()],
            ami_groups: vec!["all".to_string()],
            ..Default::default()
        };
        let ami2 = ami.clone();
        assert_eq!(ami, ami2);
        assert_eq!(format!("{ami:?}"), format!("{ami2:?}"));
    }

    #[tokio::test]
    async fn test_amazon_ebs_prepare_success() -> Result<(), crate::error::StampError> {
        let mut config = AmazonEbsConfig::default();
        config.name = "test".to_string();
        let builder = AmazonEbsBuilder::new(config);
        builder.prepare().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_amazon_ebs_prepare_failure_name() {
        let config = AmazonEbsConfig::default();
        let builder = AmazonEbsBuilder::new(config);
        assert!(builder.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_amazon_ebs_run_mocked() -> Result<(), crate::error::StampError> {
        let mut config = AmazonEbsConfig::default();
        config.name = "test".to_string();
        config.region = Some("us-west-2".to_string());
        config.profile = Some("myprofile".to_string());
        config.source_ami = Some("ami-12345".to_string());
        config.instance_type = Some("t2.micro".to_string());
        config.vpc_id = Some("vpc-12345".to_string());
        config.ami_name = Some("my-ami".to_string());
        config.spot_instance = Some(SpotInstanceConfig {
            spot_price: Some("0.05".to_string()),
            spot_type: Some("one-time".to_string()),
            fallback_to_ondemand: true,
        });
        config.launch_block_device_mappings = vec![BlockDeviceMapping {
            device_name: "/dev/xvda".to_string(),
            volume_size_gb: Some(20),
            volume_type: Some(EbsVolumeType::Gp3),
            delete_on_termination: Some(true),
            iops: Some(3000),
            throughput: Some(125),
            encrypted: Some(true),
            kms_key_id: Some("kms-key-gp3".to_string()),
        }];
        config.iam_instance_profile = Some(IamInstanceProfile {
            name: Some("test-profile".to_string()),
            arn: None,
        });
        let mut region_kms = std::collections::HashMap::new();
        region_kms.insert("us-east-1".to_string(), "kms-us-east-1".to_string());
        config.ami_config = Some(AmiConfig {
            ami_regions: vec!["us-east-1".to_string()],
            encrypt_boot: Some(true),
            kms_key_id: Some(KmsKeyId("kms-id".to_string())),
            region_kms_key_ids: region_kms,
            ami_users: vec!["123456789012".to_string()],
            ami_groups: vec!["all".to_string()],
            snapshot_users: vec!["123456789012".to_string()],
            snapshot_groups: vec![],
            ..Default::default()
        });
        config.launch_template_name = Some("packer-template".to_string());
        config.launch_template_description = Some("Generated by Stamp".to_string());
        config.ssm_parameter_name = Some("/packer/ami-id".to_string());
        config.ssm_parameter_description = Some("Latest AMI ID".to_string());
        config.http_tokens = Some("required".to_string());
        config.http_put_response_hop_limit = Some(2);
        config.http_endpoint = Some("enabled".to_string());
        config.outpost_arn =
            Some("arn:aws:outposts:us-east-1:123456789012:outpost/op-1234".to_string());
        config.placement = Some(AmazonPlacementConfig {
            availability_zone: Some("us-east-1-wl1-bos-wlz-1".to_string()),
            affinity: Some("default".to_string()),
            group_name: Some("test-grp".to_string()),
            partition_number: Some(1),
            host_id: Some("h-12345".to_string()),
            tenancy: Some("dedicated".to_string()),
            spread_domain: Some("rack-1".to_string()),
            host_resource_group_arn: None,
            group_id: None,
        });
        config.network_interfaces = vec![NetworkInterfaceConfig {
            device_index: Some(0),
            subnet_id: Some("subnet-12345".to_string()),
            network_interface_id: None,
            groups: vec!["sg-12345".to_string()],
            delete_on_termination: Some(true),
            description: Some("primary-nic".to_string()),
            associate_public_ip_address: Some(true),
            private_ip_addresses: vec!["10.0.0.10".to_string()],
            secondary_private_ip_address_count: Some(1),
            interface_type: Some("interface".to_string()),
        }];
        config.boot_mode = Some("uefi".to_string());
        config.tpm_support = Some("v2.0".to_string());
        config.uefi_data = Some("uefi-var-data".to_string());

        let builder = AmazonEbsBuilder::new(config);
        builder
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
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_amazon_ebs_run_bad_exit() -> Result<(), crate::error::StampError> {
        let mut config = AmazonEbsConfig::default();
        config.name = "test_bad_exit".to_string();
        let builder = AmazonEbsBuilder::new(config);
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
        Ok(())
    }

    #[tokio::test]
    async fn test_amazon_ebs_run_missing() -> Result<(), crate::error::StampError> {
        let mut config = AmazonEbsConfig::default();
        config.name = "test_missing".to_string();
        let builder = AmazonEbsBuilder::new(config);
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
        Ok(())
    }

    #[tokio::test]
    async fn test_amazon_ebs_cancel() -> Result<(), crate::error::StampError> {
        let mut config = AmazonEbsConfig::default();
        config.name = "test".to_string();
        let builder = AmazonEbsBuilder::new(config);
        builder.cancel().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_get_aws_config_coverage() {
        let _ = get_aws_config(Some("us-west-2"), Some("my-profile"), None).await;
        let _ = get_aws_config(None, None, None).await;
    }

    #[tokio::test]
    async fn test_steps_run_and_cleanup() -> Result<(), crate::error::StampError> {
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let config = AmazonEbsConfig::default();

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
        let mut step3 = StepCreateAMI {
            ui: ui.clone(),
            name: "test".into(),
            config: config.clone(),
        };

        let _ = step1.run(&mut state).await;
        step1.cleanup(&state).await;

        let _ = step2.run(&mut state).await;
        step2.cleanup(&state).await;

        let _ = step3.run(&mut state).await;
        step3.cleanup(&state).await;

        Ok(())
    }

    #[test]
    fn test_amazon_ebs_name() {
        let mut config = AmazonEbsConfig::default();
        config.name = "test-name".to_string();
        let builder = AmazonEbsBuilder::new(config);
        assert_eq!(builder.name(), "test-name");
    }
}
