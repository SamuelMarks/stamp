//! `.vmx` configuration file generator for `VMware` virtual machines.

use crate::builder::vmware_iso::config::{
    VmwareCdromAdapterType, VmwareDiskAdapterType, VmwareFirmware, VmwareNicType,
};
use std::collections::HashMap;
use std::fmt::Write as _;

/// Generate `.vmx` configuration file text with default and injected parameters.
#[must_use]
pub fn generate_vmx_content<S: std::hash::BuildHasher>(
    vm_name: &str,
    guest_os: &str,
    mem_size: u64,
    cpus: u32,
    iso_path: Option<&str>,
    custom_data: &HashMap<String, String, S>,
) -> String {
    generate_vmx_content_full(
        vm_name,
        guest_os,
        mem_size,
        cpus,
        None,
        iso_path,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        None,
        false,
        custom_data,
    )
}

/// Generate comprehensive `.vmx` configuration file text supporting all Bento/Packer options.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn generate_vmx_content_full<S: std::hash::BuildHasher>(
    vm_name: &str,
    guest_os: &str,
    mem_size: u64,
    cpus: u32,
    cores: Option<u32>,
    iso_path: Option<&str>,
    disk_adapter: Option<&VmwareDiskAdapterType>,
    cdrom_adapter: Option<&VmwareCdromAdapterType>,
    nic_type: Option<&VmwareNicType>,
    network: Option<&str>,
    firmware: Option<&VmwareFirmware>,
    version: Option<&str>,
    usb: Option<bool>,
    remove_ethernet: bool,
    vnc_port: Option<u16>,
    vnc_disable_password: bool,
    custom_data: &HashMap<String, String, S>,
) -> String {
    let hw_ver = version.unwrap_or("14");
    let mut vmx = format!(
        r#".encoding = "UTF-8"
config.version = "8"
virtualHW.version = "{hw_ver}"
displayName = "{vm_name}"
guestOS = "{guest_os}"
memsize = "{mem_size}"
numvcpus = "{cpus}"
"#
    );

    if let Some(c) = cores {
        let _ = writeln!(vmx, r#"cpuid.coresPerSocket = "{c}""#);
    }

    match firmware {
        Some(VmwareFirmware::Efi) => {
            let _ = writeln!(vmx, r#"firmware = "efi""#);
        }
        Some(VmwareFirmware::EfiSecure) => {
            let _ = writeln!(vmx, r#"firmware = "efi""#);
            let _ = writeln!(vmx, r#"uefi.secureBoot.enabled = "TRUE""#);
        }
        Some(VmwareFirmware::Bios) => {
            let _ = writeln!(vmx, r#"firmware = "bios""#);
        }
        Some(VmwareFirmware::Custom(s)) => {
            let _ = writeln!(vmx, r#"firmware = "{s}""#);
        }
        None => {}
    }

    match disk_adapter.unwrap_or(&VmwareDiskAdapterType::LsiLogic) {
        VmwareDiskAdapterType::Nvme => {
            let _ = writeln!(
                vmx,
                r#"nvme0.present = "TRUE"
nvme0:0.present = "TRUE"
nvme0:0.fileName = "disk.vmdk""#
            );
        }
        VmwareDiskAdapterType::Sata => {
            let _ = writeln!(
                vmx,
                r#"sata0.present = "TRUE"
sata0:0.present = "TRUE"
sata0:0.fileName = "disk.vmdk""#
            );
        }
        VmwareDiskAdapterType::Ide => {
            let _ = writeln!(
                vmx,
                r#"ide0:0.present = "TRUE"
ide0:0.fileName = "disk.vmdk""#
            );
        }
        scsi_type => {
            let dev_str = scsi_type.as_str();
            let _ = writeln!(
                vmx,
                r#"scsi0.present = "TRUE"
scsi0.virtualDev = "{dev_str}"
scsi0:0.present = "TRUE"
scsi0:0.fileName = "disk.vmdk""#
            );
        }
    }

    if let Some(iso) = iso_path {
        match cdrom_adapter.unwrap_or(&VmwareCdromAdapterType::Ide) {
            VmwareCdromAdapterType::Sata => {
                let _ = writeln!(
                    vmx,
                    r#"sata0:1.present = "TRUE"
sata0:1.deviceType = "cdrom-image"
sata0:1.fileName = "{iso}""#
                );
            }
            VmwareCdromAdapterType::Scsi => {
                let _ = writeln!(
                    vmx,
                    r#"scsi0:1.present = "TRUE"
scsi0:1.deviceType = "cdrom-image"
scsi0:1.fileName = "{iso}""#
                );
            }
            _ => {
                let _ = writeln!(
                    vmx,
                    r#"ide1:0.present = "TRUE"
ide1:0.deviceType = "cdrom-image"
ide1:0.fileName = "{iso}""#
                );
            }
        }
    }

    if !remove_ethernet {
        let net_type = network.unwrap_or("nat");
        let nic_str = nic_type.unwrap_or(&VmwareNicType::E1000).as_str();
        let _ = writeln!(
            vmx,
            r#"ethernet0.present = "TRUE"
ethernet0.connectionType = "{net_type}"
ethernet0.virtualDev = "{nic_str}"
ethernet0.addressType = "generated""#
        );
    }

    if usb == Some(true) {
        let _ = writeln!(
            vmx,
            r#"usb.present = "TRUE"
usb_xhci.present = "TRUE""#
        );
    }

    if let Some(port) = vnc_port {
        let _ = writeln!(
            vmx,
            r#"RemoteDisplay.vnc.enabled = "TRUE"
RemoteDisplay.vnc.port = "{port}""#
        );
        if vnc_disable_password {
            let _ = writeln!(vmx, r#"RemoteDisplay.vnc.password = """#);
        }
    }

    let _ = writeln!(
        vmx,
        r#"pciBridge0.present = "TRUE"
tools.upgrade.policy = "manual"
powerType.powerOff = "soft"
powerType.reset = "soft"
powerType.suspend = "soft""#
    );

    let mut sorted_keys: Vec<_> = custom_data.keys().collect();
    sorted_keys.sort();
    for k in sorted_keys {
        if let Some(v) = custom_data.get(k) {
            let _ = writeln!(vmx, r#"{k} = "{v}""#);
        }
    }

    vmx
}
