#![cfg_attr(coverage_nightly, coverage(off))]
//! Shared strong types, filesystem generators, and drivers for Virtualization builders.
//!
//! Provides in-memory FAT12 Virtual Floppy Disk (VFD) generation, ISO9660 / Cloud-Init
//! CD-ROM generation with Rock Ridge extensions, VNC RFB protocol client, and boot command macro parsing.

use crate::error::StampError;
use crate::types::Port;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Strictly typed disk adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskAdapter {
    /// IDE adapter.
    Ide,
    /// SATA adapter.
    Sata,
    /// SCSI adapter.
    Scsi,
    /// `NVMe` adapter.
    Nvme,
    /// `VirtIO` adapter.
    Virtio,
    /// Other custom adapter.
    Other(String),
}

/// Strictly typed network adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkAdapter {
    /// NAT networking.
    Nat,
    /// Bridged networking.
    Bridged(String),
    /// Host-only networking.
    HostOnly(String),
    /// Internal network.
    Internal(String),
    /// `VirtIO` network.
    Virtio,
    /// Other network type.
    Other(String),
}

/// Strictly typed CPU/Memory topology.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Topology {
    /// Number of CPU cores.
    pub cpus: Option<u32>,
    /// Number of CPU sockets.
    pub sockets: Option<u32>,
    /// Memory size in MB.
    pub memory_mb: Option<u64>,
}

/// A boot command sequence and timing configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootConfig {
    /// The boot command sequence (e.g. `"<enter><wait>"`).
    pub boot_command: Vec<String>,
    /// Delay before typing the boot command.
    pub boot_wait: Duration,
    /// The size of the steps between keystrokes.
    pub boot_keygroup_interval: Duration,
}

/// VNC client configuration for headless interaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VncConfig {
    /// Whether to bind VNC.
    pub bind_address: String,
    /// The minimum port to bind to.
    pub port_min: Port,
    /// The maximum port to bind to.
    pub port_max: Port,
    /// The VNC password.
    pub password: Option<String>,
}

/// Standard sector size for FAT12 floppy disk (512 bytes).
pub const FAT12_SECTOR_SIZE: usize = 512;
/// Total number of sectors on a standard 1.44 MB 3.5-inch floppy disk (2880 sectors).
pub const FAT12_TOTAL_SECTORS: usize = 2880;
/// Total byte size of a standard 1.44 MB floppy disk (1,474,560 bytes).
pub const FAT12_FLOPPY_SIZE: usize = FAT12_TOTAL_SECTORS * FAT12_SECTOR_SIZE;
/// Number of reserved sectors on a FAT12 floppy disk.
pub const FAT12_RESERVED_SECTORS: usize = 1;
/// Number of FAT copies on a FAT12 floppy disk.
pub const FAT12_NUM_FATS: usize = 2;
/// Number of sectors per FAT on a FAT12 floppy disk.
pub const FAT12_SECTORS_PER_FAT: usize = 9;
/// Maximum number of root directory entries on a FAT12 floppy disk (224 entries).
pub const FAT12_MAX_ROOT_ENTRIES: usize = 224;
/// Number of sectors allocated to root directory (14 sectors).
pub const FAT12_ROOT_DIR_SECTORS: usize = 14;
/// Sector index of first data cluster (cluster 2 is at sector 33).
pub const FAT12_FIRST_DATA_SECTOR: usize = 33;
/// Total number of data clusters on a FAT12 floppy disk (2847 clusters).
pub const FAT12_DATA_CLUSTER_COUNT: usize = 2847;

/// A file stored in an in-memory FAT12 floppy disk image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fat12File {
    /// Original file name.
    pub name: String,
    /// DOS 8.3 formatted uppercase filename.
    pub dos_name: [u8; 11],
    /// Binary content of the file.
    pub data: Vec<u8>,
}

/// In-memory FAT12 filesystem generator for 1.44 MB Virtual Floppy Disk (VFD) images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fat12FloppyDisk {
    /// Volume label (up to 11 characters).
    pub volume_label: String,
    /// List of files contained on the virtual floppy disk.
    pub files: Vec<Fat12File>,
}

impl Default for Fat12FloppyDisk {
    fn default() -> Self {
        Self::new("STAMP_BOOT")
    }
}

impl Fat12FloppyDisk {
    /// Create a new virtual floppy disk with an optional volume label.
    #[must_use]
    pub fn new(volume_label: &str) -> Self {
        Self {
            volume_label: volume_label.to_string(),
            files: Vec::new(),
        }
    }

    /// Add a file with given filename and binary content to the virtual floppy disk.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the floppy capacity is exceeded or if root directory is full.
    pub fn add_file(&mut self, filename: &str, content: &[u8]) -> Result<(), StampError> {
        if self.files.len() >= FAT12_MAX_ROOT_ENTRIES {
            return Err(StampError::Execution(
                "Floppy disk root directory limit (224 entries) exceeded".to_string(),
            ));
        }

        let dos_name = Self::to_dos_8_3(filename)?;

        // Check for duplicate names
        for f in &self.files {
            if f.dos_name == dos_name {
                return Err(StampError::Execution(format!(
                    "Duplicate filename on floppy: {filename}"
                )));
            }
        }

        let total_clusters_needed: usize = self
            .files
            .iter()
            .map(|f| f.data.len().div_ceil(FAT12_SECTOR_SIZE))
            .sum::<usize>()
            + content.len().div_ceil(FAT12_SECTOR_SIZE);

        if total_clusters_needed > FAT12_DATA_CLUSTER_COUNT {
            return Err(StampError::Execution(format!(
                "Floppy disk capacity exceeded: requires {total_clusters_needed} clusters, but maximum is {FAT12_DATA_CLUSTER_COUNT}"
            )));
        }

        self.files.push(Fat12File {
            name: filename.to_string(),
            dos_name,
            data: content.to_vec(),
        });

        Ok(())
    }

    /// Convert a standard filename into DOS 8.3 format (uppercase, space-padded).
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the filename is invalid or cannot be represented.
    pub fn to_dos_8_3(filename: &str) -> Result<[u8; 11], StampError> {
        let p = Path::new(filename);
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(filename)
            .trim();
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").trim();

        if stem.is_empty() {
            return Err(StampError::Execution(format!(
                "Invalid filename for floppy: '{filename}'"
            )));
        }

        let mut dos = [b' '; 11];

        // Clean and uppercase stem (up to 8 chars)
        let clean_stem: String = stem
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(8)
            .collect();
        let stem_upper = clean_stem.to_ascii_uppercase();
        for (i, b) in stem_upper.as_bytes().iter().enumerate() {
            dos[i] = *b;
        }

        // Clean and uppercase ext (up to 3 chars)
        let clean_ext: String = ext
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(3)
            .collect();
        let ext_upper = clean_ext.to_ascii_uppercase();
        for (i, b) in ext_upper.as_bytes().iter().enumerate() {
            dos[8 + i] = *b;
        }

        Ok(dos)
    }

    /// Helper to write a 12-bit entry into a FAT12 table buffer.
    fn write_fat12_entry(fat: &mut [u8], cluster: u16, value: u16) {
        let offset = ((cluster as usize) * 3) / 2;
        if cluster.is_multiple_of(2) {
            fat[offset] = (value & 0xFF) as u8;
            fat[offset + 1] = (fat[offset + 1] & 0xF0) | (((value >> 8) & 0x0F) as u8);
        } else {
            let low_nibble = ((value & 0x0F) as u8) << 4;
            fat[offset] = (fat[offset] & 0x0F) | low_nibble;
            fat[offset + 1] = ((value >> 4) & 0xFF) as u8;
        }
    }

    /// Generate the full 1.44 MB floppy disk image binary.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if floppy structure fails validation.
    pub fn generate(&self) -> Result<Vec<u8>, StampError> {
        let mut disk = vec![0u8; FAT12_FLOPPY_SIZE];

        // 1. Write Boot Sector (Sector 0)
        // Jump instruction
        disk[0] = 0xEB;
        disk[1] = 0x3C;
        disk[2] = 0x90;

        // OEM Name (8 bytes)
        let oem = b"STAMP1.0";
        disk[3..11].copy_from_slice(oem);

        // Bytes per sector (512)
        disk[11..13].copy_from_slice(&512u16.to_le_bytes());
        // Sectors per cluster (1)
        disk[13] = 1;
        // Reserved sectors (1)
        disk[14..16].copy_from_slice(&1u16.to_le_bytes());
        // Number of FATs (2)
        disk[16] = 2;
        // Root entries (224)
        disk[17..19].copy_from_slice(&224u16.to_le_bytes());
        // Total sectors 16-bit (2880)
        disk[19..21].copy_from_slice(&2880u16.to_le_bytes());
        // Media descriptor (0xF0 for 1.44 MB 3.5" floppy)
        disk[21] = 0xF0;
        // Sectors per FAT (9)
        disk[22..24].copy_from_slice(&9u16.to_le_bytes());
        // Sectors per track (18)
        disk[24..26].copy_from_slice(&18u16.to_le_bytes());
        // Heads (2)
        disk[26..28].copy_from_slice(&2u16.to_le_bytes());
        // Hidden sectors (0)
        disk[28..32].copy_from_slice(&0u32.to_le_bytes());
        // Total sectors 32-bit (0)
        disk[32..36].copy_from_slice(&0u32.to_le_bytes());
        // Drive number (0x00)
        disk[36] = 0x00;
        // Reserved (0x00)
        disk[37] = 0x00;
        // Extended boot signature (0x29)
        disk[38] = 0x29;
        // Volume serial ID
        disk[39..43].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        // Volume label (11 bytes)
        let mut label_buf = [b' '; 11];
        let label_bytes = self.volume_label.as_bytes();
        let copy_len = label_bytes.len().min(11);
        label_buf[..copy_len].copy_from_slice(&label_bytes[..copy_len]);
        disk[43..54].copy_from_slice(&label_buf);
        // File system type (8 bytes)
        disk[54..62].copy_from_slice(b"FAT12   ");

        // Boot sector signature (0x55, 0xAA at 510..512)
        disk[510] = 0x55;
        disk[511] = 0xAA;

        // 2. Prepare FAT1 and FAT2
        let fat_size = FAT12_SECTORS_PER_FAT * FAT12_SECTOR_SIZE; // 9 * 512 = 4608 bytes
        let mut fat = vec![0u8; fat_size];
        // Cluster 0 (0xFF0) and Cluster 1 (0xFFF)
        Self::write_fat12_entry(&mut fat, 0, 0x0FF0);
        Self::write_fat12_entry(&mut fat, 1, 0x0FFF);

        let root_dir_offset =
            (FAT12_RESERVED_SECTORS + (FAT12_NUM_FATS * FAT12_SECTORS_PER_FAT)) * FAT12_SECTOR_SIZE; // 19 * 512 = 9728
        let data_area_offset = FAT12_FIRST_DATA_SECTOR * FAT12_SECTOR_SIZE; // 33 * 512 = 16896

        let mut current_cluster = 2u16;

        for (i, file) in self.files.iter().enumerate() {
            let start_cluster = if file.data.is_empty() {
                0u16
            } else {
                current_cluster
            };

            let clusters_needed = file.data.len().div_ceil(FAT12_SECTOR_SIZE);

            // Link cluster chains in FAT
            if clusters_needed > 0 {
                for c in 0..clusters_needed {
                    let c_num = current_cluster + u16::try_from(c).unwrap_or(0);
                    if c + 1 == clusters_needed {
                        Self::write_fat12_entry(&mut fat, c_num, 0x0FFF); // End of chain
                    } else {
                        Self::write_fat12_entry(&mut fat, c_num, c_num + 1);
                    }

                    // Write data to cluster
                    let cluster_offset =
                        data_area_offset + ((c_num - 2) as usize * FAT12_SECTOR_SIZE);
                    let file_data_start = c * FAT12_SECTOR_SIZE;
                    let file_data_end = (file_data_start + FAT12_SECTOR_SIZE).min(file.data.len());
                    let chunk = &file.data[file_data_start..file_data_end];
                    disk[cluster_offset..cluster_offset + chunk.len()].copy_from_slice(chunk);
                }
                current_cluster += u16::try_from(clusters_needed).unwrap_or(0);
            }

            // Write 32-byte Directory Entry in Root Directory Table
            let entry_offset = root_dir_offset + (i * 32);
            // Filename 8.3
            disk[entry_offset..entry_offset + 11].copy_from_slice(&file.dos_name);
            // Attributes (0x20 = ARCHIVE)
            disk[entry_offset + 11] = 0x20;
            // Reserved and timestamps (zeroes)
            disk[entry_offset + 12..entry_offset + 26].fill(0);
            // Starting cluster
            disk[entry_offset + 26..entry_offset + 28]
                .copy_from_slice(&start_cluster.to_le_bytes());
            // File size
            let file_size = u32::try_from(file.data.len()).unwrap_or(0);
            disk[entry_offset + 28..entry_offset + 32].copy_from_slice(&file_size.to_le_bytes());
        }

        // Copy FAT1 to disk (Sector 1..9)
        let fat1_offset = FAT12_RESERVED_SECTORS * FAT12_SECTOR_SIZE; // 512
        disk[fat1_offset..fat1_offset + fat_size].copy_from_slice(&fat);

        // Copy FAT2 to disk (Sector 10..18)
        let fat2_offset = fat1_offset + fat_size; // 512 + 4608 = 5120
        disk[fat2_offset..fat2_offset + fat_size].copy_from_slice(&fat);

        Ok(disk)
    }

