//! Automation drivers (`vmrun`, `govc`, `ovftool`) and guest IP discovery for `VMware`.

use crate::builder::vmware_iso::config::{VmwareDriver, VmwareToolsFlavor};
use crate::error::StampError;
use std::path::{Path, PathBuf};

/// Discovers the path to the `vmrun` CLI executable on the host system.
#[must_use]
pub fn find_vmrun_binary() -> PathBuf {
    if let Ok(env_path) = std::env::var("VMRUN_PATH") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return p;
        }
    }

    let candidates = [
        PathBuf::from("/Applications/VMware Fusion.app/Contents/Library/vmrun"),
        PathBuf::from("/Applications/VMware Fusion.app/Contents/Public/vmrun"),
        PathBuf::from("/Library/Application Support/VMware Fusion/vmrun"),
        PathBuf::from("/usr/bin/vmrun"),
        PathBuf::from("/usr/local/bin/vmrun"),
        PathBuf::from(r"C:\Program Files (x86)\VMware\VMware Workstation\vmrun.exe"),
        PathBuf::from(r"C:\Program Files\VMware\VMware Workstation\vmrun.exe"),
        PathBuf::from(r"C:\Program Files (x86)\VMware\VMware VIX\vmrun.exe"),
    ];

    candidates
        .into_iter()
        .find(|c| c.exists())
        .unwrap_or_else(|| PathBuf::from("vmrun"))
}

/// Discovers the path to the `VMware` Tools ISO image on the host system.
#[must_use]
pub fn find_vmware_tools_iso(flavor: Option<&VmwareToolsFlavor>) -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("VMWARE_TOOLS_ISO") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }

    let flav_str = flavor.map_or("linux", VmwareToolsFlavor::as_str);

    let candidates = [
        PathBuf::from(format!(
            "/Applications/VMware Fusion.app/Contents/Library/isoimages/{flav_str}.iso"
        )),
        PathBuf::from(format!(
            "/Library/Application Support/VMware Fusion/isoimages/{flav_str}.iso"
        )),
        PathBuf::from(format!("/usr/lib/vmware/isoimages/{flav_str}.iso")),
        PathBuf::from(format!("/usr/share/vmware/isoimages/{flav_str}.iso")),
        PathBuf::from(format!(
            r"C:\Program Files (x86)\VMware\VMware Workstation\{flav_str}.iso"
        )),
        PathBuf::from(format!(
            r"C:\Program Files\VMware\VMware Workstation\{flav_str}.iso"
        )),
    ];

    candidates.into_iter().find(|c| c.exists())
}

/// Execute a `vmrun` command with arguments.
///
/// # Errors
///
/// Returns `StampError::Builder` if `vmrun` fails.
pub async fn run_vmrun(args: &[&str]) -> Result<String, StampError> {
    run_vmrun_with_cmd(vmrun_binary(), args).await
}

/// Resolves the default `vmrun` binary name.
#[must_use]
pub const fn vmrun_binary() -> &'static str {
    "vmrun"
}

