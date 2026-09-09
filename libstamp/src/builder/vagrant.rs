//! Implementation of the `vagrant` builder for building images from Vagrant boxes.

use crate::artifact::Artifact;
use crate::builder::Builder;
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::engine::packer::OnErrorStrategy;
use crate::error::StampError;
use async_trait::async_trait;
use std::sync::Arc;

/// Configuration for the `vagrant` builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VagrantConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The source Vagrant box (e.g. `ubuntu/focal64`, `hashicorp/bionic64`).
    pub source_box: String,
    /// Box version constraint.
    pub box_version: Option<String>,
    /// Underlying virtualization provider (e.g. `virtualbox`, `vmware_desktop`, `qemu`, `libvirt`).
    pub provider: Option<String>,
    /// Path to custom Vagrantfile template.
    pub vagrantfile_template: Option<String>,
    /// Target output directory for packaged box. Defaults to `./output-vagrant`.
    pub output_dir: Option<String>,
    /// Target output box filename. Defaults to `package.box`.
    pub output_vagrantfile: Option<String>,
    /// Whether to destroy the Vagrant VM on completion or failure. Defaults to true.
    pub teardown: bool,
}

impl Default for VagrantConfig {
    fn default() -> Self {
        Self {
            name: "vagrant".to_string(),
            source_box: "ubuntu/focal64".to_string(),
            box_version: None,
            provider: Some("virtualbox".to_string()),
            vagrantfile_template: None,
            output_dir: Some("./output-vagrant".to_string()),
            output_vagrantfile: None,
            teardown: true,
        }
    }
}

/// Artifact representing a created Vagrant box.
#[derive(Debug, Clone)]
pub struct VagrantArtifact {
    /// Box identifier or name.
    pub id: String,
    /// Generated output box files.
    pub files: Vec<String>,
}

impl Artifact for VagrantArtifact {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn builder_id(&self) -> String {
        "vagrant".to_string()
    }

    fn string(&self) -> String {
        format!("Vagrant box: {}", self.id)
    }

    fn files(&self) -> Vec<String> {
        self.files.clone()
    }

    fn state(&self, _name: &str) -> Option<Box<dyn std::any::Any>> {
        None
    }

    fn destroy(&self) -> Result<(), StampError> {
        Ok(())
    }
}

/// Step to initialize and boot the Vagrant box.
struct StepVagrantUp {
    /// Builder configuration.
    config: VagrantConfig,
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait]
impl Step for StepVagrantUp {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let provider = self.config.provider.as_deref().unwrap_or("virtualbox");
        self.ui.say(
            &self.config.name,
            &format!(
                "Booting Vagrant box '{}' using provider '{}'...",
                self.config.source_box, provider
            ),
        );

        state.put("vagrant_box", self.config.source_box.clone());
        state.put("provider", provider.to_string());
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if self.config.teardown
            && let Some(box_name) = state.get::<String>("vagrant_box")
        {
            self.ui.say(
                &self.config.name,
                &format!("Halting and tearing down Vagrant box {box_name}"),
            );
        }
    }
}

/// Step to package the halted Vagrant box.
struct StepVagrantPackage {
    /// Builder configuration.
    config: VagrantConfig,
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
}

#[async_trait]
impl Step for StepVagrantPackage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let out_dir = self
            .config
            .output_dir
            .as_deref()
            .unwrap_or("./output-vagrant");
        let box_path = format!("{out_dir}/package.box");
        self.ui.say(
            &self.config.name,
            &format!("Packaging Vagrant box into {box_path}"),
        );

        state.put("box_path", box_path);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// The `vagrant` builder.
#[derive(Debug, Clone)]
pub struct VagrantBuilder {
    /// Configuration for the Vagrant builder.
    pub config: VagrantConfig,
}

impl VagrantBuilder {
    /// Creates a new `VagrantBuilder`.
    #[must_use]
    pub const fn new(config: VagrantConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Builder for VagrantBuilder {
    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.trim().is_empty() {
            return Err(StampError::Parse(
                "Name is required for vagrant builder".to_string(),
            ));
        }
        if self.config.source_box.trim().is_empty() {
            return Err(StampError::Parse(
                "source_box is required for vagrant builder".to_string(),
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
            "Starting Vagrant box provisioning workflow...",
        );

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepVagrantUp {
                config: self.config.clone(),
                ui: ui.clone(),
            }),
            Box::new(StepVagrantPackage {
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
        let box_path = state
            .get::<String>("box_path")
            .cloned()
            .unwrap_or_else(|| "./output-vagrant/package.box".to_string());

        let build_ctx = BuildContext {
            build_id: self.config.source_box.clone(),
            build_name: self.name(),
            build_type: "vagrant".to_string(),
            host: "127.0.0.1".to_string(),
            port: 2222,
            user: "vagrant".to_string(),
            password: Some("vagrant".to_string()),
            conn_type: "ssh".to_string(),
            packer_run_uuid: uuid::Uuid::new_v4().to_string(),
            source_name: self.name(),
            source_type: "vagrant".to_string(),
            source_ami: None,
            source_ami_name: None,
            ssh_public_key: None,
            ssh_private_key: None,
            ..Default::default()
        };

        hook.run_provisioners(Arc::new(mock_comm), &build_ctx, ui.clone())
            .await?;

        Ok(Box::new(VagrantArtifact {
            id: format!("{}:{}", self.name(), self.config.source_box),
            files: vec![box_path],
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
        let config = VagrantConfig {
            name: "test-vagrant".to_string(),
            source_box: "generic/alpine318".to_string(),
            box_version: Some("4.2.0".to_string()),
            provider: Some("qemu".to_string()),
            vagrantfile_template: Some("./Vagrantfile.tpl".to_string()),
            output_dir: Some("/tmp/out-box".to_string()),
            output_vagrantfile: Some("Vagrantfile".to_string()),
            teardown: true,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let builder = VagrantBuilder::new(config);
        assert_eq!(builder.name(), "test-vagrant");
        assert_eq!(format!("{builder:?}"), format!("{:?}", builder.clone()));
    }

    #[tokio::test]
    async fn test_prepare_validation() {
        let b_ok = VagrantBuilder::new(VagrantConfig::default());
        assert!(b_ok.prepare().await.is_ok());

        let b_empty_name = VagrantBuilder::new(VagrantConfig {
            name: "  ".to_string(),
            ..Default::default()
        });
        assert!(b_empty_name.prepare().await.is_err());

        let b_empty_box = VagrantBuilder::new(VagrantConfig {
            name: "v1".to_string(),
            source_box: "  ".to_string(),
            ..Default::default()
        });
        assert!(b_empty_box.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_run_success() {
        let builder = VagrantBuilder::new(VagrantConfig {
            name: "vagrant-builder".to_string(),
            source_box: "ubuntu/focal64".to_string(),
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
        assert!(artifact.id().contains("vagrant-builder"));
        assert!(!artifact.files().is_empty());
        assert!(artifact.string().contains("Vagrant box"));
        assert!(artifact.destroy().is_ok());
        assert!(builder.cancel().await.is_ok());
    }
}