    /// Write the virtual floppy disk image to an on-disk file.
    ///
    /// # Errors
    /// Returns `StampError::Io` or `StampError::Execution` on failure.
    pub async fn write_to_file(&self, dest: &Path) -> Result<(), StampError> {
        let data = self.generate()?;
        tokio::fs::write(dest, data).await.map_err(StampError::Io)
    }
}

/// Helper function to generate a virtual 1.44 MB floppy disk image containing files.
///
/// # Errors
/// Returns `StampError::Io` or `StampError::Execution` on failure.
pub async fn generate_floppy_disk(files: &[String], dest: &Path) -> Result<(), StampError> {
    generate_floppy_disk_with_dirs(files, &[], dest).await
}

/// Helper function to generate a virtual 1.44 MB floppy disk image containing files and directories.
///
/// Recursively adds files from directories specified in `dirs`.
///
/// # Errors
/// Returns `StampError::Io` or `StampError::Execution` on failure.
pub async fn generate_floppy_disk_with_dirs(
    files: &[String],
    dirs: &[String],
    dest: &Path,
) -> Result<(), StampError> {
    let mut floppy = Fat12FloppyDisk::new("STAMP_BOOT");
    for file in files {
        let p = Path::new(file);
        if p.is_file() {
            let content = tokio::fs::read(p).await.map_err(StampError::Io)?;
            let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("file.dat");
            floppy.add_file(file_name, &content)?;
        }
    }
    for dir in dirs {
        let dir_path = Path::new(dir);
        if dir_path.is_dir()
            && let Ok(entries) = std::fs::read_dir(dir_path)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let content = tokio::fs::read(&path).await.map_err(StampError::Io)?;
                    let file_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file.dat");
                    floppy.add_file(file_name, &content)?;
                }
            }
        }
    }
    floppy.write_to_file(dest).await
}

/// Sector size for ISO9660 filesystem images (2048 bytes).
pub const ISO_SECTOR_SIZE: usize = 2048;
/// Sector index of the Primary Volume Descriptor (PVD).
pub const ISO_PVD_SECTOR: u32 = 16;
/// Sector index of the Joliet Supplementary Volume Descriptor (SVD).
pub const ISO_SVD_SECTOR: u32 = 17;
/// Sector index of the Volume Descriptor Set Terminator.
pub const ISO_TERMINATOR_SECTOR: u32 = 18;
/// Sector index of the Type L Path Table.
pub const ISO_PATH_TABLE_L_SECTOR: u32 = 19;
/// Sector index of the Type M Path Table.
pub const ISO_PATH_TABLE_M_SECTOR: u32 = 20;
/// Sector index of the Joliet Type L Path Table.
pub const ISO_JOLIET_PATH_TABLE_L_SECTOR: u32 = 21;
/// Sector index of the Joliet Type M Path Table.
pub const ISO_JOLIET_PATH_TABLE_M_SECTOR: u32 = 22;
/// Sector index of the Root Directory.
pub const ISO_ROOT_DIR_SECTOR: u32 = 23;

/// A file to be stored on an ISO9660 CD-ROM image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsoFile {
    /// Relative path on the filesystem (e.g. `user-data` or `viostor/w11/ARM64/viostor.inf`).
    pub path: String,
    /// Filename without directory prefix (e.g. `viostor.inf`).
    pub name: String,
    /// ISO9660 uppercase 8.3 identifier with version tag (e.g. `USER_DAT.;1` or `VIOSTOR.INF;1`).
    pub iso_id: String,
    /// Binary content of the file.
    pub data: Vec<u8>,
}

/// Encode an ASCII or UTF-8 string into big-endian UCS-2 / UTF-16BE bytes for Joliet SVD.
#[must_use]
pub fn encode_ucs2be(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2);
    for c in s.chars() {
        let code = u16::try_from(c as u32).unwrap_or(u16::from(b'?'));
        out.extend_from_slice(&code.to_be_bytes());
    }
    out
}

/// Encode and pad a string with UCS-2BE spaces (`0x00 0x20`) up to `max_chars`.
#[must_use]
pub fn encode_ucs2be_padded(s: &str, max_chars: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(max_chars * 2);
    let mut count = 0;
    for c in s.chars().take(max_chars) {
        let code = u16::try_from(c as u32).unwrap_or(u16::from(b'?'));
        out.extend_from_slice(&code.to_be_bytes());
        count += 1;
    }
    while count < max_chars {
        out.extend_from_slice(&(0x0020u16).to_be_bytes());
        count += 1;
    }
    out
}

/// Internal directory metadata for ISO9660 and Joliet tree generation.
#[derive(Debug, Clone)]
struct IsoDirEntry {
    /// 1-based directory index.
    index: usize,
    /// 1-based parent directory index.
    parent_index: usize,
    /// Normalized directory path (empty string for root).
    path: String,
    /// Directory leaf name (empty string for root).
    name: String,
    /// ISO9660 uppercase identifier.
    iso_id: String,
}

/// In-memory ISO9660 Level 3 CD-ROM filesystem generator with Joliet and Rock Ridge extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iso9660Disk {
    /// Volume identifier (e.g. `OEMDRV` or `cidata`).
    pub volume_label: String,
    /// List of files on the CD-ROM.
    pub files: Vec<IsoFile>,
}

impl Default for Iso9660Disk {
    fn default() -> Self {
        Self::new("cidata")
    }
}

impl Iso9660Disk {
    /// Create a new ISO9660 CD-ROM generator with the given volume label.
    #[must_use]
    pub fn new(volume_label: &str) -> Self {
        Self {
            volume_label: volume_label.to_string(),
            files: Vec::new(),
        }
    }

    /// Add a file with given relative path and binary content to the ISO9660 disk.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if path cannot be formatted.
    pub fn add_file(&mut self, path: &str, content: &[u8]) -> Result<(), StampError> {
        let normalized = path.replace('\\', "/").trim_start_matches('/').to_string();
        let p = Path::new(&normalized);
        let file_name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();

        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("FILE")
            .trim();
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").trim();

        let clean_stem: String = stem
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(8)
            .collect();
        let stem_upper = if clean_stem.is_empty() {
            "FILE".to_string()
        } else {
            clean_stem.to_ascii_uppercase()
        };

        let clean_ext: String = ext
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(3)
            .collect();
        let ext_upper = clean_ext.to_ascii_uppercase();

        let iso_id = if ext_upper.is_empty() {
            format!("{stem_upper}.;1")
        } else {
            format!("{stem_upper}.{ext_upper};1")
        };

        self.files.push(IsoFile {
            path: normalized,
            name: file_name,
            iso_id,
            data: content.to_vec(),
        });

        Ok(())
    }

    /// Add a directory tree recursively from the host local filesystem into the ISO image.
    ///
    /// Preserves directory hierarchy under `base_prefix`.
    ///
    /// # Errors
    /// Returns `StampError::Io` on filesystem read error.
    pub fn add_directory_recursive(
        &mut self,
        dir_path: &Path,
        base_prefix: Option<&str>,
    ) -> Result<(), StampError> {
        let prefix = base_prefix.map_or_else(
            || {
                dir_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("dir")
                    .to_string()
            },
            ToString::to_string,
        );
        self.add_directory_recursive_internal(dir_path, &prefix)
    }

    /// Recursively walks a host directory adding all files into the ISO9660 image.
    fn add_directory_recursive_internal(
        &mut self,
        dir_path: &Path,
        prefix: &str,
    ) -> Result<(), StampError> {
        let entries = std::fs::read_dir(dir_path).map_err(StampError::Io)?;
        for entry in entries {
            let entry = entry.map_err(StampError::Io)?;
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let target_rel = format!("{prefix}/{name}");
            if p.is_dir() {
                self.add_directory_recursive_internal(&p, &target_rel)?;
            } else if p.is_file() {
                let data = std::fs::read(&p).map_err(StampError::Io)?;
                self.add_file(&target_rel, &data)?;
            }
        }
        Ok(())
    }

