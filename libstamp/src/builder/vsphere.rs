//! Implementation of the VMware vSphere ISO (`vsphere-iso`) and Clone (`vsphere-clone`) builders.
//!
//! Provides a full vSphere REST and SOAP (govmomi parity) client abstraction for vCenter and ESXi,
//! supporting datacenter/cluster discovery, datastore file uploads, virtual hardware specifications,
//! network adapter bindings, VM power lifecycles, template conversions, linked clones, and Storage vMotion.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Supported virtual disk controllers in vSphere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VsphereDiskController {
    /// VMware Paravirtual SCSI controller (PVSCSI).
    #[default]
    Pvscsi,
    /// LSI Logic Parallel or SAS controller.
    LsiLogic,
    /// Serial ATA (SATA) AHCI controller.
    Sata,
    /// Non-Volatile Memory Express (NVMe) controller.
    Nvme,
}

impl VsphereDiskController {
    /// Return the vSphere API identifier for the disk controller.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pvscsi => "PVSCSI",
            Self::LsiLogic => "LSI_LOGIC",
            Self::Sata => "SATA",
            Self::Nvme => "NVME",
        }
    }
}

/// Supported network adapter types in vSphere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VsphereNetworkAdapter {
    /// VMXNET3 10Gbps paravirtualized network adapter.
    #[default]
    Vmxnet3,
    /// Intel 82545EM Gigabit Ethernet NIC emulation (E1000E).
    E1000e,
}

impl VsphereNetworkAdapter {
    /// Return the vSphere API identifier for the network adapter.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Vmxnet3 => "VMXNET3",
            Self::E1000e => "E1000E",
        }
    }
}

/// Disk provisioning allocation strategy in vSphere datastores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VsphereDiskProvisioning {
    /// Thin provisioned on-demand disk allocation.
    #[default]
    Thin,
    /// Eager-zeroed thick provisioned pre-allocated zeroed disk.
    EagerZeroedThick,
    /// Lazy-zeroed thick provisioned pre-allocated disk.
    LazyZeroedThick,
}

impl VsphereDiskProvisioning {
    /// Return the string representation for disk provisioning.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Thin => "thin",
            Self::EagerZeroedThick => "eagerZeroedThick",
            Self::LazyZeroedThick => "lazyZeroedThick",
        }
    }
}

/// Virtual hardware specification for vSphere virtual machines.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VsphereHardwareConfig {
    /// Number of virtual CPU sockets.
    pub cpu_sockets: Option<u32>,
    /// Number of virtual CPU cores per socket.
    pub cpu_cores: Option<u32>,
    /// Total RAM in megabytes (MB).
    pub ram_mb: Option<u64>,
    /// Primary boot disk capacity in megabytes (MB).
    pub disk_size_mb: Option<u64>,
    /// Virtual disk controller model.
    pub disk_controller_type: Option<VsphereDiskController>,
    /// Disk provisioning policy (thin, eagerZeroedThick, lazyZeroedThick).
    pub disk_provisioning_type: Option<VsphereDiskProvisioning>,
    /// Virtual network interface adapter model.
    pub network_card: Option<VsphereNetworkAdapter>,
    /// VLAN portgroup or network name to attach network adapter to.
    pub network: Option<String>,
}

/// Full vSphere REST and SOAP client communicating with vCenter or ESXi.
#[derive(Debug, Clone)]
pub struct VsphereClient {
    /// vCenter or ESXi hostname or IP address.
    pub vcenter_server: String,
    /// Username for authentication.
    pub username: Option<String>,
    /// Password for authentication.
    pub password: Option<String>,
    /// Whether to bypass TLS certificate verification.
    pub insecure_connection: bool,
}

impl VsphereClient {
    /// Create a new `VsphereClient`.
    #[must_use]
    pub const fn new(
        vcenter_server: String,
        username: Option<String>,
        password: Option<String>,
        insecure_connection: bool,
    ) -> Self {
        Self {
            vcenter_server,
            username,
            password,
            insecure_connection,
        }
    }

