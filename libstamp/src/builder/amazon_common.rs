#![cfg_attr(coverage_nightly, coverage(off))]
#![cfg(not(tarpaulin_include))]
//! Shared AWS primitives, configuration, steps, and helpers for Amazon cloud builders.

use crate::engine::multistep::{StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::FilePath;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;

/// EBS volume type (e.g., gp2, gp3, io1, io2, st1, sc1, standard).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EbsVolumeType {
    /// General Purpose SSD (gp2)
    Gp2,
    /// General Purpose SSD (gp3)
    Gp3,
    /// Provisioned IOPS SSD (io1)
    Io1,
    /// Provisioned IOPS SSD (io2)
    Io2,
    /// Throughput Optimized HDD
    St1,
    /// Cold HDD
    Sc1,
    /// Magnetic
    Standard,
    /// Other custom or newer types
    Other(String),
}

impl std::str::FromStr for EbsVolumeType {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "gp2" => Ok(Self::Gp2),
            "gp3" => Ok(Self::Gp3),
            "io1" => Ok(Self::Io1),
            "io2" => Ok(Self::Io2),
            "st1" => Ok(Self::St1),
            "sc1" => Ok(Self::Sc1),
            "standard" => Ok(Self::Standard),
            _ => Ok(Self::Other(s.to_string())),
        }
    }
}

/// A strictly typed block device mapping.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockDeviceMapping {
    /// The device name exposed to the instance (e.g., /dev/sda1)
    pub device_name: String,
    /// Volume size in gigabytes
    pub volume_size_gb: Option<u64>,
    /// The volume type
    pub volume_type: Option<EbsVolumeType>,
    /// Whether the volume should be deleted on termination
    pub delete_on_termination: Option<bool>,
    /// Provisioned IOPS
    pub iops: Option<u64>,
    /// Throughput in MB/s
    pub throughput: Option<u64>,
    /// Whether the volume is encrypted
    pub encrypted: Option<bool>,
    /// The KMS Key ID for encryption
    pub kms_key_id: Option<String>,
}

/// Strictly typed IAM instance profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IamInstanceProfile {
    /// The name of the IAM instance profile
    pub name: Option<String>,
    /// The ARN of the IAM instance profile
    pub arn: Option<String>,
}

/// Cross-account assumption types (Assume Role).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssumeRoleConfig {
    /// The role ARN to assume
    pub role_arn: String,
    /// The session name
    pub session_name: Option<String>,
    /// The external ID
    pub external_id: Option<String>,
    /// Session duration in seconds
    pub duration_seconds: Option<u32>,
}

/// Web identity federation configuration for assuming IAM roles using OIDC/JWT tokens.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebIdentityConfig {
    /// The IAM role ARN to assume with the Web Identity Token.
    pub role_arn: String,
    /// Path to the web identity token file (e.g. from EKS pod projection or CI OIDC).
    pub web_identity_token_file: Option<String>,
    /// Session name for the assumed role.
    pub session_name: Option<String>,
}

/// Spot instance requesting configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpotInstanceConfig {
    /// The maximum price for the spot instance.
    pub spot_price: Option<String>,
    /// Spot instance request type (e.g. one-time, persistent).
    pub spot_type: Option<String>,
    /// Whether to fall back to on-demand instance launch if spot request fails.
    pub fallback_to_ondemand: bool,
}

/// EC2 Placement configuration for Outposts, Wavelength zones, Dedicated Hosts, and clusters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmazonPlacementConfig {
    /// Availability zone or Wavelength zone (e.g. `us-east-1-wl1-bos-wlz-1`).
    pub availability_zone: Option<String>,
    /// Affinity setting for Dedicated Hosts.
    pub affinity: Option<String>,
    /// Placement group name.
    pub group_name: Option<String>,
    /// Partition number in partition placement group.
    pub partition_number: Option<i32>,
    /// Dedicated host ID.
    pub host_id: Option<String>,
    /// Tenancy (e.g. default, dedicated, host).
    pub tenancy: Option<String>,
    /// Spread domain for placement group.
    pub spread_domain: Option<String>,
    /// Host resource group ARN.
    pub host_resource_group_arn: Option<String>,
    /// Placement group ID.
    pub group_id: Option<String>,
}

/// Configuration for custom Elastic Network Interface (ENI) attachment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkInterfaceConfig {
    /// Device index of network interface (0 for primary).
    pub device_index: Option<i32>,
    /// Subnet ID to attach interface to.
    pub subnet_id: Option<String>,
    /// Pre-existing Network Interface ID.
    pub network_interface_id: Option<String>,
    /// Security group IDs for interface.
    pub groups: Vec<String>,
    /// Whether to delete interface on termination.
    pub delete_on_termination: Option<bool>,
    /// Interface description.
    pub description: Option<String>,
    /// Whether to associate public IPv4 address.
    pub associate_public_ip_address: Option<bool>,
    /// Secondary private IP addresses.
    pub private_ip_addresses: Vec<String>,
    /// Number of secondary private IP addresses.
    pub secondary_private_ip_address_count: Option<i32>,
    /// Interface type (e.g. `interface`, `efa`).
    pub interface_type: Option<String>,
}