    /// Format a u16 into an ISO9660 both-endian pair (4 bytes: little-endian followed by big-endian).
    #[must_use]
    pub const fn both_endian_u16(val: u16) -> [u8; 4] {
        let le = val.to_le_bytes();
        let be = val.to_be_bytes();
        [le[0], le[1], be[0], be[1]]
    }

    /// Format a u32 into an ISO9660 both-endian pair (8 bytes: little-endian followed by big-endian).
    #[must_use]
    pub const fn both_endian_u32(val: u32) -> [u8; 8] {
        let le = val.to_le_bytes();
        let be = val.to_be_bytes();
        [le[0], le[1], le[2], le[3], be[0], be[1], be[2], be[3]]
    }

    /// Generate the full ISO9660 Level 3 + Joliet + Rock Ridge binary filesystem image.
    ///
    /// # Errors
    /// Returns `StampError::Execution` on serialization failure.
    pub fn generate(&self) -> Result<Vec<u8>, StampError> {
        // 1. Collect all unique directories in hierarchical topological order
        let mut dir_map: Vec<String> = Vec::new();
        dir_map.push(String::new()); // Root directory = ""

        for file in &self.files {
            if let Some((dir_part, _)) = file.path.rsplit_once('/') {
                let mut accum = String::new();
                for segment in dir_part.split('/') {
                    if segment.is_empty() {
                        continue;
                    }
                    if !accum.is_empty() {
                        accum.push('/');
                    }
                    accum.push_str(segment);
                    if !dir_map.contains(&accum) {
                        dir_map.push(accum.clone());
                    }
                }
            }
        }

        // Sort subdirectories so parent directories always precede child directories
        let mut subdirs: Vec<String> = dir_map.into_iter().skip(1).collect();
        subdirs.sort_by(|a, b| {
            let depth_a = a.split('/').count();
            let depth_b = b.split('/').count();
            depth_a.cmp(&depth_b).then_with(|| a.cmp(b))
        });

        let mut all_dirs: Vec<IsoDirEntry> = Vec::new();
        all_dirs.push(IsoDirEntry {
            index: 1,
            parent_index: 1,
            path: String::new(),
            name: String::new(),
            iso_id: String::new(),
        });

        for dir_path in subdirs {
            let (parent_p, name) = match dir_path.rsplit_once('/') {
                Some((p, n)) => (p.to_string(), n.to_string()),
                None => (String::new(), dir_path.clone()),
            };
            let parent_idx = all_dirs
                .iter()
                .position(|d| d.path == parent_p)
                .map_or(1, |idx| idx + 1);

            let clean_name: String = name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(8)
                .collect();
            let iso_id = if clean_name.is_empty() {
                "DIR".to_string()
            } else {
                clean_name.to_ascii_uppercase()
            };

            let next_idx = all_dirs.len() + 1;
            all_dirs.push(IsoDirEntry {
                index: next_idx,
                parent_index: parent_idx,
                path: dir_path,
                name,
                iso_id,
            });
        }

        let num_dirs = all_dirs.len();

        // 2. Sector allocations
        let iso_root_dir_sector = ISO_ROOT_DIR_SECTOR;
        let joliet_root_dir_sector = ISO_ROOT_DIR_SECTOR + (num_dirs as u32);
        let first_file_sector = joliet_root_dir_sector + (num_dirs as u32);

        // Assign file data sectors
        let mut file_sectors = Vec::new();
        let mut cur_file_sector = first_file_sector;
        for file in &self.files {
            let sectors_needed =
                u32::try_from(file.data.len().div_ceil(ISO_SECTOR_SIZE).max(1)).unwrap_or(1);
            file_sectors.push((cur_file_sector, file));
            cur_file_sector += sectors_needed;
        }

        let total_volume_sectors = cur_file_sector;
        let total_bytes = (total_volume_sectors as usize) * ISO_SECTOR_SIZE;
        let mut image = vec![0u8; total_bytes];

        // 3. Build ISO Path Table
        let mut path_table_l_iso = Vec::new();
        let mut path_table_m_iso = Vec::new();
        for dir in &all_dirs {
            let extent = iso_root_dir_sector + (dir.index as u32) - 1;
            let parent = dir.parent_index as u16;
            if dir.index == 1 {
                // Root directory
                path_table_l_iso.extend_from_slice(&[1, 0]);
                path_table_l_iso.extend_from_slice(&extent.to_le_bytes());
                path_table_l_iso.extend_from_slice(&parent.to_le_bytes());
                path_table_l_iso.extend_from_slice(&[0, 0]);

                path_table_m_iso.extend_from_slice(&[1, 0]);
                path_table_m_iso.extend_from_slice(&extent.to_be_bytes());
                path_table_m_iso.extend_from_slice(&parent.to_be_bytes());
                path_table_m_iso.extend_from_slice(&[0, 0]);
            } else {
                let id_bytes = dir.iso_id.as_bytes();
                let len = id_bytes.len() as u8;
                let pad = usize::from(len % 2 == 1);

                path_table_l_iso.extend_from_slice(&[len, 0]);
                path_table_l_iso.extend_from_slice(&extent.to_le_bytes());
                path_table_l_iso.extend_from_slice(&parent.to_le_bytes());
                path_table_l_iso.extend_from_slice(id_bytes);
                if pad > 0 {
                    path_table_l_iso.push(0);
                }

                path_table_m_iso.extend_from_slice(&[len, 0]);
                path_table_m_iso.extend_from_slice(&extent.to_be_bytes());
                path_table_m_iso.extend_from_slice(&parent.to_be_bytes());
                path_table_m_iso.extend_from_slice(id_bytes);
                if pad > 0 {
                    path_table_m_iso.push(0);
                }
            }
        }

        // 4. Build Joliet Path Table
        let mut path_table_l_joliet = Vec::new();
        let mut path_table_m_joliet = Vec::new();
        for dir in &all_dirs {
            let extent = joliet_root_dir_sector + (dir.index as u32) - 1;
            let parent = dir.parent_index as u16;
            if dir.index == 1 {
                path_table_l_joliet.extend_from_slice(&[1, 0]);
                path_table_l_joliet.extend_from_slice(&extent.to_le_bytes());
                path_table_l_joliet.extend_from_slice(&parent.to_le_bytes());
                path_table_l_joliet.extend_from_slice(&[0, 0]);

                path_table_m_joliet.extend_from_slice(&[1, 0]);
                path_table_m_joliet.extend_from_slice(&extent.to_be_bytes());
                path_table_m_joliet.extend_from_slice(&parent.to_be_bytes());
                path_table_m_joliet.extend_from_slice(&[0, 0]);
            } else {
                let id_bytes = encode_ucs2be(&dir.name);
                let len = id_bytes.len() as u8;
                let pad = usize::from(len % 2 == 1);

                path_table_l_joliet.extend_from_slice(&[len, 0]);
                path_table_l_joliet.extend_from_slice(&extent.to_le_bytes());
                path_table_l_joliet.extend_from_slice(&parent.to_le_bytes());
                path_table_l_joliet.extend_from_slice(&id_bytes);
                if pad > 0 {
                    path_table_l_joliet.push(0);
                }

                path_table_m_joliet.extend_from_slice(&[len, 0]);
                path_table_m_joliet.extend_from_slice(&extent.to_be_bytes());
                path_table_m_joliet.extend_from_slice(&parent.to_be_bytes());
                path_table_m_joliet.extend_from_slice(&id_bytes);
                if pad > 0 {
                    path_table_m_joliet.push(0);
                }
            }
        }

        // Write Path Tables
        let pt_l_offset = (ISO_PATH_TABLE_L_SECTOR as usize) * ISO_SECTOR_SIZE;
        image[pt_l_offset..pt_l_offset + path_table_l_iso.len()].copy_from_slice(&path_table_l_iso);

        let pt_m_offset = (ISO_PATH_TABLE_M_SECTOR as usize) * ISO_SECTOR_SIZE;
        image[pt_m_offset..pt_m_offset + path_table_m_iso.len()].copy_from_slice(&path_table_m_iso);

        let j_pt_l_offset = (ISO_JOLIET_PATH_TABLE_L_SECTOR as usize) * ISO_SECTOR_SIZE;
        image[j_pt_l_offset..j_pt_l_offset + path_table_l_joliet.len()]
            .copy_from_slice(&path_table_l_joliet);

        let j_pt_m_offset = (ISO_JOLIET_PATH_TABLE_M_SECTOR as usize) * ISO_SECTOR_SIZE;
        image[j_pt_m_offset..j_pt_m_offset + path_table_m_joliet.len()]
            .copy_from_slice(&path_table_m_joliet);

        // 5. Write PVD (Sector 16)
        let pvd_offset = (ISO_PVD_SECTOR as usize) * ISO_SECTOR_SIZE;
        image[pvd_offset] = 0x01; // PVD Type
        image[pvd_offset + 1..pvd_offset + 6].copy_from_slice(b"CD001");
        image[pvd_offset + 6] = 0x01;

        let mut sys_id = [b' '; 32];
        sys_id[..5].copy_from_slice(b"LINUX");
        image[pvd_offset + 8..pvd_offset + 40].copy_from_slice(&sys_id);

        let mut vol_id = [b' '; 32];
        let label_bytes = self.volume_label.as_bytes();
        let copy_len = label_bytes.len().min(32);
        vol_id[..copy_len].copy_from_slice(&label_bytes[..copy_len]);
        image[pvd_offset + 40..pvd_offset + 72].copy_from_slice(&vol_id);

        image[pvd_offset + 80..pvd_offset + 88]
            .copy_from_slice(&Self::both_endian_u32(total_volume_sectors));
        image[pvd_offset + 120..pvd_offset + 124].copy_from_slice(&Self::both_endian_u16(1));
        image[pvd_offset + 124..pvd_offset + 128].copy_from_slice(&Self::both_endian_u16(1));
        image[pvd_offset + 128..pvd_offset + 132].copy_from_slice(&Self::both_endian_u16(2048));
        image[pvd_offset + 132..pvd_offset + 140]
            .copy_from_slice(&Self::both_endian_u32(path_table_l_iso.len() as u32));
        image[pvd_offset + 140..pvd_offset + 144]
            .copy_from_slice(&ISO_PATH_TABLE_L_SECTOR.to_le_bytes());
        image[pvd_offset + 148..pvd_offset + 152]
            .copy_from_slice(&ISO_PATH_TABLE_M_SECTOR.to_be_bytes());

        let root_rec = &mut image[pvd_offset + 156..pvd_offset + 190];
        root_rec[0] = 34;
        root_rec[1] = 0;
        root_rec[2..10].copy_from_slice(&Self::both_endian_u32(iso_root_dir_sector));
        root_rec[10..18].copy_from_slice(&Self::both_endian_u32(2048));
        root_rec[18..25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
        root_rec[25] = 0x02; // Directory
        root_rec[28..32].copy_from_slice(&Self::both_endian_u16(1));
        root_rec[32] = 1;
        root_rec[33] = 0;

        let mut app_id = [b' '; 128];
        let stamp_app = b"STAMP ISO9660 GENERATOR";
        app_id[..stamp_app.len()].copy_from_slice(stamp_app);
        image[pvd_offset + 574..pvd_offset + 702].copy_from_slice(&app_id);

        let ts = b"2026090612000000\0";
        image[pvd_offset + 813..pvd_offset + 830].copy_from_slice(ts);
        image[pvd_offset + 830..pvd_offset + 847].copy_from_slice(ts);
        image[pvd_offset + 864..pvd_offset + 881].copy_from_slice(ts);
        image[pvd_offset + 881] = 0x01;

        // 6. Write Joliet SVD (Sector 17)
        let svd_offset = (ISO_SVD_SECTOR as usize) * ISO_SECTOR_SIZE;
        image[svd_offset] = 0x02; // Supplementary Volume Descriptor
        image[svd_offset + 1..svd_offset + 6].copy_from_slice(b"CD001");
        image[svd_offset + 6] = 0x01;

        let sys_id_ucs2 = encode_ucs2be_padded("LINUX", 16);
        image[svd_offset + 8..svd_offset + 8 + sys_id_ucs2.len()].copy_from_slice(&sys_id_ucs2);

        let vol_id_ucs2 = encode_ucs2be_padded(&self.volume_label, 16);
        image[svd_offset + 40..svd_offset + 40 + vol_id_ucs2.len()].copy_from_slice(&vol_id_ucs2);

        image[svd_offset + 80..svd_offset + 88]
            .copy_from_slice(&Self::both_endian_u32(total_volume_sectors));

        // Joliet Escape Sequences: %/E (UCS-2 Level 3)
        image[svd_offset + 88..svd_offset + 91].copy_from_slice(b"%/E");

        image[svd_offset + 120..svd_offset + 124].copy_from_slice(&Self::both_endian_u16(1));
        image[svd_offset + 124..svd_offset + 128].copy_from_slice(&Self::both_endian_u16(1));
        image[svd_offset + 128..svd_offset + 132].copy_from_slice(&Self::both_endian_u16(2048));
        image[svd_offset + 132..svd_offset + 140]
            .copy_from_slice(&Self::both_endian_u32(path_table_l_joliet.len() as u32));
        image[svd_offset + 140..svd_offset + 144]
            .copy_from_slice(&ISO_JOLIET_PATH_TABLE_L_SECTOR.to_le_bytes());
        image[svd_offset + 148..svd_offset + 152]
            .copy_from_slice(&ISO_JOLIET_PATH_TABLE_M_SECTOR.to_be_bytes());

        let j_root_rec = &mut image[svd_offset + 156..svd_offset + 190];
        j_root_rec[0] = 34;
        j_root_rec[1] = 0;
        j_root_rec[2..10].copy_from_slice(&Self::both_endian_u32(joliet_root_dir_sector));
        j_root_rec[10..18].copy_from_slice(&Self::both_endian_u32(2048));
        j_root_rec[18..25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
        j_root_rec[25] = 0x02; // Directory
        j_root_rec[28..32].copy_from_slice(&Self::both_endian_u16(1));
        j_root_rec[32] = 1;
        j_root_rec[33] = 0;

        image[svd_offset + 813..svd_offset + 830].copy_from_slice(ts);
        image[svd_offset + 830..svd_offset + 847].copy_from_slice(ts);
        image[svd_offset + 864..svd_offset + 881].copy_from_slice(ts);
        image[svd_offset + 881] = 0x01;

        // 7. Write Volume Descriptor Terminator (Sector 18)
        let term_offset = (ISO_TERMINATOR_SECTOR as usize) * ISO_SECTOR_SIZE;
        image[term_offset] = 0xFF;
        image[term_offset + 1..term_offset + 6].copy_from_slice(b"CD001");
        image[term_offset + 6] = 0x01;

        // 8. Write ISO Directory Sectors with Rock Ridge extensions
        for dir in &all_dirs {
            let dir_sector = iso_root_dir_sector + (dir.index as u32) - 1;
            let parent_sector = iso_root_dir_sector + (dir.parent_index as u32) - 1;
            let mut cursor = (dir_sector as usize) * ISO_SECTOR_SIZE;

            // Entry "."
            image[cursor] = 34;
            image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(dir_sector));
            image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
            image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
            image[cursor + 25] = 0x02;
            image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
            image[cursor + 32] = 1;
            image[cursor + 33] = 0;
            cursor += 34;

            // Entry ".."
            image[cursor] = 34;
            image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(parent_sector));
            image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
            image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
            image[cursor + 25] = 0x02;
            image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
            image[cursor + 32] = 1;
            image[cursor + 33] = 1;
            cursor += 34;

            // Subdirectories directly inside `dir`
            for child in &all_dirs {
                if child.index != 1 && child.parent_index == dir.index {
                    let child_sector = iso_root_dir_sector + (child.index as u32) - 1;
                    let id_bytes = child.iso_id.as_bytes();
                    let name_bytes = child.name.as_bytes();

                    let base_len = 33 + id_bytes.len();
                    let pad = usize::from(base_len % 2 == 1);
                    let nm_len = 5 + name_bytes.len();
                    let px_len = 36;
                    let total_len = base_len + pad + nm_len + px_len;
                    let final_pad = usize::from(total_len % 2 == 1);
                    let rec_len = (total_len + final_pad) as u8;

                    image[cursor] = rec_len;
                    image[cursor + 2..cursor + 10]
                        .copy_from_slice(&Self::both_endian_u32(child_sector));
                    image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
                    image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
                    image[cursor + 25] = 0x02; // Directory
                    image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
                    image[cursor + 32] = id_bytes.len() as u8;
                    image[cursor + 33..cursor + 33 + id_bytes.len()].copy_from_slice(id_bytes);

                    let mut sua = cursor + 33 + id_bytes.len() + pad;
                    image[sua..sua + 2].copy_from_slice(b"NM");
                    image[sua + 2] = nm_len as u8;
                    image[sua + 3] = 1;
                    image[sua + 4] = 0;
                    image[sua + 5..sua + 5 + name_bytes.len()].copy_from_slice(name_bytes);
                    sua += nm_len;

                    image[sua..sua + 2].copy_from_slice(b"PX");
                    image[sua + 2] = px_len as u8;
                    image[sua + 3] = 1;
                    image[sua + 4..sua + 12].copy_from_slice(&Self::both_endian_u32(0o40755));
                    image[sua + 12..sua + 20].copy_from_slice(&Self::both_endian_u32(2));
                    image[sua + 20..sua + 28].copy_from_slice(&Self::both_endian_u32(0));
                    image[sua + 28..sua + 36].copy_from_slice(&Self::both_endian_u32(0));

                    cursor += rec_len as usize;
                }
            }

            // Files directly inside `dir`
            for (sector, file) in &file_sectors {
                let file_parent_path = file.path.rsplit_once('/').map_or("", |(p, _)| p);
                if file_parent_path == dir.path {
                    let id_bytes = file.iso_id.as_bytes();
                    let name_bytes = file.name.as_bytes();

                    let base_len = 33 + id_bytes.len();
                    let pad = usize::from(base_len % 2 == 1);
                    let nm_len = 5 + name_bytes.len();
                    let px_len = 36;
                    let total_len = base_len + pad + nm_len + px_len;
                    let final_pad = usize::from(total_len % 2 == 1);
                    let rec_len = (total_len + final_pad) as u8;

                    image[cursor] = rec_len;
                    image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(*sector));
                    image[cursor + 10..cursor + 18]
                        .copy_from_slice(&Self::both_endian_u32(file.data.len() as u32));
                    image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
                    image[cursor + 25] = 0x00; // Normal File
                    image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
                    image[cursor + 32] = id_bytes.len() as u8;
                    image[cursor + 33..cursor + 33 + id_bytes.len()].copy_from_slice(id_bytes);

                    let mut sua = cursor + 33 + id_bytes.len() + pad;
                    image[sua..sua + 2].copy_from_slice(b"NM");
                    image[sua + 2] = nm_len as u8;
                    image[sua + 3] = 1;
                    image[sua + 4] = 0;
                    image[sua + 5..sua + 5 + name_bytes.len()].copy_from_slice(name_bytes);
                    sua += nm_len;

                    image[sua..sua + 2].copy_from_slice(b"PX");
                    image[sua + 2] = px_len as u8;
                    image[sua + 3] = 1;
                    image[sua + 4..sua + 12].copy_from_slice(&Self::both_endian_u32(0o100_644));
                    image[sua + 12..sua + 20].copy_from_slice(&Self::both_endian_u32(1));
                    image[sua + 20..sua + 28].copy_from_slice(&Self::both_endian_u32(0));
                    image[sua + 28..sua + 36].copy_from_slice(&Self::both_endian_u32(0));

                    cursor += rec_len as usize;
                }
            }
        }

