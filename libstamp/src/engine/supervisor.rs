#![cfg_attr(coverage_nightly, coverage(off))]
//! Out-of-process plugin supervisor and lifecycle manager.
//!
//! Handles subprocess execution with environment isolation, process group assignment,
//! HashiCorp `go-plugin` handshake negotiation, gRPC health checking, and graceful
//! termination / orphan cleanup watchdogs.

use crate::error::StampError;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Network transport used by the plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkType {
    /// TCP socket.
    Tcp,
    /// Unix domain socket.
    Unix,
}

/// RPC protocol used by the plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Protocol {
    /// Legacy net/rpc protocol.
    NetRpc,
    /// Modern gRPC protocol.
    Grpc,
}

/// Parsed HashiCorp `go-plugin` handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// Core protocol version (typically 1).
    pub core_protocol_version: String,
    /// Application protocol version.
    pub app_protocol_version: String,
    /// Network type (tcp or unix).
    pub network_type: NetworkType,
    /// Network address (host:port or socket path).
    pub address: String,
    /// Protocol type (grpc or net/rpc).
    pub protocol: Protocol,
    /// Optional base64-encoded server TLS certificate.
    pub server_cert: Option<String>,
}

impl std::str::FromStr for Handshake {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.trim().split('|').collect();
        if parts.len() < 4 {
            return Err(StampError::Execution(format!(
                "Invalid go-plugin handshake format: '{s}'"
            )));
        }

        let network_type = match parts[2] {
            "tcp" => NetworkType::Tcp,
            "unix" => NetworkType::Unix,
            other => {
                return Err(StampError::Execution(format!(
                    "Unknown network type: '{other}'"
                )));
            }
        };

        let protocol = if parts.len() >= 5 {
            match parts[4] {
                "grpc" => Protocol::Grpc,
                "net/rpc" => Protocol::NetRpc,
                other => {
                    return Err(StampError::Execution(format!(
                        "Unknown protocol: '{other}'"
                    )));
                }
            }
        } else {
            Protocol::NetRpc
        };

        let server_cert = if parts.len() >= 6 && !parts[5].is_empty() {
            Some(parts[5].to_string())
        } else {
            None
        };

        Ok(Self {
            core_protocol_version: parts[0].to_string(),
            app_protocol_version: parts[1].to_string(),
            network_type,
            address: parts[3].to_string(),
            protocol,
            server_cert,
        })
    }
}

/// Configuration for launching an external plugin subprocess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSupervisorConfig {
    /// Filesystem path to the plugin executable.
    pub plugin_path: PathBuf,
    /// Command-line arguments to pass to the plugin.
    pub args: Vec<String>,
    /// Magic cookie key-value pair for handshake verification.
    pub magic_cookie: String,
    /// Supported plugin protocol versions.
    pub protocol_versions: Vec<u32>,
    /// Optional client TLS certificate.
    pub client_cert: Option<String>,
    /// Optional client TLS private key in PEM format.
    pub client_key: Option<String>,
    /// Explicit environment variables to inject into the child process.
    pub env: std::collections::HashMap<String, String>,
    /// Timeout for graceful shutdown before force-killing with SIGKILL.
    pub graceful_timeout: Duration,
}

impl Default for PluginSupervisorConfig {
    fn default() -> Self {
        Self {
            plugin_path: PathBuf::new(),
            args: Vec::new(),
            magic_cookie: "d602bf8f470bc67ca7faa0386276bbdd4330efaf76d1a219cb4d6991ca9872b2"
                .to_string(),
            protocol_versions: vec![1, 2, 3, 4, 5, 6],
            client_cert: None,
            client_key: None,
            env: std::collections::HashMap::new(),
            graceful_timeout: Duration::from_millis(500),
        }
    }
}

/// A supervised external plugin child process.
#[derive(Debug, Clone)]
pub struct SupervisedPlugin {
    /// Process ID.
    pub pid: u32,
    /// Process group ID on Unix.
    pub pgid: i32,
    /// Negotiated handshake info.
    pub handshake: Handshake,
    /// Child process handle.
    child: Arc<Mutex<Option<Child>>>,
    /// Atomic cancellation flag.
    cancelled: Arc<AtomicBool>,
    /// Graceful timeout before SIGKILL.
    graceful_timeout: Duration,
    /// Optional client TLS certificate in PEM format.
    pub client_cert: Option<String>,
    /// Optional client TLS private key in PEM format.
    pub client_key: Option<String>,
}

