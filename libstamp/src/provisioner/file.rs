//! Implementation of the `file` provisioner.

use crate::communicator::Communicator;
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::FilePath;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Direction for the file transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum FileDirection {
    /// Upload a file or directory from the host to the guest.
    #[default]
    Upload,
    /// Download a file or directory from the guest to the host.
    Download,
}

/// Policy for handling symbolic links during directory traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SymlinkPolicy {
    /// Preserve the symbolic link as a symlink.
    #[default]
    Preserve,
    /// Follow the symbolic link and copy its target content.
    Follow,
    /// Ignore and skip symbolic links.
    Ignore,
}

/// Metadata captured for transferred filesystem entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMetadataInfo {
    /// Relative path within the root directory.
    pub relative_path: PathBuf,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Whether this entry is a symlink.
    pub is_symlink: bool,
    /// POSIX file mode permission bits if available.
    pub permissions_mode: Option<u32>,
    /// User ID of file owner if available on POSIX systems.
    pub uid: Option<u32>,
    /// Group ID of file owner if available on POSIX systems.
    pub gid: Option<u32>,
    /// Modification time as seconds since Unix epoch.
    pub modified_secs: Option<u64>,
    /// Target path if this entry is a symlink.
    pub symlink_target: Option<PathBuf>,
}

/// Configuration for the `file` provisioner.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct FileConfig {
    /// The source path.
    pub source: FilePath,
    /// The destination path.
    pub destination: FilePath,
    /// The direction of the file transfer.
    pub direction: FileDirection,
    /// Policy for handling symbolic links during directory traversal.
    pub symlink_policy: SymlinkPolicy,
    /// Whether the target system expects Windows backslash separators.
    pub target_windows: bool,
}

/// The `file` provisioner.
#[derive(Debug, Clone)]
pub struct FileProvisioner {
    /// The provisioner configuration.
    pub config: FileConfig,
}

impl FileProvisioner {
    /// Create a new `FileProvisioner`.
    #[must_use]
    pub const fn new(config: FileConfig) -> Self {
        Self { config }
    }

    /// Normalizes path separators for Windows (backslash) or POSIX (forward slash).
    #[must_use]
    pub fn normalize_path_separators(path: &str, target_windows: bool) -> String {
        if target_windows {
            path.replace('/', r#"\"#)
        } else {
            path.replace(r#"\"#, "/")
        }
    }

    /// Applies preserved metadata (permissions mode) to a local file.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Io`] if setting permissions fails.
    pub fn apply_local_metadata(path: &Path, meta: &FileMetadataInfo) -> Result<(), StampError> {
        #[cfg(unix)]
        if let Some(mode) = meta.permissions_mode {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(mode);
            fs::set_permissions(path, perms).map_err(StampError::Io)?;
        }
        #[cfg(not(unix))]
        let _ = (path, meta);

        Ok(())
    }

    /// Recursively scans a local directory collecting file metadata using default symlink policy.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Io`] if directory traversal fails.
    pub fn scan_directory_tree(root: &Path) -> Result<Vec<FileMetadataInfo>, StampError> {
        Self::scan_directory_tree_with_policy(root, SymlinkPolicy::default())
    }

    /// Recursively scans a local directory collecting file metadata, preserving permissions,
    /// modification times, owner IDs, and detecting symlink cycles safely according to `policy`.
    ///
    /// # Errors
    ///
    /// Returns a [`StampError::Io`] if directory traversal fails.
    pub fn scan_directory_tree_with_policy(
        root: &Path,
        policy: SymlinkPolicy,
    ) -> Result<Vec<FileMetadataInfo>, StampError> {
        let mut results = Vec::new();
        let mut visited = HashSet::new();
        Self::scan_recursive(root, root, policy, &mut visited, &mut results)?;
        Ok(results)
    }

    /// Recursive directory scanning helper with cycle detection.
    fn scan_recursive(
        base_root: &Path,
        current: &Path,
        policy: SymlinkPolicy,
        visited: &mut HashSet<PathBuf>,
        results: &mut Vec<FileMetadataInfo>,
    ) -> Result<(), StampError> {
        if let Ok(canonical) = current.canonicalize()
            && !visited.insert(canonical)
        {
            // Cycle detected through symlink or hard link; safely stop recursing
            return Ok(());
        }

        let entries = match fs::read_dir(current) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let symlink_meta = fs::symlink_metadata(&path).map_err(StampError::Io)?;
            let is_symlink = symlink_meta.file_type().is_symlink();
            let is_dir = if is_symlink {
                path.is_dir()
            } else {
                symlink_meta.is_dir()
            };

            let relative_path = path.strip_prefix(base_root).unwrap_or(&path).to_path_buf();

            #[cfg(unix)]
            let (permissions_mode, uid, gid) = {
                use std::os::unix::fs::MetadataExt;
                use std::os::unix::fs::PermissionsExt;
                (
                    Some(symlink_meta.permissions().mode()),
                    Some(symlink_meta.uid()),
                    Some(symlink_meta.gid()),
                )
            };
            #[cfg(not(unix))]
            let (permissions_mode, uid, gid) = (None, None, None);

            let modified_secs = symlink_meta.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs())
            });

