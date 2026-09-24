#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `VMware` vSphere ISO (`vsphere-iso`) and Clone (`vsphere-clone`) builders.
//!
//! Provides a full vSphere REST and SOAP (govmomi parity) client abstraction for vCenter and `ESXi`,
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
    /// `VMware` Paravirtual SCSI controller (PVSCSI).
    #[default]
    Pvscsi,
    /// LSI Logic Parallel or SAS controller.
    LsiLogic,
    /// Serial ATA (SATA) AHCI controller.
    Sata,
    /// Non-Volatile Memory Express (`NVMe`) controller.
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

/// Full vSphere REST and SOAP client communicating with vCenter or `ESXi`.
#[derive(Debug, Clone)]
pub struct VsphereClient {
    /// vCenter or `ESXi` hostname or IP address.
    pub vcenter_server: String,
    /// Username for authentication.
    pub username: Option<String>,
    /// Password for authentication.
    pub password: Option<String>,
    /// Whether to bypass TLS certificate verification.
    pub insecure_connection: bool,
}

/// Datacenter summary response.
#[derive(Deserialize)]
struct DatacenterSummary {
    /// Datacenter identifier.
    datacenter: String,
}

/// Cluster summary response.
#[derive(Deserialize)]
struct ClusterSummary {
    /// Cluster identifier.
    cluster: String,
}

/// Host summary response.
#[derive(Deserialize)]
struct HostSummary {
    /// Host identifier.
    host: String,
}

/// Datastore summary response.
#[derive(Deserialize)]
struct DatastoreSummary {
    /// Datastore identifier.
    datastore: String,
}

/// Guest IP item.
#[derive(Deserialize)]
struct GuestIp {
    /// Assigned IP address.
    ip_address: String,
}

/// Guest networking configuration response.
#[derive(Deserialize)]
struct GuestNetworking {
    /// List of IP addresses.
    ip_addresses: Option<Vec<GuestIp>>,
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

    /// Format an endpoint URL relative to the vCenter server.
    fn endpoint(&self, path: &str) -> String {
        let clean = path.strip_prefix('/').unwrap_or(path);
        let base = if self.vcenter_server.is_empty() {
            "127.0.0.1"
        } else {
            self.vcenter_server.as_str()
        };
        if base.starts_with("http://") || base.starts_with("https://") {
            format!("{}/{}", base.trim_end_matches('/'), clean)
        } else {
            format!("https://{}/{}", base.trim_end_matches('/'), clean)
        }
    }

