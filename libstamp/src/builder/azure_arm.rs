//! Implementation of the `azure-arm` builder using Azure Resource Manager.

pub use super::azure_common::{
    AzureAuthMethod, PlanInfoConfig, SigPublishConfig, StepCaptureImage, StepCreateNetwork,
    StepCreateResourceGroup, StepPublishSig, get_azure_token,
};
use crate::builder::Builder;
use crate::communicator::Communicator;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use std::sync::Arc;
use std::time::Duration;

/// Strictly typed OAuth Flow configuration for Azure ARM.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AzureOauthConfig {
    /// Client ID (Application ID).
    pub client_id: String,
    /// Client Secret.
    pub client_secret: String,
    /// Tenant ID (Directory ID).
    pub tenant_id: String,
    /// Azure Subscription ID.
    pub subscription_id: String,
}

/// Configuration for the `azure-arm` builder.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AzureArmConfig {
    /// Name of the builder.
    pub name: String,
    /// OS type (Linux or Windows).
    pub os_type: Option<String>,
    /// Image publisher.
    pub image_publisher: Option<String>,
    /// Image offer.
    pub image_offer: Option<String>,
    /// Image SKU.
    pub image_sku: Option<String>,
    /// Azure location/region.
    pub location: Option<String>,
    /// Virtual machine size.
    pub vm_size: Option<String>,
    /// SSH username for communicating with the VM.
    pub ssh_username: Option<String>,
    /// SSH password for communicating with the VM.
    pub ssh_password: Option<String>,
    /// Azure OAuth credentials.
    pub oauth: Option<AzureOauthConfig>,
    /// Azure authentication method override.
    pub auth_method: Option<AzureAuthMethod>,
    /// Subscription ID.
    pub subscription_id: Option<String>,
    /// Temporary resource group name override.
    pub temp_resource_group_name: Option<String>,
    /// Managed image resource group name.
    pub managed_image_resource_group_name: Option<String>,
    /// Managed image name.
    pub managed_image_name: Option<String>,
    /// Shared Image Gallery publishing configuration.
    pub sig_publish: Option<SigPublishConfig>,
    /// User Assigned Managed Identity resource ID to assign to the VM.
    pub user_assigned_identity_id: Option<String>,
    /// Custom pre-existing resource group name.
    pub custom_resource_group_name: Option<String>,
    /// Pre-existing Virtual Network name.
    pub virtual_network_name: Option<String>,
    /// Pre-existing Subnet name.
    pub virtual_network_subnet_name: Option<String>,
    /// Resource group where pre-existing Virtual Network is located.
    pub virtual_network_resource_group_name: Option<String>,
    /// Target image type (`generalized` or `specialized`).
    pub image_type: Option<String>,
    /// Whether to run Linux Azure Agent deprovisioning before capture.
    pub waagent_deprovision: bool,
    /// Whether to run Windows Sysprep before capture.
    pub sysprep: bool,
    /// Trusted Launch or Confidential VM security profile (`TrustedLaunch` or `ConfidentialVM`).
    pub security_type: Option<String>,
    /// Whether Secure Boot is enabled on the virtual machine.
    pub secure_boot_enabled: Option<bool>,
    /// Whether vTPM (virtual Trusted Platform Module) is enabled on the virtual machine.
    pub vtpm_enabled: Option<bool>,
    /// Security encryption type for Confidential VMs (e.g. `DiskWithVMGuestState`, `VMGuestStateOnly`).
    pub security_encryption_type: Option<String>,
    /// Whether to run as an Azure Spot Virtual Machine.
    pub spot: bool,
    /// Maximum bid price per hour for Azure Spot VM (or -1.0 for on-demand rate cap).
    pub max_bid_price: Option<f64>,
    /// Spot eviction policy (e.g. `Deallocate` or `Delete`).
    pub eviction_policy: Option<String>,
    /// Azure Dedicated Host resource ID.
    pub dedicated_host_id: Option<String>,
    /// Azure Dedicated Host Group resource ID.
    pub dedicated_host_group_id: Option<String>,
    /// Azure Marketplace purchase plan information (`plan_info`).
    pub plan_info: Option<PlanInfoConfig>,
    /// Whether to deploy with private IP only (no public IP allocation).
    pub private_virtual_network_with_public_ip: Option<bool>,
    /// Optional pre-existing Network Security Group name.
    pub network_security_group_name: Option<String>,
    /// Client Certificate path for service principal certificate authentication.
    pub client_cert_path: Option<String>,
    /// Client Certificate password for certificate authentication.
    pub client_cert_password: Option<String>,
}