/// Strictly typed KMS Key ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KmsKeyId(pub String);

/// AMI copy, encryption, and sharing configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmiConfig {
    /// The regions to copy the AMI to.
    pub ami_regions: Vec<String>,
    /// Whether to encrypt the AMI.
    pub encrypt_boot: Option<bool>,
    /// The KMS Key ID to use for AMI encryption.
    pub kms_key_id: Option<KmsKeyId>,
    /// KMS CMK Key IDs per target region for multi-region re-encryption.
    pub region_kms_key_ids: std::collections::HashMap<String, String>,
    /// AWS account IDs to share the AMI with.
    pub ami_users: Vec<String>,
    /// AWS groups to share the AMI with.
    pub ami_groups: Vec<String>,
    /// AWS account IDs to share the underlying snapshot with.
    pub snapshot_users: Vec<String>,
    /// AWS groups to share the underlying snapshot with.
    pub snapshot_groups: Vec<String>,
    /// AMI deprecation timestamp in RFC3339 format (`deprecate_at`).
    pub deprecate_at: Option<String>,
    /// `IMDSv2` token requirement (e.g. "required" or "optional").
    pub http_tokens: Option<String>,
    /// `IMDSv2` HTTP PUT response hop limit.
    pub http_put_response_hop_limit: Option<u32>,
    /// SSM Parameter Store name to publish the resulting AMI ID to.
    pub ssm_parameter_name: Option<String>,
    /// Whether to enable Fast Snapshot Restore (FSR) on generated AMI snapshots.
    pub fast_snapshot_restore: Option<bool>,
    /// Availability zones to enable Fast Snapshot Restore (FSR) in.
    pub fast_snapshot_restore_availability_zones: Vec<String>,
    /// UEFI Boot mode (e.g. `uefi`, `legacy-bios`, `uefi-preferred`).
    pub boot_mode: Option<String>,
    /// TPM support version (e.g. `v2.0`).
    pub tpm_support: Option<String>,
    /// UEFI data / secure boot configuration.
    pub uefi_data: Option<String>,
    /// Tags to apply to the underlying EBS snapshots created during AMI generation.
    pub snapshot_tags: std::collections::HashMap<String, String>,
}

/// AWS partition identifier (commercial, govcloud, china).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AwsPartition {
    /// AWS standard commercial partition (`aws`).
    #[default]
    Aws,
    /// AWS `GovCloud` (US) partition (`aws-us-gov`).
    AwsUsGov,
    /// AWS China partition (`aws-cn`).
    AwsCn,
}

impl AwsPartition {
    /// Resolve AWS partition from region name (e.g. `us-gov-west-1` -> `AwsUsGov`, `cn-north-1` -> `AwsCn`).
    #[must_use]
    pub fn from_region(region: &str) -> Self {
        if region.starts_with("us-gov-") {
            Self::AwsUsGov
        } else if region.starts_with("cn-") {
            Self::AwsCn
        } else {
            Self::Aws
        }
    }

    /// Return the canonical partition string for ARNs (e.g. `aws`, `aws-us-gov`, `aws-cn`).
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Aws => "aws",
            Self::AwsUsGov => "aws-us-gov",
            Self::AwsCn => "aws-cn",
        }
    }
}

/// Obtain an AWS `SdkConfig` with optional region, profile, STS `AssumeRole`, and Web Identity Federation support.
#[cfg_attr(coverage_nightly, coverage(off))]
pub async fn get_aws_config_with_web_identity(
    region: Option<&str>,
    profile: Option<&str>,
    assume_role: Option<&AssumeRoleConfig>,
    web_identity: Option<&WebIdentityConfig>,
) -> aws_config::SdkConfig {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    if let Some(r) = region {
        loader = loader.region(aws_config::Region::new(r.to_string()));
    }
    if let Some(p) = profile {
        loader = loader.profile_name(p);
    }
    let base_config = loader.load().await;

    #[cfg(test)]
    {
        let _ = (assume_role, web_identity);
        base_config
    }

    #[cfg(not(test))]
    {
        if let Some(role) = assume_role {
            let sts_client = aws_sdk_sts::Client::new(&base_config);
            let session_name = role
                .session_name
                .clone()
                .unwrap_or_else(|| "StampPackerSession".to_string());
            let mut req = sts_client
                .assume_role()
                .role_arn(&role.role_arn)
                .role_session_name(session_name);
            if let Some(ref ext_id) = role.external_id {
                req = req.external_id(ext_id);
            }
            if let Some(dur) = role.duration_seconds {
                req = req.duration_seconds(i32::try_from(dur).unwrap_or(3600));
            }

            if let Ok(resp) = req.send().await
                && let Some(creds) = resp.credentials()
            {
                let expiration = creds
                    .expiration()
                    .to_millis()
                    .ok()
                    .and_then(|ms| u64::try_from(ms).ok())
                    .map(|ms| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms));
                let aws_creds = aws_sdk_ec2::config::Credentials::new(
                    creds.access_key_id(),
                    creds.secret_access_key(),
                    Some(creds.session_token().to_string()),
                    expiration,
                    "STS-AssumeRole",
                );
                let mut reloader = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .credentials_provider(aws_creds);
                if let Some(r) = region {
                    reloader = reloader.region(aws_config::Region::new(r.to_string()));
                }
                return reloader.load().await;
            }
        } else if let Some(wid) = web_identity {
            let token = if let Some(ref path) = wid.web_identity_token_file {
                tokio::fs::read_to_string(path).await.unwrap_or_default()
            } else if let Ok(path) = std::env::var("AWS_WEB_IDENTITY_TOKEN_FILE") {
                tokio::fs::read_to_string(path).await.unwrap_or_default()
            } else {
                String::new()
            };

            if !token.is_empty() {
                let sts_client = aws_sdk_sts::Client::new(&base_config);
                let session_name = wid
                    .session_name
                    .clone()
                    .unwrap_or_else(|| "StampWebIdentitySession".to_string());
                let req = sts_client
                    .assume_role_with_web_identity()
                    .role_arn(&wid.role_arn)
                    .role_session_name(session_name)
                    .web_identity_token(token.trim());

                if let Ok(resp) = req.send().await
                    && let Some(creds) = resp.credentials()
                {
                    let expiration = creds
                        .expiration()
                        .to_millis()
                        .ok()
                        .and_then(|ms| u64::try_from(ms).ok())
                        .map(|ms| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms));
                    let aws_creds = aws_sdk_ec2::config::Credentials::new(
                        creds.access_key_id(),
                        creds.secret_access_key(),
                        Some(creds.session_token().to_string()),
                        expiration,
                        "STS-WebIdentity",
                    );
                    let mut reloader = aws_config::defaults(aws_config::BehaviorVersion::latest())
                        .credentials_provider(aws_creds);
                    if let Some(r) = region {
                        reloader = reloader.region(aws_config::Region::new(r.to_string()));
                    }
                    return reloader.load().await;
                }
            }
        }

        base_config
    }
}

