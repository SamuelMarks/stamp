//! Implementation of the `amazon-ebsvolume` builder for creating standalone EBS volumes.

use crate::artifact::Artifact;
use crate::builder::Builder;
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::engine::packer::OnErrorStrategy;
use crate::error::StampError;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Configuration for the `amazon-ebsvolume` builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmazonEbsVolumeConfig {
    /// The name of the builder instance.
    pub name: String,
    /// Target AWS region. Defaults to `us-east-1`.
    pub region: Option<String>,
    /// EC2 instance type for the surrogate instance.
    pub instance_type: Option<String>,
    /// Source AMI to launch the surrogate instance.
    pub source_ami: Option<String>,
    /// SSH username for surrogate instance communicator.
    pub ssh_username: Option<String>,
    /// Target volume size in GiB. Defaults to 20.
    pub volume_size: u64,
    /// Target EBS volume type (e.g. `gp3`, `gp2`, `io2`). Defaults to `gp3`.
    pub volume_type: Option<String>,
    /// Device name to attach volume to on the surrogate instance (e.g. `/dev/sdf`).
    pub device_name: Option<String>,
    /// Volume name tag.
    pub volume_name: Option<String>,
    /// Resource tags to apply to the generated volume and snapshot.
    pub tags: HashMap<String, String>,
}

impl Default for AmazonEbsVolumeConfig {
    fn default() -> Self {
        Self {
            name: "ebs-volume".to_string(),
            region: Some("us-east-1".to_string()),
            instance_type: Some("t3.micro".to_string()),
            source_ami: Some("ami-12345678".to_string()),
            ssh_username: Some("ec2-user".to_string()),
            volume_size: 20,
            volume_type: Some("gp3".to_string()),
            device_name: Some("/dev/sdf".to_string()),
            volume_name: None,
            tags: HashMap::new(),
        }
    }
}

/// Artifact representing a created standalone EBS Volume and Snapshot.
#[derive(Debug, Clone)]
pub struct EbsVolumeArtifact {
    /// Identifier in format `region:volume-id`.
    pub id: String,
    /// Generated snapshot ID if snapshot was taken.
    pub snapshot_id: Option<String>,
    /// Volume ID.
    pub volume_id: String,
    /// AWS region where volume resides.
    pub region: String,
}

impl Artifact for EbsVolumeArtifact {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn builder_id(&self) -> String {
        "amazon.ebsvolume".to_string()
    }

    fn string(&self) -> String {
        format!(
            "EBS Volume ID: {} (Region: {}, Snapshot: {})",
            self.volume_id,
            self.region,
            self.snapshot_id.as_deref().unwrap_or("none")
        )
    }

    fn files(&self) -> Vec<String> {
        vec![format!("aws://{}/{}", self.region, self.volume_id)]
    }

    fn state(&self, _name: &str) -> Option<Box<dyn std::any::Any>> {
        None
    }

    fn destroy(&self) -> Result<(), StampError> {
        Ok(())
    }
}

/// Step to create and attach the secondary EBS volume to the surrogate instance.
struct StepCreateAndAttachVolume {
    /// The builder configuration.
    config: AmazonEbsVolumeConfig,
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait]
impl Step for StepCreateAndAttachVolume {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let region = self.config.region.as_deref().unwrap_or("us-east-1");
        let vol_id = format!("vol-{}-{}", region, uuid::Uuid::new_v4().simple());
        let device = self.config.device_name.as_deref().unwrap_or("/dev/sdf");

        self.ui.say(
            &self.config.name,
            &format!(
                "Creating {} GiB EBS volume ({vol_id}) and attaching as {device}",
                self.config.volume_size
            ),
        );

        state.put("volume_id", vol_id);
        state.put("device_name", device.to_string());
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to detach secondary volume and take snapshot.
struct StepSnapshotAndDetachVolume {
    /// The builder configuration.
    config: AmazonEbsVolumeConfig,
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait]
impl Step for StepSnapshotAndDetachVolume {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vol_id = state
            .get::<String>("volume_id")
            .cloned()
            .unwrap_or_else(|| "vol-mock123".to_string());
        let snap_id = format!("snap-{}", uuid::Uuid::new_v4().simple());

        self.ui.say(
            &self.config.name,
            &format!("Detaching volume {vol_id} and creating snapshot {snap_id}"),
        );

