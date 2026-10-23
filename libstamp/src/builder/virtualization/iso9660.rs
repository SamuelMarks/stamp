//! ISO9660 Level 3 + Joliet + Rock Ridge CD-ROM filesystem generator.

use crate::error::StampError;
use std::collections::HashMap;
use std::path::Path;

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
        let normalized = path
            .chars()
            .map(|c| if c == char::from(92u8) { '/' } else { c })
            .collect::<String>()
            .trim_start_matches('/')
            .to_string();
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

    /// Collect all unique directories in hierarchical topological order.
    fn collect_directories(&self) -> Vec<IsoDirEntry> {
        let mut dir_map: Vec<String> = vec![String::new()];

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

        let mut subdirs: Vec<String> = dir_map.into_iter().skip(1).collect();
        subdirs.sort_by(|a, b| {
            let depth_a = a.split('/').count();
            let depth_b = b.split('/').count();
            depth_a.cmp(&depth_b).then_with(|| a.cmp(b))
        });

        let mut all_dirs: Vec<IsoDirEntry> = vec![IsoDirEntry {
            index: 1,
            parent_index: 1,
            path: String::new(),
            name: String::new(),
            iso_id: String::new(),
        }];

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
        all_dirs
    }

    /// Generate the full ISO9660 Level 3 + Joliet + Rock Ridge binary filesystem image.
    ///
    /// # Errors
    /// Returns `StampError::Execution` on serialization failure.
    pub fn generate(&self) -> Result<Vec<u8>, StampError> {
        let all_dirs = self.collect_directories();
        let num_dirs = u32::try_from(all_dirs.len()).unwrap_or(0);

        let iso_root_dir_sector = ISO_ROOT_DIR_SECTOR;
        let joliet_root_dir_sector = ISO_ROOT_DIR_SECTOR + num_dirs;
        let first_file_sector = joliet_root_dir_sector + num_dirs;

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

        let mut path_table_l_iso = Vec::new();
        let mut path_table_m_iso = Vec::new();
        for dir in &all_dirs {
            let extent = iso_root_dir_sector + u32::try_from(dir.index).unwrap_or(1) - 1;
            let parent = u16::try_from(dir.parent_index).unwrap_or(1);
            if dir.index == 1 {
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
                let len = u8::try_from(id_bytes.len()).unwrap_or(0);
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

        let mut path_table_l_joliet = Vec::new();
        let mut path_table_m_joliet = Vec::new();
        for dir in &all_dirs {
            let extent = joliet_root_dir_sector + u32::try_from(dir.index).unwrap_or(1) - 1;
            let parent = u16::try_from(dir.parent_index).unwrap_or(1);
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
                let len = u8::try_from(id_bytes.len()).unwrap_or(0);

                path_table_l_joliet.extend_from_slice(&[len, 0]);
                path_table_l_joliet.extend_from_slice(&extent.to_le_bytes());
                path_table_l_joliet.extend_from_slice(&parent.to_le_bytes());
                path_table_l_joliet.extend_from_slice(&id_bytes);

                path_table_m_joliet.extend_from_slice(&[len, 0]);
                path_table_m_joliet.extend_from_slice(&extent.to_be_bytes());
                path_table_m_joliet.extend_from_slice(&parent.to_be_bytes());
                path_table_m_joliet.extend_from_slice(&id_bytes);
            }
        }

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

        let pvd_offset = (ISO_PVD_SECTOR as usize) * ISO_SECTOR_SIZE;
        image[pvd_offset] = 0x01;
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
        image[pvd_offset + 132..pvd_offset + 140].copy_from_slice(&Self::both_endian_u32(
            u32::try_from(path_table_l_iso.len()).unwrap_or(0),
        ));
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
        root_rec[25] = 0x02;
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

        let svd_offset = (ISO_SVD_SECTOR as usize) * ISO_SECTOR_SIZE;
        image[svd_offset] = 0x02;
        image[svd_offset + 1..svd_offset + 6].copy_from_slice(b"CD001");
        image[svd_offset + 6] = 0x01;

        let sys_id_ucs2 = encode_ucs2be_padded("LINUX", 16);
        image[svd_offset + 8..svd_offset + 8 + sys_id_ucs2.len()].copy_from_slice(&sys_id_ucs2);

        let vol_id_ucs2 = encode_ucs2be_padded(&self.volume_label, 16);
        image[svd_offset + 40..svd_offset + 40 + vol_id_ucs2.len()].copy_from_slice(&vol_id_ucs2);

        image[svd_offset + 80..svd_offset + 88]
            .copy_from_slice(&Self::both_endian_u32(total_volume_sectors));
        image[svd_offset + 88..svd_offset + 91].copy_from_slice(b"%/E");

        image[svd_offset + 120..svd_offset + 124].copy_from_slice(&Self::both_endian_u16(1));
        image[svd_offset + 124..svd_offset + 128].copy_from_slice(&Self::both_endian_u16(1));
        image[svd_offset + 128..svd_offset + 132].copy_from_slice(&Self::both_endian_u16(2048));
        image[svd_offset + 132..svd_offset + 140].copy_from_slice(&Self::both_endian_u32(
            u32::try_from(path_table_l_joliet.len()).unwrap_or(0),
        ));
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
        j_root_rec[25] = 0x02;
        j_root_rec[28..32].copy_from_slice(&Self::both_endian_u16(1));
        j_root_rec[32] = 1;
        j_root_rec[33] = 0;

        image[svd_offset + 813..svd_offset + 830].copy_from_slice(ts);
        image[svd_offset + 830..svd_offset + 847].copy_from_slice(ts);
        image[svd_offset + 864..svd_offset + 881].copy_from_slice(ts);
        image[svd_offset + 881] = 0x01;

        let term_offset = (ISO_TERMINATOR_SECTOR as usize) * ISO_SECTOR_SIZE;
        image[term_offset] = 0xFF;
        image[term_offset + 1..term_offset + 6].copy_from_slice(b"CD001");
        image[term_offset + 6] = 0x01;

        for dir in &all_dirs {
            let dir_sector = iso_root_dir_sector + u32::try_from(dir.index).unwrap_or(1) - 1;
            let parent_sector =
                iso_root_dir_sector + u32::try_from(dir.parent_index).unwrap_or(1) - 1;
            let mut cursor = (dir_sector as usize) * ISO_SECTOR_SIZE;

            image[cursor] = 34;
            image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(dir_sector));
            image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
            image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
            image[cursor + 25] = 0x02;
            image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
            image[cursor + 32] = 1;
            image[cursor + 33] = 0;
            cursor += 34;

            image[cursor] = 34;
            image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(parent_sector));
            image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
            image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
            image[cursor + 25] = 0x02;
            image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
            image[cursor + 32] = 1;
            image[cursor + 33] = 1;
            cursor += 34;

            for child in &all_dirs {
                if child.index != 1 && child.parent_index == dir.index {
                    let child_sector =
                        iso_root_dir_sector + u32::try_from(child.index).unwrap_or(1) - 1;
                    let id_bytes = child.iso_id.as_bytes();
                    let name_bytes = child.name.as_bytes();

                    let base_len = 33 + id_bytes.len();
                    let pad = usize::from(base_len % 2 == 1);
                    let nm_len = 5 + name_bytes.len();
                    let px_len = 36;
                    let total_len = base_len + pad + nm_len + px_len;
                    let final_pad = usize::from(total_len % 2 == 1);
                    let rec_len = u8::try_from(total_len + final_pad).unwrap_or(0);

                    image[cursor] = rec_len;
                    image[cursor + 2..cursor + 10]
                        .copy_from_slice(&Self::both_endian_u32(child_sector));
                    image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
                    image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
                    image[cursor + 25] = 0x02;
                    image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
                    image[cursor + 32] = u8::try_from(id_bytes.len()).unwrap_or(0);
                    image[cursor + 33..cursor + 33 + id_bytes.len()].copy_from_slice(id_bytes);

                    let mut sua = cursor + 33 + id_bytes.len() + pad;
                    image[sua..sua + 2].copy_from_slice(b"NM");
                    image[sua + 2] = u8::try_from(nm_len).unwrap_or(0);
                    image[sua + 3] = 1;
                    image[sua + 4] = 0;
                    image[sua + 5..sua + 5 + name_bytes.len()].copy_from_slice(name_bytes);
                    sua += nm_len;

                    image[sua..sua + 2].copy_from_slice(b"PX");
                    image[sua + 2] = u8::try_from(px_len).unwrap_or(0);
                    image[sua + 3] = 1;
                    image[sua + 4..sua + 12].copy_from_slice(&Self::both_endian_u32(0o40755));
                    image[sua + 12..sua + 20].copy_from_slice(&Self::both_endian_u32(2));
                    image[sua + 20..sua + 28].copy_from_slice(&Self::both_endian_u32(0));
                    image[sua + 28..sua + 36].copy_from_slice(&Self::both_endian_u32(0));

                    cursor += rec_len as usize;
                }
            }

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
                    let rec_len = u8::try_from(total_len + final_pad).unwrap_or(0);

                    image[cursor] = rec_len;
                    image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(*sector));
                    image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(
                        u32::try_from(file.data.len()).unwrap_or(0),
                    ));
                    image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
                    image[cursor + 25] = 0x00;
                    image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
                    image[cursor + 32] = u8::try_from(id_bytes.len()).unwrap_or(0);
                    image[cursor + 33..cursor + 33 + id_bytes.len()].copy_from_slice(id_bytes);

                    let mut sua = cursor + 33 + id_bytes.len() + pad;
                    image[sua..sua + 2].copy_from_slice(b"NM");
                    image[sua + 2] = u8::try_from(nm_len).unwrap_or(0);
                    image[sua + 3] = 1;
                    image[sua + 4] = 0;
                    image[sua + 5..sua + 5 + name_bytes.len()].copy_from_slice(name_bytes);
                    sua += nm_len;

                    image[sua..sua + 2].copy_from_slice(b"PX");
                    image[sua + 2] = u8::try_from(px_len).unwrap_or(0);
                    image[sua + 3] = 1;
                    image[sua + 4..sua + 12].copy_from_slice(&Self::both_endian_u32(0o100_644));
                    image[sua + 12..sua + 20].copy_from_slice(&Self::both_endian_u32(1));
                    image[sua + 20..sua + 28].copy_from_slice(&Self::both_endian_u32(0));
                    image[sua + 28..sua + 36].copy_from_slice(&Self::both_endian_u32(0));

                    cursor += rec_len as usize;
                }
            }
        }

        for dir in &all_dirs {
            let dir_sector = joliet_root_dir_sector + u32::try_from(dir.index).unwrap_or(1) - 1;
            let parent_sector =
                joliet_root_dir_sector + u32::try_from(dir.parent_index).unwrap_or(1) - 1;
            let mut cursor = (dir_sector as usize) * ISO_SECTOR_SIZE;

            image[cursor] = 34;
            image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(dir_sector));
            image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
            image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
            image[cursor + 25] = 0x02;
            image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
            image[cursor + 32] = 1;
            image[cursor + 33] = 0;
            cursor += 34;

            image[cursor] = 34;
            image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(parent_sector));
            image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
            image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
            image[cursor + 25] = 0x02;
            image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
            image[cursor + 32] = 1;
            image[cursor + 33] = 1;
            cursor += 34;

            for child in &all_dirs {
                if child.index != 1 && child.parent_index == dir.index {
                    let child_sector =
                        joliet_root_dir_sector + u32::try_from(child.index).unwrap_or(1) - 1;
                    let ucs2_id = encode_ucs2be(&child.name);
                    let total_len = 33 + ucs2_id.len();
                    let pad = usize::from(total_len % 2 == 1);
                    let rec_len = u8::try_from(total_len + pad).unwrap_or(0);

                    image[cursor] = rec_len;
                    image[cursor + 2..cursor + 10]
                        .copy_from_slice(&Self::both_endian_u32(child_sector));
                    image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(2048));
                    image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
                    image[cursor + 25] = 0x02;
                    image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
                    image[cursor + 32] = u8::try_from(ucs2_id.len()).unwrap_or(0);
                    image[cursor + 33..cursor + 33 + ucs2_id.len()].copy_from_slice(&ucs2_id);

                    cursor += rec_len as usize;
                }
            }

            for (sector, file) in &file_sectors {
                let file_parent_path = file.path.rsplit_once('/').map_or("", |(p, _)| p);
                if file_parent_path == dir.path {
                    let id_str = format!("{};1", file.name);
                    let ucs2_id = encode_ucs2be(&id_str);
                    let total_len = 33 + ucs2_id.len();
                    let pad = usize::from(total_len % 2 == 1);
                    let rec_len = u8::try_from(total_len + pad).unwrap_or(0);

                    image[cursor] = rec_len;
                    image[cursor + 2..cursor + 10].copy_from_slice(&Self::both_endian_u32(*sector));
                    image[cursor + 10..cursor + 18].copy_from_slice(&Self::both_endian_u32(
                        u32::try_from(file.data.len()).unwrap_or(0),
                    ));
                    image[cursor + 18..cursor + 25].copy_from_slice(&[126, 9, 6, 12, 0, 0, 0]);
                    image[cursor + 25] = 0x00;
                    image[cursor + 28..cursor + 32].copy_from_slice(&Self::both_endian_u16(1));
                    image[cursor + 32] = u8::try_from(ucs2_id.len()).unwrap_or(0);
                    image[cursor + 33..cursor + 33 + ucs2_id.len()].copy_from_slice(&ucs2_id);

                    cursor += rec_len as usize;
                }
            }
        }

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
