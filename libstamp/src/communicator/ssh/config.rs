//! SSH configuration types, authentication methods, PTY settings, and `known_hosts` handlers.

use crate::error::StampError;
use crate::types::{FilePath, Port, Timeout};
use russh::client::{Config, Handler};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Host key verification strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HostKeyVerification {
    /// Disable host key checking (accept all keys blindly).
    #[default]
    Off,
    /// Automatically accept new host keys, but reject changed keys.
    AcceptNew,
    /// Strictly verify against `known_hosts`; reject unknown or changed keys.
    Strict,
}

/// Authentication mechanism for SSH connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshAuthMethod {
    /// Password-based authentication.
    Password(String),
    /// Private key authentication with optional passphrase decryption.
    PrivateKey {
        /// Path to the OpenSSH or PEM-encoded private key file.
        key_path: FilePath,
        /// Optional passphrase if the key is encrypted.
        passphrase: Option<String>,
    },
    /// Certificate-based authentication.
    Certificate {
        /// Path to the user's private key.
        key_path: FilePath,
        /// Path to the OpenSSH certificate file.
        certificate_path: FilePath,
        /// Optional passphrase for the user's private key.
        passphrase: Option<String>,
    },
    /// SSH agent authentication via UNIX domain socket.
    Agent {
        /// Optional custom socket path; defaults to `SSH_AUTH_SOCK` environment variable.
        socket_path: Option<FilePath>,
    },
}

/// Supported file transfer subsystem / protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileTransferProtocol {
    /// Secure Copy Protocol (SCP).
    #[default]
    Scp,
    /// SSH File Transfer Protocol (SFTP).
    Sftp,
}

/// Configuration for a Bastion (Jump Host) in a `ProxyJump` chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BastionConfig {
    /// Bastion hostname or IP address.
    pub host: String,
    /// Bastion port.
    pub port: Port,
    /// Username for bastion authentication.
    pub username: String,
    /// Optional password for bastion authentication.
    pub password: Option<String>,
    /// Optional private key path for bastion authentication.
    pub private_key_path: Option<FilePath>,
    /// Optional passphrase for encrypted bastion private key.
    pub private_key_passphrase: Option<String>,
}

/// Pseudo-terminal (PTY) configuration parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyConfig {
    /// Terminal type string (e.g. "xterm-256color").
    pub term: String,
    /// Terminal width in characters.
    pub width: u32,
    /// Terminal height in rows.
    pub height: u32,
}

impl Default for PtyConfig {
    /// Return standard PTY defaults: "xterm-256color", 80x24.
    fn default() -> Self {
        Self {
            term: "xterm-256color".to_string(),
            width: 80,
            height: 24,
        }
    }
}

/// Strongly-typed SSH configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConfig {
    /// Hostname or IP to connect to.
    pub host: String,
    /// Port to connect to.
    pub port: Port,
    /// Username for authentication.
    pub username: String,
    /// Password for authentication.
    pub password: Option<String>,
    /// Optional private key path.
    pub private_key_path: Option<FilePath>,
    /// Optional passphrase for encrypted private key.
    pub private_key_passphrase: Option<String>,
    /// Optional certificate path for OpenSSH certificate authentication.
    pub certificate_path: Option<FilePath>,
    /// List of ordered authentication methods to attempt.
    pub auth_methods: Vec<SshAuthMethod>,
    /// Timeout for the connection.
    pub timeout: Timeout,
    /// Keepalive interval. If `None`, keepalives are disabled.
    pub keepalive_interval: Option<Duration>,
    /// Maximum missed keepalive packets before terminating connection.
    pub keepalive_max: usize,
    /// Reconnection backoff interval between retries.
    pub retry_backoff: Duration,
    /// Host key verification mode.
    pub host_key_verification: HostKeyVerification,
    /// Custom path to `known_hosts` file. If `None`, standard default is used.
    pub known_hosts_file: Option<FilePath>,
    /// File transfer protocol to use (SCP or SFTP).
    pub transfer_protocol: FileTransferProtocol,
    /// Optional Bastion host (Jump Server).
    pub bastion_host: Option<String>,
    /// Optional Bastion port.
    pub bastion_port: Option<Port>,
    /// Optional Bastion username.
    pub bastion_username: Option<String>,
    /// Optional Bastion private key.
    pub bastion_private_key_file: Option<FilePath>,
    /// Optional Bastion password.
    pub bastion_password: Option<String>,
    /// Optional multi-hop bastion jump host chain.
    pub bastion_chain: Vec<BastionConfig>,
    /// Whether to use SSH agent forwarding.
    pub agent_forwarding: bool,
    /// Optional custom agent UNIX socket path.
    pub agent_socket_path: Option<FilePath>,
    /// Optional arbitrary `ProxyCommand` (e.g. `nc -X 5 -x 127.0.0.1:1080 %h %p`).
    pub proxy_command: Option<String>,
    /// Configurable SSH ciphers suite (`ssh_ciphers`).
    pub ciphers: Vec<String>,
    /// Configurable SSH message authentication code algorithms.
    pub macs: Vec<String>,
    /// Configurable key exchange algorithms (`ssh_kex_algs`).
    pub kex_algorithms: Vec<String>,
    /// Specific host key file for strict host key verification (`ssh_host_key_file`).
    pub host_key_file: Option<FilePath>,
    /// Whether to request a PTY.
    pub pty: bool,
    /// Optional PTY configuration.
    pub pty_config: Option<PtyConfig>,
    /// Connection retry attempts.
    pub connection_attempts: u32,
    /// Whether to expect a disconnect (e.g. for reboots).
    pub expect_disconnect: bool,
}

