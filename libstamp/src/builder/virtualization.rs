//! Shared strong types, filesystem generators, and drivers for Virtualization builders.
//!
//! Provides in-memory FAT12 Virtual Floppy Disk (VFD) generation, ISO9660 / Cloud-Init
//! CD-ROM generation with Rock Ridge extensions, VNC RFB protocol client, and boot command macro parsing.

/// Boot command macro parser and keyboard layout translation.
pub mod boot_command;
/// FAT12 1.44 MB Virtual Floppy Disk (VFD) filesystem generator.
pub mod fat12;
/// ISO9660 Level 3 + Joliet + Rock Ridge CD-ROM filesystem generator.
pub mod iso9660;
/// VNC RFB, SPICE, and Apple Virtualization boot command protocol clients.
pub mod vnc;

pub use boot_command::{BootAction, BootCommandParser, KeyboardLayout};
pub use fat12::{
    FAT12_DATA_CLUSTER_COUNT, FAT12_FIRST_DATA_SECTOR, FAT12_FLOPPY_SIZE, FAT12_MAX_ROOT_ENTRIES,
    FAT12_NUM_FATS, FAT12_RESERVED_SECTORS, FAT12_ROOT_DIR_SECTORS, FAT12_SECTOR_SIZE,
    FAT12_SECTORS_PER_FAT, FAT12_TOTAL_SECTORS, Fat12File, Fat12FloppyDisk, generate_floppy_disk,
    generate_floppy_disk_with_dirs,
};
pub use iso9660::{
    ISO_JOLIET_PATH_TABLE_L_SECTOR, ISO_JOLIET_PATH_TABLE_M_SECTOR, ISO_PATH_TABLE_L_SECTOR,
    ISO_PATH_TABLE_M_SECTOR, ISO_PVD_SECTOR, ISO_ROOT_DIR_SECTOR, ISO_SECTOR_SIZE, ISO_SVD_SECTOR,
    ISO_TERMINATOR_SECTOR, Iso9660Disk, IsoFile, encode_ucs2be, encode_ucs2be_padded,
    generate_cdrom_iso, generate_cdrom_iso_with_content, generate_cdrom_iso_with_string_content,
    generate_cloud_init_cidata_iso,
};
pub use vnc::{
    VncConfig, send_apple_virtualization_boot_command,
    send_apple_virtualization_boot_command_with_bin, send_spice_boot_command,
    send_vnc_boot_command,
};