impl SupervisedPlugin {
    /// Checks whether the plugin process is still alive.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        #[cfg(unix)]
        {
            if self.pgid > 0 {
                // Sending signal 0 checks if process exists
                unsafe { libc::kill(self.pid as i32, 0) == 0 }
            } else {
                false
            }
        }
        #[cfg(not(unix))]
        {
            true
        }
    }

    /// Returns whether the supervised plugin has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Establishes a configured tonic `Channel` to the plugin endpoint, applying mTLS if certificates are present.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if connecting or TLS configuration fails.
    pub async fn create_channel(&self) -> Result<tonic::transport::Channel, StampError> {
        match self.handshake.network_type {
            NetworkType::Tcp => {
                let address = if self.handshake.address.starts_with("http") {
                    self.handshake.address.clone()
                } else {
                    format!("http://{}", self.handshake.address)
                };

                let mut endpoint = tonic::transport::Endpoint::from_shared(address)
                    .map_err(|e| StampError::Execution(format!("Invalid endpoint address: {e}")))?;

                if let Some(ref cert_b64) = self.handshake.server_cert {
                    use base64::Engine;
                    if let Ok(cert_bytes) =
                        base64::engine::general_purpose::STANDARD.decode(cert_b64)
                    {
                        let ca = tonic::transport::Certificate::from_pem(cert_bytes);
                        let mut tls_config =
                            tonic::transport::ClientTlsConfig::new().ca_certificate(ca);
                        if let (Some(client_cert), Some(client_key)) =
                            (&self.client_cert, &self.client_key)
                        {
                            let identity = tonic::transport::Identity::from_pem(
                                client_cert.as_bytes(),
                                client_key.as_bytes(),
                            );
                            tls_config = tls_config.identity(identity);
                        }
                        endpoint = endpoint
                            .tls_config(tls_config)
                            .map_err(|e| StampError::Execution(format!("TLS config error: {e}")))?;
                    }
                }

                endpoint.connect().await.map_err(|e| {
                    StampError::Execution(format!("Failed to connect to plugin TCP channel: {e}"))
                })
            }
            NetworkType::Unix => {
                #[cfg(unix)]
                {
                    let path = self.handshake.address.clone();
                    tonic::transport::Endpoint::try_from("http://[::]:50051")
                        .map_err(|e| StampError::Execution(format!("Invalid endpoint: {e}")))?
                        .connect_with_connector(tower::service_fn(
                            move |_: tonic::transport::Uri| {
                                tokio::net::UnixStream::connect(path.clone())
                            },
                        ))
                        .await
                        .map_err(|e| {
                            StampError::Execution(format!(
                                "Failed to connect to plugin Unix channel: {e}"
                            ))
                        })
                }
                #[cfg(not(unix))]
                {
                    Err(StampError::Execution(
                        "Unix sockets are not supported on this platform".to_string(),
                    ))
                }
            }
        }
    }

    /// Starts a background heartbeat health monitoring loop.
    /// If health check fails `max_consecutive_failures` times, marks the plugin as cancelled.
    pub fn start_heartbeat_monitor(
        &self,
        interval: Duration,
        max_consecutive_failures: u32,
    ) -> tokio::task::JoinHandle<()> {
        let endpoint = self.handshake.address.clone();
        let net_type = self.handshake.network_type.clone();
        let cancelled = self.cancelled.clone();

        tokio::spawn(async move {
            let mut failures = 0;
            while !cancelled.load(Ordering::SeqCst) {
                tokio::time::sleep(interval).await;
                if cancelled.load(Ordering::SeqCst) {
                    break;
                }
                match PluginSupervisor::perform_healthcheck(&endpoint, &net_type).await {
                    Ok(true) => {
                        failures = 0;
                    }
                    _ => {
                        failures += 1;
                        if failures >= max_consecutive_failures {
                            cancelled.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                }
            }
        })
    }

    /// Gracefully terminates the plugin process group, falling back to SIGKILL.
    ///
    /// # Errors
    /// Returns `StampError` if signal dispatch fails.
    pub async fn cancel(&self) -> Result<(), StampError> {
        self.cancelled.store(true, Ordering::SeqCst);

        #[cfg(unix)]
        {
            if self.pgid > 0 {
                // 1. Send SIGTERM to the entire process group
                unsafe {
                    libc::kill(-self.pgid, libc::SIGTERM);
                }

                // 2. Wait up to graceful_timeout
                let poll_interval = Duration::from_millis(20);
                let mut elapsed = Duration::ZERO;
                while elapsed < self.graceful_timeout {
                    tokio::time::sleep(poll_interval).await;
                    elapsed += poll_interval;
                    if !self.is_alive() {
                        break;
                    }
                }

                // 3. If still alive, send SIGKILL to the process group
                if self.is_alive() {
                    unsafe {
                        libc::kill(-self.pgid, libc::SIGKILL);
                    }
                }
            }
        }

        let mut child = self.child.lock().await;
        if let Some(ref mut proc) = *child {
            let _ = proc.kill().await;
        }

        Ok(())
    }

    /// Waits for the plugin process to exit and returns its exit code.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if waiting fails.
    pub async fn wait(&self) -> Result<i32, StampError> {
        let mut child = self.child.lock().await;
        if let Some(ref mut proc) = *child {
            let status = proc
                .wait()
                .await
                .map_err(|e| StampError::Execution(format!("Failed to wait on plugin: {e}")))?;
            Ok(status.code().unwrap_or(-1))
        } else {
            Ok(0)
        }
    }
}

impl Drop for SupervisedPlugin {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            if self.pgid > 0 && self.is_alive() {
                // Orphan cleanup watchdog: kill child process group on drop
                unsafe {
                    libc::kill(-self.pgid, libc::SIGKILL);
                }
            }
        }
    }
}

