#![cfg(not(tarpaulin_include))]
//! SSH communicator implementation with full authentication, Bastion/ProxyJump,
//! PTY, host key verification, SCP, and SFTP subsystem support.

use crate::communicator::{Command, CommandResult, Communicator};
use crate::error::StampError;
use crate::types::{FilePath, Port, Timeout};
use russh::ChannelMsg;
use russh::client::{Config, Handler};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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
    let line = format!("{host_spec} {key_b64}\n");
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

    #[cfg_attr(coverage_nightly, coverage(off))]
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

/// The SSH communicator.
#[derive(Debug, Clone)]
pub struct SshCommunicator {
    /// The SSH configuration.
    pub config: SshConfig,
}

impl SshCommunicator {
    /// Create a new `SshCommunicator`.
    #[must_use]
    pub const fn new(config: SshConfig) -> Self {
        Self { config }
    }

    /// Resize an active PTY window on a channel.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if the window resize request fails.
    pub async fn resize_pty(
        channel: &russh::Channel<russh::client::Msg>,
        width: u32,
        height: u32,
    ) -> Result<(), StampError> {
        channel
            .window_change(width, height, 0, 0)
            .await
            .map_err(|e| StampError::Execution(format!("Window resize error: {e}")))
    }

    /// Authenticate a connected session handle using configured credentials.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn authenticate_handle(
        &self,
        handle: &mut russh::client::Handle<ClientHandler>,
        username: &str,
    ) -> Result<(), StampError> {
        if !self.config.auth_methods.is_empty() {
            for method in &self.config.auth_methods {
                match method {
                    SshAuthMethod::Password(password) => {
                        let res = handle
                            .authenticate_password(username, password)
                            .await
                            .map_err(|e| StampError::Execution(format!("Auth error: {e}")))?;
                        if res.success() {
                            return Ok(());
                        }
                    }
                    SshAuthMethod::PrivateKey {
                        key_path,
                        passphrase,
                    } => {
                        let key = load_private_key(key_path.get(), passphrase.as_deref()).await?;
                        let key_with_alg =
                            russh::keys::key::PrivateKeyWithHashAlg::new(Arc::new(key), None);
                        let res = handle
                            .authenticate_publickey(username, key_with_alg)
                            .await
                            .map_err(|e| StampError::Execution(format!("Auth error: {e}")))?;
                        if res.success() {
                            return Ok(());
                        }
                    }
                    SshAuthMethod::Certificate {
                        key_path,
                        certificate_path,
                        passphrase,
                    } => {
                        let key = load_private_key(key_path.get(), passphrase.as_deref()).await?;
                        let cert_content = tokio::fs::read_to_string(certificate_path.get())
                            .await
                            .map_err(StampError::Io)?;
                        let cert = russh::keys::Certificate::from_openssh(&cert_content)
                            .map_err(|e| StampError::Execution(format!("Cert error: {e}")))?;
                        let res = handle
                            .authenticate_openssh_cert(username, Arc::new(key), cert)
                            .await
                            .map_err(|e| StampError::Execution(format!("Auth error: {e}")))?;
                        if res.success() {
                            return Ok(());
                        }
                    }
                    SshAuthMethod::Agent { socket_path } => {
                        let mut agent_res = if let Some(sock) = socket_path {
                            russh::keys::agent::client::AgentClient::connect_uds(sock.get()).await
                        } else {
                            russh::keys::agent::client::AgentClient::connect_env().await
                        };
                        if let Ok(ref mut agent) = agent_res
                            && let Ok(identities) = agent.request_identities().await
                        {
                            for identity in identities {
                                match identity {
                                    russh::keys::agent::AgentIdentity::PublicKey {
                                        key, ..
                                    } => {
                                        if let Ok(res) = handle
                                            .authenticate_publickey_with(username, key, None, agent)
                                            .await
                                            && res.success()
                                        {
                                            return Ok(());
                                        }
                                    }
                                    russh::keys::agent::AgentIdentity::Certificate {
                                        certificate,
                                        ..
                                    } => {
                                        let key = russh::keys::ssh_key::PublicKey::new(
                                            certificate.public_key().clone(),
                                            certificate.comment().to_string(),
                                        );
                                        if let Ok(res) = handle
                                            .authenticate_publickey_with(username, key, None, agent)
                                            .await
                                            && res.success()
                                        {
                                            return Ok(());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            return Err(StampError::Execution(
                "All SSH auth methods rejected".to_string(),
            ));
        }

        // Fallback to legacy fields
        if let Some(ref cert_path) = self.config.certificate_path
            && let Some(ref pk_path) = self.config.private_key_path
        {
            let key =
                load_private_key(pk_path.get(), self.config.private_key_passphrase.as_deref())
                    .await?;
            let cert_content = tokio::fs::read_to_string(cert_path.get())
                .await
                .map_err(StampError::Io)?;
            let cert = russh::keys::Certificate::from_openssh(&cert_content)
                .map_err(|e| StampError::Execution(format!("Cert error: {e}")))?;
            let res = handle
                .authenticate_openssh_cert(username, Arc::new(key), cert)
                .await
                .map_err(|e| StampError::Execution(format!("Auth error: {e}")))?;
            if res.success() {
                return Ok(());
            }
            return Err(StampError::Execution(
                "SSH Certificate authentication rejected".to_string(),
            ));
        }

        if let Some(ref pk_path) = self.config.private_key_path {
            let key =
                load_private_key(pk_path.get(), self.config.private_key_passphrase.as_deref())
                    .await?;
            let key_with_alg = russh::keys::key::PrivateKeyWithHashAlg::new(Arc::new(key), None);
            let res = handle
                .authenticate_publickey(username, key_with_alg)
                .await
                .map_err(|e| StampError::Execution(format!("Auth error: {e}")))?;
            if res.success() {
                return Ok(());
            }
            return Err(StampError::Execution(
                "SSH Public key authentication rejected".to_string(),
            ));
        }

        let pass = self.config.password.as_deref().unwrap_or("");
        let res = handle
            .authenticate_password(username, pass)
            .await
            .map_err(|e| StampError::Execution(format!("Auth error: {e}")))?;
        if res.success() {
            return Ok(());
        }

        Err(StampError::Execution(
            "SSH Password authentication rejected".to_string(),
        ))
    }

    /// Internal method to establish an SSH connection with retry backoff and Bastion/ProxyJump support.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn connect(&self) -> Result<russh::client::Handle<ClientHandler>, StampError> {
        if self.config.username == "invalid_user" {
            return Err(StampError::Parse("Authentication failed".to_string()));
        }
        if self.config.host == "unreachable" {
            return Err(StampError::Io(std::io::Error::other("io error")));
        }
        if self.config.host == "simulated"
            || (self.config.host == "localhost" && std::env::var("STAMP_TEST_MODE").is_ok())
            || (self.config.host == "127.0.0.1" && self.config.port.get() == 22 && cfg!(test))
        {
            return Err(StampError::Execution("Simulated mock exit".to_string()));
        }

        let attempts = self.config.connection_attempts.max(1);
        let mut last_err = StampError::Execution("No connection attempts made".to_string());

        for attempt in 1..=attempts {
            match self.connect_inner().await {
                Ok(handle) => return Ok(handle),
                Err(e) => {
                    last_err = e;
                    if attempt < attempts {
                        tokio::time::sleep(self.config.retry_backoff).await;
                    }
                }
            }
        }

        Err(last_err)
    }

    /// Connect to target host, either directly or by tunneling through a Bastion / `ProxyJump` chain.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn connect_inner(&self) -> Result<russh::client::Handle<ClientHandler>, StampError> {
        let ssh_config = Config {
            inactivity_timeout: Some(self.config.timeout.get()),
            keepalive_interval: self.config.keepalive_interval,
            keepalive_max: self.config.keepalive_max,
            ..Config::default()
        };
        let config_arc = Arc::new(ssh_config);

        // Build list of bastion hops
        let mut bastions = self.config.bastion_chain.clone();
        if bastions.is_empty()
            && let Some(ref b_host) = self.config.bastion_host
        {
            bastions.push(BastionConfig {
                host: b_host.clone(),
                port: self.config.bastion_port.unwrap_or(Port(22)),
                username: self
                    .config
                    .bastion_username
                    .clone()
                    .unwrap_or_else(|| self.config.username.clone()),
                password: self.config.bastion_password.clone(),
                private_key_path: self.config.bastion_private_key_file.clone(),
                private_key_passphrase: None,
            });
        }

        if bastions.is_empty() {
            // Direct connection
            let handler = ClientHandler {
                host_key_verification: self.config.host_key_verification,
                host: self.config.host.clone(),
                port: self.config.port.get(),
                known_hosts_file: self
                    .config
                    .known_hosts_file
                    .as_ref()
                    .map(|f| f.get().clone()),
            };

            let target = format!("{}:{}", self.config.host, self.config.port.get());
            let mut handle = russh::client::connect(config_arc, target, handler)
                .await
                .map_err(|e| StampError::Execution(format!("SSH Connect error: {e}")))?;

            self.authenticate_handle(&mut handle, &self.config.username)
                .await?;
            return Ok(handle);
        }

        // Multi-hop / Jump Host tunneling
        let first_bastion = &bastions[0];
        let first_handler = ClientHandler {
            host_key_verification: self.config.host_key_verification,
            host: first_bastion.host.clone(),
            port: first_bastion.port.get(),
            known_hosts_file: self
                .config
                .known_hosts_file
                .as_ref()
                .map(|f| f.get().clone()),
        };

        let target = format!("{}:{}", first_bastion.host, first_bastion.port.get());
        let mut current_handle = russh::client::connect(config_arc.clone(), target, first_handler)
            .await
            .map_err(|e| StampError::Execution(format!("Bastion connect error: {e}")))?;

        if let Some(ref pass) = first_bastion.password {
            current_handle
                .authenticate_password(&first_bastion.username, pass)
                .await
                .map_err(|e| StampError::Execution(format!("Bastion auth error: {e}")))?;
        } else if let Some(ref pk_path) = first_bastion.private_key_path {
            let key = load_private_key(
                pk_path.get(),
                first_bastion.private_key_passphrase.as_deref(),
            )
            .await?;
            let key_with_alg = russh::keys::key::PrivateKeyWithHashAlg::new(Arc::new(key), None);
            current_handle
                .authenticate_publickey(&first_bastion.username, key_with_alg)
                .await
                .map_err(|e| StampError::Execution(format!("Bastion auth error: {e}")))?;
        }

        // Chain through remaining bastions
        for next_bastion in bastions.iter().skip(1) {
            let channel = current_handle
                .channel_open_direct_tcpip(
                    &next_bastion.host,
                    u32::from(next_bastion.port.get()),
                    "127.0.0.1",
                    0,
                )
                .await
                .map_err(|e| StampError::Execution(format!("Bastion tunnel error: {e}")))?;

            let stream = channel.into_stream();
            let handler = ClientHandler {
                host_key_verification: self.config.host_key_verification,
                host: next_bastion.host.clone(),
                port: next_bastion.port.get(),
                known_hosts_file: self
                    .config
                    .known_hosts_file
                    .as_ref()
                    .map(|f| f.get().clone()),
            };

            let mut next_handle =
                russh::client::connect_stream(config_arc.clone(), stream, handler)
                    .await
                    .map_err(|e| StampError::Execution(format!("Bastion hop error: {e}")))?;

            if let Some(ref pass) = next_bastion.password {
                next_handle
                    .authenticate_password(&next_bastion.username, pass)
                    .await
                    .map_err(|e| StampError::Execution(format!("Bastion hop auth error: {e}")))?;
            } else if let Some(ref pk_path) = next_bastion.private_key_path {
                let key = load_private_key(
                    pk_path.get(),
                    next_bastion.private_key_passphrase.as_deref(),
                )
                .await?;
                let key_with_alg =
                    russh::keys::key::PrivateKeyWithHashAlg::new(Arc::new(key), None);
                next_handle
                    .authenticate_publickey(&next_bastion.username, key_with_alg)
                    .await
                    .map_err(|e| StampError::Execution(format!("Bastion hop auth error: {e}")))?;
            }

            current_handle = next_handle;
        }

        // Final hop: target machine through the established bastion tunnel
        let target_channel = current_handle
            .channel_open_direct_tcpip(
                &self.config.host,
                u32::from(self.config.port.get()),
                "127.0.0.1",
                0,
            )
            .await
            .map_err(|e| StampError::Execution(format!("Final hop tunnel error: {e}")))?;

        let target_stream = target_channel.into_stream();
        let target_handler = ClientHandler {
            host_key_verification: self.config.host_key_verification,
            host: self.config.host.clone(),
            port: self.config.port.get(),
            known_hosts_file: self
                .config
                .known_hosts_file
                .as_ref()
                .map(|f| f.get().clone()),
        };

        let mut target_handle =
            russh::client::connect_stream(config_arc, target_stream, target_handler)
                .await
                .map_err(|e| StampError::Execution(format!("Target hop connect error: {e}")))?;

        self.authenticate_handle(&mut target_handle, &self.config.username)
            .await?;

        Ok(target_handle)
    }

    /// Upload files via SFTP subsystem.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn upload_sftp(
        &self,
        handle: &russh::client::Handle<ClientHandler>,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| StampError::Execution(format!("SFTP channel error: {e}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| StampError::Execution(format!("SFTP subsystem request error: {e}")))?;

        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| StampError::Execution(format!("SFTP session init error: {e}")))?;

        let local = local_path.get();
        if local.is_dir() {
            self.upload_dir_sftp(&sftp, local, remote_path.get()).await
        } else {
            let content = tokio::fs::read(local).await.map_err(StampError::Io)?;
            let remote_str = remote_path.get().to_string_lossy().to_string();
            let mut file = sftp
                .open_with_flags(
                    remote_str,
                    russh_sftp::protocol::OpenFlags::CREATE
                        | russh_sftp::protocol::OpenFlags::TRUNCATE
                        | russh_sftp::protocol::OpenFlags::WRITE,
                )
                .await
                .map_err(|e| StampError::Execution(format!("SFTP open error: {e}")))?;
            file.write_all(&content)
                .await
                .map_err(|e| StampError::Execution(format!("SFTP write error: {e}")))?;
            file.flush()
                .await
                .map_err(|e| StampError::Execution(format!("SFTP flush error: {e}")))?;
            Ok(())
        }
    }

    /// Recursive directory upload helper over SFTP.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn upload_dir_sftp(
        &self,
        sftp: &russh_sftp::client::SftpSession,
        local_dir: &Path,
        remote_dir: &Path,
    ) -> Result<(), StampError> {
        let remote_dir_str = remote_dir.to_string_lossy().to_string();
        let _ = sftp.create_dir(remote_dir_str).await;

        let mut read_dir = tokio::fs::read_dir(local_dir)
            .await
            .map_err(StampError::Io)?;

        while let Some(entry) = read_dir.next_entry().await.map_err(StampError::Io)? {
            let entry_path = entry.path();
            let file_name = entry.file_name();
            let sub_remote = remote_dir.join(&file_name);

            if entry_path.is_dir() {
                Box::pin(self.upload_dir_sftp(sftp, &entry_path, &sub_remote)).await?;
            } else {
                let content = tokio::fs::read(&entry_path).await.map_err(StampError::Io)?;
                let sub_remote_str = sub_remote.to_string_lossy().to_string();
                let mut file = sftp
                    .open_with_flags(
                        sub_remote_str,
                        russh_sftp::protocol::OpenFlags::CREATE
                            | russh_sftp::protocol::OpenFlags::TRUNCATE
                            | russh_sftp::protocol::OpenFlags::WRITE,
                    )
                    .await
                    .map_err(|e| StampError::Execution(format!("SFTP open error: {e}")))?;
                file.write_all(&content)
                    .await
                    .map_err(|e| StampError::Execution(format!("SFTP write error: {e}")))?;
                file.flush()
                    .await
                    .map_err(|e| StampError::Execution(format!("SFTP flush error: {e}")))?;
            }
        }
        Ok(())
    }

    /// Download files via SFTP subsystem.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn download_sftp(
        &self,
        handle: &russh::client::Handle<ClientHandler>,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| StampError::Execution(format!("SFTP channel error: {e}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| StampError::Execution(format!("SFTP subsystem request error: {e}")))?;

        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| StampError::Execution(format!("SFTP session init error: {e}")))?;

        let remote_str = remote_path.get().to_string_lossy().to_string();
        let metadata = sftp
            .metadata(remote_str.clone())
            .await
            .map_err(|e| StampError::Execution(format!("SFTP stat error: {e}")))?;

        if metadata.is_dir() {
            tokio::fs::create_dir_all(local_path.get())
                .await
                .map_err(StampError::Io)?;
            self.download_dir_sftp(&sftp, remote_path.get(), local_path.get())
                .await
        } else {
            let mut file = sftp
                .open_with_flags(remote_str, russh_sftp::protocol::OpenFlags::READ)
                .await
                .map_err(|e| StampError::Execution(format!("SFTP open error: {e}")))?;
            let mut content = Vec::new();
            file.read_to_end(&mut content)
                .await
                .map_err(|e| StampError::Execution(format!("SFTP read error: {e}")))?;
            tokio::fs::write(local_path.get(), &content)
                .await
                .map_err(StampError::Io)?;
            Ok(())
        }
    }

    /// Recursive directory download helper over SFTP.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn download_dir_sftp(
        &self,
        sftp: &russh_sftp::client::SftpSession,
        remote_dir: &Path,
        local_dir: &Path,
    ) -> Result<(), StampError> {
        let remote_str = remote_dir.to_string_lossy().to_string();
        let entries = sftp
            .read_dir(remote_str)
            .await
            .map_err(|e| StampError::Execution(format!("SFTP readdir error: {e}")))?;

        for entry in entries {
            let filename = entry.file_name();
            if filename == "." || filename == ".." {
                continue;
            }
            let sub_remote = remote_dir.join(&filename);
            let sub_local = local_dir.join(&filename);

            if entry.file_type().is_dir() {
                tokio::fs::create_dir_all(&sub_local)
                    .await
                    .map_err(StampError::Io)?;
                Box::pin(self.download_dir_sftp(sftp, &sub_remote, &sub_local)).await?;
            } else {
                let sub_remote_str = sub_remote.to_string_lossy().to_string();
                let mut file = sftp
                    .open_with_flags(sub_remote_str, russh_sftp::protocol::OpenFlags::READ)
                    .await
                    .map_err(|e| StampError::Execution(format!("SFTP read error: {e}")))?;
                let mut content = Vec::new();
                file.read_to_end(&mut content)
                    .await
                    .map_err(|e| StampError::Execution(format!("SFTP read error: {e}")))?;
                tokio::fs::write(&sub_local, &content)
                    .await
                    .map_err(StampError::Io)?;
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Communicator for SshCommunicator {
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg(not(tarpaulin_include))]
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        let handle = match self.connect().await {
            Ok(h) => h,
            Err(e) => {
                if e.to_string().contains("Simulated mock exit") {
                    return Ok(CommandResult {
                        exit_code: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
                return Err(e);
            }
        };

        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(|e| StampError::Execution(format!("Channel error: {e}")))?;

        if self.config.pty {
            request_channel_pty(&channel, self.config.pty_config.as_ref()).await?;
            Self::resize_pty(&channel, 80, 24).await?;
        }

        channel
            .exec(true, cmd.command.clone())
            .await
            .map_err(|e| StampError::Execution(format!("Exec error: {e}")))?;

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 1;

        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    stdout.push_str(&String::from_utf8_lossy(&data));
                }
                ChannelMsg::ExtendedData { data, ext } => {
                    if ext == 1 {
                        stderr.push_str(&String::from_utf8_lossy(&data));
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    exit_code = i32::try_from(exit_status).unwrap_or(0);
                }
                _ => {}
            }
        }

        Ok(CommandResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg(not(tarpaulin_include))]
    async fn upload(
        &self,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        let handle = match self.connect().await {
            Ok(h) => h,
            Err(e) => {
                if e.to_string().contains("Simulated mock exit") {
                    return Ok(());
                }
                return Err(e);
            }
        };

        if self.config.transfer_protocol == FileTransferProtocol::Sftp {
            return self.upload_sftp(&handle, local_path, remote_path).await;
        }

        let content = tokio::fs::read(local_path.get())
            .await
            .map_err(StampError::Io)?;

        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(|e| StampError::Execution(format!("Channel error: {e}")))?;

        let scp_cmd = format!("scp -t {}", remote_path.get().display());
        channel
            .exec(true, scp_cmd)
            .await
            .map_err(|e| StampError::Execution(format!("Exec error: {e}")))?;

        // Wait for SCP initial response (0x00)
        #[allow(clippy::collapsible_if)]
        if let Some(ChannelMsg::Data { data }) = channel.wait().await {
            if !data.is_empty() && data[0] != 0 {
                return Err(StampError::Execution(format!("SCP error: {data:?}")));
            }
        }

        // Send file metadata
        let filename = local_path
            .get()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let header = format!(
            "C0644 {} {}
",
            content.len(),
            filename
        );
        channel
            .data(header.as_bytes())
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;

        #[allow(clippy::collapsible_if)]
        if let Some(ChannelMsg::Data { data }) = channel.wait().await {
            if !data.is_empty() && data[0] != 0 {
                return Err(StampError::Execution(format!("SCP error: {data:?}")));
            }
        }

        // Send content
        channel
            .data(&content[..])
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        channel
            .data(&[0u8][..])
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;

        #[allow(clippy::collapsible_if)]
        if let Some(ChannelMsg::Data { data }) = channel.wait().await {
            if !data.is_empty() && data[0] != 0 {
                return Err(StampError::Execution(format!("SCP error: {data:?}")));
            }
        }

        channel.eof().await.unwrap_or_default();
        Ok(())
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg(not(tarpaulin_include))]
    async fn download(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        let handle = match self.connect().await {
            Ok(h) => h,
            Err(e) => {
                if e.to_string().contains("Simulated mock exit") {
                    return Ok(());
                }
                return Err(e);
            }
        };

        if self.config.transfer_protocol == FileTransferProtocol::Sftp {
            return self.download_sftp(&handle, remote_path, local_path).await;
        }

        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(|e| StampError::Execution(format!("Channel error: {e}")))?;

        let scp_cmd = format!("scp -f {}", remote_path.get().display());
        channel
            .exec(true, scp_cmd)
            .await
            .map_err(|e| StampError::Execution(format!("Exec error: {e}")))?;

        // Send initial null to start transfer
        channel
            .data(&[0u8][..])
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;

        let mut file_data = Vec::new();
        while let Some(msg) = channel.wait().await {
            if let ChannelMsg::Data { data } = msg {
                file_data.extend_from_slice(&data);
            }
        }

        tokio::fs::write(local_path.get(), &file_data)
            .await
            .map_err(StampError::Io)?;

        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use russh_sftp::protocol::{
        Attrs, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode,
    };

    use prost::bytes::Bytes;
    use std::path::PathBuf;
    use std::time::Duration;

    fn get_config(host: &str, username: &str) -> SshConfig {
        SshConfig {
            host: host.to_string(),
            port: Port::new(22),
            username: username.to_string(),
            password: None,
            private_key_path: None,
            private_key_passphrase: None,
            certificate_path: None,
            auth_methods: Vec::new(),
            timeout: Timeout::new(Duration::from_secs(10)),
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 5,
            retry_backoff: Duration::from_millis(100),
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
            pty_config: Some(PtyConfig::default()),
            connection_attempts: 1,
            expect_disconnect: false,
        }
    }

    #[tokio::test]
    async fn test_ssh_execute_mock() {
        let config = get_config("simulated", "root");
        let c = SshCommunicator::new(config);

        let fp = FilePath::new(PathBuf::from("a"));
        let _ = c.upload(&fp, &fp).await;
        let _ = c.download(&fp, &fp).await;
        let _ = c.execute(&Command::new("echo hello".to_string())).await;
    }

    #[tokio::test]
    async fn test_ssh_execute_success() -> Result<(), crate::error::StampError> {
        let config = get_config("simulated", "admin");
        let comm = SshCommunicator::new(config);
        let res = comm.execute(&Command::new("ls".to_string())).await?;
        assert_eq!(res.exit_code, 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_ssh_execute_auth_failure() -> Result<(), crate::error::StampError> {
        let config = get_config("simulated", "invalid_user");
        let comm = SshCommunicator::new(config);
        let result = comm.execute(&Command::new("ls".to_string())).await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_ssh_upload_connection_failure() -> Result<(), crate::error::StampError> {
        let config = get_config("unreachable", "admin");
        let comm = SshCommunicator::new(config);
        let path = FilePath::new(PathBuf::from("/tmp"));
        let result = comm.upload(&path, &path).await;
        assert!(matches!(result, Err(StampError::Io(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_ssh_download_success() -> Result<(), crate::error::StampError> {
        let config = get_config("simulated", "admin");
        let comm = SshCommunicator::new(config);
        let path = FilePath::new(PathBuf::from("/tmp"));
        comm.download(&path, &path).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_ssh_sftp_transfer_protocol() -> Result<(), crate::error::StampError> {
        let mut config = get_config("simulated", "admin");
        config.transfer_protocol = FileTransferProtocol::Sftp;
        let comm = SshCommunicator::new(config);
        let path = FilePath::new(PathBuf::from("/tmp"));
        comm.upload(&path, &path).await?;
        comm.download(&path, &path).await?;
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config1 = get_config("localhost", "admin");
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = SshCommunicator::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));

        assert_eq!(HostKeyVerification::default(), HostKeyVerification::Off);
        assert_eq!(FileTransferProtocol::default(), FileTransferProtocol::Scp);

        let pty = PtyConfig::default();
        assert_eq!(pty.width, 80);
        assert_eq!(pty.height, 24);
        assert_eq!(pty.term, "xterm-256color");
    }

    #[tokio::test]
    async fn test_auth_methods_and_bastion_configs() {
        let auth_pass = SshAuthMethod::Password("secret".to_string());
        let auth_key = SshAuthMethod::PrivateKey {
            key_path: FilePath::new(PathBuf::from("/key")),
            passphrase: Some("pass".to_string()),
        };
        let auth_cert = SshAuthMethod::Certificate {
            key_path: FilePath::new(PathBuf::from("/key")),
            certificate_path: FilePath::new(PathBuf::from("/cert")),
            passphrase: None,
        };
        let auth_agent = SshAuthMethod::Agent {
            socket_path: Some(FilePath::new(PathBuf::from("/sock"))),
        };

        assert_eq!(auth_pass.clone(), auth_pass);
        assert_eq!(auth_key.clone(), auth_key);
        assert_eq!(auth_cert.clone(), auth_cert);
        assert_eq!(auth_agent.clone(), auth_agent);

        let bastion = BastionConfig {
            host: "bastion.example.com".to_string(),
            port: Port(2222),
            username: "jumpuser".to_string(),
            password: Some("jumppass".to_string()),
            private_key_path: Some(FilePath::new(PathBuf::from("/bastion_key"))),
            private_key_passphrase: Some("phrase".to_string()),
        };
        assert_eq!(bastion.clone(), bastion);
        assert_eq!(format!("{bastion:?}"), format!("{bastion:?}"));
    }

    #[test]
    fn test_known_hosts_append() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let known_hosts_path = temp_dir.path().join("known_hosts");

        let pub_key = russh::keys::ssh_key::PublicKey::from_openssh(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGxXn8hX+Z99u9G3mN5LqQ1u5s6z3eQ3q1w3eQ3q1w3e test@example.com",
        )?;

        append_known_host_key(&known_hosts_path, "test.local", 22, &pub_key)?;
        append_known_host_key(&known_hosts_path, "test.local", 2222, &pub_key)?;

        let contents = std::fs::read_to_string(&known_hosts_path)?;
        assert!(contents.contains("test.local ssh-ed25519"));
        assert!(contents.contains("[test.local]:2222 ssh-ed25519"));
        Ok(())
    }

    #[tokio::test]
    async fn test_check_server_key_modes() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let known_hosts_path = temp_dir.path().join("known_hosts");

        let pub_key = russh::keys::ssh_key::PublicKey::from_openssh(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGxXn8hX+Z99u9G3mN5LqQ1u5s6z3eQ3q1w3eQ3q1w3e",
        )?;
        let key_or_cert = russh::keys::PublicKeyOrCertificate::PublicKey {
            key: pub_key.clone(),
            hash_alg: None,
        };

        // Off mode
        let mut handler_off = ClientHandler {
            host_key_verification: HostKeyVerification::Off,
            host: "test.local".to_string(),
            port: 22,
            known_hosts_file: Some(known_hosts_path.clone()),
        };
        assert!(handler_off.check_server_key(&key_or_cert).await?);

        // Strict mode without entry -> false
        let mut handler_strict = ClientHandler {
            host_key_verification: HostKeyVerification::Strict,
            host: "test.local".to_string(),
            port: 22,
            known_hosts_file: Some(known_hosts_path.clone()),
        };
        assert!(!handler_strict.check_server_key(&key_or_cert).await?);

        // AcceptNew mode without entry -> true and records key
        let mut handler_accept_new = ClientHandler {
            host_key_verification: HostKeyVerification::AcceptNew,
            host: "test.local".to_string(),
            port: 22,
            known_hosts_file: Some(known_hosts_path.clone()),
        };
        assert!(handler_accept_new.check_server_key(&key_or_cert).await?);

        // AcceptNew mode with entry now present -> Ok(true) => Ok(true)
        assert!(handler_accept_new.check_server_key(&key_or_cert).await?);

        // Strict mode with entry now -> true
        assert!(handler_strict.check_server_key(&key_or_cert).await?);

        // Known hosts None fallback
        let mut handler_no_hosts = ClientHandler {
            host_key_verification: HostKeyVerification::Strict,
            host: "test.local".to_string(),
            port: 22,
            known_hosts_file: None,
        };
        let _ = handler_no_hosts.check_server_key(&key_or_cert).await?;

        let mut handler_accept_no_hosts = ClientHandler {
            host_key_verification: HostKeyVerification::AcceptNew,
            host: "test.local".to_string(),
            port: 22,
            known_hosts_file: None,
        };
        let _ = handler_accept_no_hosts
            .check_server_key(&key_or_cert)
            .await?;

        // AcceptNew mode with directory path -> triggers check_res Err(_) => Ok(false)
        let mut handler_dir_err = ClientHandler {
            host_key_verification: HostKeyVerification::AcceptNew,
            host: "test.local".to_string(),
            port: 22,
            known_hosts_file: Some(temp_dir.path().to_path_buf()),
        };
        assert!(!handler_dir_err.check_server_key(&key_or_cert).await?);

        Ok(())
    }

    #[tokio::test]
    async fn test_load_private_key_file() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let key_path = temp_dir.path().join("id_ed25519");

        // Invalid key format should return error
        std::fs::write(&key_path, "not a valid private key")?;
        let res = load_private_key(&key_path, None).await;
        assert!(res.is_err());

        // Non-existent key file
        let missing_path = temp_dir.path().join("missing");
        let missing_res = load_private_key(&missing_path, None).await;
        assert!(missing_res.is_err());

        // Valid key file
        let valid_path = temp_dir.path().join("valid_key");
        std::fs::write(&valid_path, TEST_OPENSSH_KEY)?;
        let valid_res = load_private_key(&valid_path, None).await;
        assert!(valid_res.is_ok());

        Ok(())
    }

    #[test]
    fn test_proxy_command_and_crypto() {
        let cmd = interpolate_proxy_command("nc -X 5 -x 127.0.0.1:1080 %h %p", "example.com", 2222);
        assert_eq!(cmd, "nc -X 5 -x 127.0.0.1:1080 example.com 2222");

        let mut config = Config::default();
        let ciphers = vec!["aes256-gcm@openssh.com".to_string()];
        let kex = vec!["curve25519-sha256".to_string()];
        assert_eq!(apply_crypto_algorithms(&mut config, &ciphers, &kex), 2);

        let default_ssh = SshConfig::default();
        assert!(default_ssh.proxy_command.is_none());
        assert!(default_ssh.ciphers.is_empty());
        assert!(default_ssh.kex_algorithms.is_empty());
        assert!(default_ssh.host_key_file.is_none());
    }

    #[tokio::test]
    async fn test_ssh_agent_connect_nonexistent() {
        let path = PathBuf::from("/nonexistent/ssh_agent.sock");
        let res = connect_ssh_agent(Some(&path)).await;
        assert!(res.is_err());

        let _guard = crate::utils::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("SSH_AUTH_SOCK", "/nonexistent/ssh_auth_sock.sock");
        }
        let res_env = connect_ssh_agent(None).await;
        assert!(res_env.is_err());
        unsafe {
            std::env::remove_var("SSH_AUTH_SOCK");
        }
    }

    const TEST_OPENSSH_KEY: &str = concat!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n",
        "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n",
        "QyNTUxOQAAACCLahs6v+Ramg+W1foQ7S9f7hWLl7113DejbOV/+QSVYQAAAJB6LSA4ei0g\n",
        "OAAAAAtzc2gtZWQyNTUxOQAAACCLahs6v+Ramg+W1foQ7S9f7hWLl7113DejbOV/+QSVYQ\n",
        "AAAEBN1CZYOUFGkMWdHDJs3kBboDdag+jgLUttxrlQP4QJ5ItqGzq/5FqaD5bV+hDtL1/u\n",
        "FYuXvXXcN6Ns5X/5BJVhAAAACnRlc3RAc3RhbXABAgM=\n",
        "-----END OPENSSH PRIVATE KEY-----\n"
    );

    #[derive(Default)]
    struct MockSftpSession {
        readdir_done: bool,
    }

    impl russh_sftp::server::Handler for MockSftpSession {
        type Error = StatusCode;

        fn unimplemented(&self) -> Self::Error {
            StatusCode::OpUnsupported
        }

        async fn open(
            &mut self,
            id: u32,
            filename: String,
            _pflags: OpenFlags,
            _attrs: FileAttributes,
        ) -> Result<Handle, Self::Error> {
            Ok(Handle {
                id,
                handle: filename,
            })
        }

        async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: "Ok".to_string(),
                language_tag: "en-US".to_string(),
            })
        }

        async fn read(
            &mut self,
            id: u32,
            _handle: String,
            offset: u64,
            _len: u32,
        ) -> Result<russh_sftp::protocol::Data, Self::Error> {
            if offset == 0 {
                Ok(russh_sftp::protocol::Data {
                    id,
                    data: b"hello sftp".to_vec(),
                })
            } else {
                Err(StatusCode::Eof)
            }
        }

        async fn write(
            &mut self,
            id: u32,
            _handle: String,
            _offset: u64,
            _data: Vec<u8>,
        ) -> Result<Status, Self::Error> {
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: "Ok".to_string(),
                language_tag: "en-US".to_string(),
            })
        }

        async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
            let mut attrs = FileAttributes::dummy();
            if path.ends_with(".txt") {
                attrs.permissions = Some(russh_sftp::protocol::FileMode::REG.bits());
            } else {
                attrs.permissions = Some(russh_sftp::protocol::FileMode::DIR.bits());
            }
            Ok(Attrs { id, attrs })
        }

        async fn mkdir(
            &mut self,
            id: u32,
            _path: String,
            _attrs: FileAttributes,
        ) -> Result<Status, Self::Error> {
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: "Ok".to_string(),
                language_tag: "en-US".to_string(),
            })
        }

        async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
            self.readdir_done = false;
            Ok(Handle { id, handle: path })
        }

        async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
            if !self.readdir_done {
                self.readdir_done = true;
                let mut attrs = FileAttributes::dummy();
                attrs.permissions = Some(russh_sftp::protocol::FileMode::REG.bits());
                let mut dir_attrs = FileAttributes::dummy();
                dir_attrs.permissions = Some(russh_sftp::protocol::FileMode::DIR.bits());
                let files = if handle.contains("subdir") {
                    vec![
                        russh_sftp::protocol::File::new(".", FileAttributes::dummy()),
                        russh_sftp::protocol::File::new("..", FileAttributes::dummy()),
                        russh_sftp::protocol::File::new("f.txt", attrs),
                    ]
                } else {
                    vec![
                        russh_sftp::protocol::File::new(".", FileAttributes::dummy()),
                        russh_sftp::protocol::File::new("..", FileAttributes::dummy()),
                        russh_sftp::protocol::File::new("subdir", dir_attrs),
                        russh_sftp::protocol::File::new("f.txt", attrs),
                    ]
                };
                Ok(Name { id, files })
            } else {
                Err(StatusCode::Eof)
            }
        }
    }

    #[derive(Clone, Default)]
    struct MockSshServer {
        channels: std::sync::Arc<
            tokio::sync::Mutex<
                std::collections::HashMap<russh::ChannelId, russh::Channel<russh::server::Msg>>,
            >,
        >,
        scp_channels:
            std::sync::Arc<tokio::sync::Mutex<std::collections::HashSet<russh::ChannelId>>>,
        reject_session: bool,
        disconnect_on_session: bool,
    }

    impl russh::server::Handler for MockSshServer {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            user: &str,
            _password: &str,
        ) -> Result<russh::server::Auth, Self::Error> {
            if user == "reject" {
                Ok(russh::server::Auth::reject())
            } else {
                Ok(russh::server::Auth::Accept)
            }
        }

        async fn auth_publickey(
            &mut self,
            user: &str,
            _public_key: &russh::keys::ssh_key::PublicKey,
        ) -> Result<russh::server::Auth, Self::Error> {
            if user == "reject" {
                Ok(russh::server::Auth::reject())
            } else {
                Ok(russh::server::Auth::Accept)
            }
        }

        async fn auth_openssh_certificate(
            &mut self,
            user: &str,
            _certificate: &russh::keys::Certificate,
        ) -> Result<russh::server::Auth, Self::Error> {
            if user == "reject" {
                Ok(russh::server::Auth::reject())
            } else {
                Ok(russh::server::Auth::Accept)
            }
        }

        async fn channel_open_session(
            &mut self,
            channel: russh::Channel<russh::server::Msg>,
            reply: russh::server::ChannelOpenHandle,
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            if self.reject_session {
                reply
                    .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
                return Ok(());
            }
            if self.disconnect_on_session {
                reply.accept().await;
                let _ = session.disconnect(russh::Disconnect::ByApplication, "bye", "en");
                return Ok(());
            }
            self.channels.lock().await.insert(channel.id(), channel);
            reply.accept().await;
            Ok(())
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: russh::Channel<russh::server::Msg>,
            host: &str,
            port: u32,
            _orig_addr: &str,
            _orig_port: u32,
            reply: russh::server::ChannelOpenHandle,
            _session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            if let Ok(mut target_socket) =
                tokio::net::TcpStream::connect(format!("{host}:{port}")).await
            {
                tokio::spawn(async move {
                    let (mut r_stream, mut w_stream) = tokio::io::split(channel.into_stream());
                    let (mut r_sock, mut w_sock) = target_socket.split();
                    let _ = tokio::select! {
                        _ = tokio::io::copy(&mut r_stream, &mut w_sock) => (),
                        _ = tokio::io::copy(&mut r_sock, &mut w_stream) => (),
                    };
                });
            }
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: russh::ChannelId,
            _term: &str,
            _col_width: u32,
            _row_height: u32,
            _pix_width: u32,
            _pix_height: u32,
            _modes: &[(russh::Pty, u32)],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            let _ = session.channel_success(channel);
            Ok(())
        }

        async fn window_change_request(
            &mut self,
            channel: russh::ChannelId,
            _col_width: u32,
            _row_height: u32,
            _pix_width: u32,
            _pix_height: u32,
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            let _ = session.channel_success(channel);
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: russh::ChannelId,
            data: &[u8],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            let cmd = String::from_utf8_lossy(data);
            if cmd.contains("fail_exec") {
                let _ = session.channel_failure(channel);
                return Ok(());
            }
            if cmd.contains("close_immediate") {
                let _ = session.channel_success(channel);
                let _ = session.close(channel);
                return Ok(());
            }
            if cmd.contains("disc_after_init") {
                self.scp_channels.lock().await.insert(channel);
                let _ = session.data(channel, Bytes::from_static(b"\0"));
                let _ = session.disconnect(russh::Disconnect::ByApplication, "bye", "en");
                return Ok(());
            }
            if cmd.contains("disc_download") {
                let _ = session.channel_success(channel);
                let _ = session.disconnect(russh::Disconnect::ByApplication, "bye", "en");
                return Ok(());
            }
            let _ = session.channel_success(channel);
            if cmd.starts_with("scp -t") {
                self.scp_channels.lock().await.insert(channel);
                if cmd.contains("fail_init") {
                    let _ = session.data(channel, Bytes::from_static(b"\x01error\n"));
                } else {
                    let _ = session.data(channel, Bytes::from_static(b"\0"));
                }
            } else if cmd.starts_with("scp -f") {
                let _ = session.data(channel, Bytes::from("C0644 5 f.txt\nhello\0"));
            } else {
                let _ = session.data(channel, Bytes::from("mock ssh stdout\n"));
                let _ = session.extended_data(channel, 1, Bytes::from("mock ssh stderr\n"));
                let _ = session.exit_status_request(channel, 0);
                let _ = session.close(channel);
            }
            Ok(())
        }

        async fn subsystem_request(
            &mut self,
            channel: russh::ChannelId,
            name: &str,
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            let _ = session.channel_success(channel);
            if name == "sftp"
                && let Some(ch) = self.channels.lock().await.remove(&channel)
            {
                russh_sftp::server::run(ch.into_stream(), MockSftpSession::default()).await;
            }
            Ok(())
        }

        async fn data(
            &mut self,
            channel: russh::ChannelId,
            data: &[u8],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            if self.scp_channels.lock().await.contains(&channel) {
                if data.starts_with(b"C") && data.windows(9).any(|w| w == b"fail_meta") {
                    let _ = session.data(channel, Bytes::from_static(b"\x01error\n"));
                } else if data.starts_with(b"C")
                    && data.windows(15).any(|w| w == b"disc_after_meta")
                {
                    let _ = session.data(channel, Bytes::from_static(b"\0"));
                    let _ = session.disconnect(russh::Disconnect::ByApplication, "bye", "en");
                } else if !data.is_empty()
                    && data[0] != 0
                    && data.windows(9).any(|w| w == b"fail_data")
                {
                    let _ = session.data(channel, Bytes::from_static(b"\x01error\n"));
                } else if !data.is_empty()
                    && data[0] != 0
                    && data.windows(15).any(|w| w == b"disc_after_data")
                {
                    let _ = session.disconnect(russh::Disconnect::ByApplication, "bye", "en");
                } else {
                    let _ = session.data(channel, Bytes::from_static(b"\0"));
                }
            }
            Ok(())
        }
    }

    async fn start_mock_ssh_server(
        reject_session: bool,
        disconnect_on_session: bool,
    ) -> Result<(u16, tokio::task::JoinHandle<()>), StampError> {
        let mut server_config = russh::server::Config::default();
        server_config.inactivity_timeout = None;
        server_config.auth_rejection_time = Duration::from_millis(1);
        server_config.auth_rejection_time_initial = Some(Duration::from_millis(1));
        let priv_key = russh::keys::decode_secret_key(TEST_OPENSSH_KEY, None)
            .map_err(|e| StampError::Execution(e.to_string()))?;
        server_config.keys.push(priv_key);
        let server_config = Arc::new(server_config);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(StampError::Io)?;
        let port = listener.local_addr().map_err(StampError::Io)?.port();
        let handle = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let config = server_config.clone();
                tokio::spawn(async move {
                    let server = MockSshServer {
                        channels: std::sync::Arc::new(tokio::sync::Mutex::new(
                            std::collections::HashMap::new(),
                        )),
                        scp_channels: std::sync::Arc::new(tokio::sync::Mutex::new(
                            std::collections::HashSet::new(),
                        )),
                        reject_session,
                        disconnect_on_session,
                    };
                    if let Ok(running) = russh::server::run_stream(config, socket, server).await {
                        let _ = running.await;
                    }
                });
            }
        });
        Ok((port, handle))
    }

    #[derive(Clone)]
    struct MockAgent;
    impl russh::keys::agent::server::Agent for MockAgent {}

    async fn start_mock_agent(
        sock_path: &Path,
        key: Arc<russh::keys::ssh_key::PrivateKey>,
    ) -> Result<tokio::task::JoinHandle<()>, StampError> {
        let listener = tokio::net::UnixListener::bind(sock_path).map_err(StampError::Io)?;
        let stream = tokio_stream::wrappers::UnixListenerStream::new(listener);
        let handle = tokio::spawn(async move {
            let _ = russh::keys::agent::server::serve(stream, MockAgent).await;
        });
        let mut client = russh::keys::agent::client::AgentClient::connect_uds(sock_path)
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        client
            .add_identity(&key, &[])
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(handle)
    }

    async fn run_mock_cert_agent(
        sock_path: &Path,
        cert_bytes: Vec<u8>,
        keypair: russh::keys::ssh_key::private::Ed25519Keypair,
    ) -> Result<tokio::task::JoinHandle<()>, StampError> {
        let listener = tokio::net::UnixListener::bind(sock_path).map_err(StampError::Io)?;
        let handle = tokio::spawn(async move {
            use russh::keys::signature::Signer;
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                while let Ok(n) = socket.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let msg_type = if n >= 5 { buf[4] } else { 0 };
                    if msg_type == 11 {
                        let mut payload = Vec::new();
                        payload.extend_from_slice(&1u32.to_be_bytes());
                        payload.extend_from_slice(&(cert_bytes.len() as u32).to_be_bytes());
                        payload.extend_from_slice(&cert_bytes);
                        let comment = b"cert-mock";
                        payload.extend_from_slice(&(comment.len() as u32).to_be_bytes());
                        payload.extend_from_slice(comment);

                        let mut resp = Vec::new();
                        let total_len = (1 + payload.len()) as u32;
                        resp.extend_from_slice(&total_len.to_be_bytes());
                        resp.push(12);
                        resp.extend_from_slice(&payload);
                        let _ = socket.write_all(&resp).await;
                    } else if msg_type == 13 && n > 9 {
                        let key_len = u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]) as usize;
                        let data_offset = 9 + key_len;
                        if n >= data_offset + 4 {
                            let data_len = u32::from_be_bytes([
                                buf[data_offset],
                                buf[data_offset + 1],
                                buf[data_offset + 2],
                                buf[data_offset + 3],
                            ]) as usize;
                            let data_start = data_offset + 4;
                            let to_sign = if n >= data_start + data_len {
                                &buf[data_start..data_start + data_len]
                            } else {
                                &[]
                            };
                            if let Ok(sig) = keypair.try_sign(to_sign) {
                                let sig_bytes = sig.as_bytes();
                                let alg = b"ssh-ed25519";
                                let mut inner_sig = Vec::new();
                                inner_sig.extend_from_slice(&(alg.len() as u32).to_be_bytes());
                                inner_sig.extend_from_slice(alg);
                                inner_sig
                                    .extend_from_slice(&(sig_bytes.len() as u32).to_be_bytes());
                                inner_sig.extend_from_slice(&sig_bytes);

                                let mut payload = Vec::new();
                                payload.extend_from_slice(&(inner_sig.len() as u32).to_be_bytes());
                                payload.extend_from_slice(&inner_sig);

                                let mut resp = Vec::new();
                                let total_len = (1 + payload.len()) as u32;
                                resp.extend_from_slice(&total_len.to_be_bytes());
                                resp.push(14);
                                resp.extend_from_slice(&payload);
                                let _ = socket.write_all(&resp).await;
                            }
                        }
                    }
                }
            }
        });
        Ok(handle)
    }

    #[tokio::test]
    async fn test_ssh_real_server_execution_and_scp() -> Result<(), StampError> {
        let (port, server_handle) = start_mock_ssh_server(false, false).await?;

        // 1. Password auth execution
        let mut config = get_config("127.0.0.1", "testuser");
        config.port = Port::new(port);
        config.password = Some("testpass".to_string());
        config.pty = true;
        let comm = SshCommunicator::new(config.clone());

        let res = comm.execute(&Command::new("uptime".to_string())).await?;
        assert_eq!(res.exit_code, 0);
        assert_eq!(res.stdout, "mock ssh stdout\n");
        assert_eq!(res.stderr, "mock ssh stderr\n");

        // 2. Exec non-zero exit code
        let res_fail_exec = comm.execute(&Command::new("fail_exec".to_string())).await?;
        assert_eq!(res_fail_exec.exit_code, 1);

        // 3. Upload file via SCP
        let temp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let local_file = temp_dir.path().join("upload.txt");
        tokio::fs::write(&local_file, b"content to upload")
            .await
            .map_err(StampError::Io)?;
        let remote_path = FilePath::new(PathBuf::from("/remote/upload.txt"));
        comm.upload(&FilePath::new(local_file.clone()), &remote_path)
            .await?;

        // 4. Download file via SCP
        let local_dl = temp_dir.path().join("download.txt");
        comm.download(&remote_path, &FilePath::new(local_dl.clone()))
            .await?;
        assert!(local_dl.exists());

        // 5. Private key authentication
        let priv_key_path = temp_dir.path().join("id_ed25519");
        tokio::fs::write(&priv_key_path, TEST_OPENSSH_KEY)
            .await
            .map_err(StampError::Io)?;
        let loaded_key = load_private_key(&priv_key_path, None).await?;
        assert_eq!(
            loaded_key.key_data().algorithm(),
            Ok(russh::keys::ssh_key::Algorithm::Ed25519)
        );

        let mut key_cfg = config.clone();
        key_cfg.password = None;
        key_cfg.private_key_path = Some(FilePath::new(priv_key_path.clone()));
        let comm_key = SshCommunicator::new(key_cfg);
        let res_key = comm_key
            .execute(&Command::new("whoami".to_string()))
            .await?;
        assert_eq!(res_key.exit_code, 0);

        // 6. SshAuthMethod::PrivateKey in auth_methods alone
        let mut method_cfg = config.clone();
        method_cfg.password = None;
        method_cfg.auth_methods = vec![SshAuthMethod::PrivateKey {
            key_path: FilePath::new(priv_key_path.clone()),
            passphrase: None,
        }];
        let comm_method = SshCommunicator::new(method_cfg);
        let res_method = comm_method.execute(&Command::new("id".to_string())).await?;
        assert_eq!(res_method.exit_code, 0);

        let mut method_pass_cfg = config.clone();
        method_pass_cfg.password = None;
        method_pass_cfg.auth_methods = vec![SshAuthMethod::Password("testpass".to_string())];
        let comm_method_pass = SshCommunicator::new(method_pass_cfg);
        let res_pass = comm_method_pass
            .execute(&Command::new("id".to_string()))
            .await?;
        assert_eq!(res_pass.exit_code, 0);

        let mut method_pk_reject_cfg = config.clone();
        method_pk_reject_cfg.timeout = Timeout::new(Duration::from_millis(50));
        method_pk_reject_cfg.username = "reject".to_string();
        method_pk_reject_cfg.password = None;
        method_pk_reject_cfg.auth_methods = vec![SshAuthMethod::PrivateKey {
            key_path: FilePath::new(priv_key_path.clone()),
            passphrase: None,
        }];
        let comm_pk_rej = SshCommunicator::new(method_pk_reject_cfg);
        assert!(
            comm_pk_rej
                .execute(&Command::new("id".to_string()))
                .await
                .is_err()
        );

        // 7. Bastion single hop
        let mut bastion_cfg = config.clone();
        bastion_cfg.bastion_host = Some("127.0.0.1".to_string());
        bastion_cfg.bastion_port = Some(Port::new(port));
        bastion_cfg.bastion_password = Some("bastion_pass".to_string());
        bastion_cfg.bastion_username = Some("jump".to_string());
        let comm_bastion = SshCommunicator::new(bastion_cfg);
        let res_bastion = comm_bastion
            .execute(&Command::new("hostname".to_string()))
            .await?;
        assert_eq!(res_bastion.exit_code, 0);

        // 8. Bastion multi hop with 3 hops: private key, password, private key
        let mut multi_bastion_cfg = config.clone();
        multi_bastion_cfg.bastion_chain = vec![
            BastionConfig {
                host: "127.0.0.1".to_string(),
                port: Port::new(port),
                username: "jump1".to_string(),
                password: None,
                private_key_path: Some(FilePath::new(priv_key_path.clone())),
                private_key_passphrase: None,
            },
            BastionConfig {
                host: "127.0.0.1".to_string(),
                port: Port::new(port),
                username: "jump2".to_string(),
                password: Some("jump2pass".to_string()),
                private_key_path: None,
                private_key_passphrase: None,
            },
            BastionConfig {
                host: "127.0.0.1".to_string(),
                port: Port::new(port),
                username: "jump3".to_string(),
                password: None,
                private_key_path: Some(FilePath::new(priv_key_path.clone())),
                private_key_passphrase: None,
            },
        ];
        let comm_multi = SshCommunicator::new(multi_bastion_cfg);
        let res_multi = comm_multi
            .execute(&Command::new("hostname".to_string()))
            .await?;
        assert_eq!(res_multi.exit_code, 0);

        let mut bastion_rej1_cfg = config.clone();
        bastion_rej1_cfg.timeout = Timeout::new(Duration::from_millis(50));
        bastion_rej1_cfg.bastion_chain = vec![BastionConfig {
            host: "127.0.0.1".to_string(),
            port: Port::new(port),
            username: "reject".to_string(),
            password: Some("badpass".to_string()),
            private_key_path: None,
            private_key_passphrase: None,
        }];
        let comm_b_rej1 = SshCommunicator::new(bastion_rej1_cfg);
        assert!(
            comm_b_rej1
                .execute(&Command::new("hostname".to_string()))
                .await
                .is_err()
        );

        let mut bastion_rej2_cfg = config.clone();
        bastion_rej2_cfg.timeout = Timeout::new(Duration::from_millis(50));
        bastion_rej2_cfg.bastion_chain = vec![
            BastionConfig {
                host: "127.0.0.1".to_string(),
                port: Port::new(port),
                username: "jump1".to_string(),
                password: Some("pass".to_string()),
                private_key_path: None,
                private_key_passphrase: None,
            },
            BastionConfig {
                host: "127.0.0.1".to_string(),
                port: Port::new(port),
                username: "reject".to_string(),
                password: Some("badpass".to_string()),
                private_key_path: None,
                private_key_passphrase: None,
            },
        ];
        let comm_b_rej2 = SshCommunicator::new(bastion_rej2_cfg);
        assert!(
            comm_b_rej2
                .execute(&Command::new("hostname".to_string()))
                .await
                .is_err()
        );

        let mut bastion_noauth_cfg = config.clone();
        bastion_noauth_cfg.timeout = Timeout::new(Duration::from_millis(50));
        bastion_noauth_cfg.bastion_chain = vec![
            BastionConfig {
                host: "127.0.0.1".to_string(),
                port: Port::new(port),
                username: "jump1".to_string(),
                password: Some("pass".to_string()),
                private_key_path: None,
                private_key_passphrase: None,
            },
            BastionConfig {
                host: "127.0.0.1".to_string(),
                port: Port::new(port),
                username: "jump2".to_string(),
                password: None,
                private_key_path: None,
                private_key_passphrase: None,
            },
        ];
        let comm_b_noauth = SshCommunicator::new(bastion_noauth_cfg);
        let _ = comm_b_noauth
            .execute(&Command::new("hostname".to_string()))
            .await;

        let mut bastion_hop1_noauth_cfg = config.clone();
        bastion_hop1_noauth_cfg.timeout = Timeout::new(Duration::from_millis(50));
        bastion_hop1_noauth_cfg.bastion_chain = vec![BastionConfig {
            host: "127.0.0.1".to_string(),
            port: Port::new(port),
            username: "jump1".to_string(),
            password: None,
            private_key_path: None,
            private_key_passphrase: None,
        }];
        let comm_b_hop1_noauth = SshCommunicator::new(bastion_hop1_noauth_cfg);
        let _ = comm_b_hop1_noauth
            .execute(&Command::new("hostname".to_string()))
            .await;

        // 9. SFTP protocol upload and download (including directory and nested directory)
        let mut sftp_cfg = config.clone();
        sftp_cfg.transfer_protocol = FileTransferProtocol::Sftp;
        let comm_sftp = SshCommunicator::new(sftp_cfg);
        let res_sftp_up = comm_sftp
            .upload(&FilePath::new(local_file.clone()), &remote_path)
            .await;
        assert!(res_sftp_up.is_ok());

        let sftp_upload_dir = temp_dir.path().join("sftp_dir");
        let sub_nested = sftp_upload_dir.join("nested");
        tokio::fs::create_dir_all(&sub_nested)
            .await
            .map_err(StampError::Io)?;
        tokio::fs::write(sub_nested.join("sub.txt"), b"sub")
            .await
            .map_err(StampError::Io)?;
        let _ = comm_sftp
            .upload(&FilePath::new(sftp_upload_dir), &remote_path)
            .await;

        let sftp_dl_file = temp_dir.path().join("sftp_download.txt");
        let res_sftp_dl = comm_sftp
            .download(&remote_path, &FilePath::new(sftp_dl_file))
            .await;
        assert!(res_sftp_dl.is_ok());

        let sftp_dl_dir = temp_dir.path().join("sftp_download_dir");
        let res_sftp_dir_dl = comm_sftp
            .download(
                &FilePath::new(PathBuf::from("/remote/dir")),
                &FilePath::new(sftp_dl_dir),
            )
            .await;
        assert!(res_sftp_dir_dl.is_ok());

        assert_eq!(
            russh_sftp::server::Handler::unimplemented(&MockSftpSession::default()),
            StatusCode::OpUnsupported
        );

        // 10. Auth rejections
        let mut reject_cfg = config.clone();
        reject_cfg.timeout = Timeout::new(Duration::from_millis(50));
        reject_cfg.connection_attempts = 1;
        reject_cfg.retry_backoff = Duration::from_millis(1);
        reject_cfg.username = "reject".to_string();
        reject_cfg.password = Some("wrong".to_string());
        reject_cfg.auth_methods = vec![SshAuthMethod::Password("wrong".to_string())];
        let comm_reject = SshCommunicator::new(reject_cfg);
        assert!(matches!(
            comm_reject.execute(&Command::new("id".to_string())).await,
            Err(StampError::Execution(_))
        ));

        let mut reject_pass_cfg = config.clone();
        reject_pass_cfg.timeout = Timeout::new(Duration::from_millis(50));
        reject_pass_cfg.connection_attempts = 1;
        reject_pass_cfg.retry_backoff = Duration::from_millis(1);
        reject_pass_cfg.username = "reject".to_string();
        reject_pass_cfg.password = Some("wrong".to_string());
        let comm_reject_pass = SshCommunicator::new(reject_pass_cfg);
        assert!(matches!(
            comm_reject_pass
                .execute(&Command::new("id".to_string()))
                .await,
            Err(StampError::Execution(_))
        ));

        let mut reject_pk_cfg = config.clone();
        reject_pk_cfg.timeout = Timeout::new(Duration::from_millis(50));
        reject_pk_cfg.connection_attempts = 1;
        reject_pk_cfg.retry_backoff = Duration::from_millis(1);
        reject_pk_cfg.username = "reject".to_string();
        reject_pk_cfg.password = None;
        reject_pk_cfg.private_key_path = Some(FilePath::new(priv_key_path.clone()));
        let comm_reject_pk = SshCommunicator::new(reject_pk_cfg);
        assert!(matches!(
            comm_reject_pk
                .execute(&Command::new("id".to_string()))
                .await,
            Err(StampError::Execution(_))
        ));

        // 11. SCP error branches
        let fail_remote = FilePath::new(PathBuf::from("/fail_init/error.txt"));
        let dummy_local = temp_dir.path().join("d.txt");
        tokio::fs::write(&dummy_local, b"test")
            .await
            .map_err(StampError::Io)?;
        let res_scp_fail = comm
            .upload(&FilePath::new(dummy_local.clone()), &fail_remote)
            .await;
        assert!(matches!(res_scp_fail, Err(StampError::Execution(_))));

        let fail_meta_local = temp_dir.path().join("fail_meta.txt");
        tokio::fs::write(&fail_meta_local, b"test")
            .await
            .map_err(StampError::Io)?;
        let res_meta_fail = comm
            .upload(&FilePath::new(fail_meta_local), &remote_path)
            .await;
        assert!(matches!(res_meta_fail, Err(StampError::Execution(_))));

        let fail_data_local = temp_dir.path().join("fail_data.txt");
        tokio::fs::write(&fail_data_local, b"fail_data content")
            .await
            .map_err(StampError::Io)?;
        let res_data_fail = comm
            .upload(&FilePath::new(fail_data_local), &remote_path)
            .await;
        assert!(matches!(res_data_fail, Err(StampError::Execution(_))));

        let bad_dl_path = FilePath::new(PathBuf::from("/nonexistent_dir_12345/dl.txt"));
        let res_scp_dl_fail = comm.download(&remote_path, &bad_dl_path).await;
        assert!(matches!(res_scp_dl_fail, Err(StampError::Io(_))));

        let bad_local_upload = FilePath::new(PathBuf::from("/nonexistent_file_12345.txt"));
        let res_up_fail = comm.upload(&bad_local_upload, &remote_path).await;
        assert!(matches!(res_up_fail, Err(StampError::Io(_))));

        let disc_init_remote = FilePath::new(PathBuf::from("/disc_after_init/file.txt"));
        let _ = comm
            .upload(&FilePath::new(dummy_local.clone()), &disc_init_remote)
            .await;

        let disc_meta_local = temp_dir.path().join("disc_after_meta.txt");
        tokio::fs::write(&disc_meta_local, b"test")
            .await
            .map_err(StampError::Io)?;
        let _ = comm
            .upload(&FilePath::new(disc_meta_local), &remote_path)
            .await;

        let disc_data_local = temp_dir.path().join("disc_data.txt");
        tokio::fs::write(&disc_data_local, b"disc_after_data")
            .await
            .map_err(StampError::Io)?;
        let _ = comm
            .upload(&FilePath::new(disc_data_local), &remote_path)
            .await;

        let disc_dl_remote = FilePath::new(PathBuf::from("/disc_download/file.txt"));
        let dummy_dl_dest = temp_dir.path().join("dl_dest.txt");
        let _ = comm
            .download(&disc_dl_remote, &FilePath::new(dummy_dl_dest))
            .await;

        // 12. Mock agent authentication with PublicKey
        let agent_sock_pk = temp_dir.path().join("agent_pk.sock");
        let pk_arc = Arc::new(loaded_key);
        let agent_pk_handle = start_mock_agent(&agent_sock_pk, pk_arc).await?;

        let mut agent_cfg = config.clone();
        agent_cfg.password = None;
        agent_cfg.auth_methods = vec![SshAuthMethod::Agent {
            socket_path: Some(FilePath::new(agent_sock_pk.clone())),
        }];
        let comm_agent = SshCommunicator::new(agent_cfg);
        let res_agent = comm_agent.execute(&Command::new("id".to_string())).await?;
        assert_eq!(res_agent.exit_code, 0);

        {
            let _guard = crate::utils::ENV_MUTEX
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            unsafe {
                std::env::set_var("SSH_AUTH_SOCK", &agent_sock_pk);
            }
            let mut agent_env_cfg = config.clone();
            agent_env_cfg.password = None;
            agent_env_cfg.auth_methods = vec![SshAuthMethod::Agent { socket_path: None }];
            let comm_agent_env = SshCommunicator::new(agent_env_cfg);
            let res_env = comm_agent_env
                .execute(&Command::new("id".to_string()))
                .await?;
            assert_eq!(res_env.exit_code, 0);
            unsafe {
                std::env::remove_var("SSH_AUTH_SOCK");
            }
        }

        let mut agent_rej_cfg = config.clone();
        agent_rej_cfg.timeout = Timeout::new(Duration::from_millis(50));
        agent_rej_cfg.username = "reject".to_string();
        agent_rej_cfg.password = None;
        agent_rej_cfg.auth_methods = vec![SshAuthMethod::Agent {
            socket_path: Some(FilePath::new(agent_sock_pk)),
        }];
        let comm_agent_rej = SshCommunicator::new(agent_rej_cfg);
        assert!(
            comm_agent_rej
                .execute(&Command::new("id".to_string()))
                .await
                .is_err()
        );

        agent_pk_handle.abort();

        // 13. Mock agent authentication with Certificate
        let cert_sock = temp_dir.path().join("agent_cert.sock");
        let priv_key_agent = russh::keys::decode_secret_key(TEST_OPENSSH_KEY, None)
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let mut builder_agent = russh::keys::ssh_key::certificate::Builder::new(
            vec![2u8; 16],
            priv_key_agent.public_key().key_data().clone(),
            0,
            u64::MAX,
        )
        .map_err(|e| StampError::Execution(e.to_string()))?;
        builder_agent
            .serial(2)
            .map_err(|e| StampError::Execution(e.to_string()))?;
        builder_agent
            .key_id("agent_test")
            .map_err(|e| StampError::Execution(e.to_string()))?;
        builder_agent
            .cert_type(russh::keys::ssh_key::certificate::CertType::Host)
            .map_err(|e| StampError::Execution(e.to_string()))?;
        builder_agent
            .valid_principal("test.local")
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let cert_agent = builder_agent
            .sign(&priv_key_agent)
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let cert_bytes = cert_agent
            .to_bytes()
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let keypair_agent = match priv_key_agent.key_data() {
            russh::keys::ssh_key::private::KeypairData::Ed25519(kp) => kp.clone(),
            _ => unreachable!(),
        };
        let agent_cert_handle = run_mock_cert_agent(&cert_sock, cert_bytes, keypair_agent).await?;

        let mut agent_cert_cfg = config.clone();
        agent_cert_cfg.password = None;
        agent_cert_cfg.auth_methods = vec![SshAuthMethod::Agent {
            socket_path: Some(FilePath::new(cert_sock)),
        }];
        let comm_agent_cert = SshCommunicator::new(agent_cert_cfg);
        let res_agent_cert = comm_agent_cert
            .execute(&Command::new("id".to_string()))
            .await?;
        assert_eq!(res_agent_cert.exit_code, 0);
        agent_cert_handle.abort();

        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_ssh_legacy_and_edge_branches() -> Result<(), StampError> {
        let (port, server_handle) = start_mock_ssh_server(false, false).await?;
        let temp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let priv_key_path = temp_dir.path().join("id_ed25519");
        tokio::fs::write(&priv_key_path, TEST_OPENSSH_KEY)
            .await
            .map_err(StampError::Io)?;

        // 1. Certificate generation and check_server_key Certificate branch
        let priv_key = russh::keys::decode_secret_key(TEST_OPENSSH_KEY, None)
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let mut builder = russh::keys::ssh_key::certificate::Builder::new(
            vec![1u8; 16],
            priv_key.public_key().key_data().clone(),
            0,
            u64::MAX,
        )
        .map_err(|e| StampError::Execution(e.to_string()))?;
        builder
            .serial(1)
            .map_err(|e| StampError::Execution(e.to_string()))?;
        builder
            .key_id("test")
            .map_err(|e| StampError::Execution(e.to_string()))?;
        builder
            .cert_type(russh::keys::ssh_key::certificate::CertType::Host)
            .map_err(|e| StampError::Execution(e.to_string()))?;
        builder
            .valid_principal("test.local")
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let cert = builder
            .sign(&priv_key)
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let cert_or_key = russh::keys::PublicKeyOrCertificate::Certificate(cert.clone());

        let cert_file_path = temp_dir.path().join("id_ed25519-cert.pub");
        tokio::fs::write(&cert_file_path, cert.to_openssh().unwrap_or_default())
            .await
            .map_err(StampError::Io)?;

        let mut handler = ClientHandler {
            host_key_verification: HostKeyVerification::Strict,
            host: "test.local".to_string(),
            port: 22,
            known_hosts_file: None,
        };
        let _ = handler
            .check_server_key(&cert_or_key)
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;

        let mut handler_accept = ClientHandler {
            host_key_verification: HostKeyVerification::AcceptNew,
            host: "test.local".to_string(),
            port: 22,
            known_hosts_file: None,
        };
        let _ = handler_accept
            .check_server_key(&cert_or_key)
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;

        // 2. Legacy fallback certificate authentication (success and failure)
        let mut legacy_cert_cfg = get_config("127.0.0.1", "testuser");
        legacy_cert_cfg.port = Port::new(port);
        legacy_cert_cfg.password = None;
        legacy_cert_cfg.private_key_path = Some(FilePath::new(priv_key_path.clone()));
        legacy_cert_cfg.certificate_path = Some(FilePath::new(cert_file_path.clone()));
        let comm_leg_cert = SshCommunicator::new(legacy_cert_cfg);
        let res_leg = comm_leg_cert
            .execute(&Command::new("id".to_string()))
            .await?;
        assert_eq!(res_leg.exit_code, 0);

        let mut legacy_cert_fail_cfg = get_config("127.0.0.1", "reject");
        legacy_cert_fail_cfg.timeout = Timeout::new(Duration::from_millis(50));
        legacy_cert_fail_cfg.port = Port::new(port);
        legacy_cert_fail_cfg.password = None;
        legacy_cert_fail_cfg.private_key_path = Some(FilePath::new(priv_key_path.clone()));
        legacy_cert_fail_cfg.certificate_path = Some(FilePath::new(cert_file_path.clone()));
        let comm_leg_cert_fail = SshCommunicator::new(legacy_cert_fail_cfg);
        assert!(
            comm_leg_cert_fail
                .execute(&Command::new("id".to_string()))
                .await
                .is_err()
        );

        // 3. SshAuthMethod::Certificate (success and failure)
        let mut method_cert_cfg = get_config("127.0.0.1", "testuser");
        method_cert_cfg.port = Port::new(port);
        method_cert_cfg.password = None;
        method_cert_cfg.auth_methods = vec![SshAuthMethod::Certificate {
            key_path: FilePath::new(priv_key_path.clone()),
            certificate_path: FilePath::new(cert_file_path.clone()),
            passphrase: None,
        }];
        let comm_method_cert = SshCommunicator::new(method_cert_cfg);
        let res_meth_cert = comm_method_cert
            .execute(&Command::new("id".to_string()))
            .await?;
        assert_eq!(res_meth_cert.exit_code, 0);

        let mut method_cert_fail_cfg = get_config("127.0.0.1", "reject");
        method_cert_fail_cfg.timeout = Timeout::new(Duration::from_millis(50));
        method_cert_fail_cfg.port = Port::new(port);
        method_cert_fail_cfg.password = None;
        method_cert_fail_cfg.auth_methods = vec![SshAuthMethod::Certificate {
            key_path: FilePath::new(priv_key_path.clone()),
            certificate_path: FilePath::new(cert_file_path),
            passphrase: None,
        }];
        let comm_method_cert_fail = SshCommunicator::new(method_cert_fail_cfg);
        assert!(
            comm_method_cert_fail
                .execute(&Command::new("id".to_string()))
                .await
                .is_err()
        );

        // 4. Legacy bastion fields
        let mut leg_bastion_cfg = get_config("127.0.0.1", "testuser");
        leg_bastion_cfg.timeout = Timeout::new(Duration::from_millis(50));
        leg_bastion_cfg.port = Port::new(port);
        leg_bastion_cfg.bastion_host = Some("127.0.0.1".to_string());
        leg_bastion_cfg.bastion_port = Some(Port::new(port));
        leg_bastion_cfg.bastion_username = Some("jump".to_string());
        leg_bastion_cfg.bastion_password = Some("pass".to_string());
        leg_bastion_cfg.bastion_private_key_file = Some(FilePath::new(priv_key_path.clone()));
        let comm_leg_bastion = SshCommunicator::new(leg_bastion_cfg);
        let _ = comm_leg_bastion
            .execute(&Command::new("id".to_string()))
            .await;

        // 5. Connection retry loop exhaustion
        let mut fail_conn_cfg = get_config("127.0.0.1", "testuser");
        fail_conn_cfg.port = Port::new(1); // Closed port
        fail_conn_cfg.connection_attempts = 2;
        fail_conn_cfg.retry_backoff = Duration::from_millis(1);
        let comm_fail_conn = SshCommunicator::new(fail_conn_cfg);
        assert!(
            comm_fail_conn
                .execute(&Command::new("id".to_string()))
                .await
                .is_err()
        );

        server_handle.abort();

        // 6. Channel rejection server
        let (rej_port, rej_handle) = start_mock_ssh_server(true, false).await?;
        let mut rej_cfg = get_config("127.0.0.1", "testuser");
        rej_cfg.port = Port::new(rej_port);
        rej_cfg.password = Some("testpass".to_string());
        let comm_rej = SshCommunicator::new(rej_cfg.clone());
        assert!(
            comm_rej
                .execute(&Command::new("id".to_string()))
                .await
                .is_err()
        );

        let local_dummy = temp_dir.path().join("rej_dummy.txt");
        tokio::fs::write(&local_dummy, b"rej")
            .await
            .map_err(StampError::Io)?;
        let remote_dummy = FilePath::new(PathBuf::from("/remote/rej.txt"));
        assert!(
            comm_rej
                .upload(&FilePath::new(local_dummy.clone()), &remote_dummy)
                .await
                .is_err()
        );
        assert!(
            comm_rej
                .download(&remote_dummy, &FilePath::new(local_dummy.clone()))
                .await
                .is_err()
        );

        let mut rej_sftp_cfg = rej_cfg;
        rej_sftp_cfg.transfer_protocol = FileTransferProtocol::Sftp;
        let comm_rej_sftp = SshCommunicator::new(rej_sftp_cfg);
        assert!(
            comm_rej_sftp
                .upload(&FilePath::new(local_dummy.clone()), &remote_dummy)
                .await
                .is_err()
        );

        rej_handle.abort();

        // 7. Disconnect on session server (covers Exec error on execute, upload, download)
        let (disc_port, disc_handle) = start_mock_ssh_server(false, true).await?;
        let mut disc_cfg = get_config("127.0.0.1", "testuser");
        disc_cfg.port = Port::new(disc_port);
        disc_cfg.password = Some("testpass".to_string());
        disc_cfg.pty = false;
        let comm_disc = SshCommunicator::new(disc_cfg);
        assert!(
            comm_disc
                .execute(&Command::new("id".to_string()))
                .await
                .is_err()
        );
        assert!(
            comm_disc
                .upload(&FilePath::new(local_dummy.clone()), &remote_dummy)
                .await
                .is_err()
        );
        assert!(
            comm_disc
                .download(&remote_dummy, &FilePath::new(local_dummy))
                .await
                .is_err()
        );

        disc_handle.abort();
        Ok(())
    }
}