use std::time::Duration;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StampError;
    use crate::types::Port;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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

        floppy.add_file(
            "ks.cfg",
            b"lang en_US
keyboard us
",
        )?;
        floppy.add_file(
            "preseed.cfg",
            b"d-i debian-installer/locale string en_US
",
        )?;
        floppy.add_file(
            "autounattend.xml",
            br#"<?xml version="1.0" encoding="utf-8"?><unattend></unattend>"#,
        )?;

        let image = floppy.generate()?;
        assert_eq!(image.len(), FAT12_FLOPPY_SIZE);

        assert_eq!(image[510], 0x55);
        assert_eq!(image[511], 0xAA);
        assert_eq!(&image[3..11], b"STAMP1.0");
        assert_eq!(image[21], 0xF0);
        assert_eq!(&image[512..515], &[0xF0, 0xFF, 0xFF]);
        assert_eq!(&image[5120..5123], &[0xF0, 0xFF, 0xFF]);

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

        let large_content = vec![b'A'; 1500];
        floppy.add_file("large.txt", &large_content)?;
        floppy.add_file("empty.txt", &[])?;

        let image = floppy.generate()?;
        assert_eq!(image.len(), FAT12_FLOPPY_SIZE);

        assert!(floppy.add_file("large.txt", b"new").is_err());
        assert!(Fat12FloppyDisk::to_dos_8_3("").is_err());

        Ok(())
    }

    #[test]
    fn test_fat12_capacity_limits() {
        let mut floppy = Fat12FloppyDisk::new("OVERFLOW");
        let oversized = vec![b'X'; 1_500_000];
        assert!(floppy.add_file("huge.bin", &oversized).is_err());
    }

    #[tokio::test]
    async fn test_generate_floppy_disk_helper() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let src_file = temp_dir.path().join("ks.cfg");
        tokio::fs::write(&src_file, b"install content").await?;

        let dest_img = temp_dir.path().join("floppy.img");
        generate_floppy_disk(&[src_file.to_string_lossy().to_string()], &dest_img).await?;

        assert!(dest_img.exists());
        let meta = tokio::fs::metadata(&dest_img).await?;
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
        )?;
        iso.add_file(
            "meta-data",
            b"instance-id: i-123456
",
        )?;
        iso.add_file(
            "network-config",
            b"version: 2
ethernets: {}
",
        )?;

        let image = iso.generate()?;
        assert!(image.len() >= 24 * ISO_SECTOR_SIZE);

        let pvd_offset = 16 * ISO_SECTOR_SIZE;
        assert_eq!(image[pvd_offset], 0x01);
        assert_eq!(&image[pvd_offset + 1..pvd_offset + 6], b"CD001");
        assert_eq!(image[pvd_offset + 6], 0x01);
        assert_eq!(&image[pvd_offset + 40..pvd_offset + 46], b"cidata");

        let svd_offset = 17 * ISO_SECTOR_SIZE;
        assert_eq!(image[svd_offset], 0x02);
        assert_eq!(&image[svd_offset + 1..svd_offset + 6], b"CD001");
        assert_eq!(&image[svd_offset + 88..svd_offset + 91], b"%/E");

        let term_offset = 18 * ISO_SECTOR_SIZE;
        assert_eq!(image[term_offset], 0xFF);
        assert_eq!(&image[term_offset + 1..term_offset + 6], b"CD001");

        let root_offset = 23 * ISO_SECTOR_SIZE;
        assert_eq!(image[root_offset], 34);
        assert_eq!(image[root_offset + 34], 34);

        let joliet_root_offset = 24 * ISO_SECTOR_SIZE;
        assert_eq!(image[joliet_root_offset], 34);
        assert_eq!(image[joliet_root_offset + 34], 34);

        Ok(())
    }

    #[test]
    fn test_iso9660_subdirectories_and_joliet() -> Result<(), StampError> {
        let mut iso = Iso9660Disk::new("OEMDRV");
        assert_eq!(iso.volume_label, "OEMDRV");

        iso.add_file("Autounattend.xml", b"<xml>Autounattend</xml>")?;
        iso.add_file("SetupComplete.cmd", b"REM SetupComplete")?;
        iso.add_file(
            "viostor/w11/ARM64/viostor.inf",
            br#"[Version]
Signature="$WINDOWS NT$""#,
        )?;
        iso.add_file("viostor/viostor.cat", b"CAT_DATA")?;
        iso.add_file("NetKVM/w11/ARM64/netkvm.inf", b"NETKVM_INF")?;
        iso.add_file("///---/file.txt", b"SPECIAL")?;
        iso.add_file("even12/odd1/test.bin", b"DATA")?;

        let image = iso.generate()?;

        let pvd_offset = 16 * ISO_SECTOR_SIZE;
        assert_eq!(image[pvd_offset], 0x01);
        assert_eq!(&image[pvd_offset + 40..pvd_offset + 46], b"OEMDRV");

        let svd_offset = 17 * ISO_SECTOR_SIZE;
        assert_eq!(image[svd_offset], 0x02);
        assert_eq!(&image[svd_offset + 88..svd_offset + 91], b"%/E");

        let term_offset = 18 * ISO_SECTOR_SIZE;
        assert_eq!(image[term_offset], 0xFF);

        let ucs2 = encode_ucs2be("OEMDRV");
        assert_eq!(ucs2.len(), 12);
        let padded = encode_ucs2be_padded("OEMDRV", 16);
        assert_eq!(padded.len(), 32);

        Ok(())
    }

    #[tokio::test]
    async fn test_generate_cdrom_iso_directory_recursion_and_content_maps() -> Result<(), StampError>
    {
        let temp_dir = tempfile::tempdir()?;
        let cidata_dir = temp_dir.path().join("cidata");
        let viostor_dir = cidata_dir.join("viostor").join("w11").join("ARM64");
        tokio::fs::create_dir_all(&viostor_dir).await?;
        let inf_file = viostor_dir.join("viostor.inf");
        tokio::fs::write(&inf_file, b"VIOSTOR_CONTENT").await?;

        let single_file = temp_dir.path().join("guest-tools.exe");
        tokio::fs::write(&single_file, b"EXE_DATA").await?;

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
        let meta = tokio::fs::metadata(&dest_iso).await?;
        assert!(meta.len() > 0);
        assert_eq!(meta.len() % (ISO_SECTOR_SIZE as u64), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_generate_cdrom_iso_helper() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let user_data = temp_dir.path().join("user-data");
        tokio::fs::write(&user_data, b"#cloud-config").await?;
        let meta_data = temp_dir.path().join("meta-data");
        tokio::fs::write(&meta_data, b"instance-id: id").await?;

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
        let meta = tokio::fs::metadata(&dest_iso).await?;
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

        assert_ne!(actions.len(), 0);

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
    async fn test_send_vnc_and_spice_boot_command_real_socket() -> Result<(), StampError> {
        let actions = vec![
            BootAction::Wait(Duration::from_millis(1)),
            BootAction::Key(0xFF0D),
            BootAction::KeyDown(0xFFE1),
            BootAction::KeyUp(0xFFE1),
            BootAction::Scancode(0x1C),
        ];

        // 1. Test successful VNC Security Type 1 (None) handshake and key events
        let vnc_listener = TcpListener::bind("127.0.0.1:0").await?;
        let vnc_addr = vnc_listener.local_addr()?.to_string();
        let server_task = tokio::spawn(async move {
            if let Ok((mut sock, _)) = vnc_listener.accept().await {
                let _ = sock
                    .write_all(
                        b"RFB 003.008
",
                    )
                    .await;
                let mut client_ver = [0u8; 12];
                let _ = sock.read_exact(&mut client_ver).await;
                // Security count = 1, type = 1 (None)
                let _ = sock.write_all(&[1, 1]).await;
                let _sec_choice = sock.read_u8().await;
                // Security result = 0 (OK)
                let _ = sock.write_u32(0).await;
                // ClientInit
                let _shared = sock.read_u8().await;
                // ServerInit: width(2), height(2), pixel_format(16), name_len(4), name("test")
                let _ = sock.write_u16(800).await;
                let _ = sock.write_u16(600).await;
                let _ = sock.write_all(&[0u8; 16]).await;
                let _ = sock.write_u32(4).await;
                let _ = sock.write_all(b"test").await;
                // Read 4 key packets (Key produces 2, KeyDown 1, KeyUp 1)
                let mut key_buf = [0u8; 32];
                let _ = sock.read_exact(&mut key_buf).await;
            }
        });

        send_vnc_boot_command(&vnc_addr, None, &actions).await?;
        let _ = server_task.await;

        // 2. Test VNC Security negotiation rejection (sec_count == 0)
        let vnc_reject = TcpListener::bind("127.0.0.1:0").await?;
        let reject_addr = vnc_reject.local_addr()?.to_string();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = vnc_reject.accept().await {
                let _ = sock
                    .write_all(
                        b"RFB 003.008
",
                    )
                    .await;
                let mut client_ver = [0u8; 12];
                let _ = sock.read_exact(&mut client_ver).await;
                let _ = sock.write_u8(0).await;
            }
        });
        assert!(
            send_vnc_boot_command(&reject_addr, None, &[])
                .await
                .is_err()
        );

        // 3. Test VNC Security Type 1 auth failure (sec_result != 0)
        let vnc_fail = TcpListener::bind("127.0.0.1:0").await?;
        let fail_addr = vnc_fail.local_addr()?.to_string();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = vnc_fail.accept().await {
                let _ = sock
                    .write_all(
                        b"RFB 003.008
",
                    )
                    .await;
                let mut client_ver = [0u8; 12];
                let _ = sock.read_exact(&mut client_ver).await;
                let _ = sock.write_all(&[1, 1]).await;
                let _ = sock.read_u8().await;
                let _ = sock.write_u32(1).await;
            }
        });
        assert!(send_vnc_boot_command(&fail_addr, None, &[]).await.is_err());

        // 4. Test VNC Security Type 2 (DES auth) with and without password
        let vnc_des = TcpListener::bind("127.0.0.1:0").await?;
        let des_addr = vnc_des.local_addr()?.to_string();
        tokio::spawn(async move {
            for _ in 0..2 {
                if let Ok((mut sock, _)) = vnc_des.accept().await {
                    let _ = sock
                        .write_all(
                            b"RFB 003.008
",
                        )
                        .await;
                    let mut client_ver = [0u8; 12];
                    let _ = sock.read_exact(&mut client_ver).await;
                    let _ = sock.write_all(&[1, 2]).await;
                    if sock.read_u8().await.is_ok() {
                        let _ = sock.write_all(&[0u8; 16]).await;
                    }
                }
            }
        });
        assert!(send_vnc_boot_command(&des_addr, None, &[]).await.is_err());
        assert!(
            send_vnc_boot_command(&des_addr, Some("pass"), &[])
                .await
                .is_err()
        );

        // 5. Test VNC Unsupported Security Type (e.g. 99)
        let vnc_unsup = TcpListener::bind("127.0.0.1:0").await?;
        let unsup_addr = vnc_unsup.local_addr()?.to_string();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = vnc_unsup.accept().await {
                let _ = sock
                    .write_all(
                        b"RFB 003.008
",
                    )
                    .await;
                let mut client_ver = [0u8; 12];
                let _ = sock.read_exact(&mut client_ver).await;
                let _ = sock.write_all(&[1, 99]).await;
            }
        });
        assert!(send_vnc_boot_command(&unsup_addr, None, &[]).await.is_err());

        // 6. Test SPICE boot command against local TCP listener
        let spice_listener = TcpListener::bind("127.0.0.1:0").await?;
        let spice_addr = spice_listener.local_addr()?.to_string();
        let spice_task = tokio::spawn(async move {
            if let Ok((mut sock, _)) = spice_listener.accept().await {
                let mut buf = [0u8; 12];
                let _ = sock.read_exact(&mut buf).await;
                assert_eq!(&buf[0..4], b"REDQ");
            }
        });
        send_spice_boot_command(&spice_addr, None, &actions).await?;
        let _ = spice_task.await;
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
            Some(Duration::from_millis(1)),
            Some(Duration::from_millis(2)),
        );
        assert_ne!(actions.len(), 0);
        let has_group_wait = actions
            .iter()
            .any(|a| matches!(a, BootAction::Wait(d) if *d == Duration::from_millis(2)));
        assert!(has_group_wait);

        let res = send_apple_virtualization_boot_command(
            "test-vm",
            &actions,
            Some(Duration::from_millis(1)),
        )
        .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_generate_cloud_init_cidata_iso_helper() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let dest = temp_dir.path().join("cidata_seed.iso");

        generate_cloud_init_cidata_iso(
            b"instance-id: i-999
local-hostname: myhost
",
            b"#cloud-config
users:
  - default
",
            Some(
                b"version: 2
",
            ),
            &dest,
        )
        .await?;

        assert!(dest.exists());
        let meta = tokio::fs::metadata(&dest).await?;
        assert!(meta.len() > 0);
        assert_eq!(meta.len() % (ISO_SECTOR_SIZE as u64), 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_generate_floppy_disk_with_dirs() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let sub_dir = temp_dir.path().join("floppy_sub");
        std::fs::create_dir_all(&sub_dir)?;
        let file_path = sub_dir.join("subfile.txt");
        std::fs::write(&file_path, b"subfile data")?;

        let dest = temp_dir.path().join("floppy_dirs.img");
        generate_floppy_disk_with_dirs(&[], &[sub_dir.to_string_lossy().to_string()], &dest)
            .await?;

        assert!(dest.exists());
        let meta = tokio::fs::metadata(&dest).await?;
        assert_eq!(meta.len(), FAT12_FLOPPY_SIZE as u64);
        Ok(())
    }

    #[test]
    fn test_virtualization_extra_coverage() -> Result<(), StampError> {
        let iso_def = Iso9660Disk::default();
        assert_eq!(iso_def.volume_label, "cidata");

        let mut iso = Iso9660Disk::new("test");
        iso.add_file("...", b"test")?;

        let mut floppy = Fat12FloppyDisk::new("test");
        for i in 0..FAT12_MAX_ROOT_ENTRIES {
            floppy.add_file(&format!("f{i}.txt"), b"x")?;
        }
        assert!(floppy.add_file("overflow.txt", b"x").is_err());

        let tokens = vec![
            "<wait10s>".to_string(),
            "<unknownTag>".to_string(),
            "<wait5>".to_string(),
            "<waitinvalid>".to_string(),
        ];
        let actions = BootCommandParser::parse(&tokens, None, None, None);
        assert_ne!(actions.len(), 0);

        assert_eq!(KeyboardLayout::De.translate_char('Y'), 'Z' as u32);
        assert_eq!(KeyboardLayout::De.translate_char('Z'), 'Y' as u32);
        assert_eq!(KeyboardLayout::De.translate_char('a'), 'a' as u32);

        assert_eq!(KeyboardLayout::Fr.translate_char('w'), 'z' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('z'), 'w' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('A'), 'Q' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('Q'), 'A' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('W'), 'Z' as u32);
        Ok(())
    }

    #[tokio::test]
    async fn test_virtualization_boot_command_and_iso_extra() -> Result<(), StampError> {
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

        let temp_dir = tempfile::tempdir()?;
        let dest = temp_dir.path().join("cidata_no_net.iso");
        generate_cloud_init_cidata_iso(b"inst", b"user", None, &dest).await?;
        Ok(())
    }

    #[test]
    fn test_virtualization_boot_command_extra() {
        let tokens = vec![
            "<fakekey>".to_string(),
            "<leftshiftoff>".to_string(),
            "<rightshifton>".to_string(),
            "<rightshiftoff>".to_string(),
            "<f99>".to_string(),
        ];
        let actions = BootCommandParser::parse(&tokens, None, None, None);
        assert_ne!(actions.len(), 0);
        assert_eq!(KeyboardLayout::Fr.translate_char('Z'), 'W' as u32);
        assert_eq!(KeyboardLayout::Fr.translate_char('p'), 'p' as u32);
    }

    #[tokio::test]
    async fn test_apple_virtualization_boot_command() {
        let actions = vec![
            BootAction::Wait(Duration::from_millis(1)),
            BootAction::Key(0xFF0D),
            BootAction::Key(0xFF09),
            BootAction::Key(0xFF1B),
            BootAction::Key(0xFF08),
            BootAction::Key(0x0020),
            BootAction::Key(0xFF52),
            BootAction::Key(0xFF54),
            BootAction::Key(0xFF51),
            BootAction::Key(0xFF53),
            BootAction::Key('a' as u32),
            BootAction::Key(0xFFFF),
            BootAction::KeyDown(0xFFE1),
        ];
        let res = send_apple_virtualization_boot_command(
            "test",
            &actions,
            Some(Duration::from_millis(1)),
        )
        .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_iso_builder_nested_dirs() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let iso_path = temp_dir.path().join("test_nested.iso");
        let mut iso = Iso9660Disk::new("NESTED");
        iso.add_file("deeply/nested/dir/file.txt", b"deep")?;
        iso.add_file("/absolute/path/file.txt", b"abs")?;
        iso.add_file("double//slash/file.txt", b"double")?;
        iso.write_to_file(&iso_path).await?;
        assert!(iso_path.exists());
        Ok(())
    }
}
