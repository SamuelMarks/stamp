//! FAT12 1.44 MB Virtual Floppy Disk (VFD) filesystem generator.

use crate::error::StampError;
use std::path::Path;

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

        let clean_stem: String = stem
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(8)
            .collect();
        let stem_upper = clean_stem.to_ascii_uppercase();
        for (i, b) in stem_upper.as_bytes().iter().enumerate() {
            dos[i] = *b;
        }

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

        disk[0] = 0xEB;
        disk[1] = 0x3C;
        disk[2] = 0x90;

        let oem = b"STAMP1.0";
        disk[3..11].copy_from_slice(oem);

        disk[11..13].copy_from_slice(&512u16.to_le_bytes());
        disk[13] = 1;
        disk[14..16].copy_from_slice(&1u16.to_le_bytes());
        disk[16] = 2;
        disk[17..19].copy_from_slice(&224u16.to_le_bytes());
        disk[19..21].copy_from_slice(&2880u16.to_le_bytes());
        disk[21] = 0xF0;
        disk[22..24].copy_from_slice(&9u16.to_le_bytes());
        disk[24..26].copy_from_slice(&18u16.to_le_bytes());
        disk[26..28].copy_from_slice(&2u16.to_le_bytes());
        disk[28..32].copy_from_slice(&0u32.to_le_bytes());
        disk[32..36].copy_from_slice(&0u32.to_le_bytes());
        disk[36] = 0x00;
        disk[37] = 0x00;
        disk[38] = 0x29;
        disk[39..43].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        let mut label_buf = [b' '; 11];
        let label_bytes = self.volume_label.as_bytes();
        let copy_len = label_bytes.len().min(11);
        label_buf[..copy_len].copy_from_slice(&label_bytes[..copy_len]);
        disk[43..54].copy_from_slice(&label_buf);
        disk[54..62].copy_from_slice(b"FAT12   ");

        disk[510] = 0x55;
        disk[511] = 0xAA;

        let fat_size = FAT12_SECTORS_PER_FAT * FAT12_SECTOR_SIZE;
        let mut fat = vec![0u8; fat_size];
        Self::write_fat12_entry(&mut fat, 0, 0x0FF0);
        Self::write_fat12_entry(&mut fat, 1, 0x0FFF);

        let root_dir_offset =
            (FAT12_RESERVED_SECTORS + (FAT12_NUM_FATS * FAT12_SECTORS_PER_FAT)) * FAT12_SECTOR_SIZE;
        let data_area_offset = FAT12_FIRST_DATA_SECTOR * FAT12_SECTOR_SIZE;

        let mut current_cluster = 2u16;

        for (i, file) in self.files.iter().enumerate() {
            let start_cluster = if file.data.is_empty() {
                0u16
            } else {
                current_cluster
            };

            let clusters_needed = file.data.len().div_ceil(FAT12_SECTOR_SIZE);

            if clusters_needed > 0 {
                for c in 0..clusters_needed {
                    let c_num = current_cluster + u16::try_from(c).unwrap_or(0);
                    if c + 1 == clusters_needed {
                        Self::write_fat12_entry(&mut fat, c_num, 0x0FFF);
                    } else {
                        Self::write_fat12_entry(&mut fat, c_num, c_num + 1);
                    }

                    let cluster_offset =
                        data_area_offset + ((c_num - 2) as usize * FAT12_SECTOR_SIZE);
                    let file_data_start = c * FAT12_SECTOR_SIZE;
                    let file_data_end = (file_data_start + FAT12_SECTOR_SIZE).min(file.data.len());
                    let chunk = &file.data[file_data_start..file_data_end];
                    disk[cluster_offset..cluster_offset + chunk.len()].copy_from_slice(chunk);
                }
                current_cluster += u16::try_from(clusters_needed).unwrap_or(0);
            }

            let entry_offset = root_dir_offset + (i * 32);
            disk[entry_offset..entry_offset + 11].copy_from_slice(&file.dos_name);
            disk[entry_offset + 11] = 0x20;
            disk[entry_offset + 12..entry_offset + 26].fill(0);
            disk[entry_offset + 26..entry_offset + 28]
                .copy_from_slice(&start_cluster.to_le_bytes());
            let file_size = u32::try_from(file.data.len()).unwrap_or(0);
            disk[entry_offset + 28..entry_offset + 32].copy_from_slice(&file_size.to_le_bytes());
        }

        let fat1_offset = FAT12_RESERVED_SECTORS * FAT12_SECTOR_SIZE;
        disk[fat1_offset..fat1_offset + fat_size].copy_from_slice(&fat);

        let fat2_offset = fat1_offset + fat_size;
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