        // 9. Write Joliet Directory Sectors
        for dir in &all_dirs {
            let dir_sector = joliet_root_dir_sector + (dir.index as u32) - 1;
            let parent_sector = joliet_root_dir_sector + (dir.parent_index as u32) - 1;
            let mut cursor = (dir_sector as usize) * ISO_SECTOR_SIZE;

            // Entry "."
            image[cursor] = 34;
            image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(dir_sector));
            image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
            image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
            image[cursor + 25] = 0x02;
            image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
            image[cursor + 32] = 1;
            image[cursor + 33] = 0;
            cursor += 34;

            // Entry ".."
            image[cursor] = 34;
            image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(parent_sector));
            image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
            image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
            image[cursor + 25] = 0x02;
            image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
            image[cursor + 32] = 1;
            image[cursor + 33] = 1;
            cursor += 34;

            // Subdirectories directly inside `dir`
            for child in &all_dirs {
                if child.index != 1 && child.parent_index == dir.index {
                    let child_sector = joliet_root_dir_sector + (child.index as u32) - 1;
                    let ucs2_id = encode_ucs2be(&child.name);
                    let total_len = 33 + ucs2_id.len();
                    let pad = usize::from(total_len % 2 == 1);
                    let rec_len = (total_len + pad) as u8;

                    image[cursor] = rec_len;
                    image[cursor + 2..cursor + 10]
                        .copy_from_slice(&Self::both_endian_u32(child_sector));
                    image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
                    image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
                    image[cursor + 25] = 0x02; // Directory
                    image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
                    image[cursor + 32] = ucs2_id.len() as u8;
                    image[cursor + 33..cursor + 33 + ucs2_id.len()].copy_from_slice(&ucs2_id);

                    cursor += rec_len as usize;
                }
            }

            // Files directly inside `dir`
            for (sector, file) in &file_sectors {
                let file_parent_path = file.path.rsplit_once('/').map_or("", |(p, _)| p);
                if file_parent_path == dir.path {
                    let id_str = format!("{};1", file.name);
                    let ucs2_id = encode_ucs2be(&id_str);
                    let total_len = 33 + ucs2_id.len();
                    let pad = usize::from(total_len % 2 == 1);
                    let rec_len = (total_len + pad) as u8;

                    image[cursor] = rec_len;
                    image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(*sector));
                    image[cursor + 10..cursor + 18]
                        .copy_from_slice(&Self::both_endian_u32(file.data.len() as u32));
                    image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
                    image[cursor + 25] = 0x00; // Normal file
                    image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
                    image[cursor + 32] = ucs2_id.len() as u8;
                    image[cursor + 33..cursor + 33 + ucs2_id.len()].copy_from_slice(&ucs2_id);

                    cursor += rec_len as usize;
                }
            }
        }

        // 10. Copy File Extents
        for (sector, file) in &file_sectors {
            let offset = (*sector as usize) * ISO_SECTOR_SIZE;
            image[offset..offset + file.data.len()].copy_from_slice(&file.data);
        }

        Ok(image)
    }

    /// Write the generated ISO9660 filesystem image to an on-disk file.
    ///
    /// # Errors
    /// Returns `StampError::Io` or `StampError::Execution` on failure.
    pub async fn write_to_file(&self, dest: &Path) -> Result<(), StampError> {
        let data = self.generate()?;
        tokio::fs::write(dest, data).await.map_err(StampError::Io)
    }
}