            let symlink_target = if is_symlink {
                fs::read_link(&path).ok()
            } else {
                None
            };

            if is_symlink {
                match policy {
                    SymlinkPolicy::Ignore => continue,
                    SymlinkPolicy::Follow => {
                        if let Ok(target_meta) = fs::metadata(&path) {
                            let is_target_dir = target_meta.is_dir();
                            results.push(FileMetadataInfo {
                                relative_path,
                                is_dir: is_target_dir,
                                is_symlink: false,
                                permissions_mode,
                                uid,
                                gid,
                                modified_secs,
                                symlink_target: None,
                            });
                            if is_target_dir {
                                Self::scan_recursive(base_root, &path, policy, visited, results)?;
                            }
                            continue;
                        }
                    }
                    SymlinkPolicy::Preserve => {
                        results.push(FileMetadataInfo {
                            relative_path,
                            is_dir,
                            is_symlink: true,
                            permissions_mode,
                            uid,
                            gid,
                            modified_secs,
                            symlink_target,
                        });
                        continue;
                    }
                }
            }

            results.push(FileMetadataInfo {
                relative_path,
                is_dir,
                is_symlink,
                permissions_mode,
                uid,
                gid,
                modified_secs,
                symlink_target,
            });

            if is_dir && !is_symlink {
                Self::scan_recursive(base_root, &path, policy, visited, results)?;
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl Provisioner for FileProvisioner {
    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        match self.config.direction {
            FileDirection::Upload => {
                let local_path = &self.config.source.0;
                let normalized_dst_str = Self::normalize_path_separators(
                    &self.config.destination.0.to_string_lossy(),
                    self.config.target_windows,
                );
                let normalized_dst = FilePath::new(PathBuf::from(normalized_dst_str));

                if local_path.is_dir() {
                    ui.say(
                        "file",
                        &format!(
                            "Uploading directory: {} -> {}",
                            local_path.display(),
                            normalized_dst.0.display()
                        ),
                    );
                    let scanned_items = Self::scan_directory_tree_with_policy(
                        local_path,
                        self.config.symlink_policy,
                    )?;
                    for item in scanned_items {
                        let local_file = local_path.join(&item.relative_path);
                        let remote_rel_str = Self::normalize_path_separators(
                            &item.relative_path.to_string_lossy(),
                            self.config.target_windows,
                        );
                        let remote_file = normalized_dst.0.join(remote_rel_str);
                        if item.is_dir {
                            // Directory creation handled by communicator upload parent creation
                        } else {
                            comm.upload(&FilePath::new(local_file), &FilePath::new(remote_file))
                                .await?;
                        }
                    }
                } else {
                    comm.upload(&self.config.source, &normalized_dst).await?;
                }
            }
            FileDirection::Download => {
                ui.say(
                    "file",
                    &format!(
                        "Downloading: {} -> {}",
                        self.config.source.0.display(),
                        self.config.destination.0.display()
                    ),
                );
                comm.download(&self.config.source, &self.config.destination)
                    .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_file_provision_upload() -> Result<(), StampError> {
        let config = FileConfig {
            source: FilePath::new(PathBuf::from("/tmp/src")),
            destination: FilePath::new(PathBuf::from("/tmp/dst")),
            direction: FileDirection::Upload,
            symlink_policy: SymlinkPolicy::Preserve,
            target_windows: false,
        };
        let prov = FileProvisioner::new(config);
        let comm = MockCommunicator::new();
        prov.provision(
            &comm,
            std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_file_provision_upload_dir() -> Result<(), StampError> {
        let tmp_dir =
            std::env::temp_dir().join(format!("stamp_file_test_{}", uuid::Uuid::new_v4()));
        let sub_dir = tmp_dir.join("subdir");
        fs::create_dir_all(&sub_dir).map_err(StampError::Io)?;
        let test_file = sub_dir.join("file.txt");
        fs::write(&test_file, "hello world").map_err(StampError::Io)?;

        let config = FileConfig {
            source: FilePath::new(tmp_dir.clone()),
            destination: FilePath::new(PathBuf::from("/remote/dest")),
            direction: FileDirection::Upload,
            symlink_policy: SymlinkPolicy::Follow,
            target_windows: true,
        };
        let prov = FileProvisioner::new(config);
        let comm = MockCommunicator::new();
        prov.provision(
            &comm,
            std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
        )
        .await?;

        let _ = fs::remove_dir_all(tmp_dir);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_provision_download() -> Result<(), StampError> {
        let config = FileConfig {
            source: FilePath::new(PathBuf::from("/tmp/remote")),
            destination: FilePath::new(PathBuf::from("/tmp/local")),
            direction: FileDirection::Download,
            ..Default::default()
        };
        let prov = FileProvisioner::new(config);
        let comm = MockCommunicator::new();
        prov.provision(
            &comm,
            std::sync::Arc::new(crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )),
        )
        .await?;
        Ok(())
    }

    #[test]
    fn test_scan_directory_symlink_policies() -> Result<(), StampError> {
        let tmp_dir =
            std::env::temp_dir().join(format!("stamp_symlink_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp_dir).map_err(StampError::Io)?;
        let file1 = tmp_dir.join("test.txt");
        fs::write(&file1, "sample").map_err(StampError::Io)?;

        #[cfg(unix)]
        {
            let link = tmp_dir.join("cycle_link");
            let _ = std::os::unix::fs::symlink(&tmp_dir, &link);
            let file_link = tmp_dir.join("file_link");
            let _ = std::os::unix::fs::symlink(&file1, &file_link);
        }

        let scanned_default = FileProvisioner::scan_directory_tree(&tmp_dir)?;
        assert!(!scanned_default.is_empty());

        let scanned_ignored =
            FileProvisioner::scan_directory_tree_with_policy(&tmp_dir, SymlinkPolicy::Ignore)?;
        assert!(!scanned_ignored.is_empty());

        let scanned_followed =
            FileProvisioner::scan_directory_tree_with_policy(&tmp_dir, SymlinkPolicy::Follow)?;
        assert!(!scanned_followed.is_empty());

        let meta = &scanned_default[0];
        FileProvisioner::apply_local_metadata(&file1, meta)?;

        let _ = fs::remove_dir_all(tmp_dir);
        Ok(())
    }

    #[test]
    fn test_path_separator_normalizer() {
        assert_eq!(
            FileProvisioner::normalize_path_separators("a/b/c", true),
            r#"a\b\c"#
        );
        assert_eq!(
            FileProvisioner::normalize_path_separators(r#"a\b\c"#, false),
            "a/b/c"
        );
    }

    #[test]
    fn test_derived_traits() {
        let config1 = FileConfig {
            source: FilePath::new(PathBuf::from("/a")),
            destination: FilePath::new(PathBuf::from("/b")),
            direction: FileDirection::Upload,
            symlink_policy: SymlinkPolicy::Preserve,
            target_windows: false,
        };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = FileProvisioner::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));

        let dir1 = FileDirection::Upload;
        let dir2 = FileDirection::default();
        assert_eq!(dir1, dir2);

        let pol1 = SymlinkPolicy::Preserve;
        let pol2 = SymlinkPolicy::default();
        assert_eq!(pol1, pol2);
    }
}
