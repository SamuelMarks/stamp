//! Chroot communicator implementation.
//!
//! Executes commands and performs file operations directly within a local Linux
//! chroot jail directory without requiring network or SSH connectivity.

use crate::communicator::{Command, CommandResult, Communicator};
use crate::error::StampError;
use crate::types::FilePath;
use crate::utils::{CommandRunner, RealCommandRunner};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Communicator that executes commands inside a mounted chroot directory.
#[derive(Debug, Clone)]
pub struct ChrootCommunicator {
    /// The root path of the mounted chroot filesystem.
    chroot_dir: PathBuf,
    /// Command execution wrapper template (defaults to `chroot {{.Path}} /bin/sh -c "{{.Command}}"`).
    command_wrapper: Option<String>,
    /// Subprocess command runner for executing chroot commands.
    command_runner: Arc<dyn CommandRunner>,
}

impl ChrootCommunicator {
    /// Create a new `ChrootCommunicator`.
    #[must_use]
    pub fn new(chroot_dir: impl Into<PathBuf>) -> Self {
        Self {
            chroot_dir: chroot_dir.into(),
            command_wrapper: None,
            command_runner: Arc::new(RealCommandRunner),
        }
    }

    /// Set a custom command wrapper for invoking commands in the chroot.
    #[must_use]
    pub fn with_command_wrapper(mut self, wrapper: impl Into<String>) -> Self {
        self.command_wrapper = Some(wrapper.into());
        self
    }

    /// Set a custom [`CommandRunner`] for executing subprocess commands.
    #[must_use]
    pub fn with_command_runner(mut self, runner: Arc<dyn CommandRunner>) -> Self {
        self.command_runner = runner;
        self
    }

    /// Resolve a path relative to the chroot directory.
    #[must_use]
    pub fn resolve_path(&self, remote_path: &Path) -> PathBuf {
        let relative = remote_path.strip_prefix("/").unwrap_or(remote_path);
        self.chroot_dir.join(relative)
    }

    /// Formats the effective shell command using the configured `command_wrapper` or default `chroot` invocation.
    #[must_use]
    pub fn format_effective_command(&self, raw_cmd: &str) -> String {
        let chroot_str = self.chroot_dir.to_string_lossy();
        if let Some(ref wrapper) = self.command_wrapper {
            wrapper
                .replace("{{.Path}}", &chroot_str)
                .replace("{{.Command}}", raw_cmd)
        } else {
            format!("chroot {chroot_str} /bin/sh -c '{raw_cmd}'")
        }
    }
}

#[async_trait]
impl Communicator for ChrootCommunicator {
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        let effective_cmd = self.format_effective_command(&cmd.command);
        let out = self
            .command_runner
            .run_command("sh", &["-c", &effective_cmd], None)
            .await?;