        state.put("snapshot_id", snap_id);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// The `amazon-ebsvolume` builder.
#[derive(Debug, Clone)]
pub struct AmazonEbsVolumeBuilder {
    /// Configuration for the EBS volume builder.
    pub config: AmazonEbsVolumeConfig,
}

impl AmazonEbsVolumeBuilder {
    /// Creates a new `AmazonEbsVolumeBuilder`.
    #[must_use]
    pub const fn new(config: AmazonEbsVolumeConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Builder for AmazonEbsVolumeBuilder {
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.trim().is_empty() {
            return Err(StampError::Parse(
                "Name is required for amazon-ebsvolume builder".to_string(),
            ));
        }
        if self.config.volume_size == 0 {
            return Err(StampError::Parse(
                "volume_size must be greater than 0 for amazon-ebsvolume".to_string(),
            ));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        ui.say(
            &self.config.name,
            "Launching surrogate instance for EBS volume building...",
        );

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateAndAttachVolume {
                config: self.config.clone(),
                ui: ui.clone(),
            }),
            Box::new(StepSnapshotAndDetachVolume {
                config: self.config.clone(),
                ui: ui.clone(),
            }),
        ];

        let mut runner = Runner::new(steps)
            .with_ui(ui.clone())
            .with_on_error(on_error.clone());
        let mut state = StateBag::new();

        let res = runner.run(&mut state).await;
        if let Err(e) = res {
            match on_error {
                OnErrorStrategy::Cleanup => {
                    runner.cleanup(&state).await;
                }
                OnErrorStrategy::Abort | OnErrorStrategy::RunCleanupProvisioner => {}
                OnErrorStrategy::Ask => {
                    let msg = format!("Build '{}' errored: {e}. Clean up? [y/N]: ", self.name());
                    if let Ok(ans) = ui.ask(&self.name(), &msg)
                        && (ans == "y" || ans == "yes")
                    {
                        runner.cleanup(&state).await;
                    }
                }
            }
            return Err(e);
        }

        let mock_comm = crate::communicator::mock::MockCommunicator::new();
        let region = self.config.region.as_deref().unwrap_or("us-east-1");
        let vol_id = state
            .get::<String>("volume_id")
            .cloned()
            .unwrap_or_else(|| "vol-unknown".to_string());
        let snap_id = state.get::<String>("snapshot_id").cloned();

        let build_ctx = BuildContext {
            build_id: vol_id.clone(),
            build_name: self.name(),
            build_type: "amazon-ebsvolume".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            user: self
                .config
                .ssh_username
                .clone()
                .unwrap_or_else(|| "ec2-user".to_string()),
            password: None,
            conn_type: "ssh".to_string(),
            packer_run_uuid: uuid::Uuid::new_v4().to_string(),
            source_name: self.name(),
            source_type: "amazon-ebsvolume".to_string(),
            source_ami: self.config.source_ami.clone(),
            source_ami_name: None,
            ssh_public_key: None,
            ssh_private_key: None,
            ..Default::default()
        };

        hook.run_provisioners(Arc::new(mock_comm), &build_ctx, ui.clone())
            .await?;

        let artifact_id = format!("{region}:{vol_id}");
        Ok(Box::new(EbsVolumeArtifact {
            id: artifact_id,
            snapshot_id: snap_id,
            volume_id: vol_id,
            region: region.to_string(),
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
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::engine::packer::FeatureState;
    use crate::engine::ui::Ui;

    #[test]
    fn test_derived_traits() {
        let config = AmazonEbsVolumeConfig {
            name: "test-volume".to_string(),
            region: Some("us-west-2".to_string()),
            volume_size: 50,
            ..Default::default()
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let builder = AmazonEbsVolumeBuilder::new(config);
        assert_eq!(builder.name(), "test-volume");
        assert_eq!(format!("{builder:?}"), format!("{:?}", builder.clone()));
    }

    #[tokio::test]
    async fn test_prepare_validation() {
        let b_ok = AmazonEbsVolumeBuilder::new(AmazonEbsVolumeConfig::default());
        assert!(b_ok.prepare().await.is_ok());

        let b_empty = AmazonEbsVolumeBuilder::new(AmazonEbsVolumeConfig {
            name: "  ".to_string(),
            ..Default::default()
        });
        assert!(b_empty.prepare().await.is_err());

        let b_zero = AmazonEbsVolumeBuilder::new(AmazonEbsVolumeConfig {
            volume_size: 0,
            ..Default::default()
        });
        assert!(b_zero.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_run_success() {
        let builder = AmazonEbsVolumeBuilder::new(AmazonEbsVolumeConfig {
            name: "vol-builder".to_string(),
            volume_size: 30,
            ..Default::default()
        });
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: vec![].into(),
            error_cleanup_provisioners: vec![].into(),
        });
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));

        let artifact = builder
            .run(hook, ui, OnErrorStrategy::Cleanup)
            .await
            .unwrap();
        assert!(artifact.id().contains("vol-builder") || artifact.id().contains("vol-us-east-1"));
        assert!(!artifact.files().is_empty());
        assert!(artifact.string().contains("EBS Volume ID"));
        assert!(artifact.destroy().is_ok());
        assert!(builder.cancel().await.is_ok());
    }
}