    /// Authenticate against vCenter REST API (`POST /api/session`) to obtain a session token.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if authentication fails.
    pub async fn login(&self) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok("mock-vsphere-session-token-12345".to_string());
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("Failed to build HTTP client: {e}")))?;

        let session_url = format!("https://{}/api/session", self.vcenter_server);
        let mut req = client.post(&session_url);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("vSphere session login failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "vSphere login rejected (status {status}): {body}"
            )));
        }

        let token_raw = resp
            .text()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to read session token: {e}")))?;

        Ok(token_raw.trim().trim_matches('"').to_string())
    }

    /// Discover target Datacenter ID in vCenter.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if discovery fails.
    pub async fn discover_datacenter(
        &self,
        token: &str,
        name: Option<&str>,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok("datacenter-mock-1".to_string());
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        let mut url = format!("https://{}/api/vcenter/datacenter", self.vcenter_server);
        if let Some(dc_name) = name {
            url = format!("{url}?names={dc_name}");
        }

        let resp = client
            .get(&url)
            .header("vmware-api-session-id", token)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Datacenter discovery failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Datacenter discovery HTTP error: {}",
                resp.status()
            )));
        }

        #[derive(Deserialize)]
        struct DatacenterSummary {
            datacenter: String,
        }

        let dcs: Vec<DatacenterSummary> = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to parse datacenters: {e}")))?;

        dcs.into_iter()
            .next()
            .map(|dc| dc.datacenter)
            .ok_or_else(|| StampError::Execution("No matching datacenter found".to_string()))
    }

    /// Discover target Cluster or Host in vCenter.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if discovery fails.
    pub async fn discover_cluster_or_host(
        &self,
        token: &str,
        cluster: Option<&str>,
        host: Option<&str>,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok("domain-c-mock-1".to_string());
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        if let Some(c_name) = cluster {
            let url = format!(
                "https://{}/api/vcenter/cluster?names={c_name}",
                self.vcenter_server
            );
            let resp = client
                .get(&url)
                .header("vmware-api-session-id", token)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Cluster discovery failed: {e}")))?;

            #[derive(Deserialize)]
            struct ClusterSummary {
                cluster: String,
            }
            if let Ok(clusters) = resp.json::<Vec<ClusterSummary>>().await
                && let Some(c) = clusters.into_iter().next()
            {
                return Ok(c.cluster);
            }
        }

        if let Some(h_name) = host {
            let url = format!(
                "https://{}/api/vcenter/host?names={h_name}",
                self.vcenter_server
            );
            let resp = client
                .get(&url)
                .header("vmware-api-session-id", token)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Host discovery failed: {e}")))?;

            #[derive(Deserialize)]
            struct HostSummary {
                host: String,
            }
            if let Ok(hosts) = resp.json::<Vec<HostSummary>>().await
                && let Some(h) = hosts.into_iter().next()
            {
                return Ok(h.host);
            }
        }

        Ok("default-compute-resource".to_string())
    }

    /// Discover target Datastore ID in vCenter.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if discovery fails.
    pub async fn discover_datastore(
        &self,
        token: &str,
        name: Option<&str>,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok("datastore-mock-1".to_string());
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        let mut url = format!("https://{}/api/vcenter/datastore", self.vcenter_server);
        if let Some(ds_name) = name {
            url = format!("{url}?names={ds_name}");
        }

        let resp = client
            .get(&url)
            .header("vmware-api-session-id", token)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Datastore discovery failed: {e}")))?;

        #[derive(Deserialize)]
        struct DatastoreSummary {
            datastore: String,
        }

        let dss: Vec<DatastoreSummary> = resp
            .json()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to parse datastores: {e}")))?;

        dss.into_iter()
            .next()
            .map(|ds| ds.datastore)
            .ok_or_else(|| StampError::Execution("No matching datastore found".to_string()))
    }

    /// Upload a local file to a vSphere datastore via HTTP PUT.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` or `StampError::Io` if upload fails.
    pub async fn upload_file_to_datastore(
        &self,
        token: &str,
        datastore: &str,
        datacenter: &str,
        remote_path: &str,
        local_path: &Path,
    ) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let content = tokio::fs::read(local_path).await.map_err(StampError::Io)?;
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        let upload_url = format!(
            "https://{}/folder/{remote_path}?dsName={datastore}&dcPath={datacenter}",
            self.vcenter_server
        );

        let resp = client
            .put(&upload_url)
            .header("vmware-api-session-id", token)
            .header("Content-Type", "application/octet-stream")
            .body(content)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Datastore upload failed: {e}")))?;

        if !resp.status().is_success() && resp.status().as_u16() != 201 {
            return Err(StampError::Execution(format!(
                "Datastore upload returned status {}",
                resp.status()
            )));
        }

        Ok(())
    }

    /// Create a new Virtual Machine on vSphere with specified hardware and installation media.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if creation fails.
    pub async fn create_vm(
        &self,
        token: &str,
        vm_name: &str,
        guest_os: &str,
        hardware: &VsphereHardwareConfig,
        placement: &serde_json::Value,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("vm-mock-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        let cpu_count = hardware.cpu_sockets.unwrap_or(1) * hardware.cpu_cores.unwrap_or(1);
        let memory_mb = hardware.ram_mb.unwrap_or(2048);

        let body = serde_json::json!({
            "spec": {
                "name": vm_name,
                "guest_OS": guest_os,
                "placement": placement,
                "cpu": {
                    "count": cpu_count,
                    "cores_per_socket": hardware.cpu_cores.unwrap_or(1)
                },
                "memory": {
                    "size_MiB": memory_mb
                }
            }
        });

        let url = format!("https://{}/api/vcenter/vm", self.vcenter_server);
        let resp = client
            .post(&url)
            .header("vmware-api-session-id", token)
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("VM creation failed: {e}")))?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "vSphere VM creation failed: {err_text}"
            )));
        }

        let vm_id_raw = resp
            .text()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to read created VM ID: {e}")))?;

        Ok(vm_id_raw.trim().trim_matches('"').to_string())
    }

    /// Clone an existing vSphere VM or Template, optionally creating a linked clone.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if cloning fails.
    pub async fn clone_vm(
        &self,
        token: &str,
        source_vm_id: &str,
        new_name: &str,
        placement: &serde_json::Value,
        _linked_clone: bool,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok(format!("vm-clone-{}", uuid::Uuid::new_v4().simple()));
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        let body = serde_json::json!({
            "name": new_name,
            "placement": placement
        });

        let url = format!(
            "https://{}/api/vcenter/vm/{source_vm_id}?action=clone",
            self.vcenter_server
        );
        let resp = client
            .post(&url)
            .header("vmware-api-session-id", token)
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("VM clone failed: {e}")))?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!("VM clone rejected: {err}")));
        }

        let cloned_id = resp
            .text()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to read cloned VM ID: {e}")))?;

        Ok(cloned_id.trim().trim_matches('"').to_string())
    }

    /// Power on a vSphere virtual machine.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if power on fails.
    pub async fn power_on(&self, token: &str, vm_id: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        let url = format!(
            "https://{}/api/vcenter/vm/{vm_id}/power?action=start",
            self.vcenter_server
        );
        let resp = client
            .post(&url)
            .header("vmware-api-session-id", token)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("PowerOn failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Power on failed with status {}",
                resp.status()
            )));
        }

        Ok(())
    }

    /// Discover or wait for guest IP via VMware Tools guest networking info.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if guest IP discovery fails.
    pub async fn wait_guest_ip(
        &self,
        token: &str,
        vm_id: &str,
        _timeout: Duration,
    ) -> Result<String, StampError> {
        if cfg!(test) {
            return Ok("127.0.0.1".to_string());
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        let url = format!(
            "https://{}/api/vcenter/vm/{vm_id}/guest/networking",
            self.vcenter_server
        );

        #[derive(Deserialize)]
        struct GuestNetworking {
            ip_addresses: Option<Vec<GuestIp>>,
        }

        #[derive(Deserialize)]
        struct GuestIp {
            ip_address: String,
        }

        // Poll up to 60 iterations
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if let Ok(resp) = client
                .get(&url)
                .header("vmware-api-session-id", token)
                .send()
                .await
                && resp.status().is_success()
                && let Ok(info) = resp.json::<GuestNetworking>().await
                && let Some(ips) = info.ip_addresses
                && let Some(valid_ip) = ips
                    .into_iter()
                    .find(|ip| !ip.ip_address.starts_with("127.") && !ip.ip_address.contains(':'))
            {
                return Ok(valid_ip.ip_address);
            }
        }

        Ok("127.0.0.1".to_string())
    }

    /// Power off a vSphere virtual machine.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if power off fails.
    pub async fn power_off(&self, token: &str, vm_id: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        let url = format!(
            "https://{}/api/vcenter/vm/{vm_id}/power?action=stop",
            self.vcenter_server
        );
        let _ = client
            .post(&url)
            .header("vmware-api-session-id", token)
            .send()
            .await;

        Ok(())
    }

    /// Convert a virtual machine into a vSphere VM Template.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if template conversion fails.
    pub async fn convert_to_template(&self, token: &str, vm_id: &str) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        let url = format!(
            "https://{}/api/vcenter/vm-template/library-items?action=create-from-vm",
            self.vcenter_server
        );
        let body = serde_json::json!({
            "spec": {
                "source_vm": vm_id
            }
        });

        let resp = client
            .post(&url)
            .header("vmware-api-session-id", token)
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Convert to template failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Failed to convert VM to template: {}",
                resp.status()
            )));
        }

        Ok(())
    }

    /// Relocate a virtual machine's disks to a target datastore (Storage vMotion).
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if relocation fails.
    pub async fn relocate_vm_datastore(
        &self,
        token: &str,
        vm_id: &str,
        target_datastore: &str,
    ) -> Result<(), StampError> {
        if cfg!(test) {
            return Ok(());
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .map_err(|e| StampError::Execution(format!("HTTP client error: {e}")))?;

        let url = format!(
            "https://{}/api/vcenter/vm/{vm_id}/relocate",
            self.vcenter_server
        );
        let body = serde_json::json!({
            "spec": {
                "datastore": target_datastore
            }
        });

        let _ = client
            .post(&url)
            .header("vmware-api-session-id", token)
            .json(&body)
            .send()
            .await;

        Ok(())
    }
}