        Ok(CommandResult {
            exit_code: out.exit_code,
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }

    async fn upload(
        &self,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        let target = self.resolve_path(&remote_path.0);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(StampError::Io)?;
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
            std::fs::create_dir_all(parent).map_err(StampError::Io)?;
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
    /// Optional custom unmount command executable (defaults to `"umount"` on Linux).
    umount_executable: Option<String>,
}

impl ChrootMountGuard {
    /// Standard pseudofilesystem mount targets required for Linux chroot environments.
    pub const DEFAULT_MOUNTS: &'static [&'static str] = &["proc", "sys", "dev", "dev/pts"];

    /// Creates mountpoints and binds filesystems inside the target chroot.
    ///
    /// # Errors
    /// Returns `StampError::Io` if mountpoint creation fails.
    pub fn mount(chroot_dir: impl Into<PathBuf>, mounts: &[&str]) -> Result<Self, StampError> {
        Self::mount_with_commands(chroot_dir, mounts, None, None)
    }

    /// Creates mountpoints and optionally executes `mount_cmd` for each mountpoint and `umount_cmd` on cleanup.
    ///
    /// # Errors
    /// Returns `StampError` if directory creation or mount command execution fails.
    pub fn mount_with_commands(
        chroot_dir: impl Into<PathBuf>,
        mounts: &[&str],
        mount_cmd: Option<&str>,
        umount_cmd: Option<String>,
    ) -> Result<Self, StampError> {
        let root = chroot_dir.into();
        let mut mounted = Vec::new();

        for &m in mounts {
            let target = root.join(m);
            if !target.exists() {
                std::fs::create_dir_all(&target).map_err(StampError::Io)?;
            }
            if let Some(cmd_bin) = mount_cmd {
                let status = std::process::Command::new(cmd_bin)
                    .arg("--bind")
                    .arg(format!("/{m}"))
                    .arg(&target)
                    .status()
                    .map_err(StampError::Io)?;
                if !status.success() {
                    return Err(StampError::Execution(format!(
                        "Failed to mount /{m} onto {}",
                        target.display()
                    )));
                }
            }
            mounted.push(m.to_string());
        }

        Ok(Self {
            chroot_dir: root,
            mounted,
            umount_executable: umount_cmd,
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
            if target.exists()
                && let Some(ref umount_bin) = self.umount_executable
            {
                let status = std::process::Command::new(umount_bin)
                    .arg("-l")
                    .arg(&target)
                    .status()
                    .map_err(StampError::Io)?;
                if !status.success() {
                    return Err(StampError::Execution(format!(
                        "Failed to unmount {}",
                        target.display()
                    )));
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
mod tests {
    use super::*;
    use crate::utils::{CommandOutput, MockCommandRunner};

    #[test]
    fn test_derived_traits() {
        let comm = ChrootCommunicator::new("/mnt/chroot");
        let cloned = comm.clone();
        assert_eq!(
            comm.resolve_path(Path::new("/etc/hosts")),
            cloned.resolve_path(Path::new("etc/hosts"))
        );
        assert!(format!("{comm:?}").contains("ChrootCommunicator"));
    }

    #[tokio::test]
    async fn test_chroot_communicator_execute() -> Result<(), StampError> {
        let mock_runner = Arc::new(MockCommandRunner {
            default_output: CommandOutput {
                exit_code: 0,
                stdout: "mock stdout".to_string(),
                stderr: String::new(),
            },
            ..Default::default()
        });
        mock_runner.on_pattern(
            "exit_non_zero",
            Ok(CommandOutput {
                exit_code: 1,
                stdout: String::new(),
                stderr: "command failed".to_string(),
            }),
        );

        let comm = ChrootCommunicator::new("/mnt/chroot")
            .with_command_wrapper("sudo chroot {{.Path}} {{.Command}}")
            .with_command_runner(mock_runner.clone());

        assert_eq!(
            comm.format_effective_command("ls -la"),
            "sudo chroot /mnt/chroot ls -la"
        );
        let res = comm.execute(&Command::new("ls -la".to_string())).await?;
        assert_eq!(res.exit_code, 0);
        assert_eq!(res.stdout, "mock stdout");

        let comm_unwrapped =
            ChrootCommunicator::new("/mnt/chroot").with_command_runner(mock_runner.clone());
        assert_eq!(
            comm_unwrapped.format_effective_command("whoami"),
            "chroot /mnt/chroot /bin/sh -c 'whoami'"
        );
        let res2 = comm_unwrapped
            .execute(&Command::new("whoami".to_string()))
            .await?;
        assert_eq!(res2.exit_code, 0);

        let fail_res = comm
            .execute(&Command::new("exit_non_zero".to_string()))
            .await?;
        assert_eq!(fail_res.exit_code, 1);
        assert_eq!(fail_res.stderr, "command failed");

        // Also test RealCommandRunner with echo wrapper
        let real_comm = ChrootCommunicator::new("/mnt/chroot")
            .with_command_wrapper("echo chroot_ran {{.Path}} {{.Command}}");
        let real_res = real_comm.execute(&Command::new("id".to_string())).await?;
        assert_eq!(real_res.exit_code, 0);
        assert!(real_res.stdout.contains("chroot_ran /mnt/chroot id"));
        Ok(())
    }

    #[tokio::test]
    async fn test_chroot_communicator_upload_download() -> Result<(), StampError> {
        let tmp_dir = tempfile::tempdir()?;
        let comm = ChrootCommunicator::new(tmp_dir.path());

        let local_src = tmp_dir.path().join("local.txt");
        tokio::fs::write(&local_src, b"chroot test payload").await?;

        let remote_dest = FilePath::new(PathBuf::from("/etc/test.txt"));
        comm.upload(&FilePath::new(local_src.clone()), &remote_dest)
            .await?;

        let uploaded_path = comm.resolve_path(&remote_dest.0);
        assert!(uploaded_path.exists());
        let content = tokio::fs::read(&uploaded_path).await?;
        assert_eq!(content, b"chroot test payload");

        let download_dest = tmp_dir.path().join("nested_out/downloaded.txt");
        comm.download(&remote_dest, &FilePath::new(download_dest.clone()))
            .await?;
        assert!(download_dest.exists());

        // Test root / top-level relative upload and download
        let rel_dest = FilePath::new(PathBuf::from("top_level.txt"));
        comm.upload(&FilePath::new(local_src), &rel_dest).await?;
        assert!(comm.resolve_path(&rel_dest.0).exists());

        let rel_download = tmp_dir.path().join("rel_down.txt");
        comm.download(&rel_dest, &FilePath::new(rel_download.clone()))
            .await?;
        assert!(rel_download.exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_chroot_communicator_errors() -> Result<(), StampError> {
        let tmp_dir = tempfile::tempdir()?;
        let comm = ChrootCommunicator::new(tmp_dir.path());

        // 1. Upload missing local file
        let missing_src = FilePath::new(tmp_dir.path().join("missing.txt"));
        let dest = FilePath::new(PathBuf::from("/etc/dest.txt"));
        assert!(comm.upload(&missing_src, &dest).await.is_err());

        // 2. Download missing remote file
        let missing_remote = FilePath::new(PathBuf::from("/missing.txt"));
        let local_dest = FilePath::new(tmp_dir.path().join("local_dest.txt"));
        assert!(comm.download(&missing_remote, &local_dest).await.is_err());
        Ok(())
    }

    #[test]
    fn test_chroot_mount_guard() -> Result<(), StampError> {
        let tmp_dir = tempfile::tempdir()?;
        let mut guard = ChrootMountGuard::mount(tmp_dir.path(), ChrootMountGuard::DEFAULT_MOUNTS)?;
        assert_eq!(guard.mounted_paths().len(), 4);
        assert!(tmp_dir.path().join("proc").exists());
        assert!(tmp_dir.path().join("sys").exists());
        assert!(tmp_dir.path().join("dev/pts").exists());
        assert!(format!("{guard:?}").contains("ChrootMountGuard"));

        guard.unmount()?;
        assert_eq!(guard.mounted_paths().len(), 0);

        // Test mount_with_commands using `true` and `false`
        let mut guard_cmd = ChrootMountGuard::mount_with_commands(
            tmp_dir.path(),
            &["proc"],
            Some("true"),
            Some("true".to_string()),
        )?;
        assert_eq!(guard_cmd.mounted_paths().len(), 1);
        guard_cmd.unmount()?;

        assert!(
            ChrootMountGuard::mount_with_commands(tmp_dir.path(), &["proc"], Some("false"), None,)
                .is_err()
        );

        let mut guard_umount_fail = ChrootMountGuard::mount_with_commands(
            tmp_dir.path(),
            &["proc"],
            None,
            Some("false".to_string()),
        )?;
        assert!(guard_umount_fail.unmount().is_err());

        // Test unmount when mounted directory was removed
        let mut guard_missing = ChrootMountGuard::mount(tmp_dir.path(), &["removed_dir"])?;
        std::fs::remove_dir(tmp_dir.path().join("removed_dir"))?;
        guard_missing.unmount()?;

        // Test mount error when target cannot be created as a directory
        let blocker = tmp_dir.path().join("blocker_file");
        std::fs::write(&blocker, b"file content")?;
        let err_res = ChrootMountGuard::mount(tmp_dir.path(), &["blocker_file/sub"]);
        assert!(err_res.is_err());
        Ok(())
    }
}