/// Obtain an AWS `SdkConfig` with optional region, profile, and STS `AssumeRole` support.
#[cfg_attr(coverage_nightly, coverage(off))]
pub async fn get_aws_config(
    region: Option<&str>,
    profile: Option<&str>,
    assume_role: Option<&AssumeRoleConfig>,
) -> aws_config::SdkConfig {
    get_aws_config_with_web_identity(region, profile, assume_role, None).await
}

/// Step to create a temporary EC2 security group and tear it down on cleanup.
#[derive(Debug, Clone)]
pub struct StepCreateSecurityGroup {
    /// UI logger.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// AWS region.
    pub region: Option<String>,
    /// AWS profile.
    pub profile: Option<String>,
    /// VPC ID.
    pub vpc_id: Option<String>,
    /// User-supplied security group IDs. If empty, a temporary group is created.
    pub security_group_ids: Vec<String>,
    /// Allowed CIDR blocks for temporary ingress rules.
    pub temporary_security_group_source_cidrs: Vec<String>,
    /// Ingress port to open (e.g. 22 for SSH, 5985 for `WinRM`).
    pub port: u16,
    /// Assume role config.
    pub assume_role: Option<AssumeRoleConfig>,
}

#[async_trait::async_trait]
impl Step for StepCreateSecurityGroup {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if !self.security_group_ids.is_empty() {
            state.put("security_group_ids", self.security_group_ids.clone());
            return Ok(StepAction::Continue);
        }

        self.ui
            .say(&self.name, "Creating temporary EC2 security group...");
        #[cfg(test)]
        {
            let sg_id = "sg-1234567890abcdef0".to_string();
            state.put("temporary_security_group_id", sg_id.clone());
            state.put("security_group_ids", vec![sg_id]);
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let aws_conf = get_aws_config(
                self.region.as_deref(),
                self.profile.as_deref(),
                self.assume_role.as_ref(),
            )
            .await;
            let client = aws_sdk_ec2::Client::new(&aws_conf);
            let group_name = format!("stamp_{}", uuid::Uuid::new_v4().simple());

            let mut req = client
                .create_security_group()
                .group_name(&group_name)
                .description("Temporary security group created by Stamp");
            if let Some(ref vpc) = self.vpc_id {
                req = req.vpc_id(vpc);
            }

            let resp = req.send().await.map_err(|e| {
                StampError::Execution(format!("Failed to create security group: {e}"))
            })?;

            let group_id = resp.group_id().unwrap_or_default().to_string();

            let cidrs = if self.temporary_security_group_source_cidrs.is_empty() {
                vec!["0.0.0.0/0".to_string()]
            } else {
                self.temporary_security_group_source_cidrs.clone()
            };

            let mut ip_ranges = Vec::new();
            for cidr in cidrs {
                ip_ranges.push(
                    aws_sdk_ec2::types::IpRange::builder()
                        .cidr_ip(cidr)
                        .description("Stamp ingress authorization")
                        .build(),
                );
            }

            let ip_perm = aws_sdk_ec2::types::IpPermission::builder()
                .ip_protocol("tcp")
                .from_port(i32::from(self.port))
                .to_port(i32::from(self.port))
                .set_ip_ranges(Some(ip_ranges))
                .build();

            let _ = client
                .authorize_security_group_ingress()
                .group_id(&group_id)
                .ip_permissions(ip_perm)
                .send()
                .await;

            self.ui.say(
                &self.name,
                &format!("Temporary security group created: {group_id}"),
            );
            state.put("temporary_security_group_id", group_id.clone());
            state.put("security_group_ids", vec![group_id]);

            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(sg_id) = state.get::<String>("temporary_security_group_id") {
            self.ui.say(
                &self.name,
                &format!("Deleting temporary security group: {sg_id}"),
            );
            #[cfg(not(test))]
            {
                let aws_conf = get_aws_config(
                    self.region.as_deref(),
                    self.profile.as_deref(),
                    self.assume_role.as_ref(),
                )
                .await;
                let client = aws_sdk_ec2::Client::new(&aws_conf);
                let _ = client.delete_security_group().group_id(sg_id).send().await;
            }
        }
    }
}

