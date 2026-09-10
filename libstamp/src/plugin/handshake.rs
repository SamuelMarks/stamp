//! Go-plugin stdout handshake protocol parser, formatter, and negotiation types.
//!
//! Official `HashiCorp` Packer plugins communicate their listener metadata over stdout
//! using the pipe-delimited format:
//! `CORE_PROTOCOL_VERSION|APP_PROTOCOL_VERSION|NETWORK_TYPE|ADDRESS|PROTOCOL[|SERVER_CERT]`

use crate::error::StampError;
use std::fmt;
use std::str::FromStr;

/// Official `HashiCorp` Packer plugin magic cookie key environment variable.
pub const PACKER_PLUGIN_MAGIC_COOKIE_KEY: &str = "PACKER_PLUGIN_MAGIC_COOKIE";

/// Official `HashiCorp` Packer plugin magic cookie expected value.
pub const PACKER_PLUGIN_MAGIC_COOKIE_VALUE: &str =
    "d602bf8f470bc67ca7faa0386276bbdd4330efaf76d1a219cb4d6991ca9872b2";

/// Environment variable specifying comma-separated supported plugin protocol versions.
pub const PLUGIN_PROTOCOL_VERSIONS_ENV: &str = "PLUGIN_PROTOCOL_VERSIONS";

/// Environment variable specifying the minimum port for bounded port allocation.
pub const PACKER_PLUGIN_MIN_PORT_ENV: &str = "PACKER_PLUGIN_MIN_PORT";

/// Environment variable specifying the maximum port for bounded port allocation.
pub const PACKER_PLUGIN_MAX_PORT_ENV: &str = "PACKER_PLUGIN_MAX_PORT";

/// Supported core protocol version (always 1 for `HashiCorp` go-plugin).
pub const CORE_PROTOCOL_VERSION: u32 = 1;

/// Minimum supported app protocol version (Packer legacy).
pub const MIN_APP_PROTOCOL_VERSION: u32 = 1;

/// Maximum supported app protocol version (Packer modern gRPC).
pub const MAX_APP_PROTOCOL_VERSION: u32 = 6;

/// Network transport type used by the plugin server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkType {
    /// TCP loopback or socket network.
    Tcp,
    /// Unix domain socket.
    Unix,
    /// Windows named pipe.
    NamedPipe,
}

impl NetworkType {
    /// Returns the canonical string representation for the handshake.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Unix => "unix",
            Self::NamedPipe => "namedpipe",
        }
    }
}

impl fmt::Display for NetworkType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for NetworkType {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "tcp" => Ok(Self::Tcp),
            "unix" => Ok(Self::Unix),
            "namedpipe" | "pipe" => Ok(Self::NamedPipe),
            other => Err(StampError::PluginHandshake(format!(
                "Unsupported network type in handshake: '{other}'"
            ))),
        }
    }
}

/// RPC protocol flavor spoken by the plugin server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// Legacy Go `net/rpc` protocol.
    NetRpc,
    /// Modern gRPC protocol over HTTP/2.
    Grpc,
}

impl Protocol {
    /// Returns the canonical protocol string for the handshake.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NetRpc => "net/rpc",
            Self::Grpc => "grpc",
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for Protocol {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "grpc" => Ok(Self::Grpc),
            "net/rpc" | "netrpc" => Ok(Self::NetRpc),
            other => Err(StampError::PluginHandshake(format!(
                "Unsupported RPC protocol in handshake: '{other}'"
            ))),
        }
    }
}

/// A parsed Go-plugin handshake line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// The core protocol version (typically 1).
    pub core_protocol_version: u32,
    /// The app protocol version negotiated (1 through 6).
    pub app_protocol_version: u32,
    /// The network type (`tcp`, `unix`, or `namedpipe`).
    pub network_type: NetworkType,
    /// The network address (e.g. `127.0.0.1:54321` or `/tmp/plugin.sock`).
    pub address: String,
    /// The RPC protocol (`grpc` or `net/rpc`).
    pub protocol: Protocol,
    /// Optional PEM or base64-encoded server certificate exchanged during mTLS handshake.
    pub server_cert: Option<String>,
}

