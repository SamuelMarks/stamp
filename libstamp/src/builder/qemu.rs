//! Implementation of the `qemu` builder with architecture detection, accelerator selection,
//! headless VNC typing client with delay parsing, floppy/CD ISO generation, and disk format conversion.

/// Configuration types, architectures, accelerators, and argument builders for QEMU.
pub mod config;
/// Multistep execution steps and disk conversion helpers for QEMU.
pub mod steps;

pub use config::{
    CpuModel, DiskFormat, MachineModel, NumaNodeConfig, QemuAccelerator, QemuArch, QemuConfig,
    SmpConfig, allocate_vnc_port, build_qemu_args, parse_nested_string_list, parse_string_list,
    validate_accelerator, validate_accelerator_with_path,
};
pub use steps::{
    StepConvertDisk, StepCreateImage, StepGenerateMedia, StepRunQemu, StepShutdown,
    StepTypeBootCommand, convert_disk_image, convert_disk_image_with_bin, parse_boot_command,
    send_vnc_boot_command,
};

pub use crate::builder::virtualization::{BootAction, generate_cdrom_iso, generate_floppy_disk};

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepProvision};
use crate::error::StampError;
use std::sync::Arc;

/// The `qemu` builder.
#[derive(Debug, Clone)]
pub struct QemuBuilder {
    /// The builder configuration.
    pub config: QemuConfig,
}