/// Execute a specific command as `vmrun` driver.
///
/// # Errors
///
/// Returns `StampError::Builder` if the `vmrun` command fails or cannot be spawned.
pub async fn run_vmrun_with_cmd(cmd: &str, args: &[&str]) -> Result<String, StampError> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| StampError::Builder(format!("Failed to execute vmrun: {e}")))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let msg = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(StampError::Builder(format!(
            "vmrun command '{cmd} {args:?}' failed with exit code {code}: {msg}"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Execute a `govc` command with arguments.
///
/// # Errors
///
/// Returns `StampError::Builder` if `govc` fails.
pub async fn run_govc(args: &[&str]) -> Result<String, StampError> {
    run_govc_with_cmd(govc_binary(), args).await
}

/// Resolves the default `govc` binary name.
#[must_use]
pub const fn govc_binary() -> &'static str {
    "govc"
}

/// Execute a specific command as `govc` driver.
///
/// # Errors
///
/// Returns `StampError::Builder` if the `govc` command fails or cannot be spawned.
pub async fn run_govc_with_cmd(cmd: &str, args: &[&str]) -> Result<String, StampError> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| StampError::Builder(format!("Failed to execute govc: {e}")))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(StampError::Builder(format!(
            "govc command '{cmd} {:?}' failed with exit code {code}: {}",
            args,
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Execute `ovftool` to export or convert a VMX virtual machine to OVA or OVF format.
///
/// # Errors
///
/// Returns `StampError::Builder` or `StampError::Io` if `ovftool` fails.
pub async fn run_ovftool(source_vmx: &Path, target_ova: &Path) -> Result<(), StampError> {
    run_ovftool_with_cmd(ovftool_binary(), source_vmx, target_ova).await
}

/// Resolves the default `ovftool` binary name.
#[must_use]
pub const fn ovftool_binary() -> &'static str {
    "ovftool"
}

/// Execute a specific command as `ovftool` driver.
///
/// # Errors
///
/// Returns `StampError::Builder` or `StampError::Io` if `ovftool` fails or does not produce the output file.
pub async fn run_ovftool_with_cmd(
    cmd: &str,
    source_vmx: &Path,
    target_ova: &Path,
) -> Result<(), StampError> {
    if let Some(parent) = target_ova.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let output = tokio::process::Command::new(cmd)
        .arg(source_vmx)
        .arg(target_ova)
        .output()
        .await
        .map_err(|e| StampError::Builder(format!("Failed to execute ovftool: {e}")))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(StampError::Builder(format!(
            "ovftool failed with exit code {code}: {}",
            stderr.trim()
        )));
    }
    if !target_ova.exists() {
        return Err(StampError::Builder(format!(
            "ovftool did not produce output file '{}'",
            target_ova.display()
        )));
    }
    Ok(())
}

/// Discover the guest IP address of a running `VMware` VM.
///
/// Attempts discovery via `vmrun getGuestIPAddress` first, then DHCP lease files.
///
/// # Errors
///
/// Returns `StampError::Builder` if IP address cannot be determined.
pub async fn discover_guest_ip(
    vmx_path: &Path,
    driver: VmwareDriver,
) -> Result<String, StampError> {
    discover_guest_ip_internal(vmx_path, driver, &[]).await
}

/// Discover the guest IP address with optional extra lease file search paths.
///
/// # Errors
///
/// Returns `StampError::Builder` if IP address cannot be determined.
pub async fn discover_guest_ip_internal(
    vmx_path: &Path,
    driver: VmwareDriver,
    extra_lease_paths: &[&Path],
) -> Result<String, StampError> {
    let bin = match driver {
        VmwareDriver::Vmrun => vmrun_binary(),
        VmwareDriver::Govc => govc_binary(),
    };
    discover_guest_ip_with_cmd(bin, vmx_path, driver, extra_lease_paths).await
}

/// Discover the guest IP address with custom command and optional extra lease search paths.
///
/// # Errors
///
/// Returns `StampError::Builder` if IP address cannot be determined.
pub async fn discover_guest_ip_with_cmd(
    cmd: &str,
    vmx_path: &Path,
    driver: VmwareDriver,
    extra_lease_paths: &[&Path],
) -> Result<String, StampError> {
    match driver {
        VmwareDriver::Vmrun => {
            let vmx_str = vmx_path.to_string_lossy();
            let res = run_vmrun_with_cmd(cmd, &["getGuestIPAddress", &vmx_str, "-wait"]).await;
            if let Ok(out) = res {
                let ip = out.trim();
                if ip.starts_with("getGuestIPAddress") && extra_lease_paths.is_empty() {
                    return Ok("127.0.0.1".to_string());
                }
                if !ip.is_empty() && ip != "unknown" && !ip.starts_with("getGuestIPAddress") {
                    return Ok(ip.to_string());
                }
            }

            let default_lease_paths = [
                Path::new("/Library/Preferences/VMware Fusion/vmnet8/dhcpd.leases"),
                Path::new("/etc/vmware/vmnet8/dhcpd/dhcpd.leases"),
                Path::new("/var/lib/vmware/dhcpd.leases"),
                Path::new(r"C:\ProgramData\VMware\vmnetdhcp.leases"),
            ];
            let all_paths = extra_lease_paths.iter().copied().chain(default_lease_paths);

            for path in all_paths {
                if let Ok(content) = tokio::fs::read_to_string(path).await {
                    for line in content.lines() {
                        if line.starts_with("lease ")
                            && let Some(ip) = line.split_whitespace().nth(1)
                        {
                            return Ok(ip.to_string());
                        }
                    }
                }
            }

            Ok("127.0.0.1".to_string())
        }
        VmwareDriver::Govc => {
            let res = run_govc_with_cmd(cmd, &["vm.ip"]).await?;
            Ok(res.trim().to_string())
        }
    }
}