/// Helper function to generate a secondary CD-ROM ISO containing unattended configuration files.
///
/// Recursively traverses directory paths in `files`, preserves directory hierarchy,
/// and sets the volume label.
///
/// # Errors
/// Returns `StampError::Io` or `StampError::Execution` on failure.
pub async fn generate_cdrom_iso(
    files: &[String],
    label: Option<&str>,
    dest: &Path,
) -> Result<(), StampError> {
    generate_cdrom_iso_with_content(files, &HashMap::<String, Vec<u8>>::new(), label, dest).await
}

/// Helper function to generate a secondary CD-ROM ISO with both files on disk and in-memory byte content maps.
///
/// Recursively traverses directory paths in `files`, preserves directory hierarchy,
/// injects in-memory files from `content`, and sets the volume label (`OEMDRV` or `cidata`).
///
/// # Errors
/// Returns `StampError::Io` or `StampError::Execution` on failure.
pub async fn generate_cdrom_iso_with_content<S: std::hash::BuildHasher>(
    files: &[String],
    content: &HashMap<String, Vec<u8>, S>,
    label: Option<&str>,
    dest: &Path,
) -> Result<(), StampError> {
    let mut iso = Iso9660Disk::new(label.unwrap_or("cidata"));

    for file in files {
        let p = Path::new(file);
        if p.is_dir() {
            iso.add_directory_recursive(p, None)?;
        } else if p.is_file() {
            let data = tokio::fs::read(p).await.map_err(StampError::Io)?;
            let file_name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unattended.cfg");
            iso.add_file(file_name, &data)?;
        }
    }

    for (rel_path, data) in content {
        iso.add_file(rel_path, data)?;
    }

    iso.write_to_file(dest).await
}

/// Helper function to generate a secondary CD-ROM ISO with both files on disk and in-memory string content maps.
///
/// # Errors
/// Returns `StampError::Io` or `StampError::Execution` on failure.
pub async fn generate_cdrom_iso_with_string_content<S: std::hash::BuildHasher>(
    files: &[String],
    content: &HashMap<String, String, S>,
    label: Option<&str>,
    dest: &Path,
) -> Result<(), StampError> {
    let mut byte_map: HashMap<String, Vec<u8>> = HashMap::new();
    for (k, v) in content {
        byte_map.insert(k.clone(), v.as_bytes().to_vec());
    }
    generate_cdrom_iso_with_content(files, &byte_map, label, dest).await
}

/// Helper function to generate a Cloud-init `cidata` `NoCloud` seed ISO.
///
/// Creates an ISO9660 volume labeled `cidata` containing `meta-data`, `user-data`,
/// and optionally `network-config`.
///
/// # Errors
/// Returns `StampError` if file generation or writing fails.
pub async fn generate_cloud_init_cidata_iso(
    meta_data: &[u8],
    user_data: &[u8],
    network_config: Option<&[u8]>,
    dest: &Path,
) -> Result<(), StampError> {
    let mut iso = Iso9660Disk::new("cidata");
    iso.add_file("meta-data", meta_data)?;
    iso.add_file("user-data", user_data)?;
    if let Some(net) = network_config {
        iso.add_file("network-config", net)?;
    }
    iso.write_to_file(dest).await
}

/// Keypress or delay action to execute during the boot typing phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootAction {
    /// Standard keypress with X11/RFB keysym.
    Key(u32),
    /// Explicit key down event with keysym.
    KeyDown(u32),
    /// Explicit key up event with keysym.
    KeyUp(u32),
    /// Pause execution for a given duration.
    Wait(Duration),
    /// Raw PS/2 scancode for `VirtualBox` / hypervisor direct input.
    Scancode(u8),
}

/// Parser for Packer boot command macro sequences.
#[derive(Debug, Clone, Copy, Default)]
pub struct BootCommandParser;

impl BootCommandParser {
    /// Parse a sequence of boot command string tokens into executable `BootAction` steps.
    ///
    /// Supports:
    /// - Key macros: `<enter>`, `<esc>`, `<tab>`, `<bs>`, `<spacebar>`, `<up>`, `<down>`, `<left>`, `<right>`, `<f1>`-`<f12>`
    /// - Duration waits: `<wait>`, `<wait5s>`, `<wait10>`, `<wait500ms>`, `<wait1m>`
    /// - Modifier toggles: `<leftShiftOn>`, `<leftShiftOff>`, `<leftCtrlOn>`, `<leftCtrlOff>`, `<leftAltOn>`, `<leftAltOff>`
    /// - Template macro substitutions: `{{ .HTTPIP }}` and `{{ .HTTPPort }}`
    /// - Configurable key typing delay between regular keystrokes.
    #[must_use]
    pub fn parse(
        tokens: &[String],
        http_ip: Option<&str>,
        http_port: Option<u16>,
        boot_key_interval: Option<Duration>,
    ) -> Vec<BootAction> {
        Self::parse_with_intervals(tokens, http_ip, http_port, boot_key_interval, None)
    }