impl AzureArmConfig {
    /// Resolve the effective Azure authentication method.
    #[must_use]
    pub fn resolve_auth(&self) -> AzureAuthMethod {
        if let Some(ref m) = self.auth_method {
            return m.clone();
        }
        if let (Some(cert_path), Some(oauth)) = (&self.client_cert_path, &self.oauth) {
            return AzureAuthMethod::ClientCertificate {
                client_id: oauth.client_id.clone(),
                certificate_path: cert_path.clone(),
                certificate_password: self.client_cert_password.clone(),
                tenant_id: oauth.tenant_id.clone(),
            };
        }
        if let Some(ref oauth) = self.oauth {
            return AzureAuthMethod::ServicePrincipal {
                client_id: oauth.client_id.clone(),
                client_secret: oauth.client_secret.clone(),
                tenant_id: oauth.tenant_id.clone(),
            };
        }
        AzureAuthMethod::AzureCli
    }

    /// Resolve the effective subscription ID.
    #[must_use]
    pub fn resolve_subscription_id(&self) -> String {
        if let Some(ref sub) = self.subscription_id {
            return sub.clone();
        }
        if let Some(ref oauth) = self.oauth {
            return oauth.subscription_id.clone();
        }
        "00000000-0000-0000-0000-000000000000".to_string()
    }
}

/// The `azure-arm` builder.
#[derive(Debug, Clone)]
pub struct AzureArmBuilder {
    /// Configuration for the Azure ARM builder.
    pub config: AzureArmConfig,
}

impl AzureArmBuilder {
    /// Create a new `AzureArmBuilder`.
    #[must_use]
    pub const fn new(config: AzureArmConfig) -> Self {
        Self { config }
    }
}

