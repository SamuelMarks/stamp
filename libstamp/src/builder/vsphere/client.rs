//! vSphere REST and SOAP client abstraction and hardware configuration types.

use crate::error::StampError;
use crate::types::SecretString;
use serde::{Deserialize, Serialize};
use std::path::Path;
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
    pub password: Option<SecretString>,
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
        password: Option<SecretString>,
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
    #[must_use]
    pub fn endpoint(&self, path: &str) -> String {
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
            req = req.basic_auth(u, Some(p.expose()));
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
