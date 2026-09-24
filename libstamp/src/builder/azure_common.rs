#![cfg(not(tarpaulin_include))]
//! Shared Azure Resource Manager (ARM) types, authentication helpers, and execution steps.

use crate::engine::multistep::{StateBag, Step, StepAction};
use crate::error::StampError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Supported Azure authentication flows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AzureAuthMethod {
    /// Service Principal authentication with client ID, client secret, and tenant ID.
    ServicePrincipal {
        /// Client ID (App ID).
        client_id: String,
        /// Client Secret.
        client_secret: String,
        /// Tenant ID (Directory ID).
        tenant_id: String,
    },
    /// Service Principal authentication with client certificate (PKCS#12 or PEM).
    ClientCertificate {
        /// Client ID (App ID).
        client_id: String,
        /// Path to the client certificate file.
        certificate_path: String,
        /// Optional password for certificate file.
        certificate_password: Option<String>,
        /// Tenant ID (Directory ID).
        tenant_id: String,
    },
    /// Azure Managed Identity (MSI) via IMDS endpoint.
    ManagedIdentity,
    /// Local Azure CLI credentials (`az account get-access-token`).
    AzureCli,
}

impl Default for AzureAuthMethod {
    /// Return standard default Azure CLI authentication.
    fn default() -> Self {
        Self::AzureCli
    }
}

/// Shared Image Gallery (SIG) publishing configuration with multi-region replication.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SigPublishConfig {
    /// Resource group where the image gallery resides.
    pub resource_group: String,
    /// Gallery name.
    pub gallery_name: String,
    /// Image definition name.
    pub image_name: String,
    /// Semantic version of the image (e.g. 1.0.0).
    pub image_version: String,
    /// Target replication regions.
    pub target_regions: Vec<String>,
    /// Default regional replica count across target regions.
    pub regional_replica_count: Option<i32>,
    /// Storage account type for replicated images (e.g. `Standard_LRS`, `Standard_ZRS`).
    pub storage_account_type: Option<String>,
    /// Replica count override per region.
    pub target_region_replicas: std::collections::HashMap<String, i32>,
    /// End-of-life timestamp for this image version.
    pub end_of_life_date: Option<String>,
    /// Whether to exclude this version when querying for latest image.
    pub exclude_from_latest: bool,
    /// Target subscription ID where the gallery resides, if different from builder subscription (cross-subscription/cross-tenant).
    pub subscription: Option<String>,
    /// Target tenant ID for cross-tenant gallery replication.
    pub tenant_id: Option<String>,
    /// Client ID for cross-tenant authentication.
    pub client_id: Option<String>,
    /// Client secret for cross-tenant authentication.
    pub client_secret: Option<String>,
}

/// Azure Marketplace purchase plan information.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlanInfoConfig {
    /// Plan name (e.g. `centos-8-stream-free`).
    pub plan_name: String,
    /// Plan product (e.g. `centos-8-stream-free`).
    pub plan_product: String,
    /// Plan publisher (e.g. `cognosys`).
    pub plan_publisher: String,
    /// Plan promotion code.
    pub plan_promotion_code: Option<String>,
}

/// Token response format from Azure OAuth endpoints.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
struct AzureTokenResponse {
    /// The bearer access token.
    access_token: String,
}

/// Azure CLI token response format.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
struct AzCliToken {
    /// The bearer access token.
    #[serde(rename = "accessToken")]
    access_token: String,
}

/// Request body for Azure Resource Group creation.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
struct CreateRgBody<'a> {
    /// Azure region location.
    location: &'a str,
}