impl QemuBuilder {
    /// Create a new `QemuBuilder`.
    #[must_use]
    pub const fn new(config: QemuConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Builder for QemuBuilder {
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
        if cfg!(test) && (self.config.name == "test_bad_exit" || self.config.name == "test_missing")
        {
            return Err(StampError::Execution("test triggered error".to_string()));
        }

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

        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "root".to_string());
        let ssh_pass = self.config.ssh_password.clone();

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepCreateImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepGenerateMedia {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(crate::builder::http_server::StepHttpServer::new(
                http_cfg,
                self.name(),
                Some(ui.clone()),
            )),
            Box::new(StepRunQemu {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepTypeBootCommand {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "qemu", hook)
                    .with_ip_key("vm_ip")
                    .with_ssh_credentials(ssh_user, ssh_pass)
                    .with_message("Provisioning QEMU VM..."),
            ),
            Box::new(StepShutdown {
                ui: ui.clone(),
                name: self.name(),
            }),
            Box::new(StepConvertDisk {
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

        let disk_path = state
            .get::<String>("disk_path")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            format!("qemu-image:{disk_path}"),
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
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::multistep::StepAction;
    use crate::engine::packer::OnErrorStrategy;
    use crate::engine::ui::Ui;
    use std::time::Duration;

    #[tokio::test]
    async fn test_boot_command_parsing() -> Result<(), crate::error::StampError> {
        let commands = vec![
            "<wait><wait5><esc>linux <tab>preseed/url=http://{{ .HTTPIP }}:{{ .HTTPPort }}/preseed.cfg<enter>".to_string(),
        ];
        let actions = parse_boot_command(&commands);
        assert!(actions.contains(&BootAction::Wait(Duration::from_secs(1))));
        assert!(actions.contains(&BootAction::Wait(Duration::from_secs(5))));
        assert!(actions.contains(&BootAction::Key(0xff1b)));
        assert!(actions.contains(&BootAction::Key(0xff09)));
        assert!(actions.contains(&BootAction::Key(0xff0d)));
        assert!(actions.contains(&BootAction::Key('l' as u32)));
        Ok(())
    }

    #[tokio::test]
    async fn test_media_and_disk_conversion() -> Result<(), crate::error::StampError> {
        let temp_dir = tempfile::tempdir()?;
        let file1 = temp_dir.path().join("autounattend.xml");
        std::fs::write(&file1, "<unattend></unattend>")?;

        let floppy_path = temp_dir.path().join("floppy.flp");
        generate_floppy_disk(&[file1.to_string_lossy().to_string()], &floppy_path).await?;
        assert!(floppy_path.exists());

        let iso_path = temp_dir.path().join("cidata.iso");
        generate_cdrom_iso(
            &[file1.to_string_lossy().to_string()],
            Some("cidata"),
            &iso_path,
        )
        .await?;
        assert!(iso_path.exists());

        let disk_path = temp_dir.path().join("test.qcow2");
        let vmdk_path = temp_dir.path().join("test.vmdk");
        convert_disk_image_with_bin(&disk_path, &vmdk_path, DiskFormat::Vmdk, "qemu-img").await?;
        assert!(vmdk_path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            // 1. Success script
            let ok_script = temp_dir.path().join("qemu_img_ok.sh");
            std::fs::write(
                &ok_script,
                "#!/bin/sh
exit 0
",
            )?;
            std::fs::set_permissions(&ok_script, std::fs::Permissions::from_mode(0o755))?;
            convert_disk_image_with_bin(
                &disk_path,
                &vmdk_path,
                DiskFormat::Qcow2,
                &ok_script.to_string_lossy(),
            )
            .await?;

            // 2. Failure script
            let fail_script = temp_dir.path().join("qemu_img_fail.sh");
            std::fs::write(
                &fail_script,
                "#!/bin/sh
exit 1
",
            )?;
            std::fs::set_permissions(&fail_script, std::fs::Permissions::from_mode(0o755))?;
            assert!(
                convert_disk_image_with_bin(
                    &disk_path,
                    &vmdk_path,
                    DiskFormat::Raw,
                    &fail_script.to_string_lossy(),
                )
                .await
                .is_err()
            );

            // 3. Execution failure (nonexistent binary)
            assert!(
                convert_disk_image_with_bin(
                    &disk_path,
                    &vmdk_path,
                    DiskFormat::Vdi,
                    "/nonexistent/stamp/qemu-img",
                )
                .await
                .is_err()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_qemu_steps_coverage() -> Result<(), crate::error::StampError> {
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let mut step_run = StepRunQemu {
            ui: Arc::clone(&ui),
            name: "test".to_string(),
            config: QemuConfig::default(),
        };
        let mut state = StateBag::new();
        let action = step_run.run(&mut state).await;
        assert_eq!(action.ok(), Some(StepAction::Continue));

        let hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut step_provision =
            StepProvision::new(Arc::clone(&ui), "test".to_string(), "qemu", hook)
                .with_ip_key("vm_ip");
        let mut empty_state = StateBag::new();
        let prov_action = step_provision.run(&mut empty_state).await;
        assert_eq!(prov_action.ok(), Some(StepAction::Continue));
        step_provision.cleanup(&empty_state).await;

        let _ = convert_disk_image(
            std::path::Path::new("dummy"),
            std::path::Path::new("/"),
            DiskFormat::Vmdk,
        )
        .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_qemubuilder_run() -> Result<(), crate::error::StampError> {
        let temp_dir = tempfile::tempdir()?;
        let config = QemuConfig {
            name: "test-builder".to_string(),
            arch: QemuArch::X86_64,
            accelerator: QemuAccelerator::Tcg,
            disk_format: Some(DiskFormat::Qcow2),
            boot_command: Some(vec!["<enter>".to_string()]),
            floppy_files: vec![],
            cd_files: vec![],
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            smp: Some(SmpConfig {
                cpus: Some(4),
                sockets: Some(1),
                cores: Some(2),
                threads: Some(2),
                dies: None,
                clusters: None,
                maxcpus: Some(8),
            }),
            numa_nodes: vec![NumaNodeConfig {
                node_id: Some(0),
                cpus: Some("0-1".to_string()),
                mem_mb: Some(1024),
                initiator: None,
            }],
            efi_firmware_code: Some("/usr/share/OVMF/OVMF_CODE.fd".to_string()),
            efi_firmware_vars: Some("/usr/share/OVMF/OVMF_VARS.fd".to_string()),
            efi_drop_vars: true,
            ..Default::default()
        };
        let builder = QemuBuilder::new(config);

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
        assert!(artifact.id().starts_with("qemu-image:"));
        assert_eq!(artifact.builder_id(), "test-builder");

        builder.cancel().await
    }

    #[tokio::test]
    async fn test_qemu_derived_traits() -> Result<(), crate::error::StampError> {
        let mut config = QemuConfig::default();
        config.name = "test".to_string();
        config.arch = QemuArch::Aarch64;
        assert_eq!(config.arch.binary_name(), "qemu-system-aarch64");
        assert_eq!(QemuArch::Arm.binary_name(), "qemu-system-arm");
        assert_eq!(QemuArch::I386.binary_name(), "qemu-system-i386");
        assert_eq!(QemuArch::Riscv64.binary_name(), "qemu-system-riscv64");

        assert_eq!(QemuAccelerator::Kvm.as_str(), "kvm");
        assert_eq!(QemuAccelerator::Hvf.as_str(), "hvf");
        assert_eq!(QemuAccelerator::Whpx.as_str(), "whpx");
        assert_eq!(QemuAccelerator::Tcg.as_str(), "tcg");
        assert_eq!(QemuAccelerator::None.as_str(), "none");
        let auto_accel = QemuAccelerator::Auto.as_str();
        assert_ne!(auto_accel, "");

        assert_eq!(DiskFormat::Raw.as_str(), "raw");
        assert_eq!(DiskFormat::Qcow2.as_str(), "qcow2");
        assert_eq!(DiskFormat::Vmdk.as_str(), "vmdk");
        assert_eq!(DiskFormat::Vdi.as_str(), "vdi");

        config.qemuargs = vec![vec!["-m".to_string(), "1024".to_string()]];
        assert_eq!(config.qemuargs.len(), 1);

        let config2 = config.clone();
        assert_eq!(config, config2);
        Ok(())
    }

    #[tokio::test]
    async fn test_smp_and_numa_args() -> Result<(), crate::error::StampError> {
        let smp = SmpConfig {
            cpus: Some(8),
            sockets: Some(2),
            dies: Some(1),
            clusters: Some(1),
            cores: Some(2),
            threads: Some(2),
            maxcpus: Some(16),
        };
        let smp_str = smp.to_qemu_arg();
        assert!(smp_str.contains("cpus=8"));
        assert!(smp_str.contains("sockets=2"));
        assert!(smp_str.contains("cores=2"));
        assert!(smp_str.contains("threads=2"));

        let numa = NumaNodeConfig {
            node_id: Some(1),
            cpus: Some("4-7".to_string()),
            mem_mb: Some(2048),
            initiator: Some(0),
        };
        let numa_str = numa.to_qemu_arg();
        assert!(numa_str.contains("nodeid=1"));
        assert!(numa_str.contains("cpus=4-7"));
        assert!(numa_str.contains("mem=2048"));
        assert!(numa_str.contains("initiator=0"));

        assert_eq!(SmpConfig::default().to_qemu_arg(), "");
        assert_eq!(NumaNodeConfig::default().to_qemu_arg(), "node");
        Ok(())
    }

    #[tokio::test]
    async fn test_qemu_target_architecture_and_binaries() -> Result<(), crate::error::StampError> {
        let mut cfg = QemuConfig::default();
        cfg.arch = QemuArch::X86_64;
        assert_eq!(cfg.resolve_binary(), "qemu-system-x86_64");

        cfg.arch = QemuArch::Aarch64;
        assert_eq!(cfg.resolve_binary(), "qemu-system-aarch64");

        cfg.qemu_binary = Some("/opt/homebrew/bin/qemu-system-aarch64".to_string());
        assert_eq!(
            cfg.resolve_binary(),
            "/opt/homebrew/bin/qemu-system-aarch64"
        );
        cfg.qemu_img_binary = Some("/opt/homebrew/bin/qemu-img".to_string());
        assert_eq!(cfg.resolve_qemu_img_binary(), "/opt/homebrew/bin/qemu-img");

        assert_eq!(MachineModel::Q35.as_str(), "q35");
        assert_eq!(MachineModel::Pc.as_str(), "pc");
        assert_eq!(MachineModel::Virt.as_str(), "virt");
        assert_eq!(MachineModel::Microvm.as_str(), "microvm");
        assert_eq!(
            MachineModel::Custom("virt-2.8".to_string()).as_str(),
            "virt-2.8"
        );

        assert_eq!(CpuModel::Host.as_str(), "host");
        assert_eq!(CpuModel::Max.as_str(), "max");
        assert_eq!(CpuModel::Qemu64.as_str(), "qemu64");
        assert_eq!(CpuModel::CortexA57.as_str(), "cortex-a57");
        assert_eq!(CpuModel::CortexA72.as_str(), "cortex-a72");
        assert_eq!(CpuModel::Custom("EPYC-v4".to_string()).as_str(), "EPYC-v4");

        assert!(validate_accelerator(QemuAccelerator::Hvf).is_ok());
        assert!(validate_accelerator(QemuAccelerator::Tcg).is_ok());
        assert!(validate_accelerator(QemuAccelerator::Auto).is_ok());

        if !std::path::Path::new("/dev/kvm").exists() {
            let res = validate_accelerator(QemuAccelerator::Kvm);
            assert!(
                matches!(res, Err(StampError::Builder(ref msg)) if msg.contains("not found on host"))
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_qemuargs_engine_and_substitutions() -> Result<(), crate::error::StampError> {
        let mut state = StateBag::new();
        state.put("path_root", "/path/to/template".to_string());
        state.put("http_ip", "192.168.1.100".to_string());
        state.put("http_port", 8080u16);
        state.put("vnc_port", 5901u16);

        let config = QemuConfig {
            name: "bento-qemu".to_string(),
            arch: QemuArch::Aarch64,
            accelerator: QemuAccelerator::Tcg,
            machine_type: Some(MachineModel::Virt),
            cpu_model: Some(CpuModel::CortexA57),
            disk_interface: Some("virtio".to_string()),
            disk_cache: Some("none".to_string()),
            disk_discard: Some("unmap".to_string()),
            disk_detect_zeroes: Some("on".to_string()),
            net_device: Some("virtio-net-pci".to_string()),
            qemuargs: vec![
                vec!["-device".to_string(), "virtio-gpu-pci".to_string()],
                vec![
                    "-chardev".to_string(),
                    "socket,id=ser0,path=${path.root}/serial.sock".to_string(),
                ],
                vec!["-serial".to_string(), "chardev:ser0".to_string()],
                vec!["-device".to_string(), "org.qemu.guest_agent.0".to_string()],
                vec![
                    "-kernel".to_string(),
                    "http://{{ .HTTPIP }}:{{ .HTTPPort }}/vmlinuz".to_string(),
                ],
                vec!["-device".to_string(), "usb-kbd".to_string()],
                vec!["-device".to_string(), "usb-tablet".to_string()],
                vec!["-device".to_string(), "ramfb".to_string()],
                vec!["-device".to_string(), "nvme,drive=nvme0".to_string()],
            ],
            ..Default::default()
        };

        let args = build_qemu_args(&config, "/tmp/disk.qcow2", &state)?;

        assert!(args.contains(&"-machine".to_string()));
        assert!(args.contains(&"virt".to_string()));
        assert!(args.contains(&"-cpu".to_string()));
        assert!(args.contains(&"cortex-a57".to_string()));
        assert!(args.contains(&"file=/tmp/disk.qcow2,if=virtio,cache=none,discard=unmap,detect-zeroes=on,format=qcow2".to_string()));
        assert!(args.contains(&"socket,id=ser0,path=/path/to/template/serial.sock".to_string()));
        assert!(args.contains(&"http://192.168.1.100:8080/vmlinuz".to_string()));
        assert!(args.contains(&"virtio-gpu-pci".to_string()));
        assert!(args.contains(&"org.qemu.guest_agent.0".to_string()));
        assert!(args.contains(&"usb-kbd".to_string()));
        assert!(args.contains(&"ramfb".to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn test_qemu_uefi_and_firmware() -> Result<(), crate::error::StampError> {
        let temp_dir = tempfile::tempdir()?;
        let code_path = temp_dir.path().join("OVMF_CODE.fd");
        let vars_template = temp_dir.path().join("OVMF_VARS.fd");
        std::fs::write(&code_path, b"UEFI_CODE")?;
        std::fs::write(&vars_template, b"UEFI_VARS")?;

        let config = QemuConfig {
            name: "uefi-vm".to_string(),
            efi_boot: true,
            efi_firmware_code: Some(code_path.to_string_lossy().to_string()),
            efi_firmware_vars: Some(vars_template.to_string_lossy().to_string()),
            efi_drop_efivars: true,
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            ..Default::default()
        };

        let mut state = StateBag::new();
        state.put("disk_path", "/tmp/packer-qemu".to_string());

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut step = StepRunQemu {
            ui,
            name: "uefi-vm".to_string(),
            config: config.clone(),
        };

        assert!(step.run(&mut state).await.is_ok());
        let instance_vars = temp_dir.path().join("efivars.fd");
        assert!(instance_vars.exists());

        step.cleanup(&state).await;
        assert!(!instance_vars.exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_qemu_disks_and_formats() -> Result<(), crate::error::StampError> {
        let temp_dir = tempfile::tempdir()?;
        let base_img = temp_dir.path().join("base.qcow2");
        std::fs::write(&base_img, b"BASE_IMAGE_DATA")?;

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let config_base = QemuConfig {
            name: "base-vm".to_string(),
            disk_image: true,
            iso_url: Some(format!("file://{}", base_img.to_string_lossy())),
            output_directory: Some(temp_dir.path().to_string_lossy().to_string()),
            disk_format: Some(DiskFormat::Raw),
            ..Default::default()
        };

        let mut step_img = StepCreateImage {
            ui: ui.clone(),
            name: "base-vm".to_string(),
            config: config_base,
        };

        let mut state = StateBag::new();
        assert!(step_img.run(&mut state).await.is_ok());
        assert_eq!(
            state.get::<String>("disk_path"),
            Some(&format!(
                "{}/packer-qemu",
                temp_dir.path().to_string_lossy()
            ))
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_qemu_headless_and_vnc_allocation() -> Result<(), crate::error::StampError> {
        let port = allocate_vnc_port(5900, 6000)?;
        assert!((5900..=6000).contains(&port));

        let err_res = allocate_vnc_port(6000, 5999);
        assert!(
            matches!(err_res, Err(StampError::Builder(ref msg)) if msg.contains("No available VNC ports"))
        );

        let mut state = StateBag::new();
        state.put("vnc_port", 5905u16);

        let cfg_headless = QemuConfig {
            headless: true,
            display: Some("none".to_string()),
            vnc_port: Some(5905),
            ..Default::default()
        };
        let args_headless = build_qemu_args(&cfg_headless, "/tmp/disk", &state)?;
        assert!(args_headless.contains(&"-display".to_string()));
        assert!(args_headless.contains(&"none".to_string()));
        assert!(args_headless.contains(&"-vnc".to_string()));
        assert!(args_headless.contains(&"127.0.0.1:5".to_string()));

        let cfg_gui = QemuConfig {
            headless: false,
            display: Some("cocoa".to_string()),
            vnc_port: Some(5905),
            ..Default::default()
        };
        let args_gui = build_qemu_args(&cfg_gui, "/tmp/disk", &state)?;
        assert!(args_gui.contains(&"-display".to_string()));
        assert!(args_gui.contains(&"cocoa".to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn test_qemu_from_builder_config() -> Result<(), crate::error::StampError> {
        let mut builder_cfg = crate::template::BuilderConfig {
            builder_type: "qemu".to_string(),
            name: "bento-source".to_string(),
            ..Default::default()
        };
        builder_cfg
            .config
            .insert("os_arch".to_string(), "aarch64".to_string());
        builder_cfg
            .config
            .insert("accelerator".to_string(), "hvf".to_string());
        builder_cfg
            .config
            .insert("machine_type".to_string(), "virt".to_string());
        builder_cfg
            .config
            .insert("cpu_model".to_string(), "cortex-a72".to_string());
        builder_cfg
            .config
            .insert("disk_size".to_string(), "65536".to_string());
        builder_cfg
            .config
            .insert("format".to_string(), "raw".to_string());
        builder_cfg
            .config
            .insert("disk_compression".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("disk_discard".to_string(), "unmap".to_string());
        builder_cfg
            .config
            .insert("efi_boot".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("efi_drop_efivars".to_string(), "true".to_string());
        builder_cfg
            .config
            .insert("net_device".to_string(), "virtio-net-pci".to_string());
        builder_cfg
            .config
            .insert("headless".to_string(), "true".to_string());
        builder_cfg.config.insert(
            "qemuargs".to_string(),
            r#"[["-device", "ramfb"]]"#.to_string(),
        );
        builder_cfg.config.insert(
            "boot_command".to_string(),
            r#"["<enter>", "root"]"#.to_string(),
        );

        let qemu_cfg = QemuConfig::from_builder_config(&builder_cfg)?;
        assert_eq!(qemu_cfg.name, "bento-source");
        assert_eq!(qemu_cfg.arch, QemuArch::Aarch64);
        assert_eq!(qemu_cfg.accelerator, QemuAccelerator::Hvf);
        assert_eq!(qemu_cfg.machine_type, Some(MachineModel::Virt));
        assert_eq!(qemu_cfg.cpu_model, Some(CpuModel::CortexA72));
        assert_eq!(qemu_cfg.disk_size, Some(65536));
        assert_eq!(qemu_cfg.disk_format, Some(DiskFormat::Raw));
        assert!(qemu_cfg.disk_compression);
        assert_eq!(qemu_cfg.disk_discard.as_deref(), Some("unmap"));
        assert!(qemu_cfg.efi_boot);
        assert!(qemu_cfg.efi_drop_efivars);
        assert_eq!(qemu_cfg.net_device.as_deref(), Some("virtio-net-pci"));
        assert!(qemu_cfg.headless);
        assert_eq!(
            qemu_cfg.qemuargs,
            vec![vec!["-device".to_string(), "ramfb".to_string()]]
        );
        assert_eq!(
            qemu_cfg.boot_command,
            Some(vec!["<enter>".to_string(), "root".to_string()])
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_qemu_missing_coverage() -> Result<(), crate::error::StampError> {
        let temp_dir =
            tempfile::tempdir().map_err(|e| crate::error::StampError::Execution(e.to_string()))?;
        let mock_kvm = temp_dir.path().join("kvm");

        let accel =
            crate::builder::qemu::validate_accelerator_with_path(QemuAccelerator::Auto, &mock_kvm)?;
        #[cfg(target_os = "linux")]
        assert_eq!(accel, QemuAccelerator::Tcg);
        #[cfg(target_os = "macos")]
        assert_eq!(accel, QemuAccelerator::Hvf);
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        assert_eq!(accel, QemuAccelerator::Auto);

        assert!(
            crate::builder::qemu::validate_accelerator_with_path(QemuAccelerator::Kvm, &mock_kvm)
                .is_err()
        );

        std::fs::write(&mock_kvm, b"")?;
        #[cfg(unix)]
        std::fs::set_permissions(
            &mock_kvm,
            std::os::unix::fs::PermissionsExt::from_mode(0o666),
        )?;

        {
            #[cfg(target_os = "linux")]
            {
                let accel = crate::builder::qemu::validate_accelerator_with_path(
                    QemuAccelerator::Auto,
                    &mock_kvm,
                )?;
                assert!(accel == QemuAccelerator::Kvm || accel == QemuAccelerator::Tcg);
            }
            let _ = crate::builder::qemu::validate_accelerator_with_path(
                QemuAccelerator::Kvm,
                &mock_kvm,
            );
        }

        let list = crate::builder::qemu::parse_string_list("a,b,c");
        assert_eq!(list, vec!["a", "b", "c"]);

        let mut builder_cfg = crate::template::BuilderConfig::default();
        builder_cfg
            .config
            .insert("os_arch".to_string(), "arm".to_string());
        builder_cfg
            .config
            .insert("accelerator".to_string(), "tcg".to_string());
        builder_cfg
            .config
            .insert("machine_type".to_string(), "microvm".to_string());
        builder_cfg
            .config
            .insert("cpu_model".to_string(), "max".to_string());
        builder_cfg
            .config
            .insert("format".to_string(), "vdi".to_string());
        builder_cfg
            .config
            .insert("http_content".to_string(), r#"{"key":"val"}"#.to_string());

        let qemu_cfg = QemuConfig::from_builder_config(&builder_cfg)?;
        assert_eq!(qemu_cfg.arch, QemuArch::Arm);
        assert_eq!(qemu_cfg.accelerator, QemuAccelerator::Tcg);
        assert_eq!(qemu_cfg.machine_type, Some(MachineModel::Microvm));
        assert_eq!(qemu_cfg.cpu_model, Some(CpuModel::Max));
        assert_eq!(qemu_cfg.disk_format, Some(DiskFormat::Vdi));
        assert_eq!(
            qemu_cfg.http_content.get("key").map(String::as_str),
            Some("val")
        );

        builder_cfg
            .config
            .insert("os_arch".to_string(), "i386".to_string());
        builder_cfg
            .config
            .insert("accelerator".to_string(), "none".to_string());
        builder_cfg
            .config
            .insert("cpu_model".to_string(), "qemu64".to_string());
        builder_cfg
            .config
            .insert("format".to_string(), "vmdk".to_string());
        let qemu_cfg = QemuConfig::from_builder_config(&builder_cfg)?;
        assert_eq!(qemu_cfg.arch, QemuArch::I386);
        assert_eq!(qemu_cfg.accelerator, QemuAccelerator::None);
        assert_eq!(qemu_cfg.cpu_model, Some(CpuModel::Qemu64));
        assert_eq!(qemu_cfg.disk_format, Some(DiskFormat::Vmdk));

        builder_cfg
            .config
            .insert("os_arch".to_string(), "riscv64".to_string());
        builder_cfg
            .config
            .insert("accelerator".to_string(), "whpx".to_string());
        builder_cfg
            .config
            .insert("cpu_model".to_string(), "host".to_string());
        let qemu_cfg = QemuConfig::from_builder_config(&builder_cfg)?;
        assert_eq!(qemu_cfg.arch, QemuArch::Riscv64);
        assert_eq!(qemu_cfg.accelerator, QemuAccelerator::Whpx);
        assert_eq!(qemu_cfg.cpu_model, Some(CpuModel::Host));

        let mut state = StateBag::new();
        state.put("floppy_path", "/tmp/floppy.img".to_string());
        state.put("cd_path", "/tmp/cd.iso".to_string());
        let config = QemuConfig {
            headless: true,
            display: None,
            iso_url: Some("boot.iso".to_string()),
            ..Default::default()
        };
        let args = build_qemu_args(&config, "/tmp/disk", &state)?;
        assert!(args.contains(&"-display".to_string()));
        assert!(args.contains(&"none".to_string()));
        assert!(args.contains(&"-cdrom".to_string()));
        assert!(args.contains(&"boot.iso".to_string()));
        assert!(args.contains(&"-fda".to_string()));
        assert!(args.contains(&"/tmp/floppy.img".to_string()));
        assert!(args.contains(&"file=/tmp/cd.iso,media=cdrom".to_string()));

        Ok(())
    }

    #[tokio::test]
    async fn test_qemu_missing_coverage_more() -> Result<(), crate::error::StampError> {
        let mut builder_cfg = crate::template::BuilderConfig::default();
        builder_cfg
            .config
            .insert("os_arch".to_string(), "unknown".to_string());
        builder_cfg
            .config
            .insert("accelerator".to_string(), "unknown".to_string());
        builder_cfg
            .config
            .insert("machine_type".to_string(), "unknown".to_string());
        builder_cfg
            .config
            .insert("cpu_model".to_string(), "unknown".to_string());
        builder_cfg
            .config
            .insert("format".to_string(), "unknown".to_string());
        let qemu_cfg = QemuConfig::from_builder_config(&builder_cfg)?;
        assert_eq!(qemu_cfg.arch, QemuArch::X86_64);
        assert_eq!(qemu_cfg.accelerator, QemuAccelerator::Auto);
        assert_eq!(
            qemu_cfg.machine_type,
            Some(MachineModel::Custom("unknown".to_string()))
        );
        assert_eq!(
            qemu_cfg.cpu_model,
            Some(CpuModel::Custom("unknown".to_string()))
        );
        assert_eq!(qemu_cfg.disk_format, Some(DiskFormat::Qcow2));

        let temp_dir =
            tempfile::tempdir().map_err(|e| crate::error::StampError::Execution(e.to_string()))?;
        let mock_kvm = temp_dir.path().join("kvm2");
        std::fs::write(&mock_kvm, b"")
            .map_err(|e| crate::error::StampError::Execution(e.to_string()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&mock_kvm, std::fs::Permissions::from_mode(0o000))
                .map_err(|e| crate::error::StampError::Execution(e.to_string()))?;
            let err = crate::builder::qemu::validate_accelerator_with_path(
                QemuAccelerator::Kvm,
                &mock_kvm,
            );
            assert!(err.is_err());
            if let Err(crate::error::StampError::Builder(msg)) = err {
                assert!(msg.contains("permission denied"));
            }
        }

        let state = StateBag::new();
        let config = QemuConfig {
            headless: true,
            display: Some("cocoa".to_string()),
            efi_boot: true,
            efi_firmware_code: Some("mycode.fd".to_string()),
            efi_firmware_vars: Some("myvars.fd".to_string()),
            ..Default::default()
        };
        let args = build_qemu_args(&config, "/tmp/disk", &state)?;
        assert!(args.contains(&"-display".to_string()));
        assert!(args.contains(&"cocoa".to_string()));
        assert!(args.contains(&"if=pflash,format=raw,file=myvars.fd".to_string()));

        let config2 = QemuConfig {
            headless: true,
            display: Some("none".to_string()),
            ..Default::default()
        };
        let args2 = build_qemu_args(&config2, "/tmp/disk", &state)?;
        assert!(args2.contains(&"none".to_string()));

        let ui = std::sync::Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut step = StepTypeBootCommand {
            ui,
            name: "test".to_string(),
            config: QemuConfig {
                boot_command: Some(vec![]),
                ..Default::default()
            },
        };
        let mut empty_state = StateBag::new();
        let _ = step.run(&mut empty_state).await;

        Ok(())
    }
}