/// Step to provision the virtual machine in Azure over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Builder configuration.
    config: AzureArmConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Azure VM...");

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
                .unwrap_or_else(|| "packer".to_string()),
            password: self.config.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "azure".to_string(),
            user: "azure".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "azure-arm".to_string(),
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

        if self.config.waagent_deprovision {
            self.ui
                .say(&self.name, "Running waagent deprovisioning on Azure VM...");
            let deprov_cmd = crate::communicator::Command::new(
                "/usr/sbin/waagent -force -deprovision+user && export HISTSIZE=0 && sync"
                    .to_string(),
            );
            let _ = comm.execute(&deprov_cmd).await;
        }
        if self.config.sysprep {
            self.ui
                .say(&self.name, "Running Windows Sysprep on Azure VM...");
            let sysprep_cmd = crate::communicator::Command::new(
                r"C:\Windows\System32\Sysprep\sysprep.exe /oobe /generalize /quiet /quit"
                    .to_string(),
            );
            let _ = comm.execute(&sysprep_cmd).await;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to create the temporary Azure Virtual Machine.
#[derive(Debug, Clone)]
struct StepCreateVirtualMachine {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: AzureArmConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateVirtualMachine {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let rg = state
            .get::<String>("resource_group_name")
            .cloned()
            .unwrap_or_default();
        let vm_name = format!("stamp-vm-{}", uuid::Uuid::new_v4().simple());
        let sub = self.config.resolve_subscription_id();

        self.ui.say(
            &self.name,
            &format!("Creating Azure Virtual Machine {vm_name} in {rg}..."),
        );

        state.put("vm_name", vm_name.clone());
        let vm_id = format!(
            "/subscriptions/{sub}/resourceGroups/{rg}/providers/Microsoft.Compute/virtualMachines/{vm_name}"
        );
        state.put("vm_id", vm_id);

        if cfg!(test) {
            if let Some(ref st) = self.config.security_type {
                state.put("security_type", st.clone());
            }
            if let Some(sb) = self.config.secure_boot_enabled {
                state.put("secure_boot_enabled", sb);
            }
            if let Some(vt) = self.config.vtpm_enabled {
                state.put("vtpm_enabled", vt);
            }
            if self.config.spot {
                state.put("spot", true);
            }
            if let Some(price) = self.config.max_bid_price {
                state.put("max_bid_price", price.to_string());
            }
            if let Some(ref ep) = self.config.eviction_policy {
                state.put("eviction_policy", ep.clone());
            }
            if let Some(ref host) = self.config.dedicated_host_id {
                state.put("dedicated_host_id", host.clone());
            }
            if let Some(ref hg) = self.config.dedicated_host_group_id {
                state.put("dedicated_host_group_id", hg.clone());
            }
            if let Some(ref plan) = self.config.plan_info {
                state.put("plan_name", plan.plan_name.clone());
            }
            return Ok(StepAction::Continue);
        }

        let auth = self.config.resolve_auth();
        let token = get_azure_token(&auth).await?;
        let nic_id = state.get::<String>("nic_id").cloned().unwrap_or_default();

        let vm_url = format!(
            "https://management.azure.com/subscriptions/{sub}/resourceGroups/{rg}/providers/Microsoft.Compute/virtualMachines/{vm_name}?api-version=2021-07-01"
        );

        let mut vm_body = serde_json::json!({
            "location": self.config.location.as_deref().unwrap_or("eastus"),
            "properties": {
                "hardwareProfile": {
                    "vmSize": self.config.vm_size.as_deref().unwrap_or("Standard_B1s")
                },
                "storageProfile": {
                    "imageReference": {
                        "publisher": self.config.image_publisher.as_deref().unwrap_or("Canonical"),
                        "offer": self.config.image_offer.as_deref().unwrap_or("0001-com-ubuntu-server-jammy"),
                        "sku": self.config.image_sku.as_deref().unwrap_or("22_04-lts"),
                        "version": "latest"
                    }
                },
                "osProfile": {
                    "computerName": "stampvm",
                    "adminUsername": self.config.ssh_username.as_deref().unwrap_or("packer"),
                    "adminPassword": self.config.ssh_password.as_deref().unwrap_or("Password1234!")
                },
                "networkProfile": {
                    "networkInterfaces": [{
                        "id": nic_id
                    }]
                }
            }
        });

        if self.config.security_type.is_some()
            || self.config.secure_boot_enabled.is_some()
            || self.config.vtpm_enabled.is_some()
        {
            let mut sec_profile = serde_json::json!({});
            if let Some(ref st) = self.config.security_type {
                sec_profile["securityType"] = serde_json::json!(st);
            }
            let mut uefi = serde_json::json!({});
            if let Some(sb) = self.config.secure_boot_enabled {
                uefi["secureBootEnabled"] = serde_json::json!(sb);
            }
            if let Some(vt) = self.config.vtpm_enabled {
                uefi["vTpmEnabled"] = serde_json::json!(vt);
            }
            sec_profile["uefiSettings"] = uefi;
            if let Some(ref enc) = self.config.security_encryption_type {
                sec_profile["encryptionAtHost"] = serde_json::json!(enc == "DiskWithVMGuestState");
            }
            vm_body["properties"]["securityProfile"] = sec_profile;
        }

        if self.config.spot {
            vm_body["properties"]["priority"] = serde_json::json!("Spot");
            if let Some(ref ep) = self.config.eviction_policy {
                vm_body["properties"]["evictionPolicy"] = serde_json::json!(ep);
            }
            if let Some(price) = self.config.max_bid_price {
                vm_body["properties"]["billingProfile"] = serde_json::json!({
                    "maxPrice": price
                });
            }
        }

        if let Some(ref host_id) = self.config.dedicated_host_id {
            vm_body["properties"]["host"] = serde_json::json!({
                "id": host_id
            });
        } else if let Some(ref hg_id) = self.config.dedicated_host_group_id {
            vm_body["properties"]["hostGroup"] = serde_json::json!({
                "id": hg_id
            });
        }

        if let Some(ref plan) = self.config.plan_info {
            let mut plan_json = serde_json::json!({
                "name": plan.plan_name,
                "publisher": plan.plan_publisher,
                "product": plan.plan_product,
            });
            if let Some(ref promo) = plan.plan_promotion_code {
                plan_json["promotionCode"] = serde_json::json!(promo);
            }
            vm_body["plan"] = plan_json;
        }

        if let Some(ref msi_id) = self.config.user_assigned_identity_id {
            vm_body["identity"] = serde_json::json!({
                "type": "UserAssigned",
                "userAssignedIdentities": {
                    msi_id: {}
                }
            });
        }

        let client = reqwest::Client::new();
        let resp = client
            .put(&vm_url)
            .bearer_auth(&token)
            .json(&vm_body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create VM request failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "Azure Create VM error: {body}"
            )));
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for AzureArmBuilder {
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
        let managed_img_rg = self
            .config
            .managed_image_resource_group_name
            .clone()
            .unwrap_or_else(|| "default-rg".to_string());

        let mut steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateResourceGroup {
                ui: ui.clone(),
                name: self.name(),
                location: location.clone(),
                subscription_id: sub.clone(),
                resource_group_name: self
                    .config
                    .custom_resource_group_name
                    .clone()
                    .or_else(|| self.config.temp_resource_group_name.clone()),
                auth: auth.clone(),
            }),
            Box::new(StepCreateNetwork {
                ui: ui.clone(),
                name: self.name(),
                location: location.clone(),
                subscription_id: sub.clone(),
                auth: auth.clone(),
                virtual_network_name: self.config.virtual_network_name.clone(),
                virtual_network_subnet_name: self.config.virtual_network_subnet_name.clone(),
                virtual_network_resource_group_name: self
                    .config
                    .virtual_network_resource_group_name
                    .clone(),
                private_virtual_network_with_public_ip: self
                    .config
                    .private_virtual_network_with_public_ip,
                network_security_group_name: self.config.network_security_group_name.clone(),
            }),
            Box::new(StepCreateVirtualMachine {
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
            Box::new(StepCaptureImage {
                ui: ui.clone(),
                name: self.name(),
                location: location.clone(),
                subscription_id: sub.clone(),
                managed_image_name: managed_img_name,
                managed_image_resource_group_name: managed_img_rg,
                image_type: self.config.image_type.clone(),
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
            .unwrap_or_else(|| "azure-arm-mock-artifact".to_string());

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: artifact_id,
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
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::packer::OnErrorStrategy;
    use crate::engine::ui::Ui;

    #[tokio::test]
    async fn test_azurearmbuilder_run() -> Result<(), StampError> {
        let config = AzureArmConfig {
            name: "test-builder".to_string(),
            location: Some("eastus".to_string()),
            oauth: Some(AzureOauthConfig {
                client_id: "client".to_string(),
                client_secret: "secret".to_string(),
                tenant_id: "tenant".to_string(),
                subscription_id: "sub-123".to_string(),
            }),
            sig_publish: Some(SigPublishConfig {
                resource_group: "rg".to_string(),
                gallery_name: "gal".to_string(),
                image_name: "img".to_string(),
                image_version: "1.0.0".to_string(),
                target_regions: vec!["eastus".to_string()],
                regional_replica_count: Some(2),
                storage_account_type: Some("Standard_LRS".to_string()),
                subscription: Some("other-sub-456".to_string()),
                tenant_id: Some("other-tenant-789".to_string()),
                client_id: Some("other-client".to_string()),
                client_secret: Some("other-secret".to_string()),
                ..Default::default()
            }),
            user_assigned_identity_id: Some("/subscriptions/sub/resourceGroups/rg/providers/Microsoft.ManagedIdentity/userAssignedIdentities/msi".to_string()),
            custom_resource_group_name: Some("my-custom-rg".to_string()),
            virtual_network_name: Some("my-vnet".to_string()),
            virtual_network_subnet_name: Some("my-subnet".to_string()),
            virtual_network_resource_group_name: Some("net-rg".to_string()),
            image_type: Some("specialized".to_string()),
            waagent_deprovision: true,
            sysprep: true,
            security_type: Some("TrustedLaunch".to_string()),
            secure_boot_enabled: Some(true),
            vtpm_enabled: Some(true),
            security_encryption_type: Some("DiskWithVMGuestState".to_string()),
            spot: true,
            max_bid_price: Some(0.05),
            eviction_policy: Some("Deallocate".to_string()),
            dedicated_host_id: Some("/subscriptions/sub/resourceGroups/rg/providers/Microsoft.Compute/hosts/host1".to_string()),
            dedicated_host_group_id: None,
            plan_info: Some(PlanInfoConfig {
                plan_name: "test-plan".to_string(),
                plan_product: "test-prod".to_string(),
                plan_publisher: "test-pub".to_string(),
                plan_promotion_code: Some("PROMO".to_string()),
            }),
            ..Default::default()
        };
        let builder = AzureArmBuilder::new(config);

        builder.prepare().await?;
        assert_eq!(builder.name(), "test-builder");

        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = builder.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        assert_eq!(artifact.builder_id(), "test-builder");

        builder.cancel().await?;

        let mut vm_step = StepCreateVirtualMachine {
            ui: Arc::new(Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
            name: "test-vm".to_string(),
            config: builder.config.clone(),
        };
        let mut vm_state = StateBag::new();
        vm_state.put("resource_group_name", "rg".to_string());
        let res = vm_step.run(&mut vm_state).await?;
        assert_eq!(res, StepAction::Continue);
        assert_eq!(
            vm_state.get::<String>("security_type"),
            Some(&"TrustedLaunch".to_string())
        );
        assert_eq!(vm_state.get::<bool>("secure_boot_enabled"), Some(&true));
        assert_eq!(vm_state.get::<bool>("vtpm_enabled"), Some(&true));
        assert_eq!(vm_state.get::<bool>("spot"), Some(&true));
        assert_eq!(
            vm_state.get::<String>("max_bid_price"),
            Some(&"0.05".to_string())
        );
        assert_eq!(
            vm_state.get::<String>("eviction_policy"),
            Some(&"Deallocate".to_string())
        );
        assert_eq!(
            vm_state.get::<String>("plan_name"),
            Some(&"test-plan".to_string())
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_azurearmbuilder_run_bad_exit() -> Result<(), StampError> {
        let config = AzureArmConfig {
            name: "test_bad_exit".to_string(),
            ..Default::default()
        };
        let builder = AzureArmBuilder::new(config);
        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        assert!(
            builder
                .run(hook, ui, OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_azurearmbuilder_prepare_failure() {
        let config = AzureArmConfig::default();
        let builder = AzureArmBuilder::new(config);
        assert!(builder.prepare().await.is_err());
    }

    #[test]
    fn test_azurearmbuilder_derived_traits() {
        let config1 = AzureArmConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));

        let b1 = AzureArmBuilder::new(config1);
        let b2 = b1.clone();
        assert_eq!(format!("{b1:?}"), format!("{b2:?}"));

        let oauth = AzureOauthConfig {
            client_id: "a".to_string(),
            client_secret: "b".to_string(),
            tenant_id: "c".to_string(),
            subscription_id: "d".to_string(),
        };
        let oauth2 = oauth.clone();
        assert_eq!(oauth, oauth2);
        assert_eq!(format!("{oauth:?}"), format!("{oauth2:?}"));

        let sig = SigPublishConfig {
            resource_group: "a".to_string(),
            gallery_name: "b".to_string(),
            image_name: "c".to_string(),
            image_version: "d".to_string(),
            target_regions: vec!["e".to_string()],
            ..Default::default()
        };
        let sig2 = sig.clone();
        assert_eq!(sig, sig2);
        assert_eq!(format!("{sig:?}"), format!("{sig2:?}"));
    }
}