impl Handshake {
    /// Creates a new `Handshake` instance.
    #[must_use]
    pub fn new(
        core_version: u32,
        app_version: u32,
        network_type: NetworkType,
        address: impl Into<String>,
        protocol: Protocol,
    ) -> Self {
        Self {
            core_protocol_version: core_version,
            app_protocol_version: app_version,
            network_type,
            address: address.into(),
            protocol,
            server_cert: None,
        }
    }

    /// Appends a server certificate for mutual TLS negotiation.
    #[must_use]
    pub fn with_server_cert(mut self, cert: impl Into<String>) -> Self {
        self.server_cert = Some(cert.into());
        self
    }

    /// Formats the handshake into the pipe-delimited line expected by Go-plugin clients.
    #[must_use]
    pub fn render(&self) -> String {
        let base = format!(
            "{}|{}|{}|{}|{}",
            self.core_protocol_version,
            self.app_protocol_version,
            self.network_type.as_str(),
            self.address,
            self.protocol.as_str()
        );
        if let Some(ref cert) = self.server_cert {
            format!("{base}|{cert}")
        } else {
            base
        }
    }

    /// Validates that this handshake satisfies supported core and app protocol version bounds.
    ///
    /// # Errors
    /// Returns `StampError::PluginHandshake` if versions are outside supported ranges.
    pub fn validate(&self) -> Result<(), StampError> {
        if self.core_protocol_version != CORE_PROTOCOL_VERSION {
            return Err(StampError::PluginHandshake(format!(
                "Incompatible core protocol version: expected {CORE_PROTOCOL_VERSION}, got {}",
                self.core_protocol_version
            )));
        }
        if self.app_protocol_version < MIN_APP_PROTOCOL_VERSION
            || self.app_protocol_version > MAX_APP_PROTOCOL_VERSION
        {
            return Err(StampError::PluginHandshake(format!(
                "Unsupported app protocol version: {}, supported range is {MIN_APP_PROTOCOL_VERSION}..={MAX_APP_PROTOCOL_VERSION}",
                self.app_protocol_version
            )));
        }
        Ok(())
    }
}

impl fmt::Display for Handshake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.render())
    }
}

impl FromStr for Handshake {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let line = s.trim();
        let parts: Vec<&str> = line.split('|').collect();

        if parts.len() < 4 {
            return Err(StampError::PluginHandshake(format!(
                "Malformed handshake line (expected at least 4 pipe-delimited fields, got {}): '{line}'",
                parts.len()
            )));
        }

        let core_protocol_version = parts[0].parse::<u32>().map_err(|e| {
            StampError::PluginHandshake(format!(
                "Invalid core protocol version integer '{}': {e}",
                parts[0]
            ))
        })?;

        let app_protocol_version = parts[1].parse::<u32>().map_err(|e| {
            StampError::PluginHandshake(format!(
                "Invalid app protocol version integer '{}': {e}",
                parts[1]
            ))
        })?;

        let network_type = parts[2].parse::<NetworkType>()?;
        let address = parts[3].to_string();

        let protocol = if parts.len() >= 5 {
            parts[4].parse::<Protocol>()?
        } else {
            Protocol::NetRpc
        };

        let server_cert = if parts.len() >= 6 && !parts[5].is_empty() {
            Some(parts[5].to_string())
        } else {
            None
        };

        let handshake = Self {
            core_protocol_version,
            app_protocol_version,
            network_type,
            address,
            protocol,
            server_cert,
        };

        handshake.validate()?;
        Ok(handshake)
    }
}

/// Verifies that the given magic cookie string matches the required `HashiCorp` Packer magic cookie.
///
/// # Errors
/// Returns `StampError::PluginHandshake` if the cookie value is mismatched.
pub fn validate_magic_cookie(cookie: &str) -> Result<(), StampError> {
    if cookie == PACKER_PLUGIN_MAGIC_COOKIE_VALUE {
        Ok(())
    } else {
        Err(StampError::PluginHandshake(format!(
            "Invalid magic cookie value '{cookie}', expected '{PACKER_PLUGIN_MAGIC_COOKIE_VALUE}'"
        )))
    }
}