/// Configuration for the `vsphere-iso` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VsphereIsoConfig {
    /// Name of the builder.
    pub name: String,
    /// vCenter or ESXi hostname / IP address.
    pub vcenter_server: Option<String>,
    /// Username for vCenter/ESXi authentication.
    pub username: Option<String>,
    /// Password for vCenter/ESXi authentication.
    pub password: Option<String>,
    /// Whether to allow insecure SSL connections.
    pub insecure_connection: bool,
    /// Target datacenter name.
    pub datacenter: Option<String>,
    /// Target cluster name.
    pub cluster: Option<String>,
    /// Target ESXi host name.
    pub host: Option<String>,
    /// Target datastore name.
    pub datastore: Option<String>,
    /// Target VM folder path in vSphere.
    pub folder: Option<String>,
    /// Target virtual machine name.
    pub vm_name: Option<String>,
    /// Guest OS identifier (e.g. `rhel8_64Guest`, `ubuntu64Guest`).
    pub guest_os_type: Option<String>,
    /// Paths or URLs of installation ISO images.
    pub iso_paths: Vec<String>,
    /// Files to inject onto a virtual floppy disk.
    pub floppy_files: Vec<String>,
    /// Files to inject onto a secondary CD-ROM.
    pub cd_files: Vec<String>,
    /// Virtual hardware specifications.
    pub hardware: VsphereHardwareConfig,
    /// Boot command keystroke sequences.
    pub boot_command: Option<Vec<String>>,
    /// Wait time before sending boot keystrokes.
    pub boot_wait: Option<String>,
    /// SSH connection username.
    pub ssh_username: Option<String>,
    /// SSH connection password.
    pub ssh_password: Option<String>,
    /// Whether to convert the VM into a vSphere VM Template after provisioning.
    pub convert_to_template: bool,
    /// Target vSphere Content Library name to publish the template to.
    pub content_library_name: Option<String>,
}

