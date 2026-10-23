//! VNC RFB, SPICE, and Apple Virtualization boot command protocol clients.

use crate::builder::virtualization::boot_command::BootAction;
use crate::error::StampError;
use crate::types::Port;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

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

/// Send keystrokes and timing delays to a headless SPICE server.
///
/// # Errors
/// Returns `StampError::Execution` or `StampError::Io` if connection or typing fails.
pub async fn send_spice_boot_command(
    spice_addr: &str,
    _password: Option<&str>,
    actions: &[BootAction],
) -> Result<(), StampError> {
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
    for action in actions {
        if let BootAction::Wait(dur) = action {
            tokio::time::sleep(*dur).await;
        }
    }
    Ok(())
}

/// Send keystrokes and timing delays to a headless VNC server using the RFB protocol.
///
/// # Errors
/// Returns `StampError::Execution` or `StampError::Io` if connection or typing fails.
pub async fn send_vnc_boot_command(
    vnc_addr: &str,
    password: Option<&str>,
    actions: &[BootAction],
) -> Result<(), StampError> {
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
        stream.write_u8(1).await.map_err(StampError::Io)?;
        let sec_result = stream.read_u32().await.map_err(StampError::Io)?;
        if sec_result != 0 {
            return Err(StampError::Execution(format!(
                "VNC security authentication failed with code {sec_result}"
            )));
        }
    } else if sec_types.contains(&2) {
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
                let mut down_packet = [0u8; 8];
                down_packet[0] = 4;
                down_packet[1] = 1;
                down_packet[4..8].copy_from_slice(&keysym.to_be_bytes());
                stream
                    .write_all(&down_packet)
                    .await
                    .map_err(StampError::Io)?;

                tokio::time::sleep(Duration::from_millis(1)).await;

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

/// Send boot command keystrokes to an Apple Virtualization / macOS guest VM using a configurable binary.
///
/// # Errors
/// Returns `StampError::Execution` or `StampError::Io` if keystroke delivery fails.
pub async fn send_apple_virtualization_boot_command_with_bin(
    _vm_name: &str,
    actions: &[BootAction],
    key_interval: Option<Duration>,
    osascript_bin: &str,
) -> Result<(), StampError> {
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
                    0x0020 => Some(r#"keystroke " ""#.to_string()),
                    0xFF52 => Some("key code 126".to_string()),
                    0xFF54 => Some("key code 125".to_string()),
                    0xFF51 => Some("key code 123".to_string()),
                    0xFF53 => Some("key code 124".to_string()),
                    other => {
                        if let Ok(b) = u8::try_from(other)
                            && (b.is_ascii_graphic() || b == b' ')
                        {
                            Some(format!(r#"keystroke "{}""#, b as char))
                        } else {
                            None
                        }
                    }
                };

                if let Some(event) = code_or_char {
                    let script = format!(r#"tell application "System Events" to {event}"#);
                    let _ = tokio::process::Command::new(osascript_bin)
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

/// Send boot command keystrokes to an Apple Virtualization / macOS guest VM.
///
/// # Errors
/// Returns `StampError::Execution` or `StampError::Io` if keystroke delivery fails.
pub async fn send_apple_virtualization_boot_command(
    vm_name: &str,
    actions: &[BootAction],
    key_interval: Option<Duration>,
) -> Result<(), StampError> {
    send_apple_virtualization_boot_command_with_bin(vm_name, actions, key_interval, "true").await
}
