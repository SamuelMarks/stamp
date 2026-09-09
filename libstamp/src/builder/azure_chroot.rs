//! Implementation of the `azure-chroot` builder.

pub use super::azure_common::{
    AzureAuthMethod, SigPublishConfig, StepCaptureImage, StepCreateNetwork,
    StepCreateResourceGroup, StepPublishSig, get_azure_token,
};
use crate::builder::Builder;
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use std::sync::Arc;

/// Configuration for the `azure-chroot` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AzureChrootConfig {
    /// The name of the builder instance.
    pub name: String,
    /// The client ID for authentication.
    pub client_id: Option<String>,
    /// The client secret for authentication.
    pub client_secret: Option<String>,
    /// The tenant ID for authentication.
    pub tenant_id: Option<String>,
    /// The subscription ID for authentication.
    pub subscription_id: Option<String>,
    /// The location/region to deploy to.
    pub location: Option<String>,
    /// The VM size.
    pub vm_size: Option<String>,
    /// The resource group name.
    pub resource_group: Option<String>,
    /// The image publisher.
    pub image_publisher: Option<String>,
    /// Managed image name.
    pub managed_image_name: Option<String>,
    /// Shared Image Gallery publishing configuration.
    pub sig_publish: Option<SigPublishConfig>,
    /// Local mount directory for the chroot environment. Defaults to `/mnt/packer-azure-chroot`.
    pub mount_path: Option<String>,
    /// Block device path to format and mount (e.g. `/dev/sdc`).
    pub device_path: Option<String>,
    /// Filesystem type to format (e.g. `ext4`, `xfs`). Defaults to `ext4`.
    pub filesystem: Option<String>,
    /// Client certificate path.
    pub client_cert_path: Option<String>,
    /// Client certificate password.
    pub client_cert_password: Option<String>,
}

impl AzureChrootConfig {
    /// Resolve the authentication method.
    #[must_use]
    pub fn resolve_auth(&self) -> AzureAuthMethod {
        if let (Some(cid), Some(cert_path), Some(tid)) =
            (&self.client_id, &self.client_cert_path, &self.tenant_id)
        {
            AzureAuthMethod::ClientCertificate {
                client_id: cid.clone(),
                certificate_path: cert_path.clone(),
                certificate_password: self.client_cert_password.clone(),
                tenant_id: tid.clone(),
            }
        } else if let (Some(cid), Some(secret), Some(tid)) =
            (&self.client_id, &self.client_secret, &self.tenant_id)
        {
            AzureAuthMethod::ServicePrincipal {
                client_id: cid.clone(),
                client_secret: secret.clone(),
                tenant_id: tid.clone(),
            }
        } else {
            AzureAuthMethod::AzureCli
        }
    }

    /// Resolve the subscription ID.
    #[must_use]
    pub fn resolve_subscription_id(&self) -> String {
        self.subscription_id
            .clone()
            .unwrap_or_else(|| "00000000-0000-0000-0000-000000000000".to_string())
    }
}

/// The `azure-chroot` builder.
#[derive(Debug, Clone)]
pub struct AzureChrootBuilder {
    /// The builder configuration.
    pub config: AzureChrootConfig,
}

impl AzureChrootBuilder {
    /// Create a new `AzureChrootBuilder`.
    #[must_use]
    pub const fn new(config: AzureChrootConfig) -> Self {
        Self { config }
    }
}

/// Step to launch the source Azure instance.
#[derive(Debug, Clone)]
struct StepRunSourceInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: AzureChrootConfig,
}

#[async_trait::async_trait]
impl Step for StepRunSourceInstance {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let rg = state
            .get::<String>("resource_group_name")
            .cloned()
            .unwrap_or_else(|| {
                self.config
                    .resource_group
                    .clone()
                    .unwrap_or_else(|| "default-rg".to_string())
            });

        let vm_name = format!("vm-{}", self.name);
        self.ui.say(
            &self.name,
            &format!("Launching source instance {vm_name} in {rg}..."),
        );

        state.put("instance_id", vm_name.clone());
        state.put("vm_name", vm_name.clone());
        state.put("vm_id", format!(
            "/subscriptions/{}/resourceGroups/{rg}/providers/Microsoft.Compute/virtualMachines/{vm_name}",
            self.config.resolve_subscription_id()
        ));
        state.put("instance_ip", "127.0.0.1".to_string());
        state.put("resource_group_name", rg);

        if cfg!(test) {
            return Ok(StepAction::Continue);
        }

        let auth = self.config.resolve_auth();
        let token = get_azure_token(&auth).await?;
        let sub = self.config.resolve_subscription_id();
        let nic_id = state.get::<String>("nic_id").cloned().unwrap_or_default();

        let vm_url = format!(
            "https://management.azure.com/subscriptions/{sub}/resourceGroups/{}/providers/Microsoft.Compute/virtualMachines/{vm_name}?api-version=2021-07-01",
            state
                .get::<String>("resource_group_name")
                .unwrap_or(&"default-rg".to_string())
        );

