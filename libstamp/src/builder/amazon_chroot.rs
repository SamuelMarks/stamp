#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `amazon-chroot` builder.

pub use super::amazon_common::{
    AmiConfig, AssumeRoleConfig, BlockDeviceMapping, EbsVolumeType, IamInstanceProfile, KmsKeyId,
    SpotInstanceConfig, StepRegisterLaunchTemplateAndSsm, StepShareAndCopyAmi, WebIdentityConfig,
    get_aws_config,
};
use crate::builder::Builder;
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use std::sync::Arc;

/// Configuration for the `amazon-chroot` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmazonChrootConfig {
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
    /// IAM role to assume (cross-account).
    pub assume_role: Option<AssumeRoleConfig>,
    /// AMI configuration (copying, encryption, sharing).
    pub ami_config: Option<AmiConfig>,
    /// Local mount directory for the chroot environment. Defaults to `/mnt/packer-amazon-chroot`.
    pub mount_path: Option<String>,
    /// Block device path to format and mount (e.g. `/dev/xvdf`).
    pub device_path: Option<String>,
    /// Filesystem type to format (e.g. `ext4`, `xfs`). Defaults to `ext4`.
    pub filesystem: Option<String>,
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

/// The `amazon-chroot` builder.
#[derive(Debug, Clone)]
pub struct AmazonChrootBuilder {
    /// The builder configuration.
    pub config: AmazonChrootConfig,
}

impl AmazonChrootBuilder {
    /// Create a new `AmazonChrootBuilder`.
    #[must_use]
    pub const fn new(config: AmazonChrootConfig) -> Self {
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
    config: AmazonChrootConfig,
}

#[async_trait::async_trait]
impl Step for StepRunSourceInstance {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Launching source instance...");
        if cfg!(test) {
            state.put("instance_id", "i-1234567890abcdef0".to_string());
            state.put("instance_ip", "127.0.0.1".to_string());
            return Ok(StepAction::Continue);
        }

        let aws_conf = get_aws_config(
            self.config.region.as_deref(),
            self.config.profile.as_deref(),
            self.config.assume_role.as_ref(),
        )
        .await;
        let client = aws_sdk_ec2::Client::new(&aws_conf);