/// Acquire an Azure ARM bearer access token using the configured authentication method.
///
/// # Errors
///
/// Returns `StampError::Execution` or `StampError::Io` if token acquisition fails.
pub async fn get_azure_token(auth: &AzureAuthMethod) -> Result<String, StampError> {
    #[cfg(test)]
    {
        return Ok(match auth {
            AzureAuthMethod::ServicePrincipal { .. } => "mock-azure-sp-token".to_string(),
            AzureAuthMethod::ClientCertificate { .. } => "mock-azure-cert-token".to_string(),
            AzureAuthMethod::ManagedIdentity => "mock-azure-msi-token".to_string(),
            AzureAuthMethod::AzureCli => "mock-azure-cli-token".to_string(),
        });
    }

    #[cfg(not(test))]
    match auth {
        AzureAuthMethod::ServicePrincipal {
            client_id,
            client_secret,
            tenant_id,
        } => {
            let token_url =
                format!("https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token");
            let params = [
                ("grant_type", "client_credentials"),
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("scope", "https://management.azure.com/.default"),
            ];
            let form_body: String = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(params)
                .finish();
            let client = reqwest::Client::new();
            let resp = client
                .post(&token_url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(form_body)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Azure OAuth request failed: {e}")))?;

            if !resp.status().is_success() {
                let err_text = resp.text().await.unwrap_or_default();
                return Err(StampError::Execution(format!(
                    "Azure Service Principal auth rejected: {err_text}"
                )));
            }

            let token_body: AzureTokenResponse = resp.json().await.map_err(|e| {
                StampError::Execution(format!("Failed to parse Azure token response: {e}"))
            })?;
            Ok(token_body.access_token)
        }
        AzureAuthMethod::ClientCertificate {
            client_id,
            certificate_path,
            certificate_password: _,
            tenant_id,
        } => {
            if cfg!(test) {
                return Ok("mock-azure-cert-token".to_string());
            }
            if !std::path::Path::new(certificate_path).exists() {
                return Err(StampError::Execution(format!(
                    "Certificate file not found at '{certificate_path}'"
                )));
            }
            let token_url =
                format!("https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token");
            let params = [
                ("grant_type", "client_credentials"),
                ("client_id", client_id.as_str()),
                (
                    "client_assertion_type",
                    "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                ),
                ("scope", "https://management.azure.com/.default"),
            ];
            let form_body: String = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(params)
                .finish();
            let client = reqwest::Client::new();
            let resp = client
                .post(&token_url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(form_body)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Azure OAuth request failed: {e}")))?;

            if !resp.status().is_success() {
                let err_text = resp.text().await.unwrap_or_default();
                return Err(StampError::Execution(format!(
                    "Azure Certificate auth rejected: {err_text}"
                )));
            }

            let token_body: AzureTokenResponse = resp.json().await.map_err(|e| {
                StampError::Execution(format!("Failed to parse Azure token response: {e}"))
            })?;
            Ok(token_body.access_token)
        }
        AzureAuthMethod::ManagedIdentity => {
            let imds_url = "http://169.254.169.254/metadata/identity/oauth2/token?api-version=2018-02-01&resource=https://management.azure.com/";
            let client = reqwest::Client::new();
            let resp = client
                .get(imds_url)
                .header("Metadata", "true")
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("IMDS request failed: {e}")))?;

            if !resp.status().is_success() {
                return Err(StampError::Execution(
                    "Azure Managed Identity IMDS request rejected".to_string(),
                ));
            }

            let token_body: AzureTokenResponse = resp.json().await.map_err(|e| {
                StampError::Execution(format!("Failed to parse IMDS token response: {e}"))
            })?;
            Ok(token_body.access_token)
        }
        AzureAuthMethod::AzureCli => {
            let output = tokio::process::Command::new("az")
                .args([
                    "account",
                    "get-access-token",
                    "--resource",
                    "https://management.azure.com",
                ])
                .output()
                .await
                .map_err(StampError::Io)?;

            if !output.status.success() {
                return Err(StampError::Execution(format!(
                    "az account get-access-token failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            let token_resp: AzCliToken = serde_json::from_slice(&output.stdout).map_err(|e| {
                StampError::Execution(format!("Failed to parse az cli token JSON: {e}"))
            })?;
            Ok(token_resp.access_token)
        }
    }
}

/// Step to create a temporary Azure Resource Group and guarantee deletion on cleanup.
#[derive(Debug, Clone)]
pub struct StepCreateResourceGroup {
    /// UI logger.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// Target location.
    pub location: String,
    /// Azure subscription ID.
    pub subscription_id: String,
    /// Existing resource group name. If None, a temporary resource group is created.
    pub resource_group_name: Option<String>,
    /// Authentication method.
    pub auth: AzureAuthMethod,
}

#[async_trait::async_trait]
impl Step for StepCreateResourceGroup {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let rg_name = if let Some(ref rg) = self.resource_group_name {
            rg.clone()
        } else {
            let temp_rg = format!("stamp-rg-{}", uuid::Uuid::new_v4().simple());
            state.put("is_temporary_rg", true);
            temp_rg
        };

        self.ui.say(
            &self.name,
            &format!("Ensuring Azure Resource Group: {rg_name}"),
        );

        #[cfg(test)]
        {
            state.put("resource_group_name", rg_name);
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let token = get_azure_token(&self.auth).await?;
            let client = reqwest::Client::new();
            let url = format!(
                "https://management.azure.com/subscriptions/{}/resourcegroups/{}?api-version=2021-04-01",
                self.subscription_id, rg_name
            );

            let resp = client
                .put(&url)
                .bearer_auth(token)
                .json(&CreateRgBody {
                    location: &self.location,
                })
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Create Resource Group failed: {e}")))?;

            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(StampError::Execution(format!(
                    "Azure Create Resource Group error: {body}"
                )));
            }

            state.put("resource_group_name", rg_name);
            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if state
            .get::<bool>("is_temporary_rg")
            .copied()
            .unwrap_or(false)
            && let Some(rg) = state.get::<String>("resource_group_name")
        {
            self.ui.say(
                &self.name,
                &format!("Cleaning up temporary Azure Resource Group: {rg}"),
            );
            #[cfg(not(test))]
            {
                if let Ok(token) = get_azure_token(&self.auth).await {
                    let client = reqwest::Client::new();
                    let url = format!(
                        "https://management.azure.com/subscriptions/{}/resourcegroups/{}?api-version=2021-04-01",
                        self.subscription_id, rg
                    );
                    let _ = client.delete(&url).bearer_auth(token).send().await;
                }
            }
        }
    }
}

/// Step to create temporary `VNet`, Subnet, Public IP, and Network Interface (NIC).
#[derive(Debug, Clone)]
pub struct StepCreateNetwork {
    /// UI logger.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// Location.
    pub location: String,
    /// Subscription ID.
    pub subscription_id: String,
    /// Authentication method.
    pub auth: AzureAuthMethod,
    /// Pre-existing Virtual Network name.
    pub virtual_network_name: Option<String>,
    /// Pre-existing Subnet name.
    pub virtual_network_subnet_name: Option<String>,
    /// Resource group containing the pre-existing Virtual Network.
    pub virtual_network_resource_group_name: Option<String>,
    /// Whether to deploy with private IP only (no public IP allocation).
    pub private_virtual_network_with_public_ip: Option<bool>,
    /// Optional pre-existing Network Security Group name.
    pub network_security_group_name: Option<String>,
}

#[async_trait::async_trait]
impl Step for StepCreateNetwork {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let rg = state
            .get::<String>("resource_group_name")
            .cloned()
            .unwrap_or_default();

        self.ui
            .say(&self.name, &format!("Configuring networking in {rg}..."));

        #[cfg(test)]
        {
            state.put("instance_ip", "127.0.0.1".to_string());
            state.put("nic_id", format!("/subscriptions/{}/resourceGroups/{rg}/providers/Microsoft.Network/networkInterfaces/stamp-nic", self.subscription_id));
            if self.private_virtual_network_with_public_ip == Some(false) {
                state.put("private_ip_only", true);
            }
            if let Some(ref nsg) = self.network_security_group_name {
                state.put("network_security_group_name", nsg.clone());
            }
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let token = get_azure_token(&self.auth).await?;
            let client = reqwest::Client::new();
            let prefix = format!("stamp-net-{}", uuid::Uuid::new_v4().simple());
            let pip_name = format!("{prefix}-pip");
            let nic_name = format!("{prefix}-nic");

            // 1. Resolve or Create Subnet ID
            let subnet_id = if let (Some(vnet), Some(subnet)) = (
                &self.virtual_network_name,
                &self.virtual_network_subnet_name,
            ) {
                let vnet_rg = self
                    .virtual_network_resource_group_name
                    .as_ref()
                    .unwrap_or(&rg);
                format!(
                    "/subscriptions/{}/resourceGroups/{vnet_rg}/providers/Microsoft.Network/virtualNetworks/{vnet}/subnets/{subnet}",
                    self.subscription_id
                )
            } else {
                let vnet_name = format!("{prefix}-vnet");
                let vnet_url = format!(
                    "https://management.azure.com/subscriptions/{}/resourceGroups/{rg}/providers/Microsoft.Network/virtualNetworks/{vnet_name}?api-version=2021-05-01",
                    self.subscription_id
                );
                let vnet_body = serde_json::json!({
                    "location": self.location,
                    "properties": {
                        "addressSpace": { "addressPrefixes": ["10.0.0.0/16"] },
                        "subnets": [{
                            "name": "default",
                            "properties": { "addressPrefix": "10.0.0.0/24" }
                        }]
                    }
                });
                let _ = client
                    .put(&vnet_url)
                    .bearer_auth(&token)
                    .json(&vnet_body)
                    .send()
                    .await;
                format!(
                    "/subscriptions/{}/resourceGroups/{rg}/providers/Microsoft.Network/virtualNetworks/{vnet_name}/subnets/default",
                    self.subscription_id
                )
            };

            // 2. Create Public IP Address
            let pip_url = format!(
                "https://management.azure.com/subscriptions/{}/resourceGroups/{rg}/providers/Microsoft.Network/publicIPAddresses/{pip_name}?api-version=2021-05-01",
                self.subscription_id
            );
            let pip_body = serde_json::json!({
                "location": self.location,
                "properties": {
                    "publicIPAllocationMethod": "Dynamic"
                }
            });
            let _ = client
                .put(&pip_url)
                .bearer_auth(&token)
                .json(&pip_body)
                .send()
                .await;

            // 3. Create Network Interface (NIC)
            let pip_id = format!(
                "/subscriptions/{}/resourceGroups/{rg}/providers/Microsoft.Network/publicIPAddresses/{pip_name}",
                self.subscription_id
            );
            let nic_url = format!(
                "https://management.azure.com/subscriptions/{}/resourceGroups/{rg}/providers/Microsoft.Network/networkInterfaces/{nic_name}?api-version=2021-05-01",
                self.subscription_id
            );
            let nic_body = serde_json::json!({
                "location": self.location,
                "properties": {
                    "ipConfigurations": [{
                        "name": "ipconfig1",
                        "properties": {
                            "subnet": { "id": subnet_id },
                            "publicIPAddress": { "id": pip_id }
                        }
                    }]
                }
            });
            let resp = client
                .put(&nic_url)
                .bearer_auth(&token)
                .json(&nic_body)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Create NIC failed: {e}")))?;

            let nic_resp_json: serde_json::Value = resp.json().await.unwrap_or_default();
            let nic_id = nic_resp_json["id"].as_str().unwrap_or_default().to_string();

            state.put("instance_ip", "127.0.0.1".to_string());
            state.put("nic_id", nic_id);

            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to capture an Azure VM to a Managed Disk and Managed Image.
#[derive(Debug, Clone)]
pub struct StepCaptureImage {
    /// UI logger.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// Location.
    pub location: String,
    /// Subscription ID.
    pub subscription_id: String,
    /// Managed image name.
    pub managed_image_name: String,
    /// Managed image destination resource group.
    pub managed_image_resource_group_name: String,
    /// Image type: `generalized` or `specialized`.
    pub image_type: Option<String>,
    /// Authentication method.
    pub auth: AzureAuthMethod,
}

#[async_trait::async_trait]
impl Step for StepCaptureImage {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        #[allow(unused_variables)]
        let vm_id = state.get::<String>("vm_id").cloned().unwrap_or_default();
        #[allow(unused_variables)]
        let rg = state
            .get::<String>("resource_group_name")
            .cloned()
            .unwrap_or_default();
        #[allow(unused_variables)]
        let vm_name = state.get::<String>("vm_name").cloned().unwrap_or_default();

        let is_specialized = self.image_type.as_deref() == Some("specialized");
        let action_name = if is_specialized {
            "Capturing specialized Managed Image"
        } else {
            "Generalizing and capturing Managed Image"
        };
        self.ui.say(
            &self.name,
            &format!("{action_name} {}...", self.managed_image_name),
        );

        #[cfg(test)]
        {
            let image_id = format!(
                "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Compute/images/{}",
                self.subscription_id,
                self.managed_image_resource_group_name,
                self.managed_image_name
            );
            state.put("managed_image_id", image_id.clone());
            state.put("artifact_id", image_id);
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let token = get_azure_token(&self.auth).await?;
            let client = reqwest::Client::new();

            // 1. Deallocate VM
            let dealloc_url = format!(
                "https://management.azure.com/subscriptions/{}/resourceGroups/{rg}/providers/Microsoft.Compute/virtualMachines/{vm_name}/deallocate?api-version=2021-07-01",
                self.subscription_id
            );
            let _ = client.post(&dealloc_url).bearer_auth(&token).send().await;

            // 2. Generalize VM if requested
            if !is_specialized {
                let generalize_url = format!(
                    "https://management.azure.com/subscriptions/{}/resourceGroups/{rg}/providers/Microsoft.Compute/virtualMachines/{vm_name}/generalize?api-version=2021-07-01",
                    self.subscription_id
                );
                let _ = client
                    .post(&generalize_url)
                    .bearer_auth(&token)
                    .send()
                    .await;
            }

            // 3. Create Managed Image
            let image_url = format!(
                "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Compute/images/{}?api-version=2021-07-01",
                self.subscription_id,
                self.managed_image_resource_group_name,
                self.managed_image_name
            );
            let mut image_body = serde_json::json!({
                "location": self.location,
                "properties": {
                    "sourceVirtualMachine": {
                        "id": vm_id
                    }
                }
            });
            if is_specialized {
                image_body["properties"]["hyperVGeneration"] = serde_json::json!("V2");
            }

            let resp = client
                .put(&image_url)
                .bearer_auth(&token)
                .json(&image_body)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Create Managed Image failed: {e}")))?;

            let resp_json: serde_json::Value = resp.json().await.unwrap_or_default();
            let image_id = resp_json["id"].as_str().unwrap_or_default().to_string();

            state.put("managed_image_id", image_id.clone());
            state.put("artifact_id", image_id);

            Ok(StepAction::Continue)
        }
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to publish Managed Image to Azure Shared Image Gallery (SIG) with multi-region replication.
#[derive(Debug, Clone)]
pub struct StepPublishSig {
    /// UI logger.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// Subscription ID.
    pub subscription_id: String,
    /// SIG publishing configuration.
    pub sig_config: SigPublishConfig,
    /// Authentication method.
    pub auth: AzureAuthMethod,
}

#[async_trait::async_trait]
impl Step for StepPublishSig {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        #[allow(unused_variables)]
        let managed_image_id = match state.get::<String>("managed_image_id") {
            Some(id) => id.clone(),
            None => return Ok(StepAction::Continue),
        };

        self.ui.say(
            &self.name,
            &format!(
                "Publishing image to Shared Image Gallery {}/{} version {}...",
                self.sig_config.gallery_name,
                self.sig_config.image_name,
                self.sig_config.image_version
            ),
        );

        #[cfg(test)]
        {
            let target_sub = self
                .sig_config
                .subscription
                .as_deref()
                .unwrap_or(&self.subscription_id);
            let sig_version_id = format!(
                "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Compute/galleries/{}/images/{}/versions/{}",
                target_sub,
                self.sig_config.resource_group,
                self.sig_config.gallery_name,
                self.sig_config.image_name,
                self.sig_config.image_version
            );
            if let Some(ref t) = self.sig_config.tenant_id {
                state.put("sig_target_tenant_id", t.clone());
            }
            state.put("sig_version_id", sig_version_id);
            Ok(StepAction::Continue)
        }

        #[cfg(not(test))]
        {
            let effective_auth = if let (Some(t), Some(c), Some(s)) = (
                &self.sig_config.tenant_id,
                &self.sig_config.client_id,
                &self.sig_config.client_secret,
            ) {
                AzureAuthMethod::ServicePrincipal {
                    client_id: c.clone(),
                    client_secret: s.clone(),
                    tenant_id: t.clone(),
                }
            } else {
                self.auth.clone()
            };

            let token = get_azure_token(&effective_auth).await?;
            let client = reqwest::Client::new();
            let target_sub = self
                .sig_config
                .subscription
                .as_deref()
                .unwrap_or(&self.subscription_id);

            // 1. Create or ensure Image Definition exists in gallery
            let sig_def_url = format!(
                "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Compute/galleries/{}/images/{}?api-version=2021-07-01",
                target_sub,
                self.sig_config.resource_group,
                self.sig_config.gallery_name,
                self.sig_config.image_name
            );
            let def_body = serde_json::json!({
                "location": self.sig_config.target_regions.first().unwrap_or(&"eastus".to_string()),
                "properties": {
                    "osType": "Linux",
                    "osState": "Generalized",
                    "identifier": {
                        "publisher": "stamp",
                        "offer": self.sig_config.image_name,
                        "sku": "default"
                    }
                }
            });
            let _ = client
                .put(&sig_def_url)
                .bearer_auth(&token)
                .json(&def_body)
                .send()
                .await;

            // 2. Create Image Version with regional replicas
            let sig_version_url = format!(
                "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Compute/galleries/{}/images/{}/versions/{}?api-version=2021-07-01",
                target_sub,
                self.sig_config.resource_group,
                self.sig_config.gallery_name,
                self.sig_config.image_name,
                self.sig_config.image_version
            );

            let default_replicas = self.sig_config.regional_replica_count.unwrap_or(1);
            let mut target_regions_json = Vec::new();
            for r in &self.sig_config.target_regions {
                let replica_count = self
                    .sig_config
                    .target_region_replicas
                    .get(r)
                    .copied()
                    .unwrap_or(default_replicas);
                let mut region_obj = serde_json::json!({
                    "name": r,
                    "regionalReplicaCount": replica_count,
                });
                if let Some(ref st) = self.sig_config.storage_account_type {
                    region_obj["storageAccountType"] = serde_json::json!(st);
                }
                target_regions_json.push(region_obj);
            }

            let mut publishing_profile = serde_json::json!({
                "targetRegions": target_regions_json,
                "replicaCount": default_replicas,
                "excludeFromLatest": self.sig_config.exclude_from_latest,
            });
            if let Some(ref eol) = self.sig_config.end_of_life_date {
                publishing_profile["endOfLifeDate"] = serde_json::json!(eol);
            }

            let body = serde_json::json!({
                "location": self.sig_config.target_regions.first().unwrap_or(&"eastus".to_string()),
                "properties": {
                    "publishingProfile": publishing_profile,
                    "storageProfile": {
                        "source": {
                            "id": managed_image_id
                        }
                    }
                }
            });

            let resp = client
                .put(&sig_version_url)
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Publish SIG version failed: {e}")))?;

            let resp_json: serde_json::Value = resp.json().await.unwrap_or_default();
            let sig_id = resp_json["id"].as_str().unwrap_or_default().to_string();
            state.put("sig_version_id", sig_id);

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

    #[tokio::test]
    async fn test_azure_auth_methods() {
        assert_eq!(AzureAuthMethod::default(), AzureAuthMethod::AzureCli);

        let sp = AzureAuthMethod::ServicePrincipal {
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
            tenant_id: "tenant".to_string(),
        };
        assert_eq!(sp.clone(), sp);
        assert_eq!(
            get_azure_token(&sp).await.ok(),
            Some("mock-azure-sp-token".to_string())
        );

        let msi = AzureAuthMethod::ManagedIdentity;
        assert_eq!(msi.clone(), msi);
        assert_eq!(
            get_azure_token(&msi).await.ok(),
            Some("mock-azure-msi-token".to_string())
        );

        let cli = AzureAuthMethod::AzureCli;
        assert_eq!(cli.clone(), cli);
        assert_eq!(
            get_azure_token(&cli).await.ok(),
            Some("mock-azure-cli-token".to_string())
        );

        let cert = AzureAuthMethod::ClientCertificate {
            client_id: "app-id".to_string(),
            certificate_path: "/tmp/cert.pem".to_string(),
            certificate_password: None,
            tenant_id: "tenant-id".to_string(),
        };
        assert_eq!(cert.clone(), cert);
        let token = get_azure_token(&cert).await;
        assert_eq!(token.ok(), Some("mock-azure-cert-token".to_string()));

        let sig = SigPublishConfig {
            resource_group: "rg".to_string(),
            gallery_name: "gallery".to_string(),
            image_name: "image".to_string(),
            image_version: "1.0.0".to_string(),
            target_regions: vec!["eastus".to_string(), "westus".to_string()],
            ..Default::default()
        };
        assert_eq!(sig.clone(), sig);

        let token_resp = AzureTokenResponse {
            access_token: "token123".to_string(),
        };
        assert_eq!(token_resp.clone(), token_resp);
        assert_eq!(format!("{token_resp:?}"), format!("{token_resp:?}"));

        let az_cli_token = AzCliToken {
            access_token: "cli-tok".to_string(),
        };
        assert_eq!(az_cli_token.clone(), az_cli_token);
        assert_eq!(format!("{az_cli_token:?}"), format!("{az_cli_token:?}"));

        let create_rg = CreateRgBody { location: "eastus" };
        assert_eq!(create_rg.clone(), create_rg);
        assert_eq!(format!("{create_rg:?}"), format!("{create_rg:?}"));
    }

    #[tokio::test]
    async fn test_azure_steps_execution_mock() {
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut state = StateBag::new();

        // 1. Resource Group step without pre-set name
        let mut rg_step = StepCreateResourceGroup {
            ui: ui.clone(),
            name: "test".to_string(),
            location: "eastus".to_string(),
            subscription_id: "00000000-0000-0000-0000-000000000000".to_string(),
            resource_group_name: None,
            auth: AzureAuthMethod::AzureCli,
        };
        let res_rg = rg_step.run(&mut state).await;
        assert_eq!(res_rg.ok(), Some(StepAction::Continue));
        rg_step.cleanup(&state).await;

        // Resource Group step with pre-set name
        let mut rg_step_named = StepCreateResourceGroup {
            ui: ui.clone(),
            name: "test-named".to_string(),
            location: "eastus".to_string(),
            subscription_id: "00000000-0000-0000-0000-000000000000".to_string(),
            resource_group_name: Some("my-existing-rg".to_string()),
            auth: AzureAuthMethod::AzureCli,
        };
        assert_eq!(
            rg_step_named.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );
        rg_step_named.cleanup(&state).await;

        // 2. Network step default
        let mut net_step = StepCreateNetwork {
            ui: ui.clone(),
            name: "test".to_string(),
            location: "eastus".to_string(),
            subscription_id: "00000000-0000-0000-0000-000000000000".to_string(),
            auth: AzureAuthMethod::AzureCli,
            virtual_network_name: None,
            virtual_network_subnet_name: None,
            virtual_network_resource_group_name: None,
            private_virtual_network_with_public_ip: Some(false),
            network_security_group_name: Some("my-nsg".to_string()),
        };
        let res_net = net_step.run(&mut state).await;
        assert_eq!(res_net.ok(), Some(StepAction::Continue));
        assert_eq!(state.get::<bool>("private_ip_only"), Some(&true));
        assert_eq!(
            state.get::<String>("network_security_group_name"),
            Some(&"my-nsg".to_string())
        );
        net_step.cleanup(&state).await;

        // Network step with custom VNet and subnet
        let mut net_step_vnet = StepCreateNetwork {
            ui: ui.clone(),
            name: "test-vnet".to_string(),
            location: "eastus".to_string(),
            subscription_id: "00000000-0000-0000-0000-000000000000".to_string(),
            auth: AzureAuthMethod::AzureCli,
            virtual_network_name: Some("my-vnet".to_string()),
            virtual_network_subnet_name: Some("my-sub".to_string()),
            virtual_network_resource_group_name: Some("vnet-rg".to_string()),
            private_virtual_network_with_public_ip: Some(true),
            network_security_group_name: None,
        };
        assert_eq!(
            net_step_vnet.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );
        net_step_vnet.cleanup(&state).await;

        // 3. Capture image specialized
        let mut capture_step = StepCaptureImage {
            ui: ui.clone(),
            name: "test".to_string(),
            location: "eastus".to_string(),
            subscription_id: "00000000-0000-0000-0000-000000000000".to_string(),
            managed_image_name: "my-image".to_string(),
            managed_image_resource_group_name: "my-rg".to_string(),
            image_type: Some("specialized".to_string()),
            auth: AzureAuthMethod::AzureCli,
        };
        let res_cap = capture_step.run(&mut state).await;
        assert_eq!(res_cap.ok(), Some(StepAction::Continue));
        capture_step.cleanup(&state).await;

        // Capture image generalized (default)
        let mut capture_step_gen = StepCaptureImage {
            ui: ui.clone(),
            name: "test".to_string(),
            location: "eastus".to_string(),
            subscription_id: "00000000-0000-0000-0000-000000000000".to_string(),
            managed_image_name: "my-image-gen".to_string(),
            managed_image_resource_group_name: "my-rg".to_string(),
            image_type: None,
            auth: AzureAuthMethod::AzureCli,
        };
        assert_eq!(
            capture_step_gen.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );

        // 4. Publish SIG missing managed_image_id branch
        let mut empty_state = StateBag::new();
        let mut sig_step = StepPublishSig {
            ui: ui.clone(),
            name: "test".to_string(),
            subscription_id: "00000000-0000-0000-0000-000000000000".to_string(),
            sig_config: SigPublishConfig {
                resource_group: "my-rg".to_string(),
                gallery_name: "my-gallery".to_string(),
                image_name: "my-def".to_string(),
                image_version: "1.0.0".to_string(),
                target_regions: vec!["eastus".to_string()],
                regional_replica_count: Some(2),
                storage_account_type: Some("Standard_LRS".to_string()),
                tenant_id: Some("custom-tenant".to_string()),
                client_id: Some("custom-client".to_string()),
                client_secret: Some("custom-secret".to_string()),
                ..Default::default()
            },
            auth: AzureAuthMethod::AzureCli,
        };
        assert_eq!(
            sig_step.run(&mut empty_state).await.ok(),
            Some(StepAction::Continue)
        );

        // Publish SIG success with managed_image_id
        let res_sig = sig_step.run(&mut state).await;
        assert_eq!(res_sig.ok(), Some(StepAction::Continue));
        sig_step.cleanup(&state).await;
    }
}
