//! Implementation of the `vmware-iso` builder with `vmrun` and `govc` automation drivers,
//! `.vmx` configuration file generation with parameter injection, and guest IP discovery.

/// Configuration types, drivers, firmware, and adapter enums for `vmware-iso`.
pub mod config;
/// Automation drivers (`vmrun`, `govc`, `ovftool`) and guest IP discovery for `VMware`.
pub mod driver;
/// Multistep execution steps for `vmware-iso`.
pub mod steps;
/// `.vmx` configuration file generator for `VMware` virtual machines.
pub mod vmx;

pub use config::{
    VmwareCdromAdapterType, VmwareDiskAdapterType, VmwareDriver, VmwareFirmware, VmwareIsoConfig,
    VmwareNicType, VmwareToolsFlavor, VmwareToolsMode, parse_string_list, parse_string_map,
};
pub use driver::{
    discover_guest_ip, discover_guest_ip_internal, discover_guest_ip_with_cmd, find_vmrun_binary,
    find_vmware_tools_iso, run_govc, run_govc_with_cmd, run_ovftool, run_ovftool_with_cmd,
    run_vmrun, run_vmrun_with_cmd,
};
pub use steps::{
    StepCreateVM, StepExport, StepProvision, StepRunVM, StepShutdown, StepUploadTools,
};
pub use vmx::{generate_vmx_content, generate_vmx_content_full};

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step};
use crate::error::StampError;
use std::sync::Arc;

/// The `vmware-iso` builder.
#[derive(Debug, Clone)]
pub struct VmwareIsoBuilder {
    /// The builder configuration.
    pub config: VmwareIsoConfig,
}

impl VmwareIsoBuilder {
    /// Create a new `VmwareIsoBuilder`.
    #[must_use]
    pub const fn new(config: VmwareIsoConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Builder for VmwareIsoBuilder {
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
        let mut http_cfg = crate::builder::http_server::HttpServerConfig::new();
        if let Some(ref dir) = self.config.http_directory {
            http_cfg = http_cfg.with_dir(std::path::PathBuf::from(dir));
        }
        if !self.config.http_content.is_empty() {
            http_cfg = http_cfg.with_string_content(self.config.http_content.clone());
        }
        if let Some(ref addr) = self.config.http_bind_address {
            http_cfg = http_cfg.with_bind_address(addr);
        }
        if let (Some(min), Some(max)) = (self.config.http_port_min, self.config.http_port_max) {
            http_cfg = http_cfg.with_port_range(min, max);
        }

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateVM {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(crate::builder::http_server::StepHttpServer::new(
                http_cfg,
                self.name(),
                Some(ui.clone()),
            )),
            Box::new(StepRunVM {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                hook,
                config: self.config.clone(),
            }),
            Box::new(StepUploadTools {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepShutdown {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepExport {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let vmx_path = state.get::<String>("vmx_path").cloned().unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            format!("vmware-vmx:{vmx_path}"),
            vec![],
        )))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }

    fn name(&self) -> String {
        self.config.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::communicator::Communicator;
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::packer::OnErrorStrategy;
    use crate::engine::ui::Ui;
    use crate::types::{DiskSizeMb, MemoryMb, Port};
    use std::collections::HashMap;
    use std::path::Path;
    use std::str::FromStr;

    #[derive(Clone)]
    struct FailingProvisioner;

    #[async_trait::async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn crate::communicator::Communicator,
            _ui: Arc<crate::engine::ui::Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Execution("mock provision failure".to_string()))
        }
    }

    #[tokio::test]
    async fn test_vmwareisobuilder_run() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let config = VmwareIsoConfig {
            name: "test-builder".to_string(),
            vm_name: Some("test-vm".to_string()),
            iso_url: Some("/tmp/test.iso".to_string()),
            memory: Some(MemoryMb::new(2048)),
            cpus: Some(2),
            cores: Some(2),
            guest_os_type: Some("ubuntu-64".to_string()),
            disk_adapter_type: Some(VmwareDiskAdapterType::Nvme),
            cdrom_adapter_type: Some(VmwareCdromAdapterType::Sata),
            network_adapter_type: Some(VmwareNicType::Vmxnet3),
            network: Some("nat".to_string()),
            firmware: Some(VmwareFirmware::EfiSecure),
            version: Some("16".to_string()),
            usb: Some(true),
            vnc_port: Some(Port::new(5901)),
            vnc_disable_password: true,
            tools_mode: VmwareToolsMode::Upload,
            tools_upload_flavor: Some(VmwareToolsFlavor::Linux),
            output_directory: Some(td.path().to_string_lossy().to_string()),
            vmrun_binary: Some("echo".to_string()),
            ..Default::default()
        };
        let builder = VmwareIsoBuilder::new(config);

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

        let art = builder
            .run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
            .await?;
        assert!(art.id().contains("test-vm.vmx"));
        assert_eq!(art.builder_id(), "test-builder");

        let winrm_cfg = VmwareIsoConfig {
            name: "winrm-builder".to_string(),
            communicator: Some("winrm".to_string()),
            winrm_host_port: Some(Port::new(5985)),
            tools_mode: VmwareToolsMode::Attach,
            output_directory: Some(td.path().to_string_lossy().to_string()),
            vmrun_binary: Some("echo".to_string()),
            ..Default::default()
        };
        let b_winrm = VmwareIsoBuilder::new(winrm_cfg);
        let _ = b_winrm.run(hook, ui, OnErrorStrategy::Cleanup).await?;

        builder.cancel().await
    }

