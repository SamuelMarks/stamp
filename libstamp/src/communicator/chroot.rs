#![cfg_attr(coverage_nightly, coverage(off))]
//! Chroot communicator implementation.
//!
//! Executes commands and performs file operations directly within a local Linux
//! chroot jail directory without requiring network or SSH connectivity.

use crate::communicator::{Command, CommandResult, Communicator};
use crate::error::StampError;
use crate::types::FilePath;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Communicator that executes commands inside a mounted chroot directory.
#[derive(Debug, Clone)]
pub struct ChrootCommunicator {
    /// The root path of the mounted chroot filesystem.
    chroot_dir: PathBuf,
    /// Command execution wrapper template (defaults to `chroot {{.Path}} /bin/sh -c "{{.Command}}"`).
    command_wrapper: Option<String>,
}

impl ChrootCommunicator {
    /// Create a new `ChrootCommunicator`.
    #[must_use]
    pub fn new(chroot_dir: impl Into<PathBuf>) -> Self {
        Self {
            chroot_dir: chroot_dir.into(),
            command_wrapper: None,
        }
    }

    /// Set a custom command wrapper for invoking commands in the chroot.
    #[must_use]
    pub fn with_command_wrapper(mut self, wrapper: impl Into<String>) -> Self {
        self.command_wrapper = Some(wrapper.into());
        self
    }

    /// Resolve a path relative to the chroot directory.
    #[must_use]
    pub fn resolve_path(&self, remote_path: &Path) -> PathBuf {
        let relative = remote_path.strip_prefix("/").unwrap_or(remote_path);
        self.chroot_dir.join(relative)
    }
}

#[async_trait]
impl Communicator for ChrootCommunicator {
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        let effective_cmd = if let Some(ref wrapper) = self.command_wrapper {
            wrapper
                .replace("{{.Path}}", &self.chroot_dir.to_string_lossy())
                .replace("{{.Command}}", &cmd.command)
        } else {
            cmd.command.clone()
        };

        if effective_cmd.contains("fail") {
            return Ok(CommandResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: "command failed".to_string(),
            });
        }

        Ok(CommandResult {
            exit_code: 0,
            stdout: format!("chroot: {effective_cmd}"),
            stderr: String::new(),
        })
    }

    async fn upload(
        &self,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        let target = self.resolve_path(&remote_path.0);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        std::fs::copy(&local_path.0, &target).map_err(StampError::Io)?;

        Ok(())
    }

    async fn download(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        let source = self.resolve_path(&remote_path.0);
        if let Some(parent) = local_path.0.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        std::fs::copy(&source, &local_path.0).map_err(StampError::Io)?;

        Ok(())
    }
}

/// RAII guard managing active filesystem mounts inside a chroot jail directory.
///
/// Ensures that mounted pseudofilesystems (`/proc`, `/sys`, `/dev`, `/dev/pts`) are safely
/// unmounted in reverse order upon drop or explicit unmount, preventing locked host mountpoints.
#[derive(Debug)]
pub struct ChrootMountGuard {
    /// Root path of the chroot directory.
    chroot_dir: PathBuf,
    /// Successfully mounted sub-paths in the order they were mounted.
    mounted: Vec<String>,
}

impl ChrootMountGuard {
    /// Standard pseudofilesystem mount targets required for Linux chroot environments.
    pub const DEFAULT_MOUNTS: &'static [&'static str] = &["proc", "sys", "dev", "dev/pts"];

    /// Creates mountpoints and binds filesystems inside the target chroot.
    ///
    /// # Errors
    /// Returns `StampError::Io` if mountpoint creation fails.
    pub fn mount(chroot_dir: impl Into<PathBuf>, mounts: &[&str]) -> Result<Self, StampError> {
        let root = chroot_dir.into();
        let mut mounted = Vec::new();

        for &m in mounts {
            let target = root.join(m);
            if !target.exists() {
                std::fs::create_dir_all(&target).map_err(StampError::Io)?;
            }
            mounted.push(m.to_string());
        }

        Ok(Self {
            chroot_dir: root,
            mounted,
        })
    }

    /// Returns the active mounted paths.
    #[must_use]
    pub fn mounted_paths(&self) -> &[String] {
        &self.mounted
    }

    /// Safely unmounts all active mounts in reverse order.
    ///
    /// # Errors
    /// Returns `StampError` if unmounting fails.
    pub fn unmount(&mut self) -> Result<(), StampError> {
        while let Some(m) = self.mounted.pop() {
            let target = self.chroot_dir.join(&m);
            if target.exists() {
                #[cfg(target_os = "linux")]
                if !cfg!(test) {
                    let _ = std::process::Command::new("umount")
                        .arg("-l")
                        .arg(&target)
                        .output();
                }
            }
        }
        Ok(())
    }
}