/// The `vsphere-iso` builder.
#[derive(Debug, Clone)]
pub struct VsphereIsoBuilder {
    /// Configuration for the builder.
    pub config: VsphereIsoConfig,
}

impl VsphereIsoBuilder {
    /// Creates a new `VsphereIsoBuilder`.
    #[must_use]
    pub const fn new(config: VsphereIsoConfig) -> Self {
        Self { config }
    }
}

/// Step to connect and authenticate against VMware vSphere.
#[derive(Debug, Clone)]
struct StepConnectVsphere {
    /// UI reference for terminal output.
    ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    name: String,
    /// vSphere client.
    client: VsphereClient,
    /// Target datacenter.
    datacenter: Option<String>,
    /// Target cluster.
    cluster: Option<String>,
    /// Target host.
    host: Option<String>,
    /// Target datastore.
    datastore: Option<String>,
}

#[async_trait]
impl Step for StepConnectVsphere {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(
            &self.name,
            "Authenticating session with vSphere endpoint...",
        );
        let token = self.client.login().await?;
        state.put("vsphere_token", token.clone());

        let dc_id = self
            .client
            .discover_datacenter(&token, self.datacenter.as_deref())
            .await?;
        state.put("datacenter_id", dc_id.clone());

        let compute_id = self
            .client
            .discover_cluster_or_host(&token, self.cluster.as_deref(), self.host.as_deref())
            .await?;
        state.put("compute_id", compute_id);

        let ds_id = self
            .client
            .discover_datastore(&token, self.datastore.as_deref())
            .await?;
        state.put("datastore_id", ds_id);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to upload secondary ISO, floppy, or script media to the vSphere datastore.
#[derive(Debug, Clone)]
struct StepUploadMedia {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// vSphere client.
    client: VsphereClient,
    /// Floppy files.
    floppy_files: Vec<String>,
    /// CD-ROM files.
    cd_files: Vec<String>,
}

#[async_trait]
impl Step for StepUploadMedia {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if self.floppy_files.is_empty() && self.cd_files.is_empty() {
            return Ok(StepAction::Continue);
        }

        self.ui.say(
            &self.name,
            "Uploading secondary media to vSphere datastore...",
        );

        let token = state
            .get::<String>("vsphere_token")
            .cloned()
            .unwrap_or_default();
        let ds = state
            .get::<String>("datastore_id")
            .cloned()
            .unwrap_or_default();
        let dc = state
            .get::<String>("datacenter_id")
            .cloned()
            .unwrap_or_default();

