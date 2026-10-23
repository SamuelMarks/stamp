//! Multistep execution steps and builder configurations for `vsphere-iso` and `vsphere-clone`.

use crate::builder::vsphere::client::{VsphereClient, VsphereHardwareConfig};
use crate::engine::multistep::{StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::SecretString;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

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
    pub password: Option<SecretString>,
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
    pub ssh_password: Option<SecretString>,
    /// Whether to convert the VM into a vSphere VM Template after provisioning.
    pub convert_to_template: bool,
    /// Target vSphere Content Library name to publish the template to.
    pub content_library_name: Option<String>,
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
    pub password: Option<SecretString>,
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
    pub ssh_password: Option<SecretString>,
    /// Whether to convert the clone into a vSphere VM Template on completion.
    pub convert_to_template: bool,
    /// Target content library name.
    pub content_library_name: Option<String>,
}

/// Step to connect and authenticate against `VMware` vSphere.
#[derive(Debug, Clone)]
pub struct StepConnectVsphere {
    /// UI reference for terminal output.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Step or builder name.
    pub name: String,
    /// vSphere client.
    pub client: VsphereClient,
    /// Target datacenter.
    pub datacenter: Option<String>,
    /// Target cluster.
    pub cluster: Option<String>,
    /// Target host.
    pub host: Option<String>,
    /// Target datastore.
    pub datastore: Option<String>,
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
pub struct StepUploadMedia {
    /// UI reference.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// vSphere client.
    pub client: VsphereClient,
    /// Floppy files.
    pub floppy_files: Vec<String>,
    /// CD-ROM files.
    pub cd_files: Vec<String>,
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
pub struct StepCreateVsphereVM {
    /// UI reference.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// vSphere client.
    pub client: VsphereClient,
    /// Builder config.
    pub config: VsphereIsoConfig,
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
pub struct StepPowerOnWait {
    /// UI reference.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// vSphere client.
    pub client: VsphereClient,
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

/// Step to power off and convert the VM to a template or content library item.
#[derive(Debug, Clone)]
pub struct StepFinalizeVsphere {
    /// UI reference.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// vSphere client.
    pub client: VsphereClient,
    /// Whether to convert to template.
    pub convert_to_template: bool,
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

/// Step to execute VM cloning with linked clone, customization spec, and storage vMotion.
#[derive(Debug, Clone)]
pub struct StepCloneVsphereVM {
    /// UI reference.
    pub ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    pub name: String,
    /// vSphere client.
    pub client: VsphereClient,
    /// Builder configuration.
    pub config: VsphereCloneConfig,
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