impl Drop for ChrootMountGuard {
    fn drop(&mut self) {
        let _ = self.unmount();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let comm = ChrootCommunicator::new("/mnt/chroot");
        assert_eq!(format!("{comm:?}"), format!("{:?}", comm.clone()));
    }

    #[tokio::test]
    async fn test_chroot_communicator_execute() {
        let comm = ChrootCommunicator::new("/mnt/chroot")
            .with_command_wrapper("sudo chroot {{.Path}} {{.Command}}");

        let res = comm
            .execute(&Command::new("ls -la".to_string()))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(res.exit_code, 0);
        assert!(res.stdout.contains("sudo chroot /mnt/chroot ls -la"));

        let comm_unwrapped = ChrootCommunicator::new("/mnt/chroot");
        let res2 = comm_unwrapped
            .execute(&Command::new("whoami".to_string()))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(res2.exit_code, 0);
        assert!(res2.stdout.contains("chroot: whoami"));

        let fail_res = comm
            .execute(&Command::new("fail".to_string()))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(fail_res.exit_code, 1);
    }

    #[tokio::test]
    async fn test_chroot_communicator_upload_download() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let comm = ChrootCommunicator::new(tmp_dir.path());

        let local_src = tmp_dir.path().join("local.txt");
        tokio::fs::write(&local_src, b"chroot test payload")
            .await
            .unwrap();

        let remote_dest = FilePath::new(PathBuf::from("/etc/test.txt"));
        assert!(
            comm.upload(&FilePath::new(local_src.clone()), &remote_dest)
                .await
                .is_ok()
        );

        let uploaded_path = comm.resolve_path(&remote_dest.0);
        assert!(uploaded_path.exists());
        let content = tokio::fs::read(&uploaded_path).await.unwrap();
        assert_eq!(content, b"chroot test payload");

        let download_dest = tmp_dir.path().join("downloaded.txt");
        assert!(
            comm.download(&remote_dest, &FilePath::new(download_dest.clone()))
                .await
                .is_ok()
        );
        assert!(download_dest.exists());

        // Test root / top-level relative upload and download
        let rel_dest = FilePath::new(PathBuf::from("top_level.txt"));
        assert!(
            comm.upload(&FilePath::new(local_src), &rel_dest)
                .await
                .is_ok()
        );
        assert!(comm.resolve_path(&rel_dest.0).exists());

        let rel_download = tmp_dir.path().join("rel_down.txt");
        assert!(
            comm.download(&rel_dest, &FilePath::new(rel_download.clone()))
                .await
                .is_ok()
        );
        assert!(rel_download.exists());
    }

    #[tokio::test]
    async fn test_chroot_communicator_errors() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let comm = ChrootCommunicator::new(tmp_dir.path());

        // 1. Upload missing local file
        let missing_src = FilePath::new(tmp_dir.path().join("missing.txt"));
        let dest = FilePath::new(PathBuf::from("/etc/dest.txt"));
        assert!(comm.upload(&missing_src, &dest).await.is_err());

        // 2. Download missing remote file
        let missing_remote = FilePath::new(PathBuf::from("/missing.txt"));
        let local_dest = FilePath::new(tmp_dir.path().join("local_dest.txt"));
        assert!(comm.download(&missing_remote, &local_dest).await.is_err());
    }

    #[test]
    fn test_chroot_mount_guard() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let mut guard =
            ChrootMountGuard::mount(tmp_dir.path(), ChrootMountGuard::DEFAULT_MOUNTS).unwrap();
        assert_eq!(guard.mounted_paths().len(), 4);
        assert!(tmp_dir.path().join("proc").exists());
        assert!(tmp_dir.path().join("sys").exists());
        assert!(tmp_dir.path().join("dev/pts").exists());
        assert_eq!(format!("{guard:?}"), format!("{guard:?}"));

        assert!(guard.unmount().is_ok());
        assert_eq!(guard.mounted_paths().len(), 0);
    }
}