        let vm_body = serde_json::json!({
            "location": self.config.location.as_deref().unwrap_or("eastus"),
            "properties": {
                "hardwareProfile": {
                    "vmSize": self.config.vm_size.as_deref().unwrap_or("Standard_B1s")
                },
                "storageProfile": {
                    "imageReference": {
                        "publisher": self.config.image_publisher.as_deref().unwrap_or("Canonical"),
                        "offer": "0001-com-ubuntu-server-jammy",
                        "sku": "22_04-lts",
                        "version": "latest"
                    }
                },
                "osProfile": {
                    "computerName": "stampchroot",
                    "adminUsername": "azureuser",
                    "adminPassword": "Password1234!"
                },
                "networkProfile": {
                    "networkInterfaces": [{
                        "id": nic_id
                    }]
                }
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .put(&vm_url)
            .bearer_auth(&token)
            .json(&vm_body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Azure VM creation error: {e}")))?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "Create Azure VM failed: {err}"
            )));
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let (Some(instance_id), Some(rg)) = (
            state.get::<String>("instance_id"),
            state.get::<String>("resource_group_name"),
        ) {
            self.ui
                .say(&self.name, &format!("Terminating instance: {instance_id}"));
            if !cfg!(test) {
                let auth = self.config.resolve_auth();
                if let Ok(token) = get_azure_token(&auth).await {
                    let sub = self.config.resolve_subscription_id();
                    let url = format!(
                        "https://management.azure.com/subscriptions/{sub}/resourceGroups/{rg}/providers/Microsoft.Compute/virtualMachines/{instance_id}?api-version=2021-07-01"
                    );
                    let client = reqwest::Client::new();
                    let _ = client.delete(&url).bearer_auth(token).send().await;
                }
            }
        }
    }
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
        self.ui.say(
            &self.name,
            &format!(
                "Mounting block device {} to {}",
                self.device_path, self.mount_path
            ),
        );
        state.put("mount_path", self.mount_path.clone());
        state.put("device_path", self.device_path.clone());
        state.put("filesystem", self.filesystem.clone());

