//! Implementation of the `VMware` vSphere ISO (`vsphere-iso`) and Clone (`vsphere-clone`) builders.
//!
//! Provides a full vSphere REST and SOAP (govmomi parity) client abstraction for vCenter and `ESXi`,
//! supporting datacenter/cluster discovery, datastore file uploads, virtual hardware specifications,
//! network adapter bindings, VM power lifecycles, template conversions, linked clones, and Storage vMotion.

/// vSphere REST and SOAP client abstraction and hardware configuration types.
pub mod client;
/// Multistep execution steps and builder configurations for `vsphere-iso` and `vsphere-clone`.
pub mod steps;

pub use client::{
    VsphereClient, VsphereDiskController, VsphereDiskProvisioning, VsphereHardwareConfig,
    VsphereNetworkAdapter,
};
pub use steps::{
    StepCloneVsphereVM, StepConnectVsphere, StepCreateVsphereVM, StepFinalizeVsphere,
    StepPowerOnWait, StepUploadMedia, VsphereCloneConfig, VsphereIsoConfig,
};

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepProvision};
use crate::error::StampError;
use async_trait::async_trait;
use std::sync::Arc;

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

        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "root".to_string());
        let ssh_pass = self
            .config
            .ssh_password
            .as_ref()
            .map(|s| s.expose().to_string());

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
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "vsphere", hook)
                    .with_ip_key("guest_ip")
                    .with_ssh_credentials(ssh_user, ssh_pass)
                    .with_message("Provisioning vSphere VM..."),
            ),
            Box::new(StepFinalizeVsphere {
                ui: ui.clone(),
                name: self.name(),
                client,
                convert_to_template: self.config.convert_to_template,
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();

        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            artifact_id,
            vec![],
        )))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
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

        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "root".to_string());
        let ssh_pass = self
            .config
            .ssh_password
            .as_ref()
            .map(|s| s.expose().to_string());

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
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "vsphere", hook)
                    .with_ip_key("guest_ip")
                    .with_ssh_credentials(ssh_user, ssh_pass)
                    .with_message("Provisioning vSphere VM..."),
            ),
            Box::new(StepFinalizeVsphere {
                ui: ui.clone(),
                name: self.name(),
                client,
                convert_to_template: self.config.convert_to_template,
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();

        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            artifact_id,
            vec![],
        )))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::multistep::StepAction;
    use crate::types::SecretString;
    use std::path::Path;
    use std::time::Duration;

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
        let hw_cloned = hw.clone();
        assert_eq!(hw_cloned, hw);

        let iso_cfg = VsphereIsoConfig {
            name: "iso-b".to_string(),
            ..Default::default()
        };
        let iso_cloned = iso_cfg.clone();
        assert_eq!(iso_cloned, iso_cfg);

        let clone_cfg = VsphereCloneConfig {
            name: "clone-b".to_string(),
            template: "tpl".to_string(),
            ..Default::default()
        };
        let clone_cloned = clone_cfg.clone();
        assert_eq!(clone_cloned, clone_cfg);

        let client = VsphereClient::new(
            "vcenter.local".to_string(),
            Some("u".to_string()),
            Some(SecretString::new("p")),
            false,
        );
        assert_eq!(client.vcenter_server, "vcenter.local");

        let iso_builder = VsphereIsoBuilder::new(iso_cfg);
        assert_eq!(iso_builder.name(), "iso-b");
        assert_eq!(iso_builder.depends_on().len(), 0);

        let clone_builder = VsphereCloneBuilder::new(clone_cfg);
        assert_eq!(clone_builder.name(), "clone-b");
        assert_eq!(clone_builder.depends_on().len(), 0);
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
    async fn test_vsphere_client_methods_mocked() -> Result<(), StampError> {
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
            Some(SecretString::new("secret")),
            true,
        );

        let token = client.login().await?;
        assert_eq!(token, "mock-vsphere-token-xyz");

        let dc1 = client
            .discover_datacenter(&token, Some("Datacenter1"))
            .await?;
        assert_eq!(dc1, "datacenter-mock-1");

        let dc2 = client.discover_datacenter(&token, None).await?;
        assert_eq!(dc2, "datacenter-mock-2");

        let comp_c = client
            .discover_cluster_or_host(&token, Some("Cluster1"), None)
            .await?;
        assert_eq!(comp_c, "domain-c-mock-1");

        let comp_h = client
            .discover_cluster_or_host(&token, None, Some("Host1"))
            .await?;
        assert_eq!(comp_h, "host-mock-1");

        let comp_def = client.discover_cluster_or_host(&token, None, None).await?;
        assert_eq!(comp_def, "default-compute-resource");

        let _m_cluster_empty = server
            .mock("GET", "/api/vcenter/cluster?names=ClusterEmpty")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        let comp_c_empty = client
            .discover_cluster_or_host(&token, Some("ClusterEmpty"), None)
            .await?;
        assert_eq!(comp_c_empty, "default-compute-resource");

        let _m_host_empty = server
            .mock("GET", "/api/vcenter/host?names=HostEmpty")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        let comp_host_empty = client
            .discover_cluster_or_host(&token, None, Some("HostEmpty"))
            .await?;
        assert_eq!(comp_host_empty, "default-compute-resource");

        let ds1 = client
            .discover_datastore(&token, Some("Datastore1"))
            .await?;
        assert_eq!(ds1, "datastore-mock-1");

        let ds2 = client.discover_datastore(&token, None).await?;
        assert_eq!(ds2, "datastore-mock-2");

        let td = tempfile::tempdir()?;
        let test_file = td.path().join("stamp-test-mocked-floppy.img");
        tokio::fs::write(&test_file, b"test-content").await?;

        client
            .upload_file_to_datastore(
                &token,
                "datastore-mock-1",
                "datacenter-mock-1",
                "stamp-uploads/floppy.img",
                &test_file,
            )
            .await?;

        let hw = VsphereHardwareConfig {
            cpu_sockets: Some(2),
            cpu_cores: Some(2),
            ram_mb: Some(4096),
            ..Default::default()
        };
        let placement =
            serde_json::json!({ "cluster": "domain-c-mock-1", "datastore": "datastore-mock-1" });
        let vm_id = client
            .create_vm(&token, "test-vm", "ubuntu64Guest", &hw, &placement)
            .await?;
        assert_eq!(vm_id, "vm-mock-1");

        let hw_none = VsphereHardwareConfig::default();
        let vm_id_2 = client
            .create_vm(&token, "test-vm-2", "ubuntu64Guest", &hw_none, &placement)
            .await?;
        assert_eq!(vm_id_2, "vm-mock-1");

        client.power_on(&token, &vm_id).await?;

        let ip = client
            .wait_guest_ip(&token, &vm_id, Duration::from_secs(5))
            .await?;
        assert_eq!(ip, "192.168.1.150");

        let clone_id = client
            .clone_vm(&token, &vm_id, "cloned-vm", &placement, true)
            .await?;
        assert_eq!(clone_id, "vm-clone-1");

        client
            .relocate_vm_datastore(&token, &clone_id, "datastore-mock-2")
            .await?;
        client.convert_to_template(&token, &clone_id).await?;
        client.power_off(&token, &vm_id).await
    }

    #[tokio::test]
    async fn test_vsphere_client_errors() -> Result<(), StampError> {
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
            Some(SecretString::new("bad_pass")),
            false,
        );
        assert!(client.login().await.is_err());

        let mut server2 = mockito::Server::new_async().await;
        let _m_login_ok = server2
            .mock("POST", "/api/session")
            .with_status(200)
            .with_body(r#""token-no-auth""#)
            .create_async()
            .await;
        let client_no_auth = VsphereClient::new(server2.url(), None, None, true);
        client_no_auth.login().await?;

        let mut server_cluster = mockito::Server::new_async().await;
        let _m_cluster_err = server_cluster
            .mock("GET", "/api/vcenter/cluster?names=BadCluster")
            .with_status(500)
            .create_async()
            .await;
        let client_cluster = VsphereClient::new(server_cluster.url(), None, None, true);
        let res = client_cluster
            .discover_cluster_or_host("tok", Some("BadCluster"), None)
            .await?;
        assert_eq!(res, "default-compute-resource");

        let mut server_host = mockito::Server::new_async().await;
        let _m_host_err = server_host
            .mock("GET", "/api/vcenter/host?names=BadHost")
            .with_status(500)
            .create_async()
            .await;
        let client_host = VsphereClient::new(server_host.url(), None, None, true);
        let res = client_host
            .discover_cluster_or_host("tok", None, Some("BadHost"))
            .await?;
        assert_eq!(res, "default-compute-resource");

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

        let mut server7 = mockito::Server::new_async().await;
        let _m_upload_err = server7
            .mock("PUT", "/folder/stamp-uploads/test.iso?dsName=ds&dcPath=dc")
            .with_status(500)
            .create_async()
            .await;
        let c7 = VsphereClient::new(server7.url(), None, None, true);
        let td = tempfile::tempdir()?;
        let test_file = td.path().join("stamp-test-err.iso");
        tokio::fs::write(&test_file, b"data").await?;
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

        let mut server10 = mockito::Server::new_async().await;
        let _m_power_err = server10
            .mock("POST", "/api/vcenter/vm/vm-1/power?action=start")
            .with_status(500)
            .create_async()
            .await;
        let c10 = VsphereClient::new(server10.url(), None, None, true);
        assert!(c10.power_on("tok", "vm-1").await.is_err());

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

        let mut server_twelve = mockito::Server::new_async().await;
        let _m_ip_err = server_twelve
            .mock("GET", "/api/vcenter/vm/vm-1/guest/networking")
            .with_status(404)
            .create_async()
            .await;
        let c12 = VsphereClient::new(server_twelve.url(), None, None, true);
        let fallback_ip = c12
            .wait_guest_ip("tok", "vm-1", Duration::from_millis(5))
            .await?;
        assert_eq!(fallback_ip, "127.0.0.1");

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

        let client_user_only = VsphereClient::new(
            "http://127.0.0.1:1".to_string(),
            Some("u".to_string()),
            None,
            true,
        );
        assert!(client_user_only.login().await.is_err());
        let client_pass_only = VsphereClient::new(
            "http://127.0.0.1:1".to_string(),
            None,
            Some(SecretString::new("p")),
            true,
        );
        assert!(client_pass_only.login().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_vsphere_iso_builder_lifecycle_success() -> Result<(), StampError> {
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

        let td = tempfile::tempdir()?;
        let floppy_path = td.path().join("floppy.img");
        let cd_path = td.path().join("cd.iso");
        tokio::fs::write(&floppy_path, b"floppy").await?;
        tokio::fs::write(&cd_path, b"cd").await?;

        let config = VsphereIsoConfig {
            name: "test-iso".to_string(),
            vcenter_server: Some(server.url()),
            username: Some("user".to_string()),
            password: Some(SecretString::new("pass")),
            insecure_connection: true,
            datacenter: Some("DC1".to_string()),
            cluster: Some("Cluster1".to_string()),
            datastore: Some("DS1".to_string()),
            vm_name: Some("test-iso-vm".to_string()),
            guest_os_type: Some("ubuntu64Guest".to_string()),
            floppy_files: vec![floppy_path.to_string_lossy().into_owned()],
            cd_files: vec![cd_path.to_string_lossy().into_owned(), "/".to_string()],
            ssh_username: Some("admin".to_string()),
            ssh_password: Some(SecretString::new("secret")),
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
        assert_eq!(artifact.id(), "vsphere:vm-iso-1");
        assert_eq!(artifact.builder_id(), "test-iso");
        Ok(())
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
    async fn test_vsphere_clone_builder_lifecycle_success() -> Result<(), StampError> {
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
            password: Some(SecretString::new("pass")),
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

        let artifact = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert_eq!(artifact.id(), "vsphere:vm-cloned-1");
        assert_eq!(artifact.builder_id(), "test-clone");

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
        Ok(())
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

        let mut create_step = StepCreateVsphereVM {
            ui: ui.clone(),
            name: "create".to_string(),
            client: client.clone(),
            config: VsphereIsoConfig::default(),
        };
        create_step.cleanup(&state).await;
        state.put("vm_id", "vm-123".to_string());
        create_step.cleanup(&state).await;

        let mut power_step = StepPowerOnWait {
            ui: ui.clone(),
            name: "power".to_string(),
            client: client.clone(),
        };
        let empty_state = StateBag::new();
        power_step.cleanup(&empty_state).await;
        power_step.cleanup(&state).await;

        let mut clone_step = StepCloneVsphereVM {
            ui: ui.clone(),
            name: "clone".to_string(),
            client: client.clone(),
            config: VsphereCloneConfig::default(),
        };
        clone_step.cleanup(&empty_state).await;
        clone_step.cleanup(&state).await;

        let mut fin_step = StepFinalizeVsphere {
            ui: ui.clone(),
            name: "fin".to_string(),
            client: client.clone(),
            convert_to_template: false,
        };
        fin_step.cleanup(&state).await;

        let fail_hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut prov_step =
            StepProvision::new(ui.clone(), "prov".to_string(), "vsphere", fail_hook)
                .with_ip_key("guest_ip");
        assert!(prov_step.run(&mut state).await.is_err());
        prov_step.cleanup(&state).await;
    }
}