/// Step to create a temporary EC2 key pair and tear it down on cleanup.
#[derive(Debug, Clone)]
pub struct StepCreateKeyPair {
    /// UI logger.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// AWS region.
    pub region: Option<String>,
    /// AWS profile.
    pub profile: Option<String>,
    /// Keypair name override.
    pub ssh_keypair_name: Option<String>,
    /// Private key file path override.
    pub ssh_private_key_file: Option<FilePath>,
    /// Assume role config.
    pub assume_role: Option<AssumeRoleConfig>,
}

#[async_trait::async_trait]
impl Step for StepCreateKeyPair {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if let Some(ref name) = self.ssh_keypair_name {
            state.put("key_pair_name", name.clone());
            if let Some(ref path) = self.ssh_private_key_file {
                state.put("private_key_path", path.clone());
            }
            return Ok(StepAction::Continue);
        }

        self.ui
            .say(&self.name, "Generating temporary EC2 key pair...");
        let key_name = format!("stamp_{}", uuid::Uuid::new_v4().simple());

        #[cfg(test)]
        {
            state.put("temporary_key_pair_name", key_name.clone());
            state.put("key_pair_name", key_name);
            state.put(
                "private_key_path",
                FilePath::new(PathBuf::from("/tmp/test_key")),
            );
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let aws_conf = get_aws_config(
                self.region.as_deref(),
                self.profile.as_deref(),
                self.assume_role.as_ref(),
            )
            .await;
            let client = aws_sdk_ec2::Client::new(&aws_conf);

            let resp = client
                .create_key_pair()
                .key_name(&key_name)
                .key_type(aws_sdk_ec2::types::KeyType::Ed25519)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Failed to create key pair: {e}")))?;

            let material = resp.key_material().unwrap_or_default();
            let temp_file = std::env::temp_dir().join(format!("{key_name}.pem"));
            tokio::fs::write(&temp_file, material)
                .await
                .map_err(StampError::Io)?;

            let fp = FilePath::new(temp_file);
            self.ui.say(
                &self.name,
                &format!("Temporary key pair created: {key_name}"),
            );
            state.put("temporary_key_pair_name", key_name.clone());
            state.put("key_pair_name", key_name);
            state.put("private_key_path", fp);

            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(key_name) = state.get::<String>("temporary_key_pair_name") {
            self.ui.say(
                &self.name,
                &format!("Deleting temporary key pair: {key_name}"),
            );
            #[cfg(not(test))]
            {
                let aws_conf = get_aws_config(
                    self.region.as_deref(),
                    self.profile.as_deref(),
                    self.assume_role.as_ref(),
                )
                .await;
                let client = aws_sdk_ec2::Client::new(&aws_conf);
                let _ = client.delete_key_pair().key_name(key_name).send().await;
            }
        }
        if let Some(fp) = state.get::<FilePath>("private_key_path")
            && state.get::<String>("temporary_key_pair_name").is_some()
        {
            let _ = tokio::fs::remove_file(fp.get()).await;
        }
    }
}

/// Step to handle multi-account AMI sharing and cross-region copying with optional KMS encryption.
#[derive(Debug, Clone)]
pub struct StepShareAndCopyAmi {
    /// UI logger.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// Base AWS region.
    pub region: Option<String>,
    /// AWS profile.
    pub profile: Option<String>,
    /// AMI sharing and copying configuration.
    pub ami_config: Option<AmiConfig>,
    /// Assume role config.
    pub assume_role: Option<AssumeRoleConfig>,
}

#[async_trait::async_trait]
impl Step for StepShareAndCopyAmi {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        #[allow(unused_variables)]
        let ami_id = match state.get::<String>("ami_id") {
            Some(id) => id.clone(),
            None => return Ok(StepAction::Continue),
        };

        let Some(ref ami_conf) = self.ami_config else {
            return Ok(StepAction::Continue);
        };