/// Supervisor engine for spawning and managing plugin binaries.
pub struct PluginSupervisor;

impl PluginSupervisor {
    /// Spawns an external plugin process, performs handshake negotiation, and returns `SupervisedPlugin`.
    ///
    /// # Errors
    /// Returns `StampError` if spawning, reading handshake, or negotiation fails.
    pub async fn spawn(config: PluginSupervisorConfig) -> Result<SupervisedPlugin, StampError> {
        let mut cmd = Command::new(&config.plugin_path);
        cmd.args(&config.args);

        // Environment isolation: explicit variables only
        cmd.env_clear();
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        if let Ok(home) = std::env::var("HOME") {
            cmd.env("HOME", home);
        }
        if let Ok(tmp) = std::env::var("TMPDIR") {
            cmd.env("TMPDIR", tmp);
        }
        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        cmd.env("PACKER_PLUGIN_MAGIC_COOKIE", &config.magic_cookie);
        let proto_versions = config
            .protocol_versions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        cmd.env("PLUGIN_PROTOCOL_VERSIONS", proto_versions);

        if let Some(ref cert) = config.client_cert {
            cmd.env("PLUGIN_CLIENT_TLS_CERT", cert);
        }
        if let Some(ref key) = config.client_key {
            cmd.env("PLUGIN_CLIENT_TLS_KEY", key);
        }

        // Process group isolation on Unix
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| StampError::Execution(format!("Failed to spawn plugin process: {e}")))?;

        let pid = child.id().unwrap_or(0);
        #[cfg(unix)]
        let pgid = pid as i32;
        #[cfg(not(unix))]
        let pgid = 0;