    #[test]
    fn test_vmx_generation() {
        let mut custom = HashMap::new();
        custom.insert(
            "isolation.tools.copy.disable".to_string(),
            "TRUE".to_string(),
        );

        let vmx = generate_vmx_content(
            "my-vm",
            "ubuntu-64",
            4096,
            4,
            Some("/tmp/ubuntu.iso"),
            &custom,
        );

        assert!(vmx.contains(r#"displayName = "my-vm""#));
        assert!(vmx.contains(r#"memsize = "4096""#));
        assert!(vmx.contains(r#"numvcpus = "4""#));
        assert!(vmx.contains(r#"ide1:0.fileName = "/tmp/ubuntu.iso""#));
        assert!(vmx.contains(r#"isolation.tools.copy.disable = "TRUE""#));

        let vmx_no_iso = generate_vmx_content("vm2", "other", 1024, 1, None, &custom);
        assert!(!vmx_no_iso.contains("ide1:0.fileName"));
    }

    #[test]
    fn test_vmx_generation_full_options() {
        let mut custom = HashMap::new();
        custom.insert("svga.autodetect".to_string(), "TRUE".to_string());

        let vmx1 = generate_vmx_content_full(
            "vm1",
            "windows9-64",
            8192,
            8,
            Some(4),
            Some("/path/win.iso"),
            Some(&VmwareDiskAdapterType::Nvme),
            Some(&VmwareCdromAdapterType::Sata),
            Some(&VmwareNicType::Vmxnet3),
            Some("bridged"),
            Some(&VmwareFirmware::EfiSecure),
            Some("19"),
            Some(true),
            false,
            Some(5905),
            true,
            &custom,
        );
        assert!(vmx1.contains(r#"virtualHW.version = "19""#));
        assert!(vmx1.contains(r#"cpuid.coresPerSocket = "4""#));
        assert!(vmx1.contains(r#"firmware = "efi""#));
        assert!(vmx1.contains(r#"uefi.secureBoot.enabled = "TRUE""#));
        assert!(vmx1.contains(r#"nvme0.present = "TRUE""#));
        assert!(vmx1.contains(r#"sata0:1.present = "TRUE""#));
        assert!(vmx1.contains(r#"ethernet0.virtualDev = "vmxnet3""#));
        assert!(vmx1.contains(r#"ethernet0.connectionType = "bridged""#));
        assert!(vmx1.contains(r#"usb.present = "TRUE""#));
        assert!(vmx1.contains(r#"RemoteDisplay.vnc.port = "5905""#));
        assert!(vmx1.contains(r#"RemoteDisplay.vnc.password = """#));

        let vmx2 = generate_vmx_content_full(
            "vm2",
            "other",
            2048,
            2,
            None,
            Some("/path/other.iso"),
            Some(&VmwareDiskAdapterType::Sata),
            Some(&VmwareCdromAdapterType::Scsi),
            Some(&VmwareNicType::E1000e),
            Some("hostonly"),
            Some(&VmwareFirmware::Bios),
            None,
            None,
            false,
            None,
            false,
            &custom,
        );
        assert!(vmx2.contains(r#"sata0.present = "TRUE""#));
        assert!(vmx2.contains(r#"scsi0:1.present = "TRUE""#));
        assert!(vmx2.contains(r#"firmware = "bios""#));

        let vmx3 = generate_vmx_content_full(
            "vm3",
            "other",
            1024,
            1,
            None,
            Some("/path/other.iso"),
            Some(&VmwareDiskAdapterType::Ide),
            Some(&VmwareCdromAdapterType::Custom("custom".to_string())),
            Some(&VmwareNicType::Vlance),
            None,
            Some(&VmwareFirmware::Efi),
            None,
            None,
            true,
            None,
            false,
            &custom,
        );
        assert!(vmx3.contains(r#"ide0:0.present = "TRUE""#));
        assert!(vmx3.contains(r#"firmware = "efi""#));
        assert!(!vmx3.contains("ethernet0.present"));

        let vmx4 = generate_vmx_content_full(
            "vm4",
            "other",
            1024,
            1,
            None,
            None,
            Some(&VmwareDiskAdapterType::BusLogic),
            None,
            None,
            None,
            Some(&VmwareFirmware::Custom("custom_fw".to_string())),
            None,
            None,
            false,
            None,
            false,
            &custom,
        );
        assert!(vmx4.contains(r#"scsi0.virtualDev = "buslogic""#));
        assert!(vmx4.contains(r#"firmware = "custom_fw""#));

        let vmx5 = generate_vmx_content_full(
            "vm5",
            "other",
            1024,
            1,
            None,
            None,
            Some(&VmwareDiskAdapterType::PvScsi),
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            None,
            false,
            &custom,
        );
        assert!(vmx5.contains(r#"scsi0.virtualDev = "pvscsi""#));

        let vmx6 = generate_vmx_content_full(
            "vm6",
            "other",
            1024,
            1,
            None,
            None,
            Some(&VmwareDiskAdapterType::Custom("my_scsi".to_string())),
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            None,
            false,
            &custom,
        );
        assert!(vmx6.contains(r#"scsi0.virtualDev = "my_scsi""#));
    }

    #[tokio::test]
    async fn test_vmware_iso_run_without_sizes_and_ports() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;

        let config = super::VmwareIsoConfig {
            name: "test-vmware-iso-default-ports".to_string(),
            ssh_port: None,
            winrm_host_port: None,
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            vmrun_binary: Some("echo".to_string()),
            ..Default::default()
        };

        let builder = super::VmwareIsoBuilder::new(config);

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let _ = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        Ok(())
    }

    #[test]
    fn test_vmware_derived_traits() -> Result<(), std::convert::Infallible> {
        let config1 = VmwareIsoConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);

        assert_eq!(VmwareDriver::default(), VmwareDriver::Vmrun);
        assert_eq!(VmwareDriver::Vmrun.as_str(), "vmrun");
        assert_eq!(VmwareDriver::Govc.as_str(), "govc");
        assert_eq!(format!("{}", VmwareDriver::Vmrun), "vmrun");
        assert_eq!(VmwareDriver::from_str("govc")?, VmwareDriver::Govc);
        assert_eq!(VmwareDriver::from_str("vmrun")?, VmwareDriver::Vmrun);
        assert_eq!(VmwareDriver::from_str("other")?, VmwareDriver::Vmrun);

        assert_eq!(VmwareFirmware::Bios.as_str(), "bios");
        assert_eq!(VmwareFirmware::Efi.as_str(), "efi");
        assert_eq!(VmwareFirmware::EfiSecure.as_str(), "efi-secure");
        assert_eq!(
            VmwareFirmware::Custom("my_fw".to_string()).as_str(),
            "my_fw"
        );
        assert_eq!(format!("{}", VmwareFirmware::Bios), "bios");
        assert_eq!(VmwareFirmware::from_str("bios")?, VmwareFirmware::Bios);
        assert_eq!(VmwareFirmware::from_str("efi")?, VmwareFirmware::Efi);
        assert_eq!(VmwareFirmware::from_str("uefi")?, VmwareFirmware::Efi);
        assert_eq!(
            VmwareFirmware::from_str("efi-secure")?,
            VmwareFirmware::EfiSecure
        );
        assert_eq!(
            VmwareFirmware::from_str("custom")?,
            VmwareFirmware::Custom("custom".to_string())
        );

        assert_eq!(VmwareDiskAdapterType::LsiLogic.as_str(), "lsilogic");
        assert_eq!(VmwareDiskAdapterType::PvScsi.as_str(), "pvscsi");
        assert_eq!(VmwareDiskAdapterType::BusLogic.as_str(), "buslogic");
        assert_eq!(VmwareDiskAdapterType::Nvme.as_str(), "nvme");
        assert_eq!(VmwareDiskAdapterType::Sata.as_str(), "sata");
        assert_eq!(VmwareDiskAdapterType::Ide.as_str(), "ide");
        assert_eq!(
            VmwareDiskAdapterType::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", VmwareDiskAdapterType::LsiLogic), "lsilogic");
        assert_eq!(
            VmwareDiskAdapterType::from_str("lsilogic")?,
            VmwareDiskAdapterType::LsiLogic
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("pvscsi")?,
            VmwareDiskAdapterType::PvScsi
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("buslogic")?,
            VmwareDiskAdapterType::BusLogic
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("nvme")?,
            VmwareDiskAdapterType::Nvme
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("sata")?,
            VmwareDiskAdapterType::Sata
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("ide")?,
            VmwareDiskAdapterType::Ide
        );
        assert_eq!(
            VmwareDiskAdapterType::from_str("other")?,
            VmwareDiskAdapterType::Custom("other".to_string())
        );

        assert_eq!(VmwareCdromAdapterType::Ide.as_str(), "ide");
        assert_eq!(VmwareCdromAdapterType::Sata.as_str(), "sata");
        assert_eq!(VmwareCdromAdapterType::Scsi.as_str(), "scsi");
        assert_eq!(
            VmwareCdromAdapterType::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", VmwareCdromAdapterType::Ide), "ide");
        assert_eq!(
            VmwareCdromAdapterType::from_str("ide")?,
            VmwareCdromAdapterType::Ide
        );
        assert_eq!(
            VmwareCdromAdapterType::from_str("sata")?,
            VmwareCdromAdapterType::Sata
        );
        assert_eq!(
            VmwareCdromAdapterType::from_str("scsi")?,
            VmwareCdromAdapterType::Scsi
        );
        assert_eq!(
            VmwareCdromAdapterType::from_str("other")?,
            VmwareCdromAdapterType::Custom("other".to_string())
        );

        assert_eq!(VmwareNicType::Vmxnet3.as_str(), "vmxnet3");
        assert_eq!(VmwareNicType::E1000e.as_str(), "e1000e");
        assert_eq!(VmwareNicType::E1000.as_str(), "e1000");
        assert_eq!(VmwareNicType::Vlance.as_str(), "vlance");
        assert_eq!(
            VmwareNicType::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", VmwareNicType::E1000), "e1000");
        assert_eq!(VmwareNicType::from_str("vmxnet3")?, VmwareNicType::Vmxnet3);
        assert_eq!(VmwareNicType::from_str("e1000e")?, VmwareNicType::E1000e);
        assert_eq!(VmwareNicType::from_str("e1000")?, VmwareNicType::E1000);
        assert_eq!(VmwareNicType::from_str("vlance")?, VmwareNicType::Vlance);
        assert_eq!(
            VmwareNicType::from_str("other")?,
            VmwareNicType::Custom("other".to_string())
        );

        assert_eq!(VmwareToolsMode::Attach.as_str(), "attach");
        assert_eq!(VmwareToolsMode::Upload.as_str(), "upload");
        assert_eq!(VmwareToolsMode::Disable.as_str(), "disable");
        assert_eq!(format!("{}", VmwareToolsMode::Disable), "disable");
        assert_eq!(
            VmwareToolsMode::from_str("attach")?,
            VmwareToolsMode::Attach
        );
        assert_eq!(
            VmwareToolsMode::from_str("upload")?,
            VmwareToolsMode::Upload
        );
        assert_eq!(
            VmwareToolsMode::from_str("disable")?,
            VmwareToolsMode::Disable
        );

        assert_eq!(VmwareToolsFlavor::Darwin.as_str(), "darwin");
        assert_eq!(VmwareToolsFlavor::Linux.as_str(), "linux");
        assert_eq!(VmwareToolsFlavor::Windows.as_str(), "windows");
        assert_eq!(
            VmwareToolsFlavor::Custom("custom".to_string()).as_str(),
            "custom"
        );
        assert_eq!(format!("{}", VmwareToolsFlavor::Linux), "linux");
        assert_eq!(
            VmwareToolsFlavor::from_str("darwin")?,
            VmwareToolsFlavor::Darwin
        );
        assert_eq!(
            VmwareToolsFlavor::from_str("linux")?,
            VmwareToolsFlavor::Linux
        );
        assert_eq!(
            VmwareToolsFlavor::from_str("windows")?,
            VmwareToolsFlavor::Windows
        );
        assert_eq!(
            VmwareToolsFlavor::from_str("other")?,
            VmwareToolsFlavor::Custom("other".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_vmware_from_builder_config() -> Result<(), StampError> {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "vmware-iso".to_string(),
            name: "bento-vmware".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("vm_name".to_string(), "my-bento-vmware".to_string());
        builder_cfg
            .config
            .insert("iso_url".to_string(), "/tmp/iso.iso".to_string());
        builder_cfg
            .config
            .insert("iso_checksum".to_string(), "sha256:123".to_string());
        builder_cfg
            .config
            .insert("memory".to_string(), "4096".to_string());
        builder_cfg
            .config
            .insert("cpus".to_string(), "4".to_string());
        builder_cfg
            .config
            .insert("vmware_cores".to_string(), "2".to_string());
        builder_cfg
            .config
            .insert("disk_size".to_string(), "50000".to_string());
        builder_cfg
            .config
            .insert("vmware_disk_adapter_type".to_string(), "nvme".to_string());
        builder_cfg
            .config
            .insert("vmware_cdrom_adapter_type".to_string(), "sata".to_string());
        builder_cfg.config.insert(
            "vmware_network_adapter_type".to_string(),
            "vmxnet3".to_string(),
        );
        builder_cfg
            .config
            .insert("vmware_network".to_string(), "nat".to_string());
        builder_cfg
            .config
            .insert("vmware_firmware".to_string(), "efi-secure".to_string());
        builder_cfg
            .config
            .insert("vmware_version".to_string(), "19".to_string());
        builder_cfg
            .config
            .insert("vmware_usb".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("vmware_tools_mode".to_string(), "upload".to_string());
        builder_cfg.config.insert(
            "vmware_tools_upload_flavor".to_string(),
            "linux".to_string(),
        );
        builder_cfg.config.insert(
            "vmware_tools_upload_path".to_string(),
            "/tmp/tools.iso".to_string(),
        );
        builder_cfg
            .config
            .insert("ssh_username".to_string(), "vagrant".to_string());
        builder_cfg
            .config
            .insert("ssh_password".to_string(), "vagrant".to_string());
        builder_cfg
            .config
            .insert("ssh_port".to_string(), "2222".to_string());
        builder_cfg
            .config
            .insert("headless".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("shutdown_command".to_string(), "poweroff".to_string());
        builder_cfg.config.insert(
            "http_content".to_string(),
            r#"{"index.html": "hello"}"#.to_string(),
        );

        builder_cfg
            .config
            .insert("export_format".to_string(), "ova".to_string());
        builder_cfg.config.insert(
            "vmware_vmx_data".to_string(),
            r#"{"svga.autodetect": "TRUE"}"#.to_string(),
        );

        let cfg = VmwareIsoConfig::from_builder_config(&builder_cfg)?;
        assert_eq!(cfg.name, "bento-vmware");
        assert_eq!(cfg.vm_name.as_deref(), Some("my-bento-vmware"));
        assert_eq!(cfg.memory, Some(MemoryMb::new(4096)));
        assert_eq!(cfg.cpus, Some(4));
        assert_eq!(cfg.cores, Some(2));
        assert_eq!(cfg.disk_size, Some(DiskSizeMb::new(50000)));
        assert_eq!(cfg.disk_adapter_type, Some(VmwareDiskAdapterType::Nvme));
        assert_eq!(cfg.cdrom_adapter_type, Some(VmwareCdromAdapterType::Sata));
        assert_eq!(cfg.network_adapter_type, Some(VmwareNicType::Vmxnet3));
        assert_eq!(cfg.network.as_deref(), Some("nat"));
        assert_eq!(cfg.firmware, Some(VmwareFirmware::EfiSecure));
        assert_eq!(cfg.version.as_deref(), Some("19"));
        assert_eq!(cfg.usb, Some(true));
        assert_eq!(cfg.tools_mode, VmwareToolsMode::Upload);
        assert_eq!(cfg.tools_upload_flavor, Some(VmwareToolsFlavor::Linux));
        assert_eq!(cfg.tools_upload_path.as_deref(), Some("/tmp/tools.iso"));
        assert_eq!(cfg.ssh_port, Some(Port::new(2222)));
        assert_eq!(cfg.shutdown_command.as_deref(), Some("poweroff"));
        assert_eq!(cfg.export_format.as_deref(), Some("ova"));
        assert!(cfg.headless);
        assert_eq!(
            cfg.vmx_data.get("svga.autodetect"),
            Some(&"TRUE".to_string())
        );

        let dummy = hashicorp_configuration_language_rs::span::Span::default();
        let obj_expr = hashicorp_configuration_language_rs::ast::expr::Expression::Object(
            vec![(
                hashicorp_configuration_language_rs::ast::expr::Expression::String(
                    "custom.key".to_string(),
                    dummy.clone(),
                ),
                hashicorp_configuration_language_rs::ast::expr::Expression::String(
                    "custom.val".to_string(),
                    dummy.clone(),
                ),
            )],
            dummy,
        );
        let mut builder_hcl = crate::template::BuilderConfig {
            builder_type: "vmware-iso".to_string(),
            name: "hcl-vbox".to_string(),
            ..Default::default()
        };
        builder_hcl
            .expressions
            .insert("vmx_data".to_string(), obj_expr);
        let cfg_hcl = VmwareIsoConfig::from_builder_config(&builder_hcl)?;
        assert_eq!(
            cfg_hcl.vmx_data.get("custom.key"),
            Some(&"custom.val".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_vmware_find_paths_and_helpers() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let fake_vmrun = td.path().join("vmrun");
        tokio::fs::write(
            &fake_vmrun,
            b"#!/bin/sh
",
        )
        .await?;
        let fake_tools = td.path().join("linux.iso");
        tokio::fs::write(&fake_tools, b"ISO").await?;
        {
            let _env = crate::utils::ScopedEnvVar::set(
                "VMRUN_PATH",
                fake_vmrun.to_string_lossy().as_ref(),
            );
            assert_eq!(find_vmrun_binary(), fake_vmrun);
        }
        {
            let _env = crate::utils::ScopedEnvVar::set(
                "VMWARE_TOOLS_ISO",
                fake_tools.to_string_lossy().as_ref(),
            );
            assert_eq!(find_vmware_tools_iso(None), Some(fake_tools.clone()));
        }

        assert_eq!(
            parse_string_list(r#"["val1", "val2"]"#),
            vec!["val1", "val2"]
        );
        assert_eq!(parse_string_list("a,,b"), vec!["a", "b"]);

        let json_map_str = r#"{"json_k": "json_v"}"#.to_string();
        let parsed_map = parse_string_map(Some(&json_map_str), None);
        assert_eq!(parsed_map.get("json_k"), Some(&"json_v".to_string()));
        let inv_map_str = "not-json".to_string();
        assert!(parse_string_map(Some(&inv_map_str), None).is_empty());

        let _ = find_vmrun_binary();
        let _ = find_vmware_tools_iso(Some(&VmwareToolsFlavor::Windows));

        let stdout_err =
            run_vmrun_with_cmd("sh", &["-c", "echo 'stdout vmrun failure'; exit 1"]).await;
        assert!(stdout_err.is_err());

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut step_create = StepCreateVM {
            ui: ui.clone(),
            name: "create-test".to_string(),
            config: VmwareIsoConfig::default(),
        };
        let mut state = StateBag::new();
        state.put("vmx_path", "/path/to/vm.vmx".to_string());
        step_create.cleanup(&state).await;

        let mut step_upload = StepUploadTools {
            ui: ui.clone(),
            name: "upload-test".to_string(),
            config: VmwareIsoConfig {
                tools_mode: VmwareToolsMode::Upload,
                tools_source_path: Some(fake_tools.to_string_lossy().to_string()),
                ..Default::default()
            },
        };
        let comm: Arc<dyn Communicator> =
            Arc::new(crate::communicator::mock::MockCommunicator::new());
        state.put("communicator", comm);
        step_upload.run(&mut state).await?;
        step_upload.cleanup(&state).await;

        let mut step_shutdown = StepShutdown {
            ui: ui.clone(),
            name: "shutdown-test".to_string(),
            config: VmwareIsoConfig {
                shutdown_command: Some("poweroff".to_string()),
                ..Default::default()
            },
        };
        step_shutdown.run(&mut state).await?;
        step_shutdown.cleanup(&state).await;

        let mut step_prov_fail = StepProvision {
            ui,
            name: "prov-fail".to_string(),
            hook: Arc::new(DefaultProvisionHook {
                provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
                error_cleanup_provisioners: Arc::new(vec![]),
            }),
            config: VmwareIsoConfig::default(),
        };
        assert!(step_prov_fail.run(&mut state).await.is_err());
        step_prov_fail.cleanup(&state).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_vmware_floppy_cd_export_and_boot() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let f_path = td.path().join("preseed.cfg");
        tokio::fs::write(&f_path, b"d-i test").await?;
        let cd_path = td.path().join("user-data");
        tokio::fs::write(&cd_path, b"#cloud-config").await?;

        let config = VmwareIsoConfig {
            name: "vmware-media".to_string(),
            vm_name: Some("vmware-test".to_string()),
            floppy_files: vec![f_path.to_string_lossy().to_string()],
            cd_files: vec![cd_path.to_string_lossy().to_string()],
            cd_label: Some("cidata".to_string()),
            boot_command: Some(vec!["<wait><enter>".to_string()]),
            vnc_port: Some(Port::new(5910)),
            export_format: Some("ova".to_string()),
            output_directory: Some(td.path().to_string_lossy().to_string()),
            vmrun_binary: Some("echo".to_string()),
            ovftool_binary: Some("cp".to_string()),
            ..Default::default()
        };

        let builder = VmwareIsoBuilder::new(config);
        builder.prepare().await?;

        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let art = builder.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        assert!(art.id().contains("vmware-test"));

        let ova_file = td.path().join("vmware-test.ova");
        run_ovftool_with_cmd("cp", &td.path().join("vmware-test.vmx"), &ova_file).await?;
        assert!(ova_file.exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_vmrun_and_govc_and_ovftool_commands() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let _ = run_vmrun(&["list"]).await;
        run_vmrun_with_cmd("echo", &["list"]).await?;
        assert!(
            run_vmrun_with_cmd("/nonexistent_vmrun_binary", &["list"])
                .await
                .is_err()
        );
        assert!(
            run_vmrun_with_cmd("sh", &["-c", "echo 'vmrun error' >&2; exit 1"])
                .await
                .is_err()
        );

        let _ = run_govc(&["vm.ip"]).await;
        run_govc_with_cmd("echo", &["vm.ip"]).await?;
        assert!(
            run_govc_with_cmd("/nonexistent_govc_binary", &["vm.ip"])
                .await
                .is_err()
        );
        assert!(
            run_govc_with_cmd("sh", &["-c", "echo 'govc error' >&2; exit 1"])
                .await
                .is_err()
        );

        let src = td.path().join("test.vmx");
        tokio::fs::write(&src, b".encoding = \"UTF-8\"\n").await?;
        let dst = td.path().join("test.ova");
        let _ = run_ovftool(&src, &dst).await;
        run_ovftool_with_cmd("cp", &src, &dst).await?;
        assert!(
            run_ovftool_with_cmd("echo", &src, &td.path().join("missing.ova"))
                .await
                .is_err()
        );
        assert!(
            run_ovftool_with_cmd("/nonexistent_ovftool", &src, &dst)
                .await
                .is_err()
        );
        assert!(run_ovftool_with_cmd("sh", &src, &dst).await.is_err());
        let _ = run_ovftool_with_cmd("echo", Path::new(""), Path::new("")).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_discover_guest_ip() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let vmx = td.path().join("vm.vmx");

        let _ = discover_guest_ip(&vmx, VmwareDriver::Vmrun).await;
        let _ = discover_guest_ip(&vmx, VmwareDriver::Govc).await;
        discover_guest_ip_with_cmd("echo", &vmx, VmwareDriver::Govc, &[]).await?;

        let ip_govc_err =
            discover_guest_ip_with_cmd("/nonexistent_govc", &vmx, VmwareDriver::Govc, &[]).await;
        assert!(ip_govc_err.is_err());

        let ip_err_cmd =
            discover_guest_ip_with_cmd("/nonexistent_vmrun_cmd", &vmx, VmwareDriver::Vmrun, &[])
                .await?;
        assert_eq!(ip_err_cmd, "127.0.0.1");

        discover_guest_ip_with_cmd("echo", &vmx, VmwareDriver::Vmrun, &[]).await?;

        let script = td.path().join("fake_vmrun.sh");
        tokio::fs::write(
            &script,
            b"#!/bin/sh
echo 10.0.0.42
",
        )
        .await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
        }
        let ip_sh =
            discover_guest_ip_with_cmd(&script.to_string_lossy(), &vmx, VmwareDriver::Vmrun, &[])
                .await?;
        assert_eq!(ip_sh, "10.0.0.42");

        let lease_file = td.path().join("dhcpd.leases");
        tokio::fs::write(
            &lease_file,
            b"lease 192.168.99.123 {
  starts 12345;
}
",
        )
        .await?;
        let ip_lease =
            discover_guest_ip_with_cmd("echo", &vmx, VmwareDriver::Vmrun, &[&lease_file]).await?;
        assert_eq!(ip_lease, "192.168.99.123");

        let empty_lease = td.path().join("empty.leases");
        tokio::fs::write(
            &empty_lease,
            b"no lease lines here
",
        )
        .await?;
        let ip_fb =
            discover_guest_ip_with_cmd("echo", &vmx, VmwareDriver::Vmrun, &[&empty_lease]).await?;
        assert_eq!(ip_fb, "127.0.0.1");
        Ok(())
    }

    #[tokio::test]
    async fn test_vmware_step_export_branches() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let bad_cfg = VmwareIsoConfig {
            output_directory: Some("/dev/null/impossible".to_string()),
            ..Default::default()
        };
        let mut step_bad = StepCreateVM {
            ui: ui.clone(),
            name: "test".to_string(),
            config: bad_cfg,
        };
        let mut state_bad = StateBag::new();
        assert!(step_bad.run(&mut state_bad).await.is_err());

        let src_vmx = td.path().join("vm.vmx");
        tokio::fs::write(&src_vmx, b".encoding = \"UTF-8\"\n").await?;

        let cfg_ovf = VmwareIsoConfig {
            output_directory: Some(td.path().to_string_lossy().to_string()),
            export_format: Some("ovf".to_string()),
            ovftool_binary: Some("cp".to_string()),
            ..Default::default()
        };
        let mut step_ovf = StepExport {
            ui: ui.clone(),
            name: "test".to_string(),
            config: cfg_ovf,
        };
        let mut state = StateBag::new();
        state.put("vmx_path", src_vmx.to_string_lossy().to_string());
        step_ovf.run(&mut state).await?;
        step_ovf.cleanup(&state).await;

        let cfg_other = VmwareIsoConfig {
            export_format: Some("qcow2".to_string()),
            ..Default::default()
        };
        let mut step_other = StepExport {
            ui: ui.clone(),
            name: "test".to_string(),
            config: cfg_other,
        };
        step_other.run(&mut state).await?;

        let cfg_none = VmwareIsoConfig {
            export_format: None,
            ..Default::default()
        };
        let mut step_none = StepExport {
            ui,
            name: "test".to_string(),
            config: cfg_none,
        };
        step_none.run(&mut state).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_vmware_builder_run_error_strategies() -> Result<(), StampError> {
        let td = tempfile::tempdir()?;
        let config = VmwareIsoConfig {
            name: "test-err".to_string(),
            output_directory: Some(td.path().to_string_lossy().to_string()),
            vmrun_binary: Some("echo".to_string()),
            ..Default::default()
        };
        let builder = VmwareIsoBuilder::new(config);

        let fail_hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        assert!(
            builder
                .run(fail_hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        assert!(
            builder
                .run(fail_hook.clone(), ui.clone(), OnErrorStrategy::Abort)
                .await
                .is_err()
        );
        assert!(
            builder
                .run(
                    fail_hook.clone(),
                    ui.clone(),
                    OnErrorStrategy::RunCleanupProvisioner
                )
                .await
                .is_err()
        );

        let mut queue = std::collections::VecDeque::new();
        queue.push_back("yes".to_string());
        let ui_ask = Arc::new(
            Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(Arc::new(std::sync::Mutex::new(queue))),
        );
        assert!(
            builder
                .run(fail_hook.clone(), ui_ask, OnErrorStrategy::Ask)
                .await
                .is_err()
        );

        assert!(
            builder
                .run(fail_hook, ui, OnErrorStrategy::Ask)
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_vmware_prepare_validation() {
        let mut cfg = VmwareIsoConfig::default();
        let b = VmwareIsoBuilder::new(cfg.clone());
        assert!(b.prepare().await.is_err());

        cfg.name = "valid".to_string();
        let b2 = VmwareIsoBuilder::new(cfg);
        assert!(b2.prepare().await.is_ok());
    }
}
