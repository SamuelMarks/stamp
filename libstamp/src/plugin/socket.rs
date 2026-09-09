//! Socket and transport primitives for Go-plugin IPC.
//!
//! Provides support for:
//! - Loopback TCP sockets with bounded port allocation (`PACKER_PLUGIN_MIN_PORT` ..= `PACKER_PLUGIN_MAX_PORT`).
//! - Unix domain sockets (`unix://<path>`) with automated RAII directory and socket file cleanup.
//! - Windows named pipe path utilities (`\.\pipe\<name>`).

use crate::error::StampError;
use crate::plugin::handshake::{NetworkType, get_bounded_port_range};
use std::path::{Path, PathBuf};
use tokio::net::TcpListener;

/// RAII cleanup guard for Unix domain sockets and ephemeral directories.
#[derive(Debug)]
pub struct UnixSocketGuard {
    /// Socket file path.
    socket_path: PathBuf,
    /// Parent directory to remove on drop if ephemeral.
    parent_dir: Option<PathBuf>,
}

impl UnixSocketGuard {
    /// Creates a new `UnixSocketGuard` for a specific socket path and optional directory.
    #[must_use]
    pub fn new(socket_path: PathBuf, parent_dir: Option<PathBuf>) -> Self {
        Self {
            socket_path,
            parent_dir,
        }
    }

    /// Creates an ephemeral Unix domain socket path in a new temporary directory.
    ///
    /// # Errors
    /// Returns `StampError::Io` if temporary directory creation fails.
    pub fn create_ephemeral() -> Result<Self, StampError> {
        let dir = tempfile::Builder::new()
            .prefix("packer-plugin-")
            .tempdir()
            .map_err(StampError::Io)?;
        let dir_path = dir.keep();
        let socket_path = dir_path.join("plugin.sock");
        Ok(Self {
            socket_path,
            parent_dir: Some(dir_path),
        })
    }

    /// Returns the socket path reference.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.socket_path
    }

    /// Returns the socket path formatted as a URI.
    #[must_use]
    pub fn as_uri(&self) -> String {
        format!("unix://{}", self.socket_path.display())
    }
}

impl Drop for UnixSocketGuard {
    fn drop(&mut self) {
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
        if let Some(ref dir) = self.parent_dir.as_ref().filter(|d| d.exists()) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Formats a Windows named pipe path into a canonical identifier.
#[must_use]
pub fn format_named_pipe_path(name: &str) -> String {
    let clean = name.strip_prefix(r"\\.\pipe\").unwrap_or(name);
    format!(r"\\.\pipe\{clean}")
}

/// Normalizes a connection address from the handshake string according to network type.
#[must_use]
pub fn normalize_address(network_type: NetworkType, raw_address: &str) -> String {
    match network_type {
        NetworkType::Tcp => {
            if raw_address.starts_with("http://") || raw_address.starts_with("https://") {
                raw_address.to_string()
            } else {
                format!("http://{raw_address}")
            }
        }
        NetworkType::Unix => raw_address.trim_start_matches("unix://").to_string(),
        NetworkType::NamedPipe => format_named_pipe_path(raw_address),
    }
}

/// Binds a loopback TCP listener respecting bounded port ranges if configured.
///
/// If `PACKER_PLUGIN_MIN_PORT` and `PACKER_PLUGIN_MAX_PORT` are defined, searches for
/// an available port sequentially in `[min, max]`. Otherwise binds to port `0` for OS allocation.
///
/// # Errors
/// Returns `StampError::Io` or `StampError::PluginHandshake` if binding fails or no ports are available in range.
pub async fn bind_bounded_tcp_listener() -> Result<TcpListener, StampError> {
    if let Some((min, max)) = get_bounded_port_range()? {
        for port in min..=max {
            if let Ok(listener) = TcpListener::bind(format!("127.0.0.1:{port}")).await {
                return Ok(listener);
            }
        }
        Err(StampError::PluginHandshake(format!(
            "No free ports available in configured range {min}..={max}"
        )))
    } else {
        TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(StampError::Io)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::plugin::handshake::{PACKER_PLUGIN_MAX_PORT_ENV, PACKER_PLUGIN_MIN_PORT_ENV};

    #[tokio::test]
    async fn test_unix_socket_guard_lifecycle() {
        let guard = UnixSocketGuard::create_ephemeral().unwrap();
        assert!(guard.path().to_string_lossy().contains("plugin.sock"));
        assert!(guard.as_uri().starts_with("unix://"));
        let p = guard.path().to_path_buf();
        let d = guard.parent_dir.clone().unwrap();

        // Write a dummy file to simulate socket creation
        std::fs::write(&p, b"socket").unwrap();
        assert!(p.exists());

        drop(guard);
        assert!(!p.exists());
        assert!(!d.exists());
    }

    #[test]
    fn test_format_named_pipe_path() {
        assert_eq!(
            format_named_pipe_path("packer-plugin"),
            r"\\.\pipe\packer-plugin"
        );
        assert_eq!(
            format_named_pipe_path(r"\\.\pipe\packer-plugin"),
            r"\\.\pipe\packer-plugin"
        );
    }

    #[test]
    fn test_normalize_address() {
        assert_eq!(
            normalize_address(NetworkType::Tcp, "127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            normalize_address(NetworkType::Tcp, "http://127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            normalize_address(NetworkType::Unix, "unix:///tmp/p.sock"),
            "/tmp/p.sock"
        );
        assert_eq!(
            normalize_address(NetworkType::NamedPipe, "test_pipe"),
            r"\\.\pipe\test_pipe"
        );
    }

    #[tokio::test]
    async fn test_bind_bounded_tcp_listener_default() {
        unsafe {
            std::env::remove_var(PACKER_PLUGIN_MIN_PORT_ENV);
            std::env::remove_var(PACKER_PLUGIN_MAX_PORT_ENV);
        }
        let listener = bind_bounded_tcp_listener().await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port > 0);
    }

    #[tokio::test]
    async fn test_bind_bounded_tcp_listener_bounded() {
        // Reserve an arbitrary free port range
        let temp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = temp.local_addr().unwrap().port();
        drop(temp);

        // Allow a small range in case of rapid port re-bind semantics
        let max_port = port.saturating_add(5);

        unsafe {
            std::env::set_var(PACKER_PLUGIN_MIN_PORT_ENV, port.to_string());
            std::env::set_var(PACKER_PLUGIN_MAX_PORT_ENV, max_port.to_string());
        }

        let listener = bind_bounded_tcp_listener().await.unwrap();
        let bound_port = listener.local_addr().unwrap().port();
        assert!(bound_port >= port && bound_port <= max_port);

        // Conflict test: set single port and occupy it
        let occ_port = bound_port;
        unsafe {
            std::env::set_var(PACKER_PLUGIN_MIN_PORT_ENV, occ_port.to_string());
            std::env::set_var(PACKER_PLUGIN_MAX_PORT_ENV, occ_port.to_string());
        }

        let res = bind_bounded_tcp_listener().await;
        assert!(res.is_err());

        unsafe {
            std::env::remove_var(PACKER_PLUGIN_MIN_PORT_ENV);
            std::env::remove_var(PACKER_PLUGIN_MAX_PORT_ENV);
        }
    }
}
