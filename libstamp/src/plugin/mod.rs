//! Go-plugin IPC and remote out-of-process plugin subsystem.
//!
//! Implements parity with `HashiCorp`'s `go-plugin` protocol:
//! - Stdout handshake protocol negotiation (versions 1-6).
//! - Ephemeral mutual TLS (mTLS) certificate exchange and encryption.
//! - Multi-transport socket binding (TCP bounded ranges, Unix domain sockets with cleanup, Windows named pipes).
//! - Plugin child process lifecycle, health monitoring, and zombie cleanup.

pub mod broker;
pub mod handshake;
pub mod process;
pub mod socket;
pub mod tls;
pub mod yamux;

pub use broker::PluginBroker;
pub use handshake::{
    Handshake, NetworkType, Protocol, validate_magic_cookie, verify_environment_magic_cookie,
};
pub use process::PluginSubprocess;
pub use socket::{UnixSocketGuard, bind_bounded_tcp_listener, normalize_address};
pub use tls::MtlsCertificates;
pub use yamux::{SessionRole, YamuxConfig, YamuxSession, YamuxStream};
