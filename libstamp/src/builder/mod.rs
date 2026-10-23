//! Builders for creating machine images.

use crate::error::StampError;
use async_trait::async_trait;

/// The main trait for all builders.
#[async_trait]
pub trait Builder: Send + Sync {
    /// Prepare the builder (e.g., validate configuration).
    async fn prepare(&self) -> Result<(), StampError>;

    /// Run the builder to create the artifact.
    async fn run(
        &self,
        hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError>;

    /// Cancel the builder and clean up resources.
    async fn cancel(&self) -> Result<(), StampError>;

    /// Get the builder name.
    fn name(&self) -> String;

    /// Get the builders this builder depends on.
    fn depends_on(&self) -> Vec<String> {
        Vec::new()
    }

    /// Clean up existing artifacts.
    async fn force_clean(&self) -> Result<(), StampError> {
        Ok(())
    }
}

/// `alicloud-ecs` builder.
pub mod alicloud;
/// `cloudsigma` builder.
pub mod cloudsigma;
/// `cloudstack` builder.
pub mod cloudstack;
/// `digitalocean` builder.
pub mod digitalocean;
/// `docker` builder.
pub mod docker;
/// `file` builder.
pub mod file;
/// Out-of-process Go plugin builder RPC client.
pub mod go_plugin;
/// `hetzner-cloud` builder.
pub mod hetzner_cloud;
/// Built-in HTTP micro-server for installer kickstarts and preseeds.
pub mod http_server;
/// `hyperone` builder.
pub mod hyperone;
/// `hyperv-iso` builder.
pub mod hyperv_iso;
/// `hyperv-vmcx` builder.
pub mod hyperv_vmcx;
/// `ionoscloud` builder.
pub mod ionoscloud;
/// `linode` builder.
pub mod linode;
/// `lxc` builder.
pub mod lxc;
/// `lxd` builder.
pub mod lxd;
/// `null` builder.
pub mod null;
/// `nutanix` builder.
pub mod nutanix;
/// `opennebula` builder.
pub mod opennebula;
/// `openstack` builder.
pub mod openstack;
/// `oracle-oci` builder.
pub mod oracle;
/// `outscale` builder.
pub mod outscale;
/// `parallels-ipsw` builder.
pub mod parallels_ipsw;
/// `parallels-iso` builder.
pub mod parallels_iso;
/// `parallels-pvm` builder.
pub mod parallels_pvm;
/// `podman` builder.
pub mod podman;
/// `proxmox-clone` builder.
pub mod proxmox_clone;
/// `proxmox-iso` builder.
pub mod proxmox_iso;
/// `qemu` builder.
pub mod qemu;
/// `scaleway` builder.
pub mod scaleway;
/// `tencentcloud-cvm` builder.
pub mod tencentcloud;
/// `triton` builder.
pub mod triton;
/// `upcloud` builder.
pub mod upcloud;
/// `utm-iso` builder.
pub mod utm_iso;
/// `vagrant` builder.
pub mod vagrant;
/// `virtualbox-iso` builder.
pub mod virtualbox_iso;
/// `virtualbox-ovf` builder.
pub mod virtualbox_ovf;
/// Shared virtualization media (FAT12, ISO9660) and boot command (VNC, SPICE) helpers.
pub mod virtualization;
/// `vmware-iso` builder.
pub mod vmware_iso;
/// `vmware-vmx` builder.
pub mod vmware_vmx;
/// `vsphere-iso` and `vsphere-clone` builders.
pub mod vsphere;
/// `vultr` builder.
pub mod vultr;
/// `yandex` builder.
pub mod yandex;

use crate::template::BuilderConfig;

/// Creates a new builder instance from configuration.
///
/// # Errors
///
/// Returns a `StampError` if the builder type is unknown.
fn create_raw_builder(config: &BuilderConfig) -> Result<Box<dyn Builder>, StampError> {
    match config.builder_type.as_str() {
        "file" => {
            let target = config.config.get("target").cloned().unwrap_or_default();
            let content = config.config.get("content").cloned();
            let source = config.config.get("source").cloned();
            Ok(Box::new(file::FileBuilder::new(file::FileConfig {
                name: config.name.clone(),
                target,
                content,
                source,
            })))
        }
        "null" => Ok(Box::new(null::NullBuilder::new(null::NullConfig {
            name: config.name.clone(),
            ..Default::default()
        }))),
        "alicloud-ecs" => Ok(Box::new(alicloud::AlicloudEcsBuilder::new(
            alicloud::AlicloudEcsConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "digitalocean" => Ok(Box::new(digitalocean::DigitalOceanBuilder::new(
            digitalocean::DigitalOceanConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "docker" => Ok(Box::new(docker::DockerBuilder::new(docker::DockerConfig {
            name: config.name.clone(),
            image: "ubuntu".to_string(), // Default stub for instantiation
            ..Default::default()
        }))),
        "hetzner-cloud" => Ok(Box::new(hetzner_cloud::HetznerCloudBuilder::new(
            hetzner_cloud::HetznerCloudConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "hyperv-iso" => Ok(Box::new(hyperv_iso::HypervIsoBuilder::new(
            hyperv_iso::HypervIsoConfig::from_builder_config(config)?,
        ))),
        "hyperv-vmcx" => Ok(Box::new(hyperv_vmcx::HypervVmcxBuilder::new(
            hyperv_vmcx::HypervVmcxConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "linode" => Ok(Box::new(linode::LinodeBuilder::new(linode::LinodeConfig {
            name: config.name.clone(),
            ..Default::default()
        }))),
        "lxd" => Ok(Box::new(lxd::LxdBuilder::new(lxd::LxdConfig {
            name: config.name.clone(),
            ..Default::default()
        }))),
        "openstack" => Ok(Box::new(openstack::OpenstackBuilder::new(
            openstack::OpenstackConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "parallels-iso" => Ok(Box::new(parallels_iso::ParallelsIsoBuilder::new(
            parallels_iso::ParallelsIsoConfig::from_builder_config(config)?,
        ))),
        "parallels-ipsw" => Ok(Box::new(parallels_ipsw::ParallelsIpswBuilder::new(
            parallels_ipsw::ParallelsIpswConfig::from_builder_config(config)?,
        ))),
        "parallels-pvm" => Ok(Box::new(parallels_pvm::ParallelsPvmBuilder::new(
            parallels_pvm::ParallelsPvmConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "proxmox-clone" => Ok(Box::new(proxmox_clone::ProxmoxCloneBuilder::new(
            proxmox_clone::ProxmoxCloneConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "proxmox-iso" => Ok(Box::new(proxmox_iso::ProxmoxIsoBuilder::new(
            proxmox_iso::ProxmoxIsoConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "qemu" => Ok(Box::new(qemu::QemuBuilder::new(
            qemu::QemuConfig::from_builder_config(config)?,
        ))),
        "podman" => Ok(Box::new(podman::PodmanBuilder::new(podman::PodmanConfig {
            name: config.name.clone(),
            image: "alpine".to_string(),
            ..Default::default()
        }))),
        "vagrant" => Ok(Box::new(vagrant::VagrantBuilder::new(
            vagrant::VagrantConfig {
                name: config.name.clone(),
                source_box: "ubuntu/focal64".to_string(),
                ..Default::default()
            },
        ))),
        "utm-iso" => Ok(Box::new(utm_iso::UtmIsoBuilder::new(
            utm_iso::UtmIsoConfig::from_builder_config(config)?,
        ))),
        "virtualbox-iso" => Ok(Box::new(virtualbox_iso::VirtualboxIsoBuilder::new(
            virtualbox_iso::VirtualboxIsoConfig::from_builder_config(config)?,
        ))),
        "virtualbox-ovf" => Ok(Box::new(virtualbox_ovf::VirtualboxOvfBuilder::new(
            virtualbox_ovf::VirtualboxOvfConfig::from_builder_config(config)?,
        ))),
        "vmware-iso" => Ok(Box::new(vmware_iso::VmwareIsoBuilder::new(
            vmware_iso::VmwareIsoConfig::from_builder_config(config)?,
        ))),
        "vmware-vmx" => Ok(Box::new(vmware_vmx::VmwareVmxBuilder::new(
            vmware_vmx::VmwareVmxConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "vsphere-iso" => Ok(Box::new(vsphere::VsphereIsoBuilder::new(
            vsphere::VsphereIsoConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "vsphere-clone" => Ok(Box::new(vsphere::VsphereCloneBuilder::new(
            vsphere::VsphereCloneConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "ionoscloud" | "oneandone" => Ok(Box::new(ionoscloud::IonosBuilder::new(
            ionoscloud::IonosConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "cloudsigma" => Ok(Box::new(cloudsigma::CloudSigmaBuilder::new(
            cloudsigma::CloudSigmaConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "opennebula" => Ok(Box::new(opennebula::OpenNebulaBuilder::new(
            opennebula::OpenNebulaConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "triton" => Ok(Box::new(triton::TritonBuilder::new(triton::TritonConfig {
            account: String::new(),
            image_name: config.name.clone(),
            source_machine_image: String::new(),
            machine_package: String::new(),
            ..Default::default()
        }))),
        "lxc" => Ok(Box::new(lxc::LxcBuilder::new(lxc::LxcConfig {
            output_name: config.name.clone(),
            image_name: String::new(),
            ..Default::default()
        }))),
        "oracle-oci" => Ok(Box::new(oracle::OracleOciBuilder::new(
            oracle::OracleOciConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "cloudstack" => Ok(Box::new(cloudstack::CloudstackBuilder::new(
            cloudstack::CloudstackConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "nutanix" => Ok(Box::new(nutanix::NutanixBuilder::new(
            nutanix::NutanixConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "scaleway" => Ok(Box::new(scaleway::ScalewayBuilder::new(
            scaleway::ScalewayConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        "vultr" => Ok(Box::new(vultr::VultrBuilder::new(vultr::VultrConfig {
            name: config.name.clone(),
            ..Default::default()
        }))),
        "tencentcloud-cvm" | "tencentcloud" => Ok(Box::new(
            tencentcloud::TencentCloudBuilder::new(tencentcloud::TencentCloudConfig {
                name: config.name.clone(),
                ..Default::default()
            }),
        )),
        "upcloud" => Ok(Box::new(upcloud::UpCloudBuilder::new(
            upcloud::UpCloudConfig {
                name: config.name.clone(),
                ..Default::default()
            },
        ))),
        _ => Err(StampError::Parse(format!(
            "Unknown builder type: {}",
            config.builder_type
        ))),
    }
}

/// Creates a new builder instance from configuration.
///
/// If `config.depends_on` is specified, wraps the builder in [`DependentBuilder`]
/// to retain dependency metadata.
///
/// # Errors
///
/// Returns a `StampError` if the builder type is unknown.
pub fn create_builder(config: &BuilderConfig) -> Result<Box<dyn Builder>, StampError> {
    let builder = create_raw_builder(config)?;
    if config.depends_on.is_empty() {
        Ok(builder)
    } else {
        Ok(Box::new(DependentBuilder::new(
            builder,
            config.depends_on.clone(),
        )))
    }
}

/// A builder wrapper that preserves explicit `depends_on` declarations.
pub struct DependentBuilder {
    /// Inner wrapped builder.
    pub inner: Box<dyn Builder>,
    /// List of builder names this builder depends on.
    pub depends_on_names: Vec<String>,
}

impl DependentBuilder {
    /// Creates a new `DependentBuilder`.
    #[must_use]
    pub fn new(inner: Box<dyn Builder>, depends_on_names: Vec<String>) -> Self {
        Self {
            inner,
            depends_on_names,
        }
    }
}

#[async_trait]
impl Builder for DependentBuilder {
    async fn prepare(&self) -> Result<(), StampError> {
        self.inner.prepare().await
    }

    async fn run(
        &self,
        hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        self.inner.run(hook, ui, on_error).await
    }

    async fn cancel(&self) -> Result<(), StampError> {
        self.inner.cancel().await
    }

    fn name(&self) -> String {
        self.inner.name()
    }

    fn depends_on(&self) -> Vec<String> {
        self.depends_on_names.clone()
    }

    async fn force_clean(&self) -> Result<(), StampError> {
        self.inner.force_clean().await
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_create_builder_known() {
        let types = vec![
            "file",
            "null",
            "alicloud-ecs",
            "digitalocean",
            "docker",
            "hetzner-cloud",
            "hyperv-iso",
            "hyperv-vmcx",
            "linode",
            "lxc",
            "lxd",
            "openstack",
            "oracle-oci",
            "parallels-iso",
            "parallels-ipsw",
            "parallels-pvm",
            "podman",
            "proxmox-clone",
            "proxmox-iso",
            "qemu",
            "triton",
            "utm-iso",
            "vagrant",
            "virtualbox-iso",
            "virtualbox-ovf",
            "vmware-iso",
            "vmware-vmx",
            "vsphere-iso",
            "vsphere-clone",
            "ionoscloud",
            "oneandone",
            "cloudsigma",
            "opennebula",
            "cloudstack",
            "nutanix",
            "scaleway",
            "vultr",
            "tencentcloud-cvm",
            "tencentcloud",
            "upcloud",
        ];

        for b_type in types {
            let c = BuilderConfig {
                builder_type: b_type.to_string(),
                name: "n".to_string(),
                ..Default::default()
            };
            assert!(create_builder(&c).is_ok());
        }
    }

    #[test]
    fn test_create_builder_unknown() {
        let c = BuilderConfig {
            builder_type: "unknown".to_string(),
            name: "n".to_string(),
            ..Default::default()
        };
        assert!(create_builder(&c).is_err());
    }

    #[test]
    fn test_create_builder_file_with_config() {
        let mut hm = std::collections::HashMap::new();
        hm.insert("target".to_string(), "t".to_string());
        hm.insert("content".to_string(), "c".to_string());
        hm.insert("source".to_string(), "s".to_string());
        let c = BuilderConfig {
            builder_type: "file".to_string(),
            name: "n".to_string(),
            depends_on: vec![],
            config: hm,
            ..Default::default()
        };
        assert!(create_builder(&c).is_ok());
    }

    #[tokio::test]
    async fn test_builder_force_clean_default() -> Result<(), StampError> {
        let b = null::NullBuilder::new(null::NullConfig {
            name: "n".to_string(),
            ..Default::default()
        });
        let res = b.force_clean().await;
        assert!(res.is_ok());
        Ok(())
    }

    #[test]
    fn test_builder_depends_on_default() {
        let b = null::NullBuilder::new(null::NullConfig {
            name: "n".to_string(),
            ..Default::default()
        });
        assert_eq!(b.depends_on().len(), 0);
    }

    #[tokio::test]
    async fn test_dependent_builder_lifecycle() -> Result<(), StampError> {
        let c = BuilderConfig {
            builder_type: "null".to_string(),
            name: "dep_builder".to_string(),
            depends_on: vec!["base_builder".to_string()],
            config: std::collections::HashMap::new(),
            ..Default::default()
        };
        let b = create_builder(&c)?;
        assert_eq!(b.name(), "dep_builder");
        assert_eq!(b.depends_on(), vec!["base_builder"]);

        b.prepare().await?;
        b.force_clean().await?;

        let hook = std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: std::sync::Arc::new(vec![]),
            error_cleanup_provisioners: std::sync::Arc::new(vec![]),
        });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let art = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert_eq!(art.id(), "dep_builder-artifact");

        b.cancel().await?;
        Ok(())
    }
}