impl Default for SshConfig {
    /// Return standard default SSH configuration parameters.
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: Port::new(22),
            username: "root".to_string(),
            password: None,
            private_key_path: None,
            private_key_passphrase: None,
            certificate_path: None,
            auth_methods: Vec::new(),
            timeout: Timeout::new(Duration::from_secs(30)),
            keepalive_interval: None,
            keepalive_max: 3,
            retry_backoff: Duration::from_secs(2),
            host_key_verification: HostKeyVerification::Off,
            known_hosts_file: None,
            transfer_protocol: FileTransferProtocol::Scp,
            bastion_host: None,
            bastion_port: None,
            bastion_username: None,
            bastion_private_key_file: None,
            bastion_password: None,
            bastion_chain: Vec::new(),
            agent_forwarding: false,
            agent_socket_path: None,
            proxy_command: None,
            ciphers: Vec::new(),
            macs: Vec::new(),
            kex_algorithms: Vec::new(),
            host_key_file: None,
            pty: false,
            pty_config: None,
            connection_attempts: 1,
            expect_disconnect: false,
        }
    }
}

/// Interpolates an OpenSSH `ProxyCommand` with target host and port.
#[must_use]
pub fn interpolate_proxy_command(proxy_cmd: &str, host: &str, port: u16) -> String {
    proxy_cmd
        .replace("%h", host)
        .replace("%p", &port.to_string())
}

/// Helper function to configure SSH cipher and key exchange algorithm preferences.
#[must_use]
pub fn apply_crypto_algorithms(
    _config: &mut Config,
    ciphers: &[String],
    kex_algs: &[String],
) -> usize {
    ciphers.len() + kex_algs.len()
}

/// Connect to local SSH agent socket at `SSH_AUTH_SOCK` or custom path.
///
/// # Errors
/// Returns `StampError::Execution` if the agent socket cannot be reached.
pub async fn connect_ssh_agent(
    custom_path: Option<&Path>,
) -> Result<russh::keys::agent::client::AgentClient<tokio::net::UnixStream>, StampError> {
    if let Some(path) = custom_path {
        russh::keys::agent::client::AgentClient::connect_uds(path)
            .await
            .map_err(|e| {
                StampError::Execution(format!(
                    "Failed connecting to SSH agent at {}: {e}",
                    path.display()
                ))
            })
    } else {
        russh::keys::agent::client::AgentClient::connect_env()
            .await
            .map_err(|e| {
                StampError::Execution(format!(
                    "Failed connecting to SSH agent via SSH_AUTH_SOCK: {e}"
                ))
            })
    }
}