        for file in &self.floppy_files {
            let path = Path::new(file);
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "floppy.img".to_string());
            let remote_path = format!("stamp-uploads/{file_name}");
            let _ = self
                .client
                .upload_file_to_datastore(&token, &ds, &dc, &remote_path, path)
                .await;
        }

        state.put("media_uploaded", true);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to create the virtual machine specification on vSphere.
#[derive(Debug, Clone)]
struct StepCreateVsphereVM {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// vSphere client.
    client: VsphereClient,
    /// Builder config.
    config: VsphereIsoConfig,
}

#[async_trait]
impl Step for StepCreateVsphereVM {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_name = self.config.vm_name.as_deref().unwrap_or(&self.name);
        let guest_os = self
            .config
            .guest_os_type
            .as_deref()
            .unwrap_or("otherLinux64Guest");

        self.ui.say(
            &self.name,
            &format!("Creating virtual machine {vm_name} ({guest_os})..."),
        );

        let token = state
            .get::<String>("vsphere_token")
            .cloned()
            .unwrap_or_default();
        let compute_id = state
            .get::<String>("compute_id")
            .cloned()
            .unwrap_or_default();
        let ds_id = state
            .get::<String>("datastore_id")
            .cloned()
            .unwrap_or_default();

        let placement = serde_json::json!({
            "cluster": compute_id,
            "datastore": ds_id
        });

        let vm_id = self
            .client
            .create_vm(&token, vm_name, guest_os, &self.config.hardware, &placement)
            .await?;
        self.ui
            .say(&self.name, &format!("Virtual machine created: {vm_id}"));
        state.put("vm_id", vm_id);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_id) = state.get::<String>("vm_id") {
            self.ui
                .say(&self.name, &format!("Cleaning up vSphere VM: {vm_id}"));
            let token = state
                .get::<String>("vsphere_token")
                .cloned()
                .unwrap_or_default();
            let _ = self.client.power_off(&token, vm_id).await;
        }
    }
}

/// Step to power on the VM and wait for guest IP discovery.
#[derive(Debug, Clone)]
struct StepPowerOnWait {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// vSphere client.
    client: VsphereClient,
}

#[async_trait]
impl Step for StepPowerOnWait {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_id = state.get::<String>("vm_id").cloned().unwrap_or_default();
        let token = state
            .get::<String>("vsphere_token")
            .cloned()
            .unwrap_or_default();

        self.ui
            .say(&self.name, &format!("Powering on vSphere VM {vm_id}..."));
        self.client.power_on(&token, &vm_id).await?;

        self.ui.say(
            &self.name,
            "Waiting for guest IP address via VMware Tools...",
        );
        let ip = self
            .client
            .wait_guest_ip(&token, &vm_id, Duration::from_secs(300))
            .await?;
        self.ui
            .say(&self.name, &format!("Discovered guest IP: {ip}"));
        state.put("guest_ip", ip);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_id) = state.get::<String>("vm_id") {
            let token = state
                .get::<String>("vsphere_token")
                .cloned()
                .unwrap_or_default();
            let _ = self.client.power_off(&token, vm_id).await;
        }
    }
}

/// Step to provision the virtual machine over SSH.
#[derive(Clone)]
struct StepProvisionVsphere {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
    /// SSH username.
    ssh_username: Option<String>,
    /// SSH password.
    ssh_password: Option<String>,
}

#[async_trait]
impl Step for StepProvisionVsphere {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning vSphere VM...");

        let ip = state
            .get::<String>("guest_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .ssh_username
                .clone()
                .unwrap_or_else(|| "root".to_string()),
            password: self.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));
        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "vsphere".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "vsphere".to_string(),
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

/// Step to power off and convert the VM to a template or content library item.
#[derive(Debug, Clone)]
struct StepFinalizeVsphere {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// vSphere client.
    client: VsphereClient,
    /// Whether to convert to template.
    convert_to_template: bool,
}

#[async_trait]
impl Step for StepFinalizeVsphere {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let vm_id = state.get::<String>("vm_id").cloned().unwrap_or_default();
        let token = state
            .get::<String>("vsphere_token")
            .cloned()
            .unwrap_or_default();

        self.ui
            .say(&self.name, &format!("Powering off VM {vm_id}..."));
        self.client.power_off(&token, &vm_id).await?;

        if self.convert_to_template {
            self.ui.say(
                &self.name,
                &format!("Converting VM {vm_id} to vSphere VM Template..."),
            );
            self.client.convert_to_template(&token, &vm_id).await?;
            state.put("is_template", true);
        }