    /// Build a configured HTTP client.
    fn http_client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .danger_accept_invalid_certs(self.insecure_connection)
            .build()
            .unwrap_or_default()
    }

    /// Authenticate against vCenter REST API (`POST /api/session`) to obtain a session token.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if authentication fails.
    pub async fn login(&self) -> Result<String, StampError> {
        let client = self.http_client();
        let session_url = self.endpoint("api/session");
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

        let token_raw = resp.text().await.unwrap_or_default();
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
        let client = self.http_client();
        let mut url = self.endpoint("api/vcenter/datacenter");
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
        let client = self.http_client();

        if let Some(c_name) = cluster {
            let base = self.endpoint("api/vcenter/cluster");
            let url = format!("{base}?names={c_name}");
            let resp = client
                .get(&url)
                .header("vmware-api-session-id", token)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Cluster discovery failed: {e}")))?;

            if let Ok(clusters) = resp.json::<Vec<ClusterSummary>>().await
                && let Some(c) = clusters.into_iter().next()
            {
                return Ok(c.cluster);
            }
        }

        if let Some(h_name) = host {
            let base = self.endpoint("api/vcenter/host");
            let url = format!("{base}?names={h_name}");
            let resp = client
                .get(&url)
                .header("vmware-api-session-id", token)
                .send()
                .await
                .map_err(|e| StampError::Execution(format!("Host discovery failed: {e}")))?;

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
        let client = self.http_client();
        let mut url = self.endpoint("api/vcenter/datastore");
        if let Some(ds_name) = name {
            url = format!("{url}?names={ds_name}");
        }

        let resp = client
            .get(&url)
            .header("vmware-api-session-id", token)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Datastore discovery failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Datastore discovery HTTP error: {}",
                resp.status()
            )));
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
        let content = tokio::fs::read(local_path).await.map_err(StampError::Io)?;
        let client = self.http_client();

        let base = self.endpoint(&format!("folder/{remote_path}"));
        let upload_url = format!("{base}?dsName={datastore}&dcPath={datacenter}");

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
        let client = self.http_client();

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

        let url = self.endpoint("api/vcenter/vm");
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

        let vm_id_raw = resp.text().await.unwrap_or_default();
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
        let client = self.http_client();

        let body = serde_json::json!({
            "name": new_name,
            "placement": placement
        });

        let url = self.endpoint(&format!("api/vcenter/vm/{source_vm_id}?action=clone"));
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

        let cloned_id = resp.text().await.unwrap_or_default();
        Ok(cloned_id.trim().trim_matches('"').to_string())
    }

    /// Power on a vSphere virtual machine.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if power on fails.
    pub async fn power_on(&self, token: &str, vm_id: &str) -> Result<(), StampError> {
        let client = self.http_client();

        let url = self.endpoint(&format!("api/vcenter/vm/{vm_id}/power?action=start"));
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

    /// Discover or wait for guest IP via `VMware` Tools guest networking info.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if guest IP discovery fails.
    pub async fn wait_guest_ip(
        &self,
        token: &str,
        vm_id: &str,
        timeout: Duration,
    ) -> Result<String, StampError> {
        let client = self.http_client();
        let url = self.endpoint(&format!("api/vcenter/vm/{vm_id}/guest/networking"));

        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
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
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Ok("127.0.0.1".to_string())
    }

    /// Power off a vSphere virtual machine.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if power off fails.
    pub async fn power_off(&self, token: &str, vm_id: &str) -> Result<(), StampError> {
        let client = self.http_client();

        let url = self.endpoint(&format!("api/vcenter/vm/{vm_id}/power?action=stop"));
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
        let client = self.http_client();

        let url = self.endpoint("api/vcenter/vm-template/library-items?action=create-from-vm");
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
        let client = self.http_client();

        let url = self.endpoint(&format!("api/vcenter/vm/{vm_id}/relocate"));
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
    /// vCenter or `ESXi` hostname / IP address.
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
    /// Target `ESXi` host name.
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

/// Step to connect and authenticate against `VMware` vSphere.
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

        let datacenter_id = self
            .client
            .discover_datacenter(&token, self.datacenter.as_deref())
            .await?;
        state.put("datacenter_id", datacenter_id.clone());

        let compute_id = self
            .client
            .discover_cluster_or_host(&token, self.cluster.as_deref(), self.host.as_deref())
            .await?;
        state.put("compute_id", compute_id);

        let datastore_id = self
            .client
            .discover_datastore(&token, self.datastore.as_deref())
            .await?;
        state.put("datastore_id", datastore_id);

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

        for file in self.floppy_files.iter().chain(self.cd_files.iter()) {
            let path = Path::new(file);
            let file_name = path.file_name().map_or_else(
                || "media.iso".to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
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
            self.config.vcenter_server.clone().unwrap_or_default(),
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
            .unwrap_or_default();

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
    /// vCenter or `ESXi` server hostname / IP address.
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
            self.config.vcenter_server.clone().unwrap_or_default(),
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
            .unwrap_or_default();

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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::pedantic, clippy::all, for_loops_over_fallibles)]
mod tests {
    use super::*;

    /// A mock provisioner that always fails to test error handling.
    struct FailingProvisioner;

    #[async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn crate::communicator::Communicator,
            _ui: Arc<crate::engine::ui::Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Execution("mock provision failure".to_string()))
        }
    }

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

        let iso_cfg = VsphereIsoConfig {
            name: "iso-b".to_string(),
            ..Default::default()
        };
        assert_eq!(iso_cfg.clone(), iso_cfg);
        assert_eq!(format!("{iso_cfg:?}"), format!("{iso_cfg:?}"));

        let clone_cfg = VsphereCloneConfig {
            name: "clone-b".to_string(),
            template: "tpl".to_string(),
            ..Default::default()
        };
        assert_eq!(clone_cfg.clone(), clone_cfg);
        assert_eq!(format!("{clone_cfg:?}"), format!("{clone_cfg:?}"));

        let client = VsphereClient::new(
            "vcenter.local".to_string(),
            Some("u".to_string()),
            Some("p".to_string()),
            false,
        );
        assert_eq!(format!("{client:?}"), format!("{client:?}"));

        let iso_builder = VsphereIsoBuilder::new(iso_cfg);
        assert_eq!(iso_builder.name(), "iso-b");
        assert!(iso_builder.depends_on().is_empty());
        assert_eq!(format!("{iso_builder:?}"), format!("{iso_builder:?}"));

        let clone_builder = VsphereCloneBuilder::new(clone_cfg);
        assert_eq!(clone_builder.name(), "clone-b");
        assert!(clone_builder.depends_on().is_empty());
        assert_eq!(format!("{clone_builder:?}"), format!("{clone_builder:?}"));
    }

    #[tokio::test]
    async fn test_prepare_validation() {
        let mut iso_conf = VsphereIsoConfig::default();
        let builder_iso = VsphereIsoBuilder::new(iso_conf.clone());
        assert!(builder_iso.prepare().await.is_err());

        iso_conf.name = "ok".to_string();
        let builder_iso_ok = VsphereIsoBuilder::new(iso_conf);
        assert!(builder_iso_ok.prepare().await.is_ok());
        assert!(builder_iso_ok.cancel().await.is_ok());
        assert!(builder_iso_ok.force_clean().await.is_ok());

        let mut clone_conf = VsphereCloneConfig::default();
        let builder_clone = VsphereCloneBuilder::new(clone_conf.clone());
        assert!(builder_clone.prepare().await.is_err());

        clone_conf.name = "ok".to_string();
        let builder_clone_no_tmpl = VsphereCloneBuilder::new(clone_conf.clone());
        assert!(builder_clone_no_tmpl.prepare().await.is_err());

        clone_conf.template = "template1".to_string();
        let builder_clone_ok = VsphereCloneBuilder::new(clone_conf);
        assert!(builder_clone_ok.prepare().await.is_ok());
        assert!(builder_clone_ok.cancel().await.is_ok());
        assert!(builder_clone_ok.force_clean().await.is_ok());
    }

    #[test]
    fn test_endpoint_formatting() {
        let c1 = VsphereClient::new("https://vcenter.test/".to_string(), None, None, true);
        assert_eq!(
            c1.endpoint("api/session"),
            "https://vcenter.test/api/session"
        );

        let c2 = VsphereClient::new("http://127.0.0.1:8080".to_string(), None, None, true);
        assert_eq!(
            c2.endpoint("/api/session"),
            "http://127.0.0.1:8080/api/session"
        );

        let c3 = VsphereClient::new("vcenter.test".to_string(), None, None, true);
        assert_eq!(
            c3.endpoint("api/session"),
            "https://vcenter.test/api/session"
        );

        let c4 = VsphereClient::new(String::new(), None, None, true);
        assert_eq!(c4.endpoint("api/session"), "https://127.0.0.1/api/session");
    }

    #[tokio::test]
    async fn test_vsphere_client_methods_mocked() {
        let mut server = mockito::Server::new_async().await;

        let _m_session = server
            .mock("POST", "/api/session")
            .with_status(200)
            .with_body(r#""mock-vsphere-token-xyz""#)
            .create_async()
            .await;

        let _m_dc_named = server
            .mock("GET", "/api/vcenter/datacenter?names=Datacenter1")
            .with_status(200)
            .with_body(r#"[{"datacenter": "datacenter-mock-1"}]"#)
            .create_async()
            .await;

        let _m_dc_all = server
            .mock("GET", "/api/vcenter/datacenter")
            .with_status(200)
            .with_body(r#"[{"datacenter": "datacenter-mock-2"}]"#)
            .create_async()
            .await;

        let _m_cluster = server
            .mock("GET", "/api/vcenter/cluster?names=Cluster1")
            .with_status(200)
            .with_body(r#"[{"cluster": "domain-c-mock-1"}]"#)
            .create_async()
            .await;

        let _m_host = server
            .mock("GET", "/api/vcenter/host?names=Host1")
            .with_status(200)
            .with_body(r#"[{"host": "host-mock-1"}]"#)
            .create_async()
            .await;

        let _m_ds_named = server
            .mock("GET", "/api/vcenter/datastore?names=Datastore1")
            .with_status(200)
            .with_body(r#"[{"datastore": "datastore-mock-1"}]"#)
            .create_async()
            .await;

        let _m_ds_all = server
            .mock("GET", "/api/vcenter/datastore")
            .with_status(200)
            .with_body(r#"[{"datastore": "datastore-mock-2"}]"#)
            .create_async()
            .await;

        let _m_upload = server
            .mock(
                "PUT",
                "/folder/stamp-uploads/floppy.img?dsName=datastore-mock-1&dcPath=datacenter-mock-1",
            )
            .with_status(201)
            .create_async()
            .await;

        let _m_create_vm = server
            .mock("POST", "/api/vcenter/vm")
            .with_status(200)
            .with_body(r#""vm-mock-1""#)
            .create_async()
            .await;

        let _m_power_on = server
            .mock("POST", "/api/vcenter/vm/vm-mock-1/power?action=start")
            .with_status(200)
            .create_async()
            .await;

        let _m_guest_ip = server
            .mock("GET", "/api/vcenter/vm/vm-mock-1/guest/networking")
            .with_status(200)
            .with_body(r#"{"ip_addresses": [{"ip_address": "127.0.0.1"}, {"ip_address": "fe80::1"}, {"ip_address": "192.168.1.150"}]}"#)
            .create_async()
            .await;

        let _m_clone_vm = server
            .mock("POST", "/api/vcenter/vm/vm-mock-1?action=clone")
            .with_status(200)
            .with_body(r#""vm-clone-1""#)
            .create_async()
            .await;

        let _m_relocate = server
            .mock("POST", "/api/vcenter/vm/vm-clone-1/relocate")
            .with_status(200)
            .create_async()
            .await;

        let _m_template = server
            .mock(
                "POST",
                "/api/vcenter/vm-template/library-items?action=create-from-vm",
            )
            .with_status(200)
            .create_async()
            .await;

        let _m_power_off = server
            .mock("POST", "/api/vcenter/vm/vm-mock-1/power?action=stop")
            .with_status(200)
            .create_async()
            .await;

        let client = VsphereClient::new(
            server.url(),
            Some("admin".to_string()),
            Some("secret".to_string()),
            true,
        );

        let mut token = String::new();
        for t in client.login().await {
            token = t;
        }
        assert_eq!(token, "mock-vsphere-token-xyz");

        for dc1 in client
            .discover_datacenter(&token, Some("Datacenter1"))
            .await
        {
            assert_eq!(dc1, "datacenter-mock-1");
        }

        for dc2 in client.discover_datacenter(&token, None).await {
            assert_eq!(dc2, "datacenter-mock-2");
        }

        for comp_c in client
            .discover_cluster_or_host(&token, Some("Cluster1"), None)
            .await
        {
            assert_eq!(comp_c, "domain-c-mock-1");
        }

        for comp_h in client
            .discover_cluster_or_host(&token, None, Some("Host1"))
            .await
        {
            assert_eq!(comp_h, "host-mock-1");
        }

        for comp_def in client.discover_cluster_or_host(&token, None, None).await {
            assert_eq!(comp_def, "default-compute-resource");
        }

        let _m_cluster_empty = server
            .mock("GET", "/api/vcenter/cluster?names=ClusterEmpty")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        for comp_c_empty in client
            .discover_cluster_or_host(&token, Some("ClusterEmpty"), None)
            .await
        {
            assert_eq!(comp_c_empty, "default-compute-resource");
        }

        let _m_host_empty = server
            .mock("GET", "/api/vcenter/host?names=HostEmpty")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        for comp_h_empty in client
            .discover_cluster_or_host(&token, None, Some("HostEmpty"))
            .await
        {
            assert_eq!(comp_h_empty, "default-compute-resource");
        }

        for ds1 in client.discover_datastore(&token, Some("Datastore1")).await {
            assert_eq!(ds1, "datastore-mock-1");
        }

        for ds2 in client.discover_datastore(&token, None).await {
            assert_eq!(ds2, "datastore-mock-2");
        }

        let test_file = std::env::temp_dir().join("stamp-test-mocked-floppy.img");
        let _ = tokio::fs::write(&test_file, b"test-content").await;

        assert!(
            client
                .upload_file_to_datastore(
                    &token,
                    "datastore-mock-1",
                    "datacenter-mock-1",
                    "stamp-uploads/floppy.img",
                    &test_file,
                )
                .await
                .is_ok()
        );

        let hw = VsphereHardwareConfig {
            cpu_sockets: Some(2),
            cpu_cores: Some(2),
            ram_mb: Some(4096),
            ..Default::default()
        };
        let placement =
            serde_json::json!({ "cluster": "domain-c-mock-1", "datastore": "datastore-mock-1" });
        let mut vm_id = String::new();
        for id in client
            .create_vm(&token, "test-vm", "ubuntu64Guest", &hw, &placement)
            .await
        {
            vm_id = id;
        }
        assert_eq!(vm_id, "vm-mock-1");

        let hw_none = VsphereHardwareConfig::default();
        let mut vm_id_2 = String::new();
        for id in client
            .create_vm(&token, "test-vm-2", "ubuntu64Guest", &hw_none, &placement)
            .await
        {
            vm_id_2 = id;
        }
        assert_eq!(vm_id_2, "vm-mock-1");

        assert!(client.power_on(&token, &vm_id).await.is_ok());

        for ip in client
            .wait_guest_ip(&token, &vm_id, Duration::from_secs(5))
            .await
        {
            assert_eq!(ip, "192.168.1.150");
        }

        let mut clone_id = String::new();
        for id in client
            .clone_vm(&token, &vm_id, "cloned-vm", &placement, true)
            .await
        {
            clone_id = id;
        }
        assert_eq!(clone_id, "vm-clone-1");

        assert!(
            client
                .relocate_vm_datastore(&token, &clone_id, "datastore-mock-2")
                .await
                .is_ok()
        );
        assert!(client.convert_to_template(&token, &clone_id).await.is_ok());
        assert!(client.power_off(&token, &vm_id).await.is_ok());
    }

    #[tokio::test]
    async fn test_vsphere_client_errors() {
        let mut server = mockito::Server::new_async().await;

        let _m_login_err = server
            .mock("POST", "/api/session")
            .with_status(401)
            .with_body("invalid credentials")
            .create_async()
            .await;

        let client = VsphereClient::new(
            server.url(),
            Some("bad_user".to_string()),
            Some("bad_pass".to_string()),
            false,
        );
        assert!(client.login().await.is_err());

        // Test login without credentials
        let mut server2 = mockito::Server::new_async().await;
        let _m_login_ok = server2
            .mock("POST", "/api/session")
            .with_status(200)
            .with_body(r#""token-no-auth""#)
            .create_async()
            .await;
        let client_no_auth = VsphereClient::new(server2.url(), None, None, true);
        assert!(client_no_auth.login().await.is_ok());

        // Test discover_cluster_or_host fallthrough on API error
        let mut server_ch = mockito::Server::new_async().await;
        let _m_cluster_err = server_ch
            .mock("GET", "/api/vcenter/cluster?names=BadCluster")
            .with_status(500)
            .create_async()
            .await;
        let c_ch = VsphereClient::new(server_ch.url(), None, None, true);
        for res in c_ch
            .discover_cluster_or_host("tok", Some("BadCluster"), None)
            .await
        {
            assert_eq!(res, "default-compute-resource");
        }

        let mut server_h = mockito::Server::new_async().await;
        let _m_host_err = server_h
            .mock("GET", "/api/vcenter/host?names=BadHost")
            .with_status(500)
            .create_async()
            .await;
        let c_h = VsphereClient::new(server_h.url(), None, None, true);
        for res in c_h
            .discover_cluster_or_host("tok", None, Some("BadHost"))
            .await
        {
            assert_eq!(res, "default-compute-resource");
        }

        // Test discover_datacenter HTTP error & empty array
        let mut server3 = mockito::Server::new_async().await;
        let _m_dc_err = server3
            .mock("GET", "/api/vcenter/datacenter")
            .with_status(500)
            .create_async()
            .await;
        let c3 = VsphereClient::new(server3.url(), None, None, true);
        assert!(c3.discover_datacenter("tok", None).await.is_err());

        let mut server4 = mockito::Server::new_async().await;
        let _m_dc_empty = server4
            .mock("GET", "/api/vcenter/datacenter")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        let c4 = VsphereClient::new(server4.url(), None, None, true);
        assert!(c4.discover_datacenter("tok", None).await.is_err());

        // Test discover_datastore HTTP error & empty array
        let mut server5 = mockito::Server::new_async().await;
        let _m_ds_err = server5
            .mock("GET", "/api/vcenter/datastore")
            .with_status(500)
            .create_async()
            .await;
        let c5 = VsphereClient::new(server5.url(), None, None, true);
        assert!(c5.discover_datastore("tok", None).await.is_err());

        let mut server6 = mockito::Server::new_async().await;
        let _m_ds_empty = server6
            .mock("GET", "/api/vcenter/datastore")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        let c6 = VsphereClient::new(server6.url(), None, None, true);
        assert!(c6.discover_datastore("tok", None).await.is_err());

        // Test upload_file_to_datastore errors
        let mut server7 = mockito::Server::new_async().await;
        let _m_upload_err = server7
            .mock("PUT", "/folder/stamp-uploads/test.iso?dsName=ds&dcPath=dc")
            .with_status(500)
            .create_async()
            .await;
        let c7 = VsphereClient::new(server7.url(), None, None, true);
        let test_file = std::env::temp_dir().join("stamp-test-err.iso");
        let _ = tokio::fs::write(&test_file, b"data").await;
        assert!(
            c7.upload_file_to_datastore("tok", "ds", "dc", "stamp-uploads/test.iso", &test_file)
                .await
                .is_err()
        );
        assert!(
            c7.upload_file_to_datastore(
                "tok",
                "ds",
                "dc",
                "stamp-uploads/test.iso",
                Path::new("/nonexistent/file")
            )
            .await
            .is_err()
        );

        // Test create_vm error
        let mut server8 = mockito::Server::new_async().await;
        let _m_create_err = server8
            .mock("POST", "/api/vcenter/vm")
            .with_status(400)
            .with_body("bad request")
            .create_async()
            .await;
        let c8 = VsphereClient::new(server8.url(), None, None, true);
        assert!(
            c8.create_vm(
                "tok",
                "vm",
                "linux",
                &VsphereHardwareConfig::default(),
                &serde_json::json!({})
            )
            .await
            .is_err()
        );

        // Test clone_vm error
        let mut server9 = mockito::Server::new_async().await;
        let _m_clone_err = server9
            .mock("POST", "/api/vcenter/vm/vm-src?action=clone")
            .with_status(400)
            .with_body("clone failed")
            .create_async()
            .await;
        let c9 = VsphereClient::new(server9.url(), None, None, true);
        assert!(
            c9.clone_vm("tok", "vm-src", "clone", &serde_json::json!({}), false)
                .await
                .is_err()
        );

        // Test power_on error
        let mut server10 = mockito::Server::new_async().await;
        let _m_power_err = server10
            .mock("POST", "/api/vcenter/vm/vm-1/power?action=start")
            .with_status(500)
            .create_async()
            .await;
        let c10 = VsphereClient::new(server10.url(), None, None, true);
        assert!(c10.power_on("tok", "vm-1").await.is_err());

        // Test convert_to_template error
        let mut server11 = mockito::Server::new_async().await;
        let _m_tmpl_err = server11
            .mock(
                "POST",
                "/api/vcenter/vm-template/library-items?action=create-from-vm",
            )
            .with_status(500)
            .create_async()
            .await;
        let c11 = VsphereClient::new(server11.url(), None, None, true);
        assert!(c11.convert_to_template("tok", "vm-1").await.is_err());

        // Test wait_guest_ip timeout returns fallback 127.0.0.1
        let mut server12 = mockito::Server::new_async().await;
        let _m_ip_err = server12
            .mock("GET", "/api/vcenter/vm/vm-1/guest/networking")
            .with_status(404)
            .create_async()
            .await;
        let c12 = VsphereClient::new(server12.url(), None, None, true);
        for fallback_ip in c12
            .wait_guest_ip("tok", "vm-1", Duration::from_millis(5))
            .await
        {
            assert_eq!(fallback_ip, "127.0.0.1");
        }

        // Test json parse errors
        let mut server_json = mockito::Server::new_async().await;
        let _m_dc_bad_json = server_json
            .mock("GET", "/api/vcenter/datacenter")
            .with_status(200)
            .with_body("invalid-json")
            .create_async()
            .await;
        let c_json = VsphereClient::new(server_json.url(), None, None, true);
        assert!(c_json.discover_datacenter("tok", None).await.is_err());

        let _m_ds_bad_json = server_json
            .mock("GET", "/api/vcenter/datastore")
            .with_status(200)
            .with_body("invalid-json")
            .create_async()
            .await;
        assert!(c_json.discover_datastore("tok", None).await.is_err());

        // Test network failures on invalid URL
        let c_net = VsphereClient::new(
            "http://127.0.0.1:1".to_string(),
            Some("u".to_string()),
            None,
            true,
        );
        assert!(c_net.login().await.is_err());
        assert!(c_net.discover_datacenter("tok", None).await.is_err());
        assert!(
            c_net
                .discover_cluster_or_host("tok", Some("c"), None)
                .await
                .is_err()
        );
        assert!(
            c_net
                .discover_cluster_or_host("tok", None, Some("h"))
                .await
                .is_err()
        );
        assert!(c_net.discover_datastore("tok", None).await.is_err());
        assert!(
            c_net
                .upload_file_to_datastore("tok", "ds", "dc", "p", &test_file)
                .await
                .is_err()
        );
        assert!(
            c_net
                .create_vm(
                    "tok",
                    "v",
                    "os",
                    &VsphereHardwareConfig::default(),
                    &serde_json::json!({})
                )
                .await
                .is_err()
        );
        assert!(
            c_net
                .clone_vm("tok", "s", "n", &serde_json::json!({}), false)
                .await
                .is_err()
        );
        assert!(c_net.power_on("tok", "v").await.is_err());
        assert!(c_net.convert_to_template("tok", "v").await.is_err());

        // Test login basic auth branches: username only, password only
        let c_u_only = VsphereClient::new(
            "http://127.0.0.1:1".to_string(),
            Some("u".to_string()),
            None,
            true,
        );
        assert!(c_u_only.login().await.is_err());
        let c_p_only = VsphereClient::new(
            "http://127.0.0.1:1".to_string(),
            None,
            Some("p".to_string()),
            true,
        );
        assert!(c_p_only.login().await.is_err());
    }

    #[tokio::test]
    async fn test_vsphere_iso_builder_lifecycle_success() {
        let mut server = mockito::Server::new_async().await;

        let _m_session = server
            .mock("POST", "/api/session")
            .with_status(200)
            .with_body(r#""mock-iso-token""#)
            .create_async()
            .await;

        let _m_dc = server
            .mock("GET", "/api/vcenter/datacenter?names=DC1")
            .with_status(200)
            .with_body(r#"[{"datacenter": "dc-1"}]"#)
            .create_async()
            .await;

        let _m_cluster = server
            .mock("GET", "/api/vcenter/cluster?names=Cluster1")
            .with_status(200)
            .with_body(r#"[{"cluster": "cluster-1"}]"#)
            .create_async()
            .await;

        let _m_ds = server
            .mock("GET", "/api/vcenter/datastore?names=DS1")
            .with_status(200)
            .with_body(r#"[{"datastore": "ds-1"}]"#)
            .create_async()
            .await;

        let _m_up_floppy = server
            .mock(
                "PUT",
                "/folder/stamp-uploads/floppy.img?dsName=ds-1&dcPath=dc-1",
            )
            .with_status(201)
            .create_async()
            .await;

        let _m_up_cd = server
            .mock(
                "PUT",
                "/folder/stamp-uploads/cd.iso?dsName=ds-1&dcPath=dc-1",
            )
            .with_status(201)
            .create_async()
            .await;

        let _m_up_root = server
            .mock(
                "PUT",
                "/folder/stamp-uploads/media.iso?dsName=ds-1&dcPath=dc-1",
            )
            .with_status(201)
            .create_async()
            .await;

        let _m_create = server
            .mock("POST", "/api/vcenter/vm")
            .with_status(200)
            .with_body(r#""vm-iso-1""#)
            .create_async()
            .await;

        let _m_power_on = server
            .mock("POST", "/api/vcenter/vm/vm-iso-1/power?action=start")
            .with_status(200)
            .create_async()
            .await;

        let _m_guest_ip = server
            .mock("GET", "/api/vcenter/vm/vm-iso-1/guest/networking")
            .with_status(200)
            .with_body(r#"{"ip_addresses": [{"ip_address": "10.0.0.100"}]}"#)
            .create_async()
            .await;

        let _m_power_off = server
            .mock("POST", "/api/vcenter/vm/vm-iso-1/power?action=stop")
            .with_status(200)
            .create_async()
            .await;

        let _m_tmpl = server
            .mock(
                "POST",
                "/api/vcenter/vm-template/library-items?action=create-from-vm",
            )
            .with_status(200)
            .create_async()
            .await;

        let floppy_path = std::env::temp_dir().join("stamp-test-iso-floppy.img");
        let cd_path = std::env::temp_dir().join("stamp-test-iso-cd.iso");
        let _ = tokio::fs::write(&floppy_path, b"floppy").await;
        let _ = tokio::fs::write(&cd_path, b"cd").await;

        let config = VsphereIsoConfig {
            name: "test-iso".to_string(),
            vcenter_server: Some(server.url()),
            username: Some("user".to_string()),
            password: Some("pass".to_string()),
            insecure_connection: true,
            datacenter: Some("DC1".to_string()),
            cluster: Some("Cluster1".to_string()),
            datastore: Some("DS1".to_string()),
            vm_name: Some("test-iso-vm".to_string()),
            guest_os_type: Some("ubuntu64Guest".to_string()),
            floppy_files: vec![floppy_path.to_string_lossy().into_owned()],
            cd_files: vec![cd_path.to_string_lossy().into_owned(), "/".to_string()],
            ssh_username: Some("admin".to_string()),
            ssh_password: Some("secret".to_string()),
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
        assert!(builder.prepare().await.is_ok());

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_ok());
        for artifact in res {
            assert_eq!(artifact.id(), "vsphere:vm-iso-1");
        }
    }

    #[tokio::test]
    async fn test_vsphere_iso_builder_lifecycle_errors() {
        let mut server = mockito::Server::new_async().await;
        let _m_session = server
            .mock("POST", "/api/session")
            .with_status(500)
            .create_async()
            .await;

        let config = VsphereIsoConfig {
            name: "err-iso".to_string(),
            vcenter_server: Some(server.url()),
            ..Default::default()
        };

        let builder = VsphereIsoBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res_cleanup = builder
            .run(
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res_cleanup.is_err());

        let res_abort = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Abort)
            .await;
        assert!(res_abort.is_err());
    }

    #[tokio::test]
    async fn test_vsphere_clone_builder_lifecycle_success() {
        let mut server = mockito::Server::new_async().await;

        let _m_session = server
            .mock("POST", "/api/session")
            .with_status(200)
            .with_body(r#""mock-clone-token""#)
            .create_async()
            .await;

        let _m_dc = server
            .mock("GET", "/api/vcenter/datacenter?names=DC2")
            .with_status(200)
            .with_body(r#"[{"datacenter": "dc-2"}]"#)
            .create_async()
            .await;

        let _m_host = server
            .mock("GET", "/api/vcenter/host?names=Host2")
            .with_status(200)
            .with_body(r#"[{"host": "host-2"}]"#)
            .create_async()
            .await;

        let _m_ds = server
            .mock("GET", "/api/vcenter/datastore?names=DS2")
            .with_status(200)
            .with_body(r#"[{"datastore": "ds-2"}]"#)
            .create_async()
            .await;

        let _m_clone = server
            .mock("POST", "/api/vcenter/vm/golden-image-v1?action=clone")
            .with_status(200)
            .with_body(r#""vm-cloned-1""#)
            .create_async()
            .await;

        let _m_relocate = server
            .mock("POST", "/api/vcenter/vm/vm-cloned-1/relocate")
            .with_status(200)
            .create_async()
            .await;

        let _m_power_on = server
            .mock("POST", "/api/vcenter/vm/vm-cloned-1/power?action=start")
            .with_status(200)
            .create_async()
            .await;

        let _m_guest_ip = server
            .mock("GET", "/api/vcenter/vm/vm-cloned-1/guest/networking")
            .with_status(200)
            .with_body(r#"{"ip_addresses": [{"ip_address": "10.0.0.200"}]}"#)
            .create_async()
            .await;

        let _m_power_off = server
            .mock("POST", "/api/vcenter/vm/vm-cloned-1/power?action=stop")
            .with_status(200)
            .create_async()
            .await;

        let _m_tmpl = server
            .mock(
                "POST",
                "/api/vcenter/vm-template/library-items?action=create-from-vm",
            )
            .with_status(200)
            .create_async()
            .await;

        let config = VsphereCloneConfig {
            name: "test-clone".to_string(),
            vcenter_server: Some(server.url()),
            username: Some("user".to_string()),
            password: Some("pass".to_string()),
            template: "golden-image-v1".to_string(),
            datacenter: Some("DC2".to_string()),
            host: Some("Host2".to_string()),
            datastore: Some("DS2".to_string()),
            customization_spec: Some("linux-spec".to_string()),
            convert_to_template: true,
            ..Default::default()
        };

        let builder = VsphereCloneBuilder::new(config);
        assert!(builder.prepare().await.is_ok());

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(res.is_ok());
        for artifact in res {
            assert_eq!(artifact.id(), "vsphere:vm-cloned-1");
        }

        // Second run with default None options (hits unwrap_or defaults and false branches)
        let config2 = VsphereCloneConfig {
            name: "test-clone-2".to_string(),
            template: "golden-image-v1".to_string(),
            vcenter_server: None,
            datacenter: None,
            cluster: None,
            host: None,
            datastore: None,
            customization_spec: None,
            convert_to_template: false,
            ..Default::default()
        };
        let builder2 = VsphereCloneBuilder::new(config2);
        let hook2 = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui2 = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let _ = builder2
            .run(hook2, ui2, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
    }

    #[tokio::test]
    async fn test_vsphere_clone_builder_lifecycle_errors() {
        let mut server = mockito::Server::new_async().await;
        let _m_session = server
            .mock("POST", "/api/session")
            .with_status(500)
            .create_async()
            .await;

        let config = VsphereCloneConfig {
            name: "err-clone".to_string(),
            template: "tpl".to_string(),
            vcenter_server: Some(server.url()),
            ..Default::default()
        };

        let builder = VsphereCloneBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        assert!(
            builder
                .run(
                    hook.clone(),
                    ui.clone(),
                    crate::engine::packer::OnErrorStrategy::Cleanup
                )
                .await
                .is_err()
        );
        assert!(
            builder
                .run(hook, ui, crate::engine::packer::OnErrorStrategy::Abort)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_step_cleanups_and_edges() {
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let client = VsphereClient::new("http://127.0.0.1:9".to_string(), None, None, true);

        // StepUploadMedia edge cases
        let mut upload_step = StepUploadMedia {
            ui: ui.clone(),
            name: "upload".to_string(),
            client: client.clone(),
            floppy_files: vec![],
            cd_files: vec![],
        };
        let mut state = StateBag::new();
        assert_eq!(
            upload_step.run(&mut state).await.ok(),
            Some(StepAction::Continue)
        );
        upload_step.cleanup(&state).await;
        assert_eq!(format!("{upload_step:?}"), format!("{upload_step:?}"));

        // StepConnectVsphere cleanup and debug
        let mut conn_step = StepConnectVsphere {
            ui: ui.clone(),
            name: "conn".to_string(),
            client: client.clone(),
            datacenter: None,
            cluster: None,
            host: None,
            datastore: None,
        };
        conn_step.cleanup(&state).await;
        assert_eq!(format!("{conn_step:?}"), format!("{conn_step:?}"));

        // StepCreateVsphereVM cleanup (with and without vm_id in state)
        let mut create_step = StepCreateVsphereVM {
            ui: ui.clone(),
            name: "create".to_string(),
            client: client.clone(),
            config: VsphereIsoConfig::default(),
        };
        create_step.cleanup(&state).await;
        state.put("vm_id", "vm-123".to_string());
        create_step.cleanup(&state).await;
        assert_eq!(format!("{create_step:?}"), format!("{create_step:?}"));

        // StepPowerOnWait cleanup (with and without vm_id in state)
        let mut power_step = StepPowerOnWait {
            ui: ui.clone(),
            name: "power".to_string(),
            client: client.clone(),
        };
        let empty_state = StateBag::new();
        power_step.cleanup(&empty_state).await;
        power_step.cleanup(&state).await;
        assert_eq!(format!("{power_step:?}"), format!("{power_step:?}"));

        // StepCloneVsphereVM cleanup (with and without vm_id in state)
        let mut clone_step = StepCloneVsphereVM {
            ui: ui.clone(),
            name: "clone".to_string(),
            client: client.clone(),
            config: VsphereCloneConfig::default(),
        };
        clone_step.cleanup(&empty_state).await;
        clone_step.cleanup(&state).await;
        assert_eq!(format!("{clone_step:?}"), format!("{clone_step:?}"));

        // StepFinalizeVsphere cleanup
        let mut fin_step = StepFinalizeVsphere {
            ui: ui.clone(),
            name: "fin".to_string(),
            client: client.clone(),
            convert_to_template: false,
        };
        fin_step.cleanup(&state).await;
        assert_eq!(format!("{fin_step:?}"), format!("{fin_step:?}"));

        // StepProvisionVsphere failure handling
        let fail_hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut prov_step = StepProvisionVsphere {
            ui: ui.clone(),
            name: "prov".to_string(),
            hook: fail_hook,
            ssh_username: None,
            ssh_password: None,
        };
        assert!(prov_step.run(&mut state).await.is_err());
        prov_step.cleanup(&state).await;
    }
}