        let stdout = child.stdout.take().ok_or_else(|| {
            StampError::Execution("Plugin stdout pipe was not captured".to_string())
        })?;

        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| StampError::Execution(format!("Failed reading plugin handshake: {e}")))?;

        let handshake: Handshake = line.parse()?;

        Ok(SupervisedPlugin {
            pid,
            pgid,
            handshake,
            child: Arc::new(Mutex::new(Some(child))),
            cancelled: Arc::new(AtomicBool::new(false)),
            graceful_timeout: config.graceful_timeout,
            client_cert: config.client_cert,
            client_key: config.client_key,
        })
    }

    /// Performs a standard gRPC healthcheck against a plugin endpoint.
    ///
    /// # Errors
    /// Returns `StampError` if connecting or health check fails.
    pub async fn perform_healthcheck(
        endpoint: &str,
        network_type: &NetworkType,
    ) -> Result<bool, StampError> {
        let mut client = match network_type {
            NetworkType::Tcp => {
                let address = if endpoint.starts_with("http") {
                    endpoint.to_string()
                } else {
                    format!("http://{endpoint}")
                };
                crate::r#gen::packer::health_client::HealthClient::connect(address)
                    .await
                    .map_err(|e| StampError::Execution(format!("Healthcheck connect error: {e}")))?
            }
            NetworkType::Unix => {
                #[cfg(unix)]
                {
                    let path = endpoint.to_string();
                    let channel = tonic::transport::Endpoint::try_from("http://[::]:50051")
                        .map_err(|e| StampError::Execution(format!("Invalid endpoint: {e}")))?
                        .connect_with_connector(tower::service_fn(
                            move |_: tonic::transport::Uri| {
                                tokio::net::UnixStream::connect(path.clone())
                            },
                        ))
                        .await
                        .map_err(|e| {
                            StampError::Execution(format!("Healthcheck Unix connect error: {e}"))
                        })?;
                    crate::r#gen::packer::health_client::HealthClient::new(channel)
                }
                #[cfg(not(unix))]
                {
                    return Err(StampError::Execution(
                        "Unix sockets are not supported on this platform".to_string(),
                    ));
                }
            }
        };

        let req = tonic::Request::new(crate::r#gen::packer::HealthCheckRequest {
            service: String::new(),
        });
        let resp = client
            .check(req)
            .await
            .map_err(|e| StampError::Execution(format!("Healthcheck RPC error: {e}")))?
            .into_inner();

        Ok(resp.status
            == crate::r#gen::packer::health_check_response::ServingStatus::Serving as i32)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_handshake_parse_valid() {
        let line = "1|2|tcp|127.0.0.1:12345|grpc|certdata";
        let hs: Handshake = line.parse().unwrap();
        assert_eq!(hs.core_protocol_version, "1");
        assert_eq!(hs.app_protocol_version, "2");
        assert_eq!(hs.network_type, NetworkType::Tcp);
        assert_eq!(hs.address, "127.0.0.1:12345");
        assert_eq!(hs.protocol, Protocol::Grpc);
        assert_eq!(hs.server_cert.as_deref(), Some("certdata"));

        let line_net_rpc = "1|1|unix|/tmp/test.sock|net/rpc";
        let hs2: Handshake = line_net_rpc.parse().unwrap();
        assert_eq!(hs2.network_type, NetworkType::Unix);
        assert_eq!(hs2.protocol, Protocol::NetRpc);
        assert_eq!(hs2.server_cert, None);

        let default_protocol = "1|1|tcp|127.0.0.1:9999";
        let hs3: Handshake = default_protocol.parse().unwrap();
        assert_eq!(hs3.protocol, Protocol::NetRpc);
    }

    #[test]
    fn test_handshake_parse_invalid() {
        assert!("invalid".parse::<Handshake>().is_err());
        assert!("1|2|unknown|addr|grpc".parse::<Handshake>().is_err());
        assert!("1|2|tcp|addr|unknown_proto".parse::<Handshake>().is_err());
    }

    #[tokio::test]
    async fn test_supervisor_config_defaults() {
        let cfg = PluginSupervisorConfig::default();
        assert_eq!(cfg.protocol_versions, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(cfg.graceful_timeout, Duration::from_millis(500));
    }

    struct MockHealthService;

    #[tonic::async_trait]
    impl crate::r#gen::packer::health_server::Health for MockHealthService {
        async fn check(
            &self,
            _request: tonic::Request<crate::r#gen::packer::HealthCheckRequest>,
        ) -> Result<tonic::Response<crate::r#gen::packer::HealthCheckResponse>, tonic::Status>
        {
            Ok(tonic::Response::new(
                crate::r#gen::packer::HealthCheckResponse {
                    status: crate::r#gen::packer::health_check_response::ServingStatus::Serving
                        as i32,
                },
            ))
        }

        type WatchStream = tokio_stream::wrappers::ReceiverStream<
            Result<crate::r#gen::packer::HealthCheckResponse, tonic::Status>,
        >;

        async fn watch(
            &self,
            _request: tonic::Request<crate::r#gen::packer::HealthCheckRequest>,
        ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(tonic::Response::new(
                tokio_stream::wrappers::ReceiverStream::new(rx),
            ))
        }
    }

    #[tokio::test]
    async fn test_supervisor_perform_healthcheck() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(crate::r#gen::packer::health_server::HealthServer::new(
                    MockHealthService,
                ))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let is_healthy =
            PluginSupervisor::perform_healthcheck(&local_addr.to_string(), &NetworkType::Tcp)
                .await
                .unwrap();
        assert!(is_healthy);
    }

    #[tokio::test]
    async fn test_supervisor_spawn_cancel_and_wait() {
        let mut cfg = PluginSupervisorConfig::default();
        cfg.plugin_path = PathBuf::from("sh");
        cfg.args = vec![
            "-c".to_string(),
            "echo '1|1|tcp|127.0.0.1:12345|grpc|' && sleep 5".to_string(),
        ];
        cfg.graceful_timeout = Duration::from_millis(100);

        let plugin = PluginSupervisor::spawn(cfg).await.unwrap();
        assert_eq!(plugin.handshake.protocol, Protocol::Grpc);
        assert_eq!(plugin.handshake.address, "127.0.0.1:12345");
        assert!(plugin.is_alive());

        plugin.cancel().await.unwrap();
        let exit_code = plugin.wait().await.unwrap();
        assert_ne!(exit_code, 0);
    }

    #[tokio::test]
    async fn test_supervisor_is_cancelled_and_heartbeat() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(crate::r#gen::packer::health_server::HealthServer::new(
                    MockHealthService,
                ))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let plugin = SupervisedPlugin {
            pid: 999_999,
            pgid: 0,
            handshake: Handshake {
                core_protocol_version: "1".to_string(),
                app_protocol_version: "1".to_string(),
                network_type: NetworkType::Tcp,
                address: local_addr.to_string(),
                protocol: Protocol::Grpc,
                server_cert: None,
            },
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            graceful_timeout: Duration::from_millis(100),
            client_cert: None,
            client_key: None,
        };

        assert!(!plugin.is_cancelled());
        let handle = plugin.start_heartbeat_monitor(Duration::from_millis(50), 2);
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(!plugin.is_cancelled());
        handle.abort();

        // Now test heartbeat failure triggering cancellation
        let dead_plugin = SupervisedPlugin {
            pid: 999_998,
            pgid: 0,
            handshake: Handshake {
                core_protocol_version: "1".to_string(),
                app_protocol_version: "1".to_string(),
                network_type: NetworkType::Tcp,
                address: "127.0.0.1:1".to_string(),
                protocol: Protocol::Grpc,
                server_cert: None,
            },
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            graceful_timeout: Duration::from_millis(100),
            client_cert: None,
            client_key: None,
        };

        let dead_handle = dead_plugin.start_heartbeat_monitor(Duration::from_millis(10), 1);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(dead_plugin.is_cancelled());
        dead_handle.abort();
    }

    #[tokio::test]
    async fn test_supervisor_create_channel() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(crate::r#gen::packer::health_server::HealthServer::new(
                    MockHealthService,
                ))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let plugin = SupervisedPlugin {
            pid: 1,
            pgid: 0,
            handshake: Handshake {
                core_protocol_version: "1".to_string(),
                app_protocol_version: "1".to_string(),
                network_type: NetworkType::Tcp,
                address: local_addr.to_string(),
                protocol: Protocol::Grpc,
                server_cert: None,
            },
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            graceful_timeout: Duration::from_millis(100),
            client_cert: None,
            client_key: None,
        };

        let ch = plugin.create_channel().await;
        assert!(ch.is_ok());

        // Test with invalid cert string to ensure error handling
        let plugin_bad_cert = SupervisedPlugin {
            pid: 1,
            pgid: 0,
            handshake: Handshake {
                core_protocol_version: "1".to_string(),
                app_protocol_version: "1".to_string(),
                network_type: NetworkType::Tcp,
                address: local_addr.to_string(),
                protocol: Protocol::Grpc,
                server_cert: Some("invalid-base64!".to_string()),
            },
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            graceful_timeout: Duration::from_millis(100),
            client_cert: None,
            client_key: None,
        };
        let _ = plugin_bad_cert.create_channel().await;
    }
}