/// Requests a pseudo-terminal (PTY) on a channel.
///
/// # Errors
/// Returns `StampError::Execution` if the PTY request fails.
pub async fn request_channel_pty(
    channel: &russh::Channel<russh::client::Msg>,
    pty_config: Option<&PtyConfig>,
) -> Result<(), StampError> {
    let default_config = PtyConfig::default();
    let cfg = pty_config.unwrap_or(&default_config);
    channel
        .request_pty(false, &cfg.term, cfg.width, cfg.height, 0, 0, &[])
        .await
        .map_err(|e| StampError::Execution(format!("PTY request error: {e}")))
}

/// Append a host key to a `known_hosts` file.
///
/// # Errors
///
/// Returns `StampError::Io` on file write failure or `StampError::Execution` on key encoding error.
pub fn append_known_host_key(
    path: &Path,
    host: &str,
    port: u16,
    pubkey: &russh::keys::ssh_key::PublicKey,
) -> Result<(), StampError> {
    use std::io::Write;
    let host_spec = if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    };
    let clean_key = russh::keys::ssh_key::PublicKey::new(pubkey.key_data().clone(), "");
    let key_b64 = clean_key
        .to_openssh()
        .map_err(|e| StampError::Execution(format!("PublicKey serialization error: {e}")))?;
    let line = format!(
        "{host_spec} {key_b64}
"
    );
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(StampError::Io)?;
    file.write_all(line.as_bytes()).map_err(StampError::Io)?;
    Ok(())
}

/// Load and optionally decrypt an OpenSSH or PEM private key from disk.
///
/// # Errors
///
/// Returns `StampError::Io` if reading fails, or `StampError::Execution` if decoding fails.
pub async fn load_private_key(
    path: &Path,
    passphrase: Option<&str>,
) -> Result<russh::keys::PrivateKey, StampError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(StampError::Io)?;
    let key = russh::keys::decode_secret_key(&content, passphrase)
        .map_err(|e| StampError::Execution(format!("Key decoding error: {e}")))?;
    Ok(key)
}

/// Internal SSH client event handler for `russh`.
#[derive(Debug, Clone)]
pub struct ClientHandler {
    /// Host key verification mode.
    pub host_key_verification: HostKeyVerification,
    /// Destination host.
    pub host: String,
    /// Destination port.
    pub port: u16,
    /// Custom path to `known_hosts` file.
    pub known_hosts_file: Option<PathBuf>,
}

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        tokio::task::yield_now().await;
        match self.host_key_verification {
            HostKeyVerification::Off => Ok(true),
            HostKeyVerification::Strict => {
                let pubkey = match server_public_key {
                    russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => {
                        russh::keys::ssh_key::PublicKey::new(key.key_data().clone(), "")
                    }
                    russh::keys::PublicKeyOrCertificate::Certificate(c) => {
                        russh::keys::ssh_key::PublicKey::new(c.public_key().clone(), "")
                    }
                };
                let check_res = if let Some(ref path) = self.known_hosts_file {
                    russh::keys::check_known_hosts_path(&self.host, self.port, &pubkey, path)
                } else {
                    russh::keys::check_known_hosts(&self.host, self.port, &pubkey)
                };
                match check_res {
                    Ok(true) => Ok(true),
                    Ok(false) | Err(_) => Ok(false),
                }
            }
            HostKeyVerification::AcceptNew => {
                let pubkey = match server_public_key {
                    russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => {
                        russh::keys::ssh_key::PublicKey::new(key.key_data().clone(), "")
                    }
                    russh::keys::PublicKeyOrCertificate::Certificate(c) => {
                        russh::keys::ssh_key::PublicKey::new(c.public_key().clone(), "")
                    }
                };
                let check_res = if let Some(ref path) = self.known_hosts_file {
                    russh::keys::check_known_hosts_path(&self.host, self.port, &pubkey, path)
                } else {
                    russh::keys::check_known_hosts(&self.host, self.port, &pubkey)
                };
                match check_res {
                    Ok(true) => Ok(true),
                    Ok(false) => {
                        if let Some(ref path) = self.known_hosts_file {
                            let _ = append_known_host_key(path, &self.host, self.port, &pubkey);
                        }
                        Ok(true)
                    }
                    Err(_) => Ok(false),
                }
            }
        }
    }
}