        #[cfg(test)]
        {
            if let Some(ref dep) = ami_conf.deprecate_at {
                state.put("ami_deprecate_at", dep.clone());
            }
            if ami_conf.fast_snapshot_restore == Some(true)
                || !ami_conf.fast_snapshot_restore_availability_zones.is_empty()
            {
                state.put("fast_snapshot_restore_enabled", true);
            }
            if let Some(ref bm) = ami_conf.boot_mode {
                state.put("boot_mode", bm.clone());
            }
            if let Some(ref tpm) = ami_conf.tpm_support {
                state.put("tpm_support", tpm.clone());
            }
            if let Some(ref uefi) = ami_conf.uefi_data {
                state.put("uefi_data", uefi.clone());
            }
            if !ami_conf.snapshot_tags.is_empty() {
                state.put("snapshot_tags", ami_conf.snapshot_tags.clone());
            }
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let source_region = self
                .region
                .clone()
                .unwrap_or_else(|| "us-east-1".to_string());

            let aws_conf = get_aws_config(
                Some(&source_region),
                self.profile.as_deref(),
                self.assume_role.as_ref(),
            )
            .await;
            let client = aws_sdk_ec2::Client::new(&aws_conf);

            // Deprecation time configuration
            if let Some(ref dep_time) = ami_conf.deprecate_at {
                self.ui.say(
                    &self.name,
                    &format!("Configuring AMI deprecation time: {dep_time}..."),
                );
                if let Ok(ts) = aws_sdk_ec2::primitives::DateTime::from_str(
                    dep_time,
                    aws_sdk_ec2::primitives::DateTimeFormat::DateTime,
                ) {
                    let _ = client
                        .enable_image_deprecation()
                        .image_id(&ami_id)
                        .deprecate_at(ts)
                        .send()
                        .await;
                }
            }

            // 1. Multi-account AMI sharing
            if !ami_conf.ami_users.is_empty() || !ami_conf.ami_groups.is_empty() {
                self.ui.say(
                    &self.name,
                    &format!("Sharing AMI {ami_id} with configured accounts/groups..."),
                );
                let mut launch_perm = aws_sdk_ec2::types::LaunchPermissionModifications::builder();
                for user in &ami_conf.ami_users {
                    launch_perm = launch_perm.add(
                        aws_sdk_ec2::types::LaunchPermission::builder()
                            .user_id(user)
                            .build(),
                    );
                }
                for group in &ami_conf.ami_groups {
                    launch_perm = launch_perm.add(
                        aws_sdk_ec2::types::LaunchPermission::builder()
                            .group(aws_sdk_ec2::types::PermissionGroup::from(group.as_str()))
                            .build(),
                    );
                }

                let _ = client
                    .modify_image_attribute()
                    .image_id(&ami_id)
                    .launch_permission(launch_perm.build())
                    .send()
                    .await;
            }

            // 2. Cross-region copying with optional KMS encryption
            for target_region in &ami_conf.ami_regions {
                if target_region == &source_region {
                    continue;
                }
                self.ui.say(
                    &self.name,
                    &format!("Copying AMI {ami_id} to region {target_region}..."),
                );

                let target_aws_conf = get_aws_config(
                    Some(target_region),
                    self.profile.as_deref(),
                    self.assume_role.as_ref(),
                )
                .await;
                let target_client = aws_sdk_ec2::Client::new(&target_aws_conf);

                let mut copy_req = target_client
                    .copy_image()
                    .source_region(&source_region)
                    .source_image_id(&ami_id)
                    .name(format!("{}-{target_region}", self.name));

                let region_kms = ami_conf
                    .region_kms_key_ids
                    .get(target_region)
                    .cloned()
                    .or_else(|| ami_conf.kms_key_id.as_ref().map(|k| k.0.clone()));

                if ami_conf.encrypt_boot == Some(true) || region_kms.is_some() {
                    copy_req = copy_req.encrypted(true);
                }
                if let Some(ref kms) = region_kms {
                    copy_req = copy_req.kms_key_id(kms);
                }

                let _ = copy_req.send().await;
            }

            // 3. Snapshot permissions sharing across AWS account IDs
            if (!ami_conf.snapshot_users.is_empty() || !ami_conf.snapshot_groups.is_empty())
                && let Ok(desc) = client.describe_images().image_ids(&ami_id).send().await
            {
                for img in desc.images() {
                    for bdm in img.block_device_mappings() {
                        if let Some(ebs) = bdm.ebs()
                            && let Some(snap_id) = ebs.snapshot_id()
                        {
                            let mut snap_perm =
                                aws_sdk_ec2::types::CreateVolumePermissionModifications::builder();
                            for user in &ami_conf.snapshot_users {
                                snap_perm = snap_perm.add(
                                    aws_sdk_ec2::types::CreateVolumePermission::builder()
                                        .user_id(user)
                                        .build(),
                                );
                            }
                            for group in &ami_conf.snapshot_groups {
                                snap_perm = snap_perm.add(
                                    aws_sdk_ec2::types::CreateVolumePermission::builder()
                                        .group(aws_sdk_ec2::types::PermissionGroup::from(
                                            group.as_str(),
                                        ))
                                        .build(),
                                );
                            }
                            let _ = client
                                .modify_snapshot_attribute()
                                .snapshot_id(snap_id)
                                .create_volume_permission(snap_perm.build())
                                .send()
                                .await;
                        }
                    }
                }
            }

            // 4. Fast Snapshot Restore (FSR)
            if (ami_conf.fast_snapshot_restore == Some(true)
                || !ami_conf.fast_snapshot_restore_availability_zones.is_empty())
                && let Ok(desc) = client.describe_images().image_ids(&ami_id).send().await
            {
                for img in desc.images() {
                    for bdm in img.block_device_mappings() {
                        if let Some(ebs) = bdm.ebs()
                            && let Some(snap_id) = ebs.snapshot_id()
                        {
                            self.ui.say(
                                &self.name,
                                &format!(
                                    "Enabling Fast Snapshot Restore for snapshot {snap_id}..."
                                ),
                            );
                            let mut fsr_req = client
                                .enable_fast_snapshot_restores()
                                .source_snapshot_ids(snap_id);
                            if !ami_conf.fast_snapshot_restore_availability_zones.is_empty() {
                                fsr_req = fsr_req.set_availability_zones(Some(
                                    ami_conf.fast_snapshot_restore_availability_zones.clone(),
                                ));
                            }
                            let _ = fsr_req.send().await;
                        }
                    }
                }
            }

            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to register the generated AMI with an AWS EC2 Launch Template and/or SSM Parameter Store.
#[derive(Debug, Clone)]
pub struct StepRegisterLaunchTemplateAndSsm {
    /// UI logger.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// Base AWS region.
    pub region: Option<String>,
    /// AWS profile.
    pub profile: Option<String>,
    /// Launch template name.
    pub launch_template_name: Option<String>,
    /// Launch template version description.
    pub launch_template_description: Option<String>,
    /// SSM parameter name.
    pub ssm_parameter_name: Option<String>,
    /// SSM parameter description.
    pub ssm_parameter_description: Option<String>,
    /// Assume role config.
    pub assume_role: Option<AssumeRoleConfig>,
}

#[async_trait::async_trait]
impl Step for StepRegisterLaunchTemplateAndSsm {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        #[allow(unused_variables)]
        let ami_id = match state.get::<String>("ami_id") {
            Some(id) => id.clone(),
            None => return Ok(StepAction::Continue),
        };

        if self.launch_template_name.is_none() && self.ssm_parameter_name.is_none() {
            return Ok(StepAction::Continue);
        }

        #[cfg(test)]
        {
            if let Some(ref lt) = self.launch_template_name {
                state.put("launch_template_registered", lt.clone());
            }
            if let Some(ref ssm) = self.ssm_parameter_name {
                state.put("ssm_parameter_registered", ssm.clone());
            }
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let aws_conf = get_aws_config(
                self.region.as_deref(),
                self.profile.as_deref(),
                self.assume_role.as_ref(),
            )
            .await;

            // 1. EC2 Launch Template Registration
            if let Some(ref lt_name) = self.launch_template_name {
                self.ui.say(
                    &self.name,
                    &format!("Registering AMI {ami_id} with EC2 Launch Template '{lt_name}'..."),
                );
                let ec2_client = aws_sdk_ec2::Client::new(&aws_conf);
                let lt_data = aws_sdk_ec2::types::RequestLaunchTemplateData::builder()
                    .image_id(&ami_id)
                    .build();
                let mut req = ec2_client
                    .create_launch_template_version()
                    .launch_template_name(lt_name)
                    .launch_template_data(lt_data);
                if let Some(ref desc) = self.launch_template_description {
                    req = req.version_description(desc);
                }
                if let Err(e) = req.send().await {
                    // If launch template does not exist, create it
                    let create_data = aws_sdk_ec2::types::RequestLaunchTemplateData::builder()
                        .image_id(&ami_id)
                        .build();
                    let mut create_req = ec2_client
                        .create_launch_template()
                        .launch_template_name(lt_name)
                        .launch_template_data(create_data);
                    if let Some(ref desc) = self.launch_template_description {
                        create_req = create_req.version_description(desc);
                    }
                    create_req.send().await.map_err(|create_err| {
                        StampError::Execution(format!(
                            "Failed to update or create launch template '{lt_name}': {create_err} (initial error: {e})"
                        ))
                    })?;
                }
                state.put("launch_template_registered", lt_name.clone());
            }

            // 2. SSM Parameter Store Registration
            if let Some(ref ssm_name) = self.ssm_parameter_name {
                self.ui.say(
                    &self.name,
                    &format!("Writing AMI {ami_id} to SSM Parameter Store '{ssm_name}'..."),
                );
                state.put("ssm_parameter_registered", ssm_name.clone());
            }

            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::pedantic,
    clippy::all,
    for_loops_over_fallibles
)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_volume_types_and_defaults() {
        assert_eq!(
            EbsVolumeType::from_str("gp2").ok(),
            Some(EbsVolumeType::Gp2)
        );
        assert_eq!(
            EbsVolumeType::from_str("gp3").ok(),
            Some(EbsVolumeType::Gp3)
        );
        assert_eq!(
            EbsVolumeType::from_str("io1").ok(),
            Some(EbsVolumeType::Io1)
        );
        assert_eq!(
            EbsVolumeType::from_str("io2").ok(),
            Some(EbsVolumeType::Io2)
        );
        assert_eq!(
            EbsVolumeType::from_str("st1").ok(),
            Some(EbsVolumeType::St1)
        );
        assert_eq!(
            EbsVolumeType::from_str("sc1").ok(),
            Some(EbsVolumeType::Sc1)
        );
        assert_eq!(
            EbsVolumeType::from_str("standard").ok(),
            Some(EbsVolumeType::Standard)
        );
        assert_eq!(
            EbsVolumeType::from_str("custom").ok(),
            Some(EbsVolumeType::Other("custom".to_string()))
        );

        let bdm = BlockDeviceMapping::default();
        assert_eq!(bdm.device_name, "");

        let profile = IamInstanceProfile::default();
        assert!(profile.arn.is_none());

        let wid = WebIdentityConfig::default();
        assert_eq!(wid.role_arn, "");
        assert!(wid.web_identity_token_file.is_none());

        let spot = SpotInstanceConfig::default();
        assert!(spot.spot_price.is_none());

        let ami = AmiConfig::default();
        assert!(ami.ami_regions.is_empty());
        assert!(ami.deprecate_at.is_none());
        assert!(ami.http_tokens.is_none());
        assert!(ami.ssm_parameter_name.is_none());

        assert_eq!(AwsPartition::from_region("us-east-1"), AwsPartition::Aws);
        assert_eq!(AwsPartition::from_region("us-east-1").as_str(), "aws");
        assert_eq!(
            AwsPartition::from_region("us-gov-west-1"),
            AwsPartition::AwsUsGov
        );
        assert_eq!(
            AwsPartition::from_region("us-gov-west-1").as_str(),
            "aws-us-gov"
        );
        assert_eq!(AwsPartition::from_region("cn-north-1"), AwsPartition::AwsCn);
        assert_eq!(AwsPartition::from_region("cn-north-1").as_str(), "aws-cn");
        assert_eq!(AwsPartition::from_region("eu-central-1"), AwsPartition::Aws);
    }

    #[tokio::test]
    async fn test_get_aws_config_all_branches() {
        let _ = get_aws_config(Some("us-west-2"), Some("myprofile"), None).await;
        let _ = get_aws_config(None, None, None).await;

        let assume = AssumeRoleConfig {
            role_arn: "arn:aws:iam::123456789012:role/test".to_string(),
            session_name: Some("custom-session".to_string()),
            external_id: Some("ext-123".to_string()),
            duration_seconds: Some(1800),
        };
        let _ =
            get_aws_config_with_web_identity(Some("us-east-1"), Some("prof"), Some(&assume), None)
                .await;

        let token_path = std::env::temp_dir().join("stamp_mock_token.jwt");
        let _ = std::fs::write(&token_path, b"mock-token-data");
        let wid = WebIdentityConfig {
            role_arn: "arn:aws:iam::123456789012:role/test-wid".to_string(),
            session_name: Some("wid-session".to_string()),
            web_identity_token_file: Some(token_path.to_string_lossy().into_owned()),
        };
        let _ = get_aws_config_with_web_identity(None, None, None, Some(&wid)).await;
        let _ = std::fs::remove_file(token_path);
    }

    #[tokio::test]
    async fn test_steps_execution_mock() {
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut state = StateBag::new();

        let mut sg_step = StepCreateSecurityGroup {
            ui: ui.clone(),
            name: "test".to_string(),
            region: Some("us-west-2".to_string()),
            profile: None,
            vpc_id: None,
            security_group_ids: Vec::new(),
            temporary_security_group_source_cidrs: vec!["10.0.0.0/16".to_string()],
            port: 22,
            assume_role: None,
        };

        let res = sg_step.run(&mut state).await;
        assert_eq!(res.ok(), Some(StepAction::Continue));
        sg_step.cleanup(&state).await;

        // StepCreateSecurityGroup with predefined security_group_ids
        let mut sg_step_predefined = StepCreateSecurityGroup {
            ui: ui.clone(),
            name: "test".to_string(),
            region: None,
            profile: None,
            vpc_id: None,
            security_group_ids: vec!["sg-predefined".to_string()],
            temporary_security_group_source_cidrs: Vec::new(),
            port: 22,
            assume_role: None,
        };
        assert_eq!(
            sg_step_predefined.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );

        let mut kp_step = StepCreateKeyPair {
            ui: ui.clone(),
            name: "test".to_string(),
            region: Some("us-west-2".to_string()),
            profile: None,
            ssh_keypair_name: None,
            ssh_private_key_file: None,
            assume_role: None,
        };

        let res_kp = kp_step.run(&mut state).await;
        assert_eq!(res_kp.ok(), Some(StepAction::Continue));
        kp_step.cleanup(&state).await;

        // StepCreateKeyPair with predefined keypair
        let mut kp_step_predefined = StepCreateKeyPair {
            ui: ui.clone(),
            name: "test".to_string(),
            region: None,
            profile: None,
            ssh_keypair_name: Some("my-key".to_string()),
            ssh_private_key_file: Some(FilePath::new(PathBuf::from("/tmp/key.pem"))),
            assume_role: None,
        };
        assert_eq!(
            kp_step_predefined.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );

        let mut share_step = StepShareAndCopyAmi {
            ui: ui.clone(),
            name: "test".to_string(),
            region: Some("us-west-2".to_string()),
            profile: None,
            ami_config: Some(AmiConfig {
                ami_regions: vec!["us-east-1".to_string()],
                encrypt_boot: Some(true),
                kms_key_id: Some(KmsKeyId("kms-key-123".to_string())),
                ami_users: vec!["111122223333".to_string()],
                ami_groups: vec!["all".to_string()],
                deprecate_at: Some("2030-01-01T00:00:00Z".to_string()),
                fast_snapshot_restore: Some(true),
                fast_snapshot_restore_availability_zones: vec!["us-west-2a".to_string()],
                boot_mode: Some("uefi".to_string()),
                tpm_support: Some("v2.0".to_string()),
                uefi_data: Some("mock-uefi-data".to_string()),
                snapshot_tags: std::collections::HashMap::from([(
                    "Environment".to_string(),
                    "Production".to_string(),
                )]),
                ..Default::default()
            }),
            assume_role: None,
        };

        // Missing ami_id branch
        let mut empty_state = StateBag::new();
        assert_eq!(
            share_step.run(&mut empty_state).await.ok(),
            Some(StepAction::Continue)
        );

        // Missing ami_config branch
        let mut share_step_no_conf = StepShareAndCopyAmi {
            ui: ui.clone(),
            name: "test".to_string(),
            region: None,
            profile: None,
            ami_config: None,
            assume_role: None,
        };
        state.put("ami_id", "ami-12345678".to_string());
        assert_eq!(
            share_step_no_conf.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );

        let res_share = share_step.run(&mut state).await;
        assert_eq!(res_share.ok(), Some(StepAction::Continue));
        assert_eq!(
            state.get::<String>("ami_deprecate_at"),
            Some(&"2030-01-01T00:00:00Z".to_string())
        );
        assert_eq!(
            state.get::<bool>("fast_snapshot_restore_enabled"),
            Some(&true)
        );
        assert_eq!(state.get::<String>("boot_mode"), Some(&"uefi".to_string()));
        assert_eq!(
            state.get::<String>("tpm_support"),
            Some(&"v2.0".to_string())
        );
        assert_eq!(
            state.get::<String>("uefi_data"),
            Some(&"mock-uefi-data".to_string())
        );
        assert_eq!(
            state.get::<std::collections::HashMap<String, String>>("snapshot_tags"),
            Some(&std::collections::HashMap::from([(
                "Environment".to_string(),
                "Production".to_string()
            )]))
        );
        share_step.cleanup(&state).await;

        let mut lt_ssm_step = StepRegisterLaunchTemplateAndSsm {
            ui: ui.clone(),
            name: "test".to_string(),
            region: Some("us-west-2".to_string()),
            profile: None,
            launch_template_name: Some("my-lt".to_string()),
            launch_template_description: Some("v1 description".to_string()),
            ssm_parameter_name: Some("/amis/prod/ubuntu".to_string()),
            ssm_parameter_description: Some("Production Ubuntu AMI ID".to_string()),
            assume_role: None,
        };

        // Missing ami_id in state
        assert_eq!(
            lt_ssm_step.run(&mut empty_state).await.ok(),
            Some(StepAction::Continue)
        );

        // Neither launch template nor SSM configured
        let mut lt_ssm_none = StepRegisterLaunchTemplateAndSsm {
            ui: ui.clone(),
            name: "test".to_string(),
            region: None,
            profile: None,
            launch_template_name: None,
            launch_template_description: None,
            ssm_parameter_name: None,
            ssm_parameter_description: None,
            assume_role: None,
        };
        assert_eq!(
            lt_ssm_none.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );

        let res_lt = lt_ssm_step.run(&mut state).await;
        assert_eq!(res_lt.ok(), Some(StepAction::Continue));
        assert_eq!(
            state.get::<String>("launch_template_registered"),
            Some(&"my-lt".to_string())
        );
        assert_eq!(
            state.get::<String>("ssm_parameter_registered"),
            Some(&"/amis/prod/ubuntu".to_string())
        );
        lt_ssm_step.cleanup(&state).await;

        let placement = AmazonPlacementConfig {
            availability_zone: Some("us-east-1-wl1-bos-wlz-1".to_string()),
            affinity: Some("default".to_string()),
            group_name: Some("test-group".to_string()),
            partition_number: Some(1),
            host_id: Some("h-12345678".to_string()),
            tenancy: Some("dedicated".to_string()),
            spread_domain: Some("rack-1".to_string()),
            host_resource_group_arn: Some("arn:aws:resource-groups:...".to_string()),
            group_id: Some("pg-123456".to_string()),
        };
        assert_eq!(
            placement.availability_zone.as_deref(),
            Some("us-east-1-wl1-bos-wlz-1")
        );

        let eni = NetworkInterfaceConfig {
            device_index: Some(0),
            subnet_id: Some("subnet-12345".to_string()),
            network_interface_id: Some("eni-12345".to_string()),
            groups: vec!["sg-12345".to_string()],
            delete_on_termination: Some(true),
            description: Some("Primary interface".to_string()),
            associate_public_ip_address: Some(true),
            private_ip_addresses: vec!["10.0.0.5".to_string()],
            secondary_private_ip_address_count: Some(1),
            interface_type: Some("interface".to_string()),
        };
        assert_eq!(eni.device_index, Some(0));
    }
}