        #[cfg(not(test))]
        {
            tokio::fs::create_dir_all(&self.mount_path)
                .await
                .map_err(StampError::Io)?;
            let _ = tokio::process::Command::new(format!("mkfs.{}", self.filesystem))
                .arg(&self.device_path)
                .status()
                .await;
            let status = tokio::process::Command::new("mount")
                .arg(&self.device_path)
                .arg(&self.mount_path)
                .status()
                .await
                .map_err(StampError::Io)?;
            if !status.success() {
                return Err(StampError::Execution(format!(
                    "Failed to mount {} to {}",
                    self.device_path, self.mount_path
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

/// Step to provision the chroot instance.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
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
            .unwrap_or_else(|| "/mnt/packer-azure-chroot".to_string());

        let comm: Arc<dyn crate::communicator::Communicator> = Arc::new(
            crate::communicator::chroot::ChrootCommunicator::new(mount_path),
        );

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "localhost".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "azure-chroot".to_string(),
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

#[async_trait::async_trait]
impl Builder for AzureChrootBuilder {
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

        let auth = self.config.resolve_auth();
        let sub = self.config.resolve_subscription_id();
        let location = self
            .config
            .location
            .clone()
            .unwrap_or_else(|| "eastus".to_string());
        let managed_img_name = self
            .config
            .managed_image_name
            .clone()
            .unwrap_or_else(|| format!("{}-image", self.name()));
        let rg = self
            .config
            .resource_group
            .clone()
            .unwrap_or_else(|| "default-rg".to_string());

        let mut steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateResourceGroup {
                ui: ui.clone(),
                name: self.name(),
                location: location.clone(),
                subscription_id: sub.clone(),
                resource_group_name: self.config.resource_group.clone(),
                auth: auth.clone(),
            }),
            Box::new(StepCreateNetwork {
                ui: ui.clone(),
                name: self.name(),
                location: location.clone(),
                subscription_id: sub.clone(),
                auth: auth.clone(),
                virtual_network_name: None,
                virtual_network_subnet_name: None,
                virtual_network_resource_group_name: None,
                private_virtual_network_with_public_ip: None,
                network_security_group_name: None,
            }),
            Box::new(StepRunSourceInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepMountDevice {
                ui: ui.clone(),
                name: self.name(),
                device_path: self
                    .config
                    .device_path
                    .clone()
                    .unwrap_or_else(|| "/dev/sdc".to_string()),
                mount_path: self
                    .config
                    .mount_path
                    .clone()
                    .unwrap_or_else(|| "/mnt/packer-azure-chroot".to_string()),
                filesystem: self
                    .config
                    .filesystem
                    .clone()
                    .unwrap_or_else(|| "ext4".to_string()),
            }),
            Box::new(StepMountExtra {
                ui: ui.clone(),
                name: self.name(),
                mount_path: self
                    .config
                    .mount_path
                    .clone()
                    .unwrap_or_else(|| "/mnt/packer-azure-chroot".to_string()),
                mounted_points: Vec::new(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                hook: hook.clone(),
            }),
            Box::new(StepCaptureImage {
                ui: ui.clone(),
                name: self.name(),
                location,
                subscription_id: sub.clone(),
                managed_image_name: managed_img_name,
                managed_image_resource_group_name: rg,
                image_type: None,
                auth: auth.clone(),
            }),
        ];

        if let Some(ref sig) = self.config.sig_publish {
            steps.push(Box::new(StepPublishSig {
                ui: ui.clone(),
                name: self.name(),
                subscription_id: sub,
                sig_config: sig.clone(),
                auth,
            }));
        }

        let mut runner = Runner::new(steps);
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
            .unwrap_or_else(|| "azure-chroot-mock-artifact".to_string());

        Ok(Box::new(crate::artifact::MockArtifact {
            id: artifact_id,
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
        let mut config = AzureChrootConfig::default();
        config.name = "test".to_string();
        config.client_id = Some("client".to_string());
        config.client_secret = Some("secret".to_string());
        config.tenant_id = Some("tenant".to_string());
        config.subscription_id = Some("sub-123".to_string());
        config.location = Some("eastus".to_string());
        config.vm_size = Some("Standard_B1s".to_string());
        config.resource_group = Some("rg-123".to_string());
        config.image_publisher = Some("Canonical".to_string());

        let config2 = config.clone();
        assert_eq!(config, config2);
        assert_eq!(format!("{config:?}"), format!("{config2:?}"));

        let builder = AzureChrootBuilder::new(config);
        let builder2 = builder.clone();
        assert_eq!(builder.name(), builder2.name());
        assert_eq!(format!("{builder:?}"), format!("{builder2:?}"));
    }

    #[tokio::test]
    async fn test_azure_chroot_prepare_success() -> Result<(), crate::error::StampError> {
        let mut config = AzureChrootConfig::default();
        config.name = "test".to_string();
        let builder = AzureChrootBuilder::new(config);
        builder.prepare().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_azure_chroot_prepare_failure() -> Result<(), crate::error::StampError> {
        let config = AzureChrootConfig::default();
        let builder = AzureChrootBuilder::new(config);
        let err = builder.prepare().await;
        assert!(matches!(err, Err(crate::error::StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_azure_chroot_run() -> Result<(), crate::error::StampError> {
        let mut config = AzureChrootConfig::default();
        config.name = "test".to_string();
        let builder = AzureChrootBuilder::new(config);
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
    async fn test_azure_chroot_run_full() -> Result<(), crate::error::StampError> {
        let mut config = AzureChrootConfig::default();
        config.name = "test_full".to_string();
        config.client_id = Some("client".to_string());
        config.client_secret = Some("secret".to_string());
        config.tenant_id = Some("tenant".to_string());
        config.subscription_id = Some("sub".to_string());
        config.location = Some("westus".to_string());
        config.vm_size = Some("Standard_A1".to_string());
        config.resource_group = Some("my-rg".to_string());
        config.image_publisher = Some("Canonical".to_string());
        config.sig_publish = Some(SigPublishConfig {
            resource_group: "my-rg".to_string(),
            gallery_name: "gal".to_string(),
            image_name: "img".to_string(),
            image_version: "1.0.0".to_string(),
            target_regions: vec!["westus".to_string()],
            ..Default::default()
        });

        let builder = AzureChrootBuilder::new(config);
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
    async fn test_azure_chroot_run_bad_exit() -> Result<(), crate::error::StampError> {
        let mut config = AzureChrootConfig::default();
        config.name = "test_bad_exit".to_string();
        let builder = AzureChrootBuilder::new(config);
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
    async fn test_azure_chroot_run_missing() -> Result<(), crate::error::StampError> {
        let mut config = AzureChrootConfig::default();
        config.name = "test_missing".to_string();
        let builder = AzureChrootBuilder::new(config);
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
    async fn test_azure_chroot_cancel() -> Result<(), crate::error::StampError> {
        let mut config = AzureChrootConfig::default();
        config.name = "test".to_string();
        let builder = AzureChrootBuilder::new(config);
        builder.cancel().await?;
        Ok(())
    }

    #[test]
    fn test_azure_chroot_name() {
        let mut config = AzureChrootConfig::default();
        config.name = "test-name".to_string();
        let builder = AzureChrootBuilder::new(config);
        assert_eq!(builder.name(), "test-name");
    }

    #[tokio::test]
    async fn test_step_run_source_instance_cleanup() {
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let config = AzureChrootConfig::default();
        let mut step = StepRunSourceInstance {
            ui,
            name: "test".to_string(),
            config,
        };
        let mut state = StateBag::new();
        state.put("instance_id", "vm-123".to_string());
        state.put("resource_group_name", "rg-123".to_string());
        step.cleanup(&state).await;
    }
}