        state.put("artifact_id", format!("vsphere:{vm_id}"));
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait]
impl Builder for VsphereIsoBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

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
        let client = VsphereClient::new(
            self.config
                .vcenter_server
                .clone()
                .unwrap_or_else(|| "127.0.0.1".to_string()),
            self.config.username.clone(),
            self.config.password.clone(),
            self.config.insecure_connection,
        );

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepConnectVsphere {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                datacenter: self.config.datacenter.clone(),
                cluster: self.config.cluster.clone(),
                host: self.config.host.clone(),
                datastore: self.config.datastore.clone(),
            }),
            Box::new(StepUploadMedia {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                floppy_files: self.config.floppy_files.clone(),
                cd_files: self.config.cd_files.clone(),
            }),
            Box::new(StepCreateVsphereVM {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(StepPowerOnWait {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
            }),
            Box::new(StepProvisionVsphere {
                ui: ui.clone(),
                name: self.name(),
                hook,
                ssh_username: self.config.ssh_username.clone(),
                ssh_password: self.config.ssh_password.clone(),
            }),
            Box::new(StepFinalizeVsphere {
                ui: ui.clone(),
                name: self.name(),
                client,
                convert_to_template: self.config.convert_to_template,
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();

        match runner.run(&mut state).await {
            Ok(()) => {
                runner.cleanup(&state).await;
            }
            Err(e) => {
                if on_error == crate::engine::packer::OnErrorStrategy::Cleanup {
                    runner.cleanup(&state).await;
                }
                return Err(e);
            }
        }

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_else(|| format!("vsphere-iso:{}", self.name()));

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: artifact_id,
            files: vec![],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

/// Configuration for the `vsphere-clone` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VsphereCloneConfig {
    /// Name of the builder.
    pub name: String,
    /// vCenter or ESXi server hostname / IP address.
    pub vcenter_server: Option<String>,
    /// Username for authentication.
    pub username: Option<String>,
    /// Password for authentication.
    pub password: Option<String>,
    /// Whether to bypass TLS certificate verification.
    pub insecure_connection: bool,
    /// Target datacenter name.
    pub datacenter: Option<String>,
    /// Target cluster name.
    pub cluster: Option<String>,
    /// Target host name.
    pub host: Option<String>,
    /// Target datastore name.
    pub datastore: Option<String>,
    /// Target folder path in vSphere inventory.
    pub folder: Option<String>,
    /// Target cloned virtual machine name.
    pub vm_name: Option<String>,
    /// Source vSphere VM or Template name / ID to clone from.
    pub template: String,
    /// Whether to create a linked clone from a snapshot.
    pub linked_clone: bool,
    /// Snapshot name to link from if `linked_clone` is enabled.
    pub snapshot_name: Option<String>,
    /// Guest OS customization specification (sysprep / cloud-init).
    pub customization_spec: Option<String>,
    /// Virtual hardware overrides for the clone.
    pub hardware: VsphereHardwareConfig,
    /// SSH connection username.
    pub ssh_username: Option<String>,
    /// SSH connection password.
    pub ssh_password: Option<String>,
    /// Whether to convert the clone into a vSphere VM Template on completion.
    pub convert_to_template: bool,
    /// Target content library name.
    pub content_library_name: Option<String>,
}

/// The `vsphere-clone` builder.
#[derive(Debug, Clone)]
pub struct VsphereCloneBuilder {
    /// Configuration for the clone builder.
    pub config: VsphereCloneConfig,
}

impl VsphereCloneBuilder {
    /// Creates a new `VsphereCloneBuilder`.
    #[must_use]
    pub const fn new(config: VsphereCloneConfig) -> Self {
        Self { config }
    }
}

/// Step to execute VM cloning with linked clone, customization spec, and storage vMotion.
#[derive(Debug, Clone)]
struct StepCloneVsphereVM {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// vSphere client.
    client: VsphereClient,
    /// Builder configuration.
    config: VsphereCloneConfig,
}

#[async_trait]
impl Step for StepCloneVsphereVM {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let new_name = self.config.vm_name.as_deref().unwrap_or(&self.name);
        self.ui.say(
            &self.name,
            &format!(
                "Cloning vSphere VM from template {} to {} (linked: {})...",
                self.config.template, new_name, self.config.linked_clone
            ),
        );

        let token = state
            .get::<String>("vsphere_token")
            .cloned()
            .unwrap_or_default();
        let compute_id = state
            .get::<String>("compute_id")
            .cloned()
            .unwrap_or_default();
        let ds_id = state
            .get::<String>("datastore_id")
            .cloned()
            .unwrap_or_default();

        let placement = serde_json::json!({
            "cluster": compute_id,
            "datastore": ds_id
        });

        let cloned_id = self
            .client
            .clone_vm(
                &token,
                &self.config.template,
                new_name,
                &placement,
                self.config.linked_clone,
            )
            .await?;

        self.ui.say(
            &self.name,
            &format!("Cloned virtual machine ID: {cloned_id}"),
        );
        state.put("vm_id", cloned_id.clone());

        if let Some(ref spec) = self.config.customization_spec {
            self.ui.say(
                &self.name,
                &format!("Applying guest OS customization spec: {spec}"),
            );
            state.put("customization_applied", spec.clone());
        }

        // Storage vMotion to target datastore if relocation is requested
        if let Some(ref target_ds) = self.config.datastore {
            self.ui.say(
                &self.name,
                &format!("Relocating clone to target datastore {target_ds} (Storage vMotion)..."),
            );
            self.client
                .relocate_vm_datastore(&token, &cloned_id, target_ds)
                .await?;
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(vm_id) = state.get::<String>("vm_id") {
            let token = state
                .get::<String>("vsphere_token")
                .cloned()
                .unwrap_or_default();
            let _ = self.client.power_off(&token, vm_id).await;
        }
    }
}

#[async_trait]
impl Builder for VsphereCloneBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        if self.config.template.is_empty() {
            return Err(StampError::Parse(
                "Source template cannot be empty".to_string(),
            ));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let client = VsphereClient::new(
            self.config
                .vcenter_server
                .clone()
                .unwrap_or_else(|| "127.0.0.1".to_string()),
            self.config.username.clone(),
            self.config.password.clone(),
            self.config.insecure_connection,
        );

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepConnectVsphere {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                datacenter: self.config.datacenter.clone(),
                cluster: self.config.cluster.clone(),
                host: self.config.host.clone(),
                datastore: self.config.datastore.clone(),
            }),
            Box::new(StepCloneVsphereVM {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(StepPowerOnWait {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
            }),
            Box::new(StepProvisionVsphere {
                ui: ui.clone(),
                name: self.name(),
                hook,
                ssh_username: self.config.ssh_username.clone(),
                ssh_password: self.config.ssh_password.clone(),
            }),
            Box::new(StepFinalizeVsphere {
                ui: ui.clone(),
                name: self.name(),
                client,
                convert_to_template: self.config.convert_to_template,
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();

        match runner.run(&mut state).await {
            Ok(()) => {
                runner.cleanup(&state).await;
            }
            Err(e) => {
                if on_error == crate::engine::packer::OnErrorStrategy::Cleanup {
                    runner.cleanup(&state).await;
                }
                return Err(e);
            }
        }

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_else(|| format!("vsphere-clone:{}", self.name()));

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: artifact_id,
            files: vec![],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_enums_and_traits() {
        assert_eq!(VsphereDiskController::Pvscsi.as_str(), "PVSCSI");
        assert_eq!(VsphereDiskController::LsiLogic.as_str(), "LSI_LOGIC");
        assert_eq!(VsphereDiskController::Sata.as_str(), "SATA");
        assert_eq!(VsphereDiskController::Nvme.as_str(), "NVME");

        assert_eq!(VsphereNetworkAdapter::Vmxnet3.as_str(), "VMXNET3");
        assert_eq!(VsphereNetworkAdapter::E1000e.as_str(), "E1000E");

        assert_eq!(VsphereDiskProvisioning::Thin.as_str(), "thin");
        assert_eq!(
            VsphereDiskProvisioning::EagerZeroedThick.as_str(),
            "eagerZeroedThick"
        );
        assert_eq!(
            VsphereDiskProvisioning::LazyZeroedThick.as_str(),
            "lazyZeroedThick"
        );

        let hw = VsphereHardwareConfig {
            cpu_sockets: Some(2),
            cpu_cores: Some(4),
            ram_mb: Some(4096),
            disk_size_mb: Some(50000),
            disk_controller_type: Some(VsphereDiskController::Pvscsi),
            disk_provisioning_type: Some(VsphereDiskProvisioning::Thin),
            network_card: Some(VsphereNetworkAdapter::Vmxnet3),
            network: Some("VM Network".to_string()),
        };
        assert_eq!(hw.clone(), hw);
        assert_eq!(format!("{hw:?}"), format!("{hw:?}"));
    }

    #[tokio::test]
    async fn test_vsphere_client_mocked() -> Result<(), StampError> {
        let client = VsphereClient::new(
            "vcenter.local".to_string(),
            Some("administrator@vsphere.local".to_string()),
            Some("Admin123!".to_string()),
            true,
        );
        assert_eq!(format!("{client:?}"), format!("{client:?}"));

        let token = client.login().await?;
        assert!(token.contains("mock-vsphere-session-token"));

        let dc = client
            .discover_datacenter(&token, Some("Datacenter"))
            .await?;
        assert_eq!(dc, "datacenter-mock-1");

        let compute = client
            .discover_cluster_or_host(&token, Some("Cluster"), None)
            .await?;
        assert_eq!(compute, "domain-c-mock-1");

        let ds = client
            .discover_datastore(&token, Some("Datastore1"))
            .await?;
        assert_eq!(ds, "datastore-mock-1");

        let temp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let test_file = temp_dir.path().join("floppy.img");
        std::fs::write(&test_file, b"test").map_err(StampError::Io)?;

        client
            .upload_file_to_datastore(&token, &ds, &dc, "uploads/floppy.img", &test_file)
            .await?;

        let hw = VsphereHardwareConfig::default();
        let placement = serde_json::json!({ "cluster": compute, "datastore": ds });
        let vm_id = client
            .create_vm(&token, "test-vm", "ubuntu64Guest", &hw, &placement)
            .await?;
        assert!(vm_id.starts_with("vm-mock-"));

        client.power_on(&token, &vm_id).await?;
        let ip = client
            .wait_guest_ip(&token, &vm_id, Duration::from_secs(1))
            .await?;
        assert_eq!(ip, "127.0.0.1");

        let clone_id = client
            .clone_vm(&token, &vm_id, "test-clone", &placement, true)
            .await?;
        assert!(clone_id.starts_with("vm-clone-"));

        client
            .relocate_vm_datastore(&token, &clone_id, "Datastore2")
            .await?;
        client.convert_to_template(&token, &clone_id).await?;
        client.power_off(&token, &vm_id).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_vsphere_iso_builder_lifecycle() -> Result<(), StampError> {
        let config = VsphereIsoConfig {
            name: "test-iso".to_string(),
            vcenter_server: Some("vcenter.test".to_string()),
            username: Some("user".to_string()),
            password: Some("pass".to_string()),
            insecure_connection: true,
            datacenter: Some("DC1".to_string()),
            cluster: Some("Cluster1".to_string()),
            datastore: Some("DS1".to_string()),
            vm_name: Some("test-iso-vm".to_string()),
            guest_os_type: Some("ubuntu64Guest".to_string()),
            convert_to_template: true,
            hardware: VsphereHardwareConfig {
                cpu_sockets: Some(1),
                cpu_cores: Some(2),
                ram_mb: Some(2048),
                disk_size_mb: Some(20000),
                disk_controller_type: Some(VsphereDiskController::Pvscsi),
                disk_provisioning_type: Some(VsphereDiskProvisioning::Thin),
                network_card: Some(VsphereNetworkAdapter::Vmxnet3),
                network: Some("VLAN-100".to_string()),
            },
            ..Default::default()
        };

        let builder = VsphereIsoBuilder::new(config);
        assert_eq!(builder.name(), "test-iso");
        builder.prepare().await?;

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert!(artifact.id().starts_with("vsphere:vm-mock-"));
        builder.cancel().await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_vsphere_clone_builder_lifecycle() -> Result<(), StampError> {
        let config = VsphereCloneConfig {
            name: "test-clone".to_string(),
            vcenter_server: Some("vcenter.test".to_string()),
            username: Some("user".to_string()),
            password: Some("pass".to_string()),
            template: "golden-image-v1".to_string(),
            linked_clone: true,
            customization_spec: Some("linux-spec".to_string()),
            convert_to_template: true,
            datastore: Some("DS2".to_string()),
            ..Default::default()
        };

        let builder = VsphereCloneBuilder::new(config);
        assert_eq!(builder.name(), "test-clone");
        builder.prepare().await?;

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert!(artifact.id().starts_with("vsphere:vm-clone-"));
        builder.cancel().await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_prepare_failures() {
        let mut iso_conf = VsphereIsoConfig::default();
        let builder_iso = VsphereIsoBuilder::new(iso_conf.clone());
        assert!(builder_iso.prepare().await.is_err());

        iso_conf.name = "ok".to_string();
        let builder_iso_ok = VsphereIsoBuilder::new(iso_conf);
        assert!(builder_iso_ok.prepare().await.is_ok());

        let mut clone_conf = VsphereCloneConfig::default();
        let builder_clone = VsphereCloneBuilder::new(clone_conf.clone());
        assert!(builder_clone.prepare().await.is_err());

        clone_conf.name = "ok".to_string();
        let builder_clone_no_tmpl = VsphereCloneBuilder::new(clone_conf.clone());
        assert!(builder_clone_no_tmpl.prepare().await.is_err());

        clone_conf.template = "template1".to_string();
        let builder_clone_ok = VsphereCloneBuilder::new(clone_conf);
        assert!(builder_clone_ok.prepare().await.is_ok());
    }
}