/// Verifies that the current environment contains the required `PACKER_PLUGIN_MAGIC_COOKIE`.
///
/// # Errors
/// Returns `StampError::PluginHandshake` if the environment variable is missing or invalid.
pub fn verify_environment_magic_cookie() -> Result<(), StampError> {
    match std::env::var(PACKER_PLUGIN_MAGIC_COOKIE_KEY) {
        Ok(ref val) => validate_magic_cookie(val),
        Err(_) => Err(StampError::PluginHandshake(format!(
            "Missing required environment variable '{PACKER_PLUGIN_MAGIC_COOKIE_KEY}'"
        ))),
    }
}

/// Reads and parses the bounded port range from `PACKER_PLUGIN_MIN_PORT` and `PACKER_PLUGIN_MAX_PORT`.
///
/// Returns `Ok(Some((min, max)))` if both are set and valid, `Ok(None)` if neither is set.
///
/// # Errors
/// Returns `StampError::PluginHandshake` if ports are invalid numbers, one is missing, or `min > max`.
pub fn get_bounded_port_range() -> Result<Option<(u16, u16)>, StampError> {
    let min_var = std::env::var(PACKER_PLUGIN_MIN_PORT_ENV).ok();
    let max_var = std::env::var(PACKER_PLUGIN_MAX_PORT_ENV).ok();

    match (min_var, max_var) {
        (Some(min_s), Some(max_s)) => {
            let min = min_s.parse::<u16>().map_err(|e| {
                StampError::PluginHandshake(format!(
                    "Invalid {PACKER_PLUGIN_MIN_PORT_ENV} '{min_s}': {e}"
                ))
            })?;
            let max = max_s.parse::<u16>().map_err(|e| {
                StampError::PluginHandshake(format!(
                    "Invalid {PACKER_PLUGIN_MAX_PORT_ENV} '{max_s}': {e}"
                ))
            })?;
            if min > max {
                return Err(StampError::PluginHandshake(format!(
                    "{PACKER_PLUGIN_MIN_PORT_ENV} ({min}) cannot exceed {PACKER_PLUGIN_MAX_PORT_ENV} ({max})"
                )));
            }
            Ok(Some((min, max)))
        }
        (Some(_), None) | (None, Some(_)) => Err(StampError::PluginHandshake(format!(
            "Both {PACKER_PLUGIN_MIN_PORT_ENV} and {PACKER_PLUGIN_MAX_PORT_ENV} must be set together"
        ))),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_network_type_display_and_parse() {
        assert_eq!(NetworkType::Tcp.as_str(), "tcp");
        assert_eq!(NetworkType::Unix.as_str(), "unix");
        assert_eq!(NetworkType::NamedPipe.as_str(), "namedpipe");

        assert_eq!("TCP".parse::<NetworkType>().unwrap(), NetworkType::Tcp);
        assert_eq!("unix".parse::<NetworkType>().unwrap(), NetworkType::Unix);
        assert_eq!(
            "namedpipe".parse::<NetworkType>().unwrap(),
            NetworkType::NamedPipe
        );
        assert_eq!(
            "pipe".parse::<NetworkType>().unwrap(),
            NetworkType::NamedPipe
        );
        assert!("invalid".parse::<NetworkType>().is_err());
    }

    #[test]
    fn test_protocol_display_and_parse() {
        assert_eq!(Protocol::Grpc.as_str(), "grpc");
        assert_eq!(Protocol::NetRpc.as_str(), "net/rpc");

        assert_eq!("grpc".parse::<Protocol>().unwrap(), Protocol::Grpc);
        assert_eq!("GRPC".parse::<Protocol>().unwrap(), Protocol::Grpc);
        assert_eq!("net/rpc".parse::<Protocol>().unwrap(), Protocol::NetRpc);
        assert_eq!("netrpc".parse::<Protocol>().unwrap(), Protocol::NetRpc);
        assert!("unknown".parse::<Protocol>().is_err());
    }

    #[test]
    fn test_handshake_render_and_parse() {
        let hs = Handshake::new(1, 5, NetworkType::Tcp, "127.0.0.1:12345", Protocol::Grpc);
        assert_eq!(hs.render(), "1|5|tcp|127.0.0.1:12345|grpc");
        assert_eq!(format!("{hs}"), "1|5|tcp|127.0.0.1:12345|grpc");

        let parsed = "1|5|tcp|127.0.0.1:12345|grpc".parse::<Handshake>().unwrap();
        assert_eq!(parsed, hs);

        let hs_tls = hs.clone().with_server_cert("CERT_BASE64");
        assert_eq!(hs_tls.render(), "1|5|tcp|127.0.0.1:12345|grpc|CERT_BASE64");
        let parsed_tls = "1|5|tcp|127.0.0.1:12345|grpc|CERT_BASE64"
            .parse::<Handshake>()
            .unwrap();
        assert_eq!(parsed_tls, hs_tls);

        // NetRpc 4-part fallback
        let net_rpc = "1|2|unix|/tmp/test.sock".parse::<Handshake>().unwrap();
        assert_eq!(net_rpc.protocol, Protocol::NetRpc);
        assert_eq!(net_rpc.network_type, NetworkType::Unix);
    }

    #[test]
    fn test_handshake_parse_failures() {
        assert!("1|5".parse::<Handshake>().is_err());
        assert!(
            "abc|5|tcp|127.0.0.1:1234|grpc"
                .parse::<Handshake>()
                .is_err()
        );
        assert!(
            "1|xyz|tcp|127.0.0.1:1234|grpc"
                .parse::<Handshake>()
                .is_err()
        );
        // Incompatible core version
        assert!("2|5|tcp|127.0.0.1:1234|grpc".parse::<Handshake>().is_err());
        // App version out of bounds
        assert!("1|0|tcp|127.0.0.1:1234|grpc".parse::<Handshake>().is_err());
        assert!("1|7|tcp|127.0.0.1:1234|grpc".parse::<Handshake>().is_err());
    }

    #[test]
    fn test_magic_cookie_validation() {
        assert!(validate_magic_cookie(PACKER_PLUGIN_MAGIC_COOKIE_VALUE).is_ok());
        assert!(validate_magic_cookie("invalid_cookie").is_err());

        // Environment check
        unsafe {
            std::env::set_var(
                PACKER_PLUGIN_MAGIC_COOKIE_KEY,
                PACKER_PLUGIN_MAGIC_COOKIE_VALUE,
            );
        }
        assert!(verify_environment_magic_cookie().is_ok());

        unsafe {
            std::env::set_var(PACKER_PLUGIN_MAGIC_COOKIE_KEY, "wrong");
        }
        assert!(verify_environment_magic_cookie().is_err());

        unsafe {
            std::env::remove_var(PACKER_PLUGIN_MAGIC_COOKIE_KEY);
        }
        assert!(verify_environment_magic_cookie().is_err());
    }

    #[test]
    fn test_bounded_port_range_parsing() {
        unsafe {
            std::env::remove_var(PACKER_PLUGIN_MIN_PORT_ENV);
            std::env::remove_var(PACKER_PLUGIN_MAX_PORT_ENV);
        }
        assert_eq!(get_bounded_port_range().unwrap(), None);

        unsafe {
            std::env::set_var(PACKER_PLUGIN_MIN_PORT_ENV, "10000");
            std::env::set_var(PACKER_PLUGIN_MAX_PORT_ENV, "10050");
        }
        assert_eq!(get_bounded_port_range().unwrap(), Some((10000, 10050)));

        // Min > max
        unsafe {
            std::env::set_var(PACKER_PLUGIN_MIN_PORT_ENV, "20000");
            std::env::set_var(PACKER_PLUGIN_MAX_PORT_ENV, "10000");
        }
        assert!(get_bounded_port_range().is_err());

        // Single var set
        unsafe {
            std::env::remove_var(PACKER_PLUGIN_MAX_PORT_ENV);
        }
        assert!(get_bounded_port_range().is_err());

        // Clean up
        unsafe {
            std::env::remove_var(PACKER_PLUGIN_MIN_PORT_ENV);
        }
    }
}