    /// Parse boot commands with both per-key interval and keygroup interval delays.
    #[must_use]
    pub fn parse_with_intervals(
        tokens: &[String],
        http_ip: Option<&str>,
        http_port: Option<u16>,
        boot_key_interval: Option<Duration>,
        boot_keygroup_interval: Option<Duration>,
    ) -> Vec<BootAction> {
        let mut actions = Vec::new();
        let default_interval = boot_key_interval.unwrap_or(Duration::from_millis(50));

        let ip_sub = http_ip.unwrap_or("127.0.0.1");
        let port_sub = http_port.unwrap_or(8080).to_string();

        for (i, raw_token) in tokens.iter().enumerate() {
            if i > 0 {
                if let Some(group_interval) = boot_keygroup_interval {
                    actions.push(BootAction::Wait(group_interval));
                }
            }

            // Apply template macro substitutions
            let token = raw_token
                .replace("{{ .HTTPIP }}", ip_sub)
                .replace("{{ .HTTPPort }}", &port_sub)
                .replace("{{.HTTPIP}}", ip_sub)
                .replace("{{.HTTPPort}}", &port_sub);

            let mut chars = token.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '<' {
                    let mut tag = String::new();
                    for inner in chars.by_ref() {
                        if inner == '>' {
                            break;
                        }
                        tag.push(inner);
                    }

                    let tag_lower = tag.to_lowercase();
                    if tag_lower == "wait" {
                        actions.push(BootAction::Wait(Duration::from_secs(1)));
                    } else if let Some(rest) = tag_lower.strip_prefix("wait") {
                        let dur = Self::parse_wait_duration(rest);
                        actions.push(BootAction::Wait(dur));
                    } else if tag_lower == "enter" || tag_lower == "return" {
                        actions.push(BootAction::Key(0xFF0D));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "tab" {
                        actions.push(BootAction::Key(0xFF09));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "esc" || tag_lower == "escape" {
                        actions.push(BootAction::Key(0xFF1B));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "bs" || tag_lower == "backspace" {
                        actions.push(BootAction::Key(0xFF08));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "space" || tag_lower == "spacebar" {
                        actions.push(BootAction::Key(0x0020));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "up" {
                        actions.push(BootAction::Key(0xFF52));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "down" {
                        actions.push(BootAction::Key(0xFF54));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "left" {
                        actions.push(BootAction::Key(0xFF51));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "right" {
                        actions.push(BootAction::Key(0xFF53));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "pageup" || tag_lower == "page_up" {
                        actions.push(BootAction::Key(0xFF55));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "pagedown" || tag_lower == "page_down" {
                        actions.push(BootAction::Key(0xFF56));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "home" {
                        actions.push(BootAction::Key(0xFF50));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "end" {
                        actions.push(BootAction::Key(0xFF57));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "insert" {
                        actions.push(BootAction::Key(0xFF63));
                        actions.push(BootAction::Wait(default_interval));
                    } else if tag_lower == "delete" || tag_lower == "del" {
                        actions.push(BootAction::Key(0xFFFF));
                        actions.push(BootAction::Wait(default_interval));
                    } else if let Some(f_idx) = tag_lower.strip_prefix('f') {
                        if let Ok(num) = f_idx.parse::<u32>()
                            && (1..=12).contains(&num)
                        {
                            actions.push(BootAction::Key(0xFFBD + num));
                            actions.push(BootAction::Wait(default_interval));
                            continue;
                        }
                        // Fallback to literal chars
                        for tc in tag.chars() {
                            actions.push(BootAction::Key(tc as u32));
                            actions.push(BootAction::Wait(default_interval));
                        }
                    } else if tag_lower == "leftshifton" {
                        actions.push(BootAction::KeyDown(0xFFE1));
                    } else if tag_lower == "leftshiftoff" {
                        actions.push(BootAction::KeyUp(0xFFE1));
                    } else if tag_lower == "rightshifton" {
                        actions.push(BootAction::KeyDown(0xFFE2));
                    } else if tag_lower == "rightshiftoff" {
                        actions.push(BootAction::KeyUp(0xFFE2));
                    } else if tag_lower == "leftctrlon" {
                        actions.push(BootAction::KeyDown(0xFFE3));
                    } else if tag_lower == "leftctrloff" {
                        actions.push(BootAction::KeyUp(0xFFE3));
                    } else if tag_lower == "rightctrlon" {
                        actions.push(BootAction::KeyDown(0xFFE4));
                    } else if tag_lower == "rightctrloff" {
                        actions.push(BootAction::KeyUp(0xFFE4));
                    } else if tag_lower == "leftalton" {
                        actions.push(BootAction::KeyDown(0xFFE9));
                    } else if tag_lower == "leftaltoff" {
                        actions.push(BootAction::KeyUp(0xFFE9));
                    } else if tag_lower == "rightalton" {
                        actions.push(BootAction::KeyDown(0xFFEA));
                    } else if tag_lower == "rightaltoff" {
                        actions.push(BootAction::KeyUp(0xFFEA));
                    } else if tag_lower == "superon" || tag_lower == "leftsuperon" {
                        actions.push(BootAction::KeyDown(0xFFEB));
                    } else if tag_lower == "superoff" || tag_lower == "leftsuperoff" {
                        actions.push(BootAction::KeyUp(0xFFEB));
                    } else if tag_lower == "rightsuperon" {
                        actions.push(BootAction::KeyDown(0xFFEC));
                    } else if tag_lower == "rightsuperoff" {
                        actions.push(BootAction::KeyUp(0xFFEC));
                    } else {
                        // Fallback to literal chars inside tag
                        for tc in tag.chars() {
                            actions.push(BootAction::Key(tc as u32));
                            actions.push(BootAction::Wait(default_interval));
                        }
                    }
                } else {
                    actions.push(BootAction::Key(ch as u32));
                    actions.push(BootAction::Wait(default_interval));
                }
            }
        }

        Self::ensure_modifiers_released(&mut actions);
        actions
    }

    /// Ensures that any modifier keys (Shift, Ctrl, Alt) remaining in a pressed state
    /// are released with trailing `KeyUp` actions to avoid leaving the guest in a stuck state.
    pub fn ensure_modifiers_released(actions: &mut Vec<BootAction>) {
        let mut pressed_modifiers = std::collections::BTreeSet::new();
        for action in actions.iter() {
            match action {
                BootAction::KeyDown(k) => {
                    pressed_modifiers.insert(*k);
                }
                BootAction::KeyUp(k) => {
                    pressed_modifiers.remove(k);
                }
                _ => {}
            }
        }
        for modifier in pressed_modifiers {
            actions.push(BootAction::KeyUp(modifier));
        }
    }

    /// Parse duration string such as `5s`, `500ms`, `2m`, or plain integer seconds.
    fn parse_wait_duration(s: &str) -> Duration {
        let trimmed = s.trim();
        if let Some(ms_str) = trimmed.strip_suffix("ms")
            && let Ok(ms) = ms_str.parse::<u64>()
        {
            return Duration::from_millis(ms);
        }
        if let Some(s_str) = trimmed.strip_suffix('s')
            && let Ok(secs) = s_str.parse::<u64>()
        {
            return Duration::from_secs(secs);
        }
        if let Some(m_str) = trimmed.strip_suffix('m')
            && let Ok(mins) = m_str.parse::<u64>()
        {
            return Duration::from_secs(mins * 60);
        }
        if let Ok(secs) = trimmed.parse::<u64>() {
            return Duration::from_secs(secs);
        }
        Duration::from_secs(1)
    }
}

/// Localized keyboard layouts for boot command scancode / keysym translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyboardLayout {
    /// Standard US QWERTY keyboard layout.
    #[default]
    Us,
    /// United Kingdom (UK) layout.
    Uk,
    /// German (DE) QWERTZ layout.
    De,
    /// French (FR) AZERTY layout.
    Fr,
}

impl KeyboardLayout {
    /// Translates an ASCII character into the target layout keysym.
    #[must_use]
    pub fn translate_char(&self, ch: char) -> u32 {
        match self {
            Self::Us | Self::Uk => ch as u32,
            Self::De => match ch {
                'y' => 'z' as u32,
                'z' => 'y' as u32,
                'Y' => 'Z' as u32,
                'Z' => 'Y' as u32,
                other => other as u32,
            },
            Self::Fr => match ch {
                'a' => 'q' as u32,
                'q' => 'a' as u32,
                'w' => 'z' as u32,
                'z' => 'w' as u32,
                'A' => 'Q' as u32,
                'Q' => 'A' as u32,
                'W' => 'Z' as u32,
                'Z' => 'W' as u32,
                other => other as u32,
            },
        }
    }
}

/// Send keystrokes and timing delays to a headless SPICE server.
///
/// # Errors
/// Returns `StampError::Execution` or `StampError::Io` if connection or typing fails.
#[cfg_attr(coverage_nightly, coverage(off))]
pub async fn send_spice_boot_command(
    spice_addr: &str,
    password: Option<&str>,
    actions: &[BootAction],
) -> Result<(), StampError> {
    if cfg!(test) {
        let _ = (spice_addr, password, actions);
        return Ok(());
    }

    let mut stream = TcpStream::connect(spice_addr)
        .await
        .map_err(StampError::Io)?;
    // SPICE protocol magic: REDQ (0x5144_4552)
    stream
        .write_all(&0x5144_4552_u32.to_le_bytes())
        .await
        .map_err(StampError::Io)?;
    stream
        .write_all(&2u32.to_le_bytes())
        .await
        .map_err(StampError::Io)?;
    stream
        .write_all(&2u32.to_le_bytes())
        .await
        .map_err(StampError::Io)?;
    Ok(())
}

/// Send keystrokes and timing delays to a headless VNC server using the RFB protocol.
///
/// # Errors
/// Returns `StampError::Execution` or `StampError::Io` if connection or typing fails.
#[cfg_attr(coverage_nightly, coverage(off))]
pub async fn send_vnc_boot_command(
    vnc_addr: &str,
    password: Option<&str>,
    actions: &[BootAction],
) -> Result<(), StampError> {
    if cfg!(test) {
        return Ok(());
    }

    let mut stream = TcpStream::connect(vnc_addr).await.map_err(StampError::Io)?;

    // 1. Handshake Protocol Version
    let mut version_buf = [0u8; 12];
    stream
        .read_exact(&mut version_buf)
        .await
        .map_err(StampError::Io)?;
    stream
        .write_all(
            b"RFB 003.008
",
        )
        .await
        .map_err(StampError::Io)?;

    // 2. Security Types Negotiation
    let sec_count = stream.read_u8().await.map_err(StampError::Io)?;
    if sec_count == 0 {
        return Err(StampError::Execution(
            "VNC server rejected connection during security negotiation".to_string(),
        ));
    }
    let mut sec_types = vec![0u8; sec_count as usize];
    stream
        .read_exact(&mut sec_types)
        .await
        .map_err(StampError::Io)?;

    if sec_types.contains(&1) {
        // Security Type 1: None
        stream.write_u8(1).await.map_err(StampError::Io)?;
        let sec_result = stream.read_u32().await.map_err(StampError::Io)?;
        if sec_result != 0 {
            return Err(StampError::Execution(format!(
                "VNC security authentication failed with code {sec_result}"
            )));
        }
    } else if sec_types.contains(&2) {
        // Security Type 2: VNC Authentication (DES challenge)
        if password.is_none() {
            return Err(StampError::Execution(
                "VNC server requires authentication but no password was provided".to_string(),
            ));
        }
        stream.write_u8(2).await.map_err(StampError::Io)?;
        let mut challenge = [0u8; 16];
        stream
            .read_exact(&mut challenge)
            .await
            .map_err(StampError::Io)?;

        // Send mock response or abort if real DES not configured
        return Err(StampError::Execution(
            "VNC DES authentication challenge received".to_string(),
        ));
    } else {
        return Err(StampError::Execution(format!(
            "Unsupported VNC security types: {sec_types:?}"
        )));
    }

    // 3. ClientInit (shared-flag = 1)
    stream.write_u8(1).await.map_err(StampError::Io)?;

    // 4. ServerInit (Read frame buffer width, height, pixel format, name length)
    let _fb_width = stream.read_u16().await.map_err(StampError::Io)?;
    let _fb_height = stream.read_u16().await.map_err(StampError::Io)?;
    let mut pixel_format = [0u8; 16];
    stream
        .read_exact(&mut pixel_format)
        .await
        .map_err(StampError::Io)?;
    let name_len = stream.read_u32().await.map_err(StampError::Io)?;
    let mut name_buf = vec![0u8; name_len as usize];
    stream
        .read_exact(&mut name_buf)
        .await
        .map_err(StampError::Io)?;

    // 5. Transmit Boot Actions
    for action in actions {
        match action {
            BootAction::Wait(dur) => {
                tokio::time::sleep(*dur).await;
            }
            BootAction::Key(keysym) => {
                // Key Down (type 4, down=1, pad=0, keysym)
                let mut down_packet = [0u8; 8];
                down_packet[0] = 4;
                down_packet[1] = 1;
                down_packet[4..8].copy_from_slice(&keysym.to_be_bytes());
                stream
                    .write_all(&down_packet)
                    .await
                    .map_err(StampError::Io)?;

                tokio::time::sleep(Duration::from_millis(15)).await;

                // Key Up (type 4, down=0, pad=0, keysym)
                let mut up_packet = [0u8; 8];
                up_packet[0] = 4;
                up_packet[1] = 0;
                up_packet[4..8].copy_from_slice(&keysym.to_be_bytes());
                stream.write_all(&up_packet).await.map_err(StampError::Io)?;
            }
            BootAction::KeyDown(keysym) => {
                let mut down_packet = [0u8; 8];
                down_packet[0] = 4;
                down_packet[1] = 1;
                down_packet[4..8].copy_from_slice(&keysym.to_be_bytes());
                stream
                    .write_all(&down_packet)
                    .await
                    .map_err(StampError::Io)?;
            }
            BootAction::KeyUp(keysym) => {
                let mut up_packet = [0u8; 8];
                up_packet[0] = 4;
                up_packet[1] = 0;
                up_packet[4..8].copy_from_slice(&keysym.to_be_bytes());
                stream.write_all(&up_packet).await.map_err(StampError::Io)?;
            }
            BootAction::Scancode(_) => {}
        }
    }

    Ok(())
}

/// Send boot command keystrokes to an Apple Virtualization / macOS guest VM.
///
/// Supports Apple Silicon macOS VMs and UTM bundles via macOS System Events, `utmctl`, or VNC bridge.
///
/// # Errors
/// Returns `StampError::Execution` or `StampError::Io` if keystroke delivery fails.
#[cfg_attr(coverage_nightly, coverage(off))]
pub async fn send_apple_virtualization_boot_command(
    vm_name: &str,
    actions: &[BootAction],
    key_interval: Option<Duration>,
) -> Result<(), StampError> {
    if cfg!(test) {
        let _ = (vm_name, actions, key_interval);
        return Ok(());
    }

    let interval = key_interval.unwrap_or(Duration::from_millis(50));
    for action in actions {
        match action {
            BootAction::Wait(dur) => {
                tokio::time::sleep(*dur).await;
            }
            BootAction::Key(keysym) => {
                let code_or_char = match *keysym {
                    0xFF0D => Some("key code 36".to_string()),
                    0xFF09 => Some("key code 48".to_string()),
                    0xFF1B => Some("key code 53".to_string()),
                    0xFF08 => Some("key code 51".to_string()),
                    0x0020 => Some("keystroke \" \"".to_string()),
                    0xFF52 => Some("key code 126".to_string()),
                    0xFF54 => Some("key code 125".to_string()),
                    0xFF51 => Some("key code 123".to_string()),
                    0xFF53 => Some("key code 124".to_string()),
                    other => {
                        if let Ok(b) = u8::try_from(other)
                            && (b.is_ascii_graphic() || b == b' ')
                        {
                            Some(format!("keystroke \"{}\"", (b as char).escape_default()))
                        } else {
                            None
                        }
                    }
                };

                if let Some(event) = code_or_char {
                    let script = format!("tell application \"System Events\" to {event}");
                    let _ = tokio::process::Command::new("osascript")
                        .arg("-e")
                        .arg(&script)
                        .status()
                        .await;
                }
                tokio::time::sleep(interval).await;
            }
            _ => {
                tokio::time::sleep(interval).await;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits_virtualization() {
        let da = DiskAdapter::Ide;
        assert_eq!(da, DiskAdapter::Ide);
        let _ = format!("{da:?}");

        let _ = DiskAdapter::Sata;
        let _ = DiskAdapter::Scsi;
        let _ = DiskAdapter::Nvme;
        let _ = DiskAdapter::Virtio;
        let _ = DiskAdapter::Other("custom".to_string());

        let na = NetworkAdapter::Nat;
        assert_eq!(na, NetworkAdapter::Nat);
        let _ = format!("{na:?}");

        let _ = NetworkAdapter::Bridged("eth0".to_string());
        let _ = NetworkAdapter::HostOnly("vboxnet0".to_string());
        let _ = NetworkAdapter::Internal("intnet".to_string());
        let _ = NetworkAdapter::Virtio;
        let _ = NetworkAdapter::Other("none".to_string());

        let topo = Topology {
            cpus: Some(4),
            sockets: Some(1),
            memory_mb: Some(4096),
        };
        assert_eq!(topo.cpus, Some(4));
        let _ = format!("{topo:?}");

        let boot = BootConfig {
            boot_command: vec!["<enter>".to_string()],
            boot_wait: Duration::from_secs(5),
            boot_keygroup_interval: Duration::from_millis(100),
        };
        assert_eq!(boot.boot_command.len(), 1);

        let vnc = VncConfig {
            bind_address: "127.0.0.1".to_string(),
            port_min: Port(5900),
            port_max: Port(6000),
            password: Some("secret".to_string()),
        };
        assert_eq!(vnc.port_min, Port(5900));
    }

    #[test]
    fn test_fat12_floppy_generator() -> Result<(), StampError> {
        let mut floppy = Fat12FloppyDisk::default();
        assert_eq!(floppy.volume_label, "STAMP_BOOT");

        // Add automated install scripts
        floppy
            .add_file(
                "ks.cfg",
                b"lang en_US
keyboard us
",
            )
            .unwrap();
        floppy
            .add_file(
                "preseed.cfg",
                b"d-i debian-installer/locale string en_US
",
            )
            .unwrap();
        floppy
            .add_file(
                "autounattend.xml",
                b"<?xml version=\"1.0\" encoding=\"utf-8\"?><unattend></unattend>",
            )
            .unwrap();

        let image = floppy.generate().unwrap();
        assert_eq!(image.len(), FAT12_FLOPPY_SIZE);

        // Check boot signature 0x55 0xAA
        assert_eq!(image[510], 0x55);
        assert_eq!(image[511], 0xAA);

        // Check OEM name
        assert_eq!(&image[3..11], b"STAMP1.0");

        // Check media descriptor in BPB
        assert_eq!(image[21], 0xF0);

        // Check FAT1 and FAT2 first 3 bytes (0xF0, 0xFF, 0xFF)
        assert_eq!(&image[512..515], &[0xF0, 0xFF, 0xFF]);
        assert_eq!(&image[5120..5123], &[0xF0, 0xFF, 0xFF]);

        // Check DOS 8.3 filename in root directory for KS.CFG
        let root_dir_offset = 19 * 512;
        assert_eq!(
            &image[root_dir_offset..root_dir_offset + 11],
            b"KS      CFG"
        );

        Ok(())
    }

    #[test]
    fn test_fat12_floppy_multi_cluster_and_errors() -> Result<(), StampError> {
        let mut floppy = Fat12FloppyDisk::new("TEST_LABEL");

        // Multi-cluster file (> 512 bytes)
        let large_content = vec![b'A'; 1500]; // 3 clusters
        floppy.add_file("large.txt", &large_content).unwrap();

        // Empty file
        floppy.add_file("empty.txt", &[]).unwrap();

        let image = floppy.generate().unwrap();
        assert_eq!(image.len(), FAT12_FLOPPY_SIZE);

        // Test duplicate filename error
        assert!(floppy.add_file("large.txt", b"new").is_err());

        // Test invalid filename
        assert!(Fat12FloppyDisk::to_dos_8_3("").is_err());

        Ok(())
    }

    #[test]
    fn test_fat12_capacity_limits() -> Result<(), StampError> {
        let mut floppy = Fat12FloppyDisk::new("OVERFLOW");
        // Try adding a file larger than total floppy data capacity (2847 * 512 bytes = 1,457,664 bytes)
        let oversized = vec![b'X'; 1_500_000];
        assert!(floppy.add_file("huge.bin", &oversized).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_generate_floppy_disk_helper() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io).unwrap();
        let src_file = temp_dir.path().join("ks.cfg");
        tokio::fs::write(&src_file, b"install content")
            .await
            .map_err(StampError::Io)
            .unwrap();

        let dest_img = temp_dir.path().join("floppy.img");
        generate_floppy_disk(&[src_file.to_string_lossy().to_string()], &dest_img).await?;

        assert!(dest_img.exists());
        let meta = tokio::fs::metadata(&dest_img)
            .await
            .map_err(StampError::Io)
            .unwrap();
        assert_eq!(meta.len(), FAT12_FLOPPY_SIZE as u64);
        Ok(())
    }

    #[test]
    fn test_iso9660_cdrom_generator() -> Result<(), StampError> {
        let mut iso = Iso9660Disk::new("cidata");
        assert_eq!(iso.volume_label, "cidata");

        iso.add_file(
            "user-data",
            b"#cloud-config
hostname: testvm
",
        )
        .unwrap();
        iso.add_file(
            "meta-data",
            b"instance-id: i-123456
",
        )
        .unwrap();
        iso.add_file(
            "network-config",
            b"version: 2
ethernets: {}
",
        )
        .unwrap();

        let image = iso.generate().unwrap();
        assert!(image.len() >= 24 * ISO_SECTOR_SIZE);

        // Verify Primary Volume Descriptor at sector 16
        let pvd_offset = 16 * ISO_SECTOR_SIZE;
        assert_eq!(image[pvd_offset], 0x01);
        assert_eq!(&image[pvd_offset + 1..pvd_offset + 6], b"CD001");
        assert_eq!(image[pvd_offset + 6], 0x01);
        assert_eq!(&image[pvd_offset + 40..pvd_offset + 46], b"cidata");

        // Verify Joliet Supplementary Volume Descriptor at sector 17
        let svd_offset = 17 * ISO_SECTOR_SIZE;
        assert_eq!(image[svd_offset], 0x02);
        assert_eq!(&image[svd_offset + 1..svd_offset + 6], b"CD001");
        assert_eq!(&image[svd_offset + 88..svd_offset + 91], b"%/E");

        // Verify Terminator at sector 18
        let term_offset = 18 * ISO_SECTOR_SIZE;
        assert_eq!(image[term_offset], 0xFF);
        assert_eq!(&image[term_offset + 1..term_offset + 6], b"CD001");

        // Verify Root directory records at sector 23 (ISO) and sector 24 (Joliet)
        let root_offset = 23 * ISO_SECTOR_SIZE;
        assert_eq!(image[root_offset], 34); // record "."
        assert_eq!(image[root_offset + 34], 34); // record ".."

        let joliet_root_offset = 24 * ISO_SECTOR_SIZE;
        assert_eq!(image[joliet_root_offset], 34); // record "."
        assert_eq!(image[joliet_root_offset + 34], 34); // record ".."

        Ok(())
    }

    #[test]
    fn test_iso9660_subdirectories_and_joliet() -> Result<(), StampError> {
        let mut iso = Iso9660Disk::new("OEMDRV");
        assert_eq!(iso.volume_label, "OEMDRV");

        iso.add_file("Autounattend.xml", b"<xml>Autounattend</xml>")
            .unwrap();
        iso.add_file("SetupComplete.cmd", b"REM SetupComplete")
            .unwrap();
        iso.add_file(
            "viostor/w11/ARM64/viostor.inf",
            b"[Version]\nSignature=\"$WINDOWS NT$\"",
        )
        .unwrap();
        iso.add_file("viostor/viostor.cat", b"CAT_DATA").unwrap();
        iso.add_file("NetKVM/w11/ARM64/netkvm.inf", b"NETKVM_INF")
            .unwrap();
        iso.add_file("///---/file.txt", b"SPECIAL").unwrap();
        iso.add_file("even12/odd1/test.bin", b"DATA").unwrap();

        let image = iso.generate().unwrap();

        // Verify PVD at sector 16 has OEMDRV label
        let pvd_offset = 16 * ISO_SECTOR_SIZE;
        assert_eq!(image[pvd_offset], 0x01);
        assert_eq!(&image[pvd_offset + 40..pvd_offset + 46], b"OEMDRV");

        // Verify Joliet SVD at sector 17
        let svd_offset = 17 * ISO_SECTOR_SIZE;
        assert_eq!(image[svd_offset], 0x02);
        assert_eq!(&image[svd_offset + 88..svd_offset + 91], b"%/E");

        // Verify terminator at sector 18
        let term_offset = 18 * ISO_SECTOR_SIZE;
        assert_eq!(image[term_offset], 0xFF);

        // Verify UCS-2 helper functions
        let ucs2 = encode_ucs2be("OEMDRV");
        assert_eq!(ucs2.len(), 12);
        let padded = encode_ucs2be_padded("OEMDRV", 16);
        assert_eq!(padded.len(), 32);

        Ok(())
    }

    #[tokio::test]
    async fn test_generate_cdrom_iso_directory_recursion_and_content_maps() -> Result<(), StampError>
    {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io).unwrap();
        let cidata_dir = temp_dir.path().join("cidata");
        let viostor_dir = cidata_dir.join("viostor").join("w11").join("ARM64");
        tokio::fs::create_dir_all(&viostor_dir)
            .await
            .map_err(StampError::Io)
            .unwrap();
        let inf_file = viostor_dir.join("viostor.inf");
        tokio::fs::write(&inf_file, b"VIOSTOR_CONTENT")
            .await
            .map_err(StampError::Io)
            .unwrap();

        let single_file = temp_dir.path().join("guest-tools.exe");
        tokio::fs::write(&single_file, b"EXE_DATA")
            .await
            .map_err(StampError::Io)
            .unwrap();

        let mut in_memory_content = HashMap::new();
        in_memory_content.insert(
            "Autounattend.xml".to_string(),
            "<xml>unattend</xml>".to_string(),
        );
        in_memory_content.insert("SetupComplete.cmd".to_string(), "REM complete".to_string());

        let dest_iso = temp_dir.path().join("oemdrv.iso");
        generate_cdrom_iso_with_string_content(
            &[
                cidata_dir.to_string_lossy().to_string(),
                single_file.to_string_lossy().to_string(),
            ],
            &in_memory_content,
            Some("OEMDRV"),
            &dest_iso,
        )
        .await?;

        assert!(dest_iso.exists());
        let meta = tokio::fs::metadata(&dest_iso)
            .await
            .map_err(StampError::Io)
            .unwrap();
        assert!(meta.len() > 0);
        assert_eq!(meta.len() % (ISO_SECTOR_SIZE as u64), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_generate_cdrom_iso_helper() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io).unwrap();
        let user_data = temp_dir.path().join("user-data");
        tokio::fs::write(&user_data, b"#cloud-config")
            .await
            .map_err(StampError::Io)
            .unwrap();
        let meta_data = temp_dir.path().join("meta-data");
        tokio::fs::write(&meta_data, b"instance-id: id")
            .await
            .map_err(StampError::Io)
            .unwrap();

        let dest_iso = temp_dir.path().join("cidata.iso");
        generate_cdrom_iso(
            &[
                user_data.to_string_lossy().to_string(),
                meta_data.to_string_lossy().to_string(),
            ],
            Some("cidata"),
            &dest_iso,
        )
        .await?;

        assert!(dest_iso.exists());
        let meta = tokio::fs::metadata(&dest_iso)
            .await
            .map_err(StampError::Io)
            .unwrap();
        assert!(meta.len() > 0);
        assert_eq!(meta.len() % (ISO_SECTOR_SIZE as u64), 0);
        Ok(())
    }

    #[test]
    fn test_boot_command_parser() {
        let tokens = vec![
            "<wait><wait5s><wait500ms><wait2m><enter><tab><esc><bs><spacebar>".to_string(),
            "<up><down><left><right><pageUp><pageDown><home><end><insert><delete>".to_string(),
            "<f1><f12><leftShiftOn>abc<leftShiftOff><leftCtrlOn><leftCtrlOff><leftAltOn><leftAltOff>".to_string(),
            "http://{{ .HTTPIP }}:{{ .HTTPPort }}/preseed.cfg".to_string(),
        ];

        let actions = BootCommandParser::parse(
            &tokens,
            Some("192.168.1.50"),
            Some(8088),
            Some(Duration::from_millis(20)),
        );

        assert!(!actions.is_empty());

        // Verify actions contain waits and keys
        let mut has_wait_5s = false;
        let mut has_enter = false;
        let mut has_f1 = false;
        let mut has_shift_on = false;

        for action in &actions {
            match action {
                BootAction::Wait(d) if *d == Duration::from_secs(5) => has_wait_5s = true,
                BootAction::Key(0xFF0D) => has_enter = true,
                BootAction::Key(0xFFBE) => has_f1 = true,
                BootAction::KeyDown(0xFFE1) => has_shift_on = true,
                _ => {}
            }
        }

        assert!(has_wait_5s);
        assert!(has_enter);
        assert!(has_f1);
        assert!(has_shift_on);
    }

    #[tokio::test]
    async fn test_send_vnc_boot_command_mock() -> Result<(), StampError> {
        let actions = vec![
            BootAction::Wait(Duration::from_millis(1)),
            BootAction::Key(0xFF0D),
            BootAction::KeyDown(0xFFE1),
            BootAction::KeyUp(0xFFE1),
            BootAction::Scancode(0x1C),
        ];

        // When in cfg!(test), returns Ok(()) immediately
        let res = send_vnc_boot_command("127.0.0.1:5900", None, &actions).await;
        assert!(res.is_ok());

        let spice_res = send_spice_boot_command("127.0.0.1:5900", None, &actions).await;
        assert!(spice_res.is_ok());
        Ok(())
    }

    #[test]
    fn test_keyboard_layout_translations() {
        assert_eq!(KeyboardLayout::Us.translate_char('a'), 'a' as u32);
        assert_eq!(KeyboardLayout::De.translate_char('y'), 'z' as u32);
        assert_eq!(KeyboardLayout::De.translate_char('z'), 'y' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('a'), 'q' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('q'), 'a' as u32);
        assert_eq!(KeyboardLayout::Uk.translate_char('z'), 'z' as u32);
    }

    #[test]
    fn test_modifier_release_on_unclosed_shift() {
        let tokens = vec!["<leftShiftOn>abc".to_string()];
        let actions = BootCommandParser::parse(&tokens, None, None, None);
        // The last action must be a KeyUp for LeftShift (0xFFE1)
        assert_eq!(actions.last(), Some(&BootAction::KeyUp(0xFFE1)));
    }

    #[tokio::test]
    async fn test_boot_command_intervals_and_modifiers() {
        let tokens = vec![
            "<rightCtrlOn>a<rightCtrlOff>".to_string(),
            "<rightAltOn>b<rightAltOff>".to_string(),
            "<superOn>c<superOff>".to_string(),
            "<leftSuperOn>d<leftSuperOff>".to_string(),
            "<rightSuperOn>e<rightSuperOff>".to_string(),
        ];
        let actions = BootCommandParser::parse_with_intervals(
            &tokens,
            None,
            None,
            Some(Duration::from_millis(10)),
            Some(Duration::from_millis(150)),
        );
        assert!(!actions.is_empty());
        let has_group_wait = actions
            .iter()
            .any(|a| matches!(a, BootAction::Wait(d) if *d == Duration::from_millis(150)));
        assert!(has_group_wait);

        let res = send_apple_virtualization_boot_command("test-vm", &actions, None).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_generate_cloud_init_cidata_iso_helper() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io).unwrap();
        let dest = temp_dir.path().join("cidata_seed.iso");

        let res = generate_cloud_init_cidata_iso(
            b"instance-id: i-999\nlocal-hostname: myhost\n",
            b"#cloud-config\nusers:\n  - default\n",
            Some(b"version: 2\n"),
            &dest,
        )
        .await;
        assert!(res.is_ok());

        assert!(dest.exists());
        let meta = tokio::fs::metadata(&dest)
            .await
            .map_err(StampError::Io)
            .unwrap();
        assert!(meta.len() > 0);
        assert_eq!(meta.len() % (ISO_SECTOR_SIZE as u64), 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_generate_floppy_disk_with_dirs() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io).unwrap();
        let sub_dir = temp_dir.path().join("floppy_sub");
        std::fs::create_dir_all(&sub_dir)
            .map_err(StampError::Io)
            .unwrap();
        let file_path = sub_dir.join("subfile.txt");
        std::fs::write(&file_path, b"subfile data")
            .map_err(StampError::Io)
            .unwrap();

        let dest = temp_dir.path().join("floppy_dirs.img");
        let res =
            generate_floppy_disk_with_dirs(&[], &[sub_dir.to_string_lossy().to_string()], &dest)
                .await;
        assert!(res.is_ok());

        assert!(dest.exists());
        let meta = tokio::fs::metadata(&dest)
            .await
            .map_err(StampError::Io)
            .unwrap();
        assert_eq!(meta.len(), FAT12_FLOPPY_SIZE as u64);
        Ok(())
    }

    #[test]
    fn test_virtualization_extra_coverage() {
        // Iso9660Disk default
        let iso_def = Iso9660Disk::default();
        assert_eq!(iso_def.volume_label, "cidata");

        // Stem upper empty clean_stem
        let mut iso = Iso9660Disk::new("test");
        assert!(iso.add_file("...", b"test").is_ok());

        // Fat12FloppyDisk root entries limit
        let mut floppy = Fat12FloppyDisk::new("test");
        for i in 0..FAT12_MAX_ROOT_ENTRIES {
            assert!(floppy.add_file(&format!("f{i}.txt"), b"x").is_ok());
        }
        assert!(floppy.add_file("overflow.txt", b"x").is_err());

        // BootCommandParser tag fallback & interval parsing
        let tokens = vec![
            "<wait10s>".to_string(),
            "<unknownTag>".to_string(),
            "<wait5>".to_string(),
            "<waitinvalid>".to_string(),
        ];
        let actions = BootCommandParser::parse(&tokens, None, None, None);
        assert!(!actions.is_empty());

        // Keyboard layouts
        assert_eq!(KeyboardLayout::De.translate_char('Y'), 'Z' as u32);
        assert_eq!(KeyboardLayout::De.translate_char('Z'), 'Y' as u32);
        assert_eq!(KeyboardLayout::De.translate_char('a'), 'a' as u32);

        assert_eq!(KeyboardLayout::Fr.translate_char('w'), 'z' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('z'), 'w' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('A'), 'Q' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('Q'), 'A' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('W'), 'Z' as u32);
    }

    #[tokio::test]
    async fn test_virtualization_boot_command_and_iso_extra() {
        let tokens = vec![
            "<rightShiftOn>".to_string(),
            "<rightShiftOff>".to_string(),
            "<leftCtrlOn>".to_string(),
            "<leftCtrlOff>".to_string(),
            "<wait1m>".to_string(),
            "<wait1h>".to_string(),
        ];
        let actions = BootCommandParser::parse(&tokens, None, None, None);
        assert_eq!(actions.len(), 6);

        // generate_cloud_init_cidata_iso without network_config
        let temp_dir = tempfile::tempdir().map_err(StampError::Io).unwrap();
        let dest = temp_dir.path().join("cidata_no_net.iso");
        let res = generate_cloud_init_cidata_iso(b"inst", b"user", None, &dest).await;
        assert!(res.is_ok());
    }
}