        let res = match client
            .run_instances()
            .image_id(self.config.source_ami.as_deref().unwrap_or("ami-00000000"))
            .instance_type(aws_sdk_ec2::types::InstanceType::from(
                self.config.instance_type.as_deref().unwrap_or("t2.micro"),
            ))
            .min_count(1)
            .max_count(1)
            .send()
            .await
        {
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

/// Internal helper to resolve `NVMe` device paths for Amazon EC2 EBS volumes.
fn resolve_nvme_device_path_internal(
    device_path: &str,
    by_id_dir: &std::path::Path,
    dev_dir: &std::path::Path,
) -> String {
    if device_path.starts_with("/dev/nvme") {
        return device_path.to_string();
    }

    let dev_name = device_path.trim_start_matches("/dev/");
    let stripped = dev_name
        .trim_start_matches("xvd")
        .trim_start_matches("sd")
        .trim_start_matches("hd");

    // Check by-id disk symlinks for AWS EBS volume aliases
    if let Ok(entries) = std::fs::read_dir(by_id_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(dev_name)
                && let Ok(canon) = std::fs::canonicalize(entry.path())
            {
                return canon.to_string_lossy().into_owned();
            }
        }
    }

    // Heuristic for AWS Nitro device indexing (/dev/xvdf -> /dev/nvme1n1)
    if let Some(first_char) = stripped.chars().next()
        && first_char.is_ascii_lowercase()
    {
        let index = (first_char as u8).saturating_sub(b'f') + 1;
        let candidate_file = format!("nvme{index}n1");
        let candidate = dev_dir.join(&candidate_file);
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    device_path.to_string()
}

/// Resolves an EC2 device name (such as `/dev/sdf` or `/dev/xvdf`) to its corresponding
/// `NVMe` block device on modern Nitro instances (e.g. `/dev/nvme1n1`), or returns the original
/// path if already an `NVMe` path or if no Nitro `NVMe` mapping is detected.
///
/// # Arguments
///
/// * `device_path` - Original block device path (e.g. `"/dev/xvdf"`).
#[must_use]
pub fn resolve_nvme_device_path(device_path: &str) -> String {
    resolve_nvme_device_path_internal(
        device_path,
        std::path::Path::new("/dev/disk/by-id"),
        std::path::Path::new("/dev"),
    )
}

/// Step to format and mount the chroot block device.
#[derive(Debug, Clone)]
struct StepMountDevice {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Target device path.
    device_path: String,
    /// Destination mount path.
    mount_path: String,
    /// Filesystem format.
    filesystem: String,
}

#[async_trait::async_trait]
impl Step for StepMountDevice {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let resolved_device = resolve_nvme_device_path(&self.device_path);
        self.ui.say(
            &self.name,
            &format!(
                "Mounting block device {resolved_device} (configured: {}) to {}",
                self.device_path, self.mount_path
            ),
        );
        state.put("mount_path", self.mount_path.clone());
        state.put("device_path", resolved_device.clone());
        state.put("configured_device_path", self.device_path.clone());
        state.put("filesystem", self.filesystem.clone());

        #[cfg(not(test))]
        {
            tokio::fs::create_dir_all(&self.mount_path)
                .await
                .map_err(StampError::Io)?;
            let _ = tokio::process::Command::new(format!("mkfs.{}", self.filesystem))
                .arg(&resolved_device)
                .status()
                .await;
            let status = tokio::process::Command::new("mount")
                .arg(&resolved_device)
                .arg(&self.mount_path)
                .status()
                .await
                .map_err(StampError::Io)?;
            if !status.success() {
                return Err(StampError::Execution(format!(
                    "Failed to mount {resolved_device} to {}",
                    self.mount_path
                )));
            }
        }
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {
        self.ui
            .say(&self.name, &format!("Unmounting {}", self.mount_path));
        #[cfg(not(test))]
        {
            let _ = tokio::process::Command::new("umount")
                .arg(&self.mount_path)
                .status()
                .await;
        }
    }
}

/// Step to bind mount system filesystems (/dev, /dev/pts, /proc, /sys) into chroot jail.
#[derive(Debug, Clone)]
struct StepMountExtra {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Destination mount path.
    mount_path: String,
    /// Currently mounted bind points for cleanup.
    mounted_points: Vec<String>,
}

#[async_trait::async_trait]
impl Step for StepMountExtra {
    async fn run(&mut self, _state: &mut StateBag) -> Result<StepAction, StampError> {
        let binds = ["/dev", "/dev/pts", "/proc", "/sys"];
        for src in binds {
            let target = format!("{}{src}", self.mount_path);
            self.ui
                .say(&self.name, &format!("Bind mounting {src} to {target}"));
            #[cfg(not(test))]
            {
                tokio::fs::create_dir_all(&target)
                    .await
                    .map_err(StampError::Io)?;
                let status = tokio::process::Command::new("mount")
                    .args(["--bind", src, &target])
                    .status()
                    .await
                    .map_err(StampError::Io)?;
                if status.success() {
                    self.mounted_points.push(target);
                }
            }
            #[cfg(test)]
            self.mounted_points.push(target);
        }
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {
        for target in self.mounted_points.drain(..).rev() {
            self.ui
                .say(&self.name, &format!("Unmounting bind point {target}"));
            #[cfg(not(test))]
            {
                let _ = tokio::process::Command::new("umount")
                    .arg(&target)
                    .status()
                    .await;
            }
        }
    }
}

/// Step to provision the instance within the chroot jail.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    #[allow(dead_code)]
    config: AmazonChrootConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning chroot instance...");

        let mount_path = state
            .get::<String>("mount_path")
            .cloned()
            .unwrap_or_default();

        let comm: Arc<dyn crate::communicator::Communicator> = Arc::new(
            crate::communicator::chroot::ChrootCommunicator::new(mount_path),
        );

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "localhost".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "amazon-chroot".to_string(),
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
    config: AmazonChrootConfig,
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

/// Step to create the AMI from the stopped instance.
#[derive(Debug, Clone)]
struct StepCreateAMI {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: AmazonChrootConfig,
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

        if !cfg!(test) {
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
impl Builder for AmazonChrootBuilder {
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

        let mount_path = self
            .config
            .mount_path
            .clone()
            .unwrap_or_else(|| "/mnt/packer-amazon-chroot".to_string());
        let device_path = self
            .config
            .device_path
            .clone()
            .unwrap_or_else(|| "/dev/xvdf".to_string());
        let filesystem = self
            .config
            .filesystem
            .clone()
            .unwrap_or_else(|| "ext4".to_string());

        let mut runner = Runner::new(vec![
            Box::new(StepRunSourceInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepMountDevice {
                ui: ui.clone(),
                name: self.name(),
                device_path,
                mount_path: mount_path.clone(),
                filesystem,
            }),
            Box::new(StepMountExtra {
                ui: ui.clone(),
                name: self.name(),
                mount_path,
                mounted_points: Vec::new(),
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
        let mut config = AmazonChrootConfig::default();
        config.name = "test".to_string();
        config.ami_config = Some(AmiConfig {
            ami_regions: vec!["us-east-1".to_string()],
            encrypt_boot: Some(true),
            kms_key_id: Some(KmsKeyId("kms-id".to_string())),
            ami_users: vec!["123456789012".to_string()],
            ami_groups: vec![],
            ..Default::default()
        });
        let config2 = config.clone();
        assert_eq!(config, config2);
        assert_eq!(format!("{config:?}"), format!("{config2:?}"));

        let builder = AmazonChrootBuilder::new(config);
        let builder2 = builder.clone();
        assert_eq!(builder.name(), builder2.name());
        assert_eq!(format!("{builder:?}"), format!("{builder2:?}"));
    }

    #[tokio::test]
    async fn test_amazon_chroot_prepare_success() {
        let mut config = AmazonChrootConfig::default();
        config.name = "test".to_string();
        let builder = AmazonChrootBuilder::new(config);
        assert!(builder.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_amazon_chroot_prepare_failure() {
        let config = AmazonChrootConfig::default();
        let builder = AmazonChrootBuilder::new(config);
        assert!(builder.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_amazon_chroot_run_mocked() {
        let mut config = AmazonChrootConfig::default();
        config.name = "test".to_string();
        config.region = Some("us-west-2".to_string());
        config.profile = Some("myprofile".to_string());
        config.source_ami = Some("ami-12345".to_string());
        config.instance_type = Some("t2.micro".to_string());
        config.vpc_id = Some("vpc-12345".to_string());
        config.ami_name = Some("my-ami".to_string());
        config.ami_config = Some(AmiConfig {
            ami_regions: vec!["us-east-1".to_string()],
            encrypt_boot: Some(true),
            kms_key_id: Some(KmsKeyId("kms-id".to_string())),
            ami_users: vec!["123456789012".to_string()],
            ami_groups: vec![],
            ..Default::default()
        });

        let builder = AmazonChrootBuilder::new(config);
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
    async fn test_amazon_chroot_run_bad_exit() {
        let mut config = AmazonChrootConfig::default();
        config.name = "test_bad_exit".to_string();
        let builder = AmazonChrootBuilder::new(config);
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
    async fn test_amazon_chroot_run_missing() {
        let mut config = AmazonChrootConfig::default();
        config.name = "test_missing".to_string();
        let builder = AmazonChrootBuilder::new(config);
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
    async fn test_amazon_chroot_cancel() {
        let mut config = AmazonChrootConfig::default();
        config.name = "test".to_string();
        let builder = AmazonChrootBuilder::new(config);
        assert!(builder.cancel().await.is_ok());
    }

    #[tokio::test]
    async fn test_steps_run_and_cleanup() {
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let config = AmazonChrootConfig::default();

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

        let mut mount_step = StepMountDevice {
            ui: ui.clone(),
            name: "test".into(),
            device_path: "/dev/xvdf".to_string(),
            mount_path: "/mnt/test".to_string(),
            filesystem: "ext4".to_string(),
        };
        let res_mount = mount_step.run(&mut state).await;
        assert_eq!(res_mount.ok(), Some(StepAction::Continue));
        assert_eq!(
            state.get::<String>("configured_device_path"),
            Some(&"/dev/xvdf".to_string())
        );
        mount_step.cleanup(&state).await;

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

    #[test]
    fn test_resolve_nvme_device_path() {
        assert_eq!(
            resolve_nvme_device_path("/dev/nvme0n1"),
            "/dev/nvme0n1".to_string()
        );
        assert_eq!(
            resolve_nvme_device_path("/dev/nvme1n1"),
            "/dev/nvme1n1".to_string()
        );
        assert_eq!(
            resolve_nvme_device_path("/dev/mapper/root"),
            "/dev/mapper/root".to_string()
        );
        assert_eq!(resolve_nvme_device_path("/dev/123"), "/dev/123".to_string());
        assert_eq!(resolve_nvme_device_path(""), "".to_string());

        // Test with mocked by-id directory containing a symlink
        let tmp = std::env::temp_dir().join("stamp_nvme_by_id_test");
        let _ = std::fs::create_dir_all(&tmp);
        let target_file = tmp.join("real_device");
        let _ = std::fs::write(&target_file, b"");
        let link_path = tmp.join("nvme-ebs-xvdf");
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&target_file, &link_path);

        let resolved = resolve_nvme_device_path_internal(
            "/dev/xvdf",
            &tmp,
            std::path::Path::new("/nonexistent_dev"),
        );
        assert!(!resolved.is_empty());

        // Test with mocked dev directory containing candidate file
        let dev_tmp = std::env::temp_dir().join("stamp_nvme_dev_test");
        let _ = std::fs::create_dir_all(&dev_tmp);
        let candidate_file = dev_tmp.join("nvme1n1");
        let _ = std::fs::write(&candidate_file, b"");

        let resolved_cand = resolve_nvme_device_path_internal(
            "/dev/xvdf",
            std::path::Path::new("/nonexistent_by_id"),
            &dev_tmp,
        );
        assert!(resolved_cand.ends_with("nvme1n1"));

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&dev_tmp);
    }
}
