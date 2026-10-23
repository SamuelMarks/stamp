//! Strongly typed primitives for Stamp configuration and operations.

use crate::error::StampError;
use std::fmt;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

/// A strongly-typed non-empty string.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    /// Parse and validate that a string is not empty or whitespace-only.
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if the string is empty or only whitespace.
    pub fn new(value: impl Into<String>) -> Result<Self, StampError> {
        let s = value.into();
        if s.trim().is_empty() {
            return Err(StampError::InvalidType(
                "NonEmptyString cannot be empty or whitespace".to_string(),
            ));
        }
        Ok(Self(s))
    }

    /// Returns a string slice of the underlying value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for NonEmptyString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NonEmptyString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for NonEmptyString {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// A strongly-typed secret string that redacts its contents in `Debug` and `Display` output.
#[derive(Clone, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct SecretString(pub String);

impl SecretString {
    /// Create a new `SecretString`.
    #[must_use]
    pub const fn new(secret: String) -> Self {
        Self(secret)
    }

    /// Expose the underlying secret string slice.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Returns `true` if the underlying secret is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "***REDACTED***")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "***REDACTED***")
    }
}

impl From<String> for SecretString {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SecretString {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// A strongly-typed network port.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Port(pub u16);

impl Port {
    /// Create a new `Port`.
    #[must_use]
    pub const fn new(port: u16) -> Self {
        Self(port)
    }

    /// Retrieve the underlying port number.
    #[must_use]
    pub const fn get(&self) -> u16 {
        self.0
    }
}

impl Default for Port {
    /// Return the default port (22).
    fn default() -> Self {
        Self(22)
    }
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Port {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u16>()
            .map(Self)
            .map_err(|e| StampError::InvalidType(format!("invalid port '{s}': {e}")))
    }
}

/// A validated inclusive network port range (`min <= max`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct PortRange {
    /// Minimum inclusive port.
    min: Port,
    /// Maximum inclusive port.
    max: Port,
}

impl PortRange {
    /// Create a new `PortRange`, validating that `min <= max`.
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if `min > max`.
    pub fn new(min: u16, max: u16) -> Result<Self, StampError> {
        if min > max {
            return Err(StampError::InvalidType(format!(
                "invalid port range: min ({min}) > max ({max})"
            )));
        }
        Ok(Self {
            min: Port::new(min),
            max: Port::new(max),
        })
    }

    /// Returns the minimum port in the range.
    #[must_use]
    pub const fn min_port(&self) -> Port {
        self.min
    }

    /// Returns the maximum port in the range.
    #[must_use]
    pub const fn max_port(&self) -> Port {
        self.max
    }

    /// Returns the range as a `(u16, u16)` tuple.
    #[must_use]
    pub const fn as_tuple(&self) -> (u16, u16) {
        (self.min.get(), self.max.get())
    }
}

impl Default for PortRange {
    fn default() -> Self {
        Self {
            min: Port::new(8000),
            max: Port::new(9000),
        }
    }
}

impl fmt::Display for PortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.min, self.max)
    }
}

/// A strongly-typed host address (either an IP address or a hostname).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum HostAddress {
    /// An IPv4 or IPv6 address.
    Ip(std::net::IpAddr),
    /// A DNS hostname.
    Hostname(NonEmptyString),
}

impl HostAddress {
    /// Parse a host address string as either an `IpAddr` or a `NonEmptyString` hostname.
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if the string is empty or whitespace.
    pub fn parse(s: &str) -> Result<Self, StampError> {
        let trimmed = s.trim();
        if let Ok(ip) = trimmed.parse::<std::net::IpAddr>() {
            Ok(Self::Ip(ip))
        } else {
            Ok(Self::Hostname(NonEmptyString::new(trimmed)?))
        }
    }
}

impl fmt::Display for HostAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip(ip) => write!(f, "{ip}"),
            Self::Hostname(h) => write!(f, "{h}"),
        }
    }
}

impl FromStr for HostAddress {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Strongly-typed communicator protocol kind.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum CommunicatorKind {
    /// No communicator (`"none"`).
    None,
    /// SSH communicator (`"ssh"`).
    #[default]
    Ssh,
    /// Windows Remote Management communicator (`"winrm"`).
    WinRm,
    /// Docker container communicator (`"docker"`).
    Docker,
    /// Podman container communicator (`"podman"`).
    Podman,
    /// Local chroot communicator (`"chroot"`).
    Chroot,
    /// AWS SSM Session Manager communicator (`"ssm"`).
    Ssm,
    /// gRPC proxy communicator (`"grpc-proxy"`).
    GrpcProxy,
    /// Mock communicator (`"mock"`).
    Mock,
}

impl fmt::Display for CommunicatorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::None => "none",
            Self::Ssh => "ssh",
            Self::WinRm => "winrm",
            Self::Docker => "docker",
            Self::Podman => "podman",
            Self::Chroot => "chroot",
            Self::Ssm => "ssm",
            Self::GrpcProxy => "grpc-proxy",
            Self::Mock => "mock",
        };
        write!(f, "{s}")
    }
}

impl FromStr for CommunicatorKind {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "ssh" => Ok(Self::Ssh),
            "winrm" => Ok(Self::WinRm),
            "docker" => Ok(Self::Docker),
            "podman" => Ok(Self::Podman),
            "chroot" => Ok(Self::Chroot),
            "ssm" => Ok(Self::Ssm),
            "grpc-proxy" | "grpc_proxy" => Ok(Self::GrpcProxy),
            "mock" => Ok(Self::Mock),
            other => Err(StampError::InvalidType(format!(
                "unknown communicator kind '{other}'"
            ))),
        }
    }
}

/// Strongly-typed disk/appliance image export format.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum ExportFormat {
    /// QEMU Copy-On-Write v2 (`"qcow2"`).
    #[default]
    Qcow2,
    /// Raw disk image (`"raw"`).
    Raw,
    /// VMware Virtual Machine Disk (`"vmdk"`).
    Vmdk,
    /// VirtualBox Disk Image (`"vdi"`).
    Vdi,
    /// Hyper-V Virtual Hard Disk (`"vhd"`).
    Vhd,
    /// Hyper-V Virtual Hard Disk v2 (`"vhdx"`).
    Vhdx,
    /// Open Virtual Appliance archive (`"ova"`).
    Ova,
    /// Open Virtualization Format directory/descriptor (`"ovf"`).
    Ovf,
    /// Tar archive (`"tar"`).
    Tar,
    /// Vagrant box archive (`"box"`).
    Box,
}

impl fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Qcow2 => "qcow2",
            Self::Raw => "raw",
            Self::Vmdk => "vmdk",
            Self::Vdi => "vdi",
            Self::Vhd => "vhd",
            Self::Vhdx => "vhdx",
            Self::Ova => "ova",
            Self::Ovf => "ovf",
            Self::Tar => "tar",
            Self::Box => "box",
        };
        write!(f, "{s}")
    }
}

impl FromStr for ExportFormat {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "qcow2" => Ok(Self::Qcow2),
            "raw" | "img" => Ok(Self::Raw),
            "vmdk" => Ok(Self::Vmdk),
            "vdi" => Ok(Self::Vdi),
            "vhd" => Ok(Self::Vhd),
            "vhdx" => Ok(Self::Vhdx),
            "ova" => Ok(Self::Ova),
            "ovf" => Ok(Self::Ovf),
            "tar" => Ok(Self::Tar),
            "box" => Ok(Self::Box),
            other => Err(StampError::InvalidType(format!(
                "unknown export format '{other}'"
            ))),
        }
    }
}

/// A strongly-typed timeout duration.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Timeout(pub Duration);

impl Timeout {
    /// Create a new `Timeout` from a `Duration`.
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        Self(duration)
    }

    /// Retrieve the underlying duration.
    #[must_use]
    pub const fn get(&self) -> Duration {
        self.0
    }
}

impl Default for Timeout {
    /// Return the default timeout duration (30 seconds).
    fn default() -> Self {
        Self(Duration::from_secs(30))
    }
}

impl fmt::Display for Timeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}s", self.0.as_secs())
    }
}

impl FromStr for Timeout {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if let Some(ms_str) = trimmed.strip_suffix("ms")
            && let Ok(ms) = ms_str.parse::<u64>()
        {
            return Ok(Self(Duration::from_millis(ms)));
        }
        if let Some(s_str) = trimmed.strip_suffix('s')
            && let Ok(secs) = s_str.parse::<u64>()
        {
            return Ok(Self(Duration::from_secs(secs)));
        }
        if let Some(m_str) = trimmed.strip_suffix('m')
            && let Ok(mins) = m_str.parse::<u64>()
        {
            return Ok(Self(Duration::from_secs(mins * 60)));
        }
        if let Some(h_str) = trimmed.strip_suffix('h')
            && let Ok(hours) = h_str.parse::<u64>()
        {
            return Ok(Self(Duration::from_secs(hours * 3600)));
        }
        if let Ok(secs) = trimmed.parse::<u64>() {
            return Ok(Self(Duration::from_secs(secs)));
        }
        Err(StampError::InvalidType(format!(
            "invalid duration string '{s}'"
        )))
    }
}

/// A strongly-typed file path wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct FilePath(pub PathBuf);

impl FilePath {
    /// Create a new `FilePath` from a `PathBuf`.
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// Retrieve a reference to the underlying `PathBuf`.
    #[must_use]
    pub const fn get(&self) -> &PathBuf {
        &self.0
    }

    /// Retrieve a reference to the underlying `Path`.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl fmt::Display for FilePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl FromStr for FilePath {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(PathBuf::from(s)))
    }
}

/// A strongly-typed Amazon Machine Image (AMI) identifier.
///
/// Validates that the identifier starts with `ami-` followed by 8 to 17 hex characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct AmiId(pub String);

impl AmiId {
    /// Parse and validate an `AmiId`.
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if the AMI format is invalid.
    pub fn parse(s: &str) -> Result<Self, StampError> {
        let Some(hex_part) = s.strip_prefix("ami-") else {
            return Err(StampError::InvalidType(format!(
                "AMI ID must start with 'ami-': {s}"
            )));
        };

        if hex_part.len() < 8 || hex_part.len() > 17 {
            return Err(StampError::InvalidType(format!(
                "AMI ID hex suffix must be 8-17 characters, got {}: {s}",
                hex_part.len()
            )));
        }

        if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(StampError::InvalidType(format!(
                "AMI ID hex suffix contains non-hexadecimal characters: {s}"
            )));
        }

        Ok(Self(s.to_string()))
    }

    /// Returns a string slice of the underlying AMI ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AmiId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for AmiId {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// A strongly-typed IPv4 CIDR block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ipv4Cidr(pub ipnet::Ipv4Net);

impl Ipv4Cidr {
    /// Parse an IPv4 CIDR string (e.g. `10.0.0.0/16`).
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if the CIDR format is invalid.
    pub fn parse(s: &str) -> Result<Self, StampError> {
        s.parse::<ipnet::Ipv4Net>()
            .map(Self)
            .map_err(|e| StampError::InvalidType(format!("invalid IPv4 CIDR '{s}': {e}")))
    }

    /// Retrieve the underlying `ipnet::Ipv4Net`.
    #[must_use]
    pub const fn get(&self) -> ipnet::Ipv4Net {
        self.0
    }
}

impl fmt::Display for Ipv4Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Ipv4Cidr {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl serde::Serialize for Ipv4Cidr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Ipv4Cidr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// A strongly-typed IPv6 CIDR block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ipv6Cidr(pub ipnet::Ipv6Net);

impl Ipv6Cidr {
    /// Parse an IPv6 CIDR string (e.g. `2001:db8::/32`).
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if the CIDR format is invalid.
    pub fn parse(s: &str) -> Result<Self, StampError> {
        s.parse::<ipnet::Ipv6Net>()
            .map(Self)
            .map_err(|e| StampError::InvalidType(format!("invalid IPv6 CIDR '{s}': {e}")))
    }

    /// Retrieve the underlying `ipnet::Ipv6Net`.
    #[must_use]
    pub const fn get(&self) -> ipnet::Ipv6Net {
        self.0
    }
}

impl fmt::Display for Ipv6Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Ipv6Cidr {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl serde::Serialize for Ipv6Cidr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Ipv6Cidr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// A strongly-typed MAC address (EUI-48).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    /// Create a new `MacAddress` from raw octets.
    #[must_use]
    pub const fn new(octets: [u8; 6]) -> Self {
        Self(octets)
    }

    /// Parse a MAC address string in colon or hyphen format (`00:1A:2B:3C:4D:5E`).
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if parsing fails.
    pub fn parse(s: &str) -> Result<Self, StampError> {
        let parts: Vec<&str> = if s.contains(':') {
            s.split(':').collect()
        } else if s.contains('-') {
            s.split('-').collect()
        } else {
            return Err(StampError::InvalidType(format!(
                "MAC address must be delimited with ':' or '-': {s}"
            )));
        };

        if parts.len() != 6 {
            return Err(StampError::InvalidType(format!(
                "MAC address must have 6 octets, got {}: {s}",
                parts.len()
            )));
        }

        let mut octets = [0u8; 6];
        for (i, part) in parts.iter().enumerate() {
            if part.len() != 2 {
                return Err(StampError::InvalidType(format!(
                    "MAC address octet must be 2 hex characters: '{part}' in '{s}'"
                )));
            }
            octets[i] = u8::from_str_radix(part, 16).map_err(|e| {
                StampError::InvalidType(format!("invalid hex octet '{part}' in '{s}': {e}"))
            })?;
        }

        Ok(Self(octets))
    }

    /// Returns the underlying 6 bytes.
    #[must_use]
    pub const fn octets(&self) -> [u8; 6] {
        self.0
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

impl FromStr for MacAddress {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// A strongly-typed memory size in Megabytes.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct MemoryMb(pub u64);

/// A strongly-typed memory size in Gigabytes.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct MemoryGb(pub u64);

impl MemoryMb {
    /// Create a new `MemoryMb`.
    #[must_use]
    pub const fn new(mb: u64) -> Self {
        Self(mb)
    }

    /// Convert to Gigabytes rounding down.
    #[must_use]
    pub const fn to_gb(&self) -> MemoryGb {
        MemoryGb(self.0 / 1024)
    }

    /// Retrieve the underlying megabyte count.
    #[must_use]
    pub const fn get(&self) -> u64 {
        self.0
    }
}

impl MemoryGb {
    /// Create a new `MemoryGb`.
    #[must_use]
    pub const fn new(gb: u64) -> Self {
        Self(gb)
    }

    /// Convert to Megabytes.
    #[must_use]
    pub const fn to_mb(&self) -> MemoryMb {
        MemoryMb(self.0 * 1024)
    }

    /// Retrieve the underlying gigabyte count.
    #[must_use]
    pub const fn get(&self) -> u64 {
        self.0
    }
}

impl From<MemoryGb> for MemoryMb {
    fn from(gb: MemoryGb) -> Self {
        gb.to_mb()
    }
}

impl fmt::Display for MemoryMb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}MB", self.0)
    }
}

impl fmt::Display for MemoryGb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}GB", self.0)
    }
}

/// A strongly-typed CPU core count guaranteed to be non-zero.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct CpuCount(pub NonZeroU32);

impl CpuCount {
    /// Create a new `CpuCount`.
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if `count` is 0.
    pub fn new(count: u32) -> Result<Self, StampError> {
        NonZeroU32::new(count).map(Self).ok_or_else(|| {
            StampError::InvalidType("CPU count must be greater than zero".to_string())
        })
    }

    /// Retrieve the underlying CPU count as `u32`.
    #[must_use]
    pub const fn get(&self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for CpuCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for CpuCount {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let count = s
            .parse::<u32>()
            .map_err(|e| StampError::InvalidType(format!("invalid CPU count '{s}': {e}")))?;
        Self::new(count)
    }
}

/// A strongly-typed disk size in Megabytes.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct DiskSizeMb(pub u64);

impl DiskSizeMb {
    /// Create a new `DiskSizeMb`.
    #[must_use]
    pub const fn new(size_mb: u64) -> Self {
        Self(size_mb)
    }

    /// Convert the disk size to bytes.
    #[must_use]
    pub const fn to_bytes(&self) -> u64 {
        self.0 * 1024 * 1024
    }

    /// Retrieve the underlying megabyte count.
    #[must_use]
    pub const fn get(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for DiskSizeMb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}MB", self.0)
    }
}

impl FromStr for DiskSizeMb {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(Self)
            .map_err(|e| StampError::InvalidType(format!("invalid disk size MB '{s}': {e}")))
    }
}

/// A strongly-typed disk size in Gigabytes.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct DiskSizeGb(pub u64);

impl DiskSizeGb {
    /// Create a new `DiskSizeGb`.
    #[must_use]
    pub const fn new(size: u64) -> Self {
        Self(size)
    }

    /// Convert the disk size to bytes.
    #[must_use]
    pub const fn to_bytes(&self) -> u64 {
        self.0 * 1024 * 1024 * 1024
    }

    /// Retrieve the underlying gigabyte count.
    #[must_use]
    pub const fn get(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for DiskSizeGb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}GB", self.0)
    }
}

impl FromStr for DiskSizeGb {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(Self)
            .map_err(|e| StampError::InvalidType(format!("invalid disk size '{s}': {e}")))
    }
}

/// A strongly-typed plugin source address (e.g. `github.com/hashicorp/amazon` or `hashicorp/amazon`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PluginAddress(pub String);

impl PluginAddress {
    /// Parse and validate a `PluginAddress`.
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if the format does not contain at least namespace and type.
    pub fn parse(s: &str) -> Result<Self, StampError> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
            return Err(StampError::InvalidType(format!(
                "Plugin address must be 'hostname/namespace/type' or 'namespace/type': '{s}'"
            )));
        }
        Ok(Self(s.to_string()))
    }

    /// Returns the hostname of the plugin source, defaulting to `github.com` if omitted.
    #[must_use]
    pub fn hostname(&self) -> &str {
        let parts: Vec<&str> = self.0.split('/').collect();
        if parts.len() >= 3 {
            parts[0]
        } else {
            "github.com"
        }
    }

    /// Returns the namespace / organization of the plugin source.
    #[must_use]
    pub fn namespace(&self) -> &str {
        let parts: Vec<&str> = self.0.split('/').collect();
        if parts.len() >= 3 { parts[1] } else { parts[0] }
    }

    /// Returns the plugin type (the final path component).
    #[must_use]
    pub fn plugin_type(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Returns the full repository name following standard naming conventions (e.g. `packer-plugin-amazon`).
    #[must_use]
    pub fn full_repo_name(&self) -> String {
        let pt = self.plugin_type();
        if pt.starts_with("packer-plugin-") {
            pt.to_string()
        } else {
            format!("packer-plugin-{pt}")
        }
    }

    /// Returns a string slice of the address.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for PluginAddress {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// A strongly-typed semantic version requirement constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVerConstraint(pub semver::VersionReq);

impl SemVerConstraint {
    /// Parse a semantic version constraint string (e.g. `>= 1.2.0, < 2.0.0`, `~> 1.2.0`).
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if parsing fails.
    pub fn parse(s: &str) -> Result<Self, StampError> {
        let normalized = s.replace("~>", "~");
        semver::VersionReq::parse(normalized.trim())
            .map(Self)
            .map_err(|e| StampError::InvalidType(format!("invalid semver constraint '{s}': {e}")))
    }

    /// Check if a given `semver::Version` satisfies this constraint.
    #[must_use]
    pub fn matches(&self, version: &semver::Version) -> bool {
        self.0.matches(version)
    }
}

impl fmt::Display for SemVerConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SemVerConstraint {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl serde::Serialize for SemVerConstraint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for SemVerConstraint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// A strongly-typed SHA-256 hexadecimal checksum.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Sha256Checksum(pub String);

impl Sha256Checksum {
    /// Parse and validate a 64-character hex SHA-256 checksum.
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if the checksum is not a 64-character hexadecimal string.
    pub fn parse(s: &str) -> Result<Self, StampError> {
        let trimmed = s.trim();
        if trimmed.len() != 64 {
            return Err(StampError::InvalidType(format!(
                "SHA-256 checksum must be exactly 64 characters, got {}: '{s}'",
                trimmed.len()
            )));
        }
        if !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(StampError::InvalidType(format!(
                "SHA-256 checksum must only contain hexadecimal characters: '{s}'"
            )));
        }
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    /// Returns a string slice of the lowercase hex digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha256Checksum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Sha256Checksum {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// A strongly-typed SHA-512 hexadecimal checksum.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Sha512Checksum(pub String);

impl Sha512Checksum {
    /// Parse and validate a 128-character hex SHA-512 checksum.
    ///
    /// # Errors
    /// Returns `StampError::InvalidType` if the checksum is not a 128-character hexadecimal string.
    pub fn parse(s: &str) -> Result<Self, StampError> {
        let trimmed = s.trim();
        if trimmed.len() != 128 {
            return Err(StampError::InvalidType(format!(
                "SHA-512 checksum must be exactly 128 characters, got {}: '{s}'",
                trimmed.len()
            )));
        }
        if !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(StampError::InvalidType(format!(
                "SHA-512 checksum must only contain hexadecimal characters: '{s}'"
            )));
        }
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    /// Returns a string slice of the lowercase hex digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha512Checksum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Sha512Checksum {
    type Err = StampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_empty_string() -> Result<(), StampError> {
        let s = NonEmptyString::new("hello")?;
        assert_eq!(s.as_str(), "hello");
        assert_eq!(s.as_ref(), "hello");
        assert_eq!(s.to_string(), "hello");
        assert_eq!(NonEmptyString::from_str("world")?.as_str(), "world");
        assert!(NonEmptyString::new("").is_err());
        assert!(NonEmptyString::new("   ").is_err());
        Ok(())
    }

    #[test]
    fn test_secret_string() {
        let sec = SecretString::new("super-secret".to_string());
        assert_eq!(sec.expose_secret(), "super-secret");
        assert!(!sec.is_empty());
        assert_eq!(format!("{sec:?}"), "***REDACTED***");
        assert_eq!(format!("{sec}"), "***REDACTED***");

        let from_str_ref = SecretString::from("s1");
        let from_string = SecretString::from("s1".to_string());
        assert_eq!(from_str_ref, from_string);
        assert!(SecretString::default().is_empty());
    }

    #[test]
    fn test_port_and_port_range() -> Result<(), StampError> {
        let port = Port::new(8080);
        assert_eq!(port.get(), 8080);
        let port_clone = port;
        assert_eq!(port, port_clone);
        assert_eq!(port.to_string(), "8080");
        assert_eq!(Port::default().get(), 22);
        assert_eq!(Port::from_str("9090")?.get(), 9090);
        assert!(Port::from_str("invalid").is_err());

        let range = PortRange::new(8000, 8100)?;
        assert_eq!(range.min_port().get(), 8000);
        assert_eq!(range.max_port().get(), 8100);
        assert_eq!(range.as_tuple(), (8000, 8100));
        assert_eq!(range.to_string(), "8000-8100");
        assert_eq!(PortRange::default().as_tuple(), (8000, 9000));
        assert!(PortRange::new(9000, 8000).is_err());
        Ok(())
    }

    #[test]
    fn test_host_address_communicator_and_export_format() -> Result<(), StampError> {
        let ip_host = HostAddress::from_str("127.0.0.1")?;
        assert_eq!(ip_host.to_string(), "127.0.0.1");
        let dns_host = HostAddress::parse("example.com")?;
        assert_eq!(dns_host.to_string(), "example.com");
        assert!(HostAddress::parse("   ").is_err());

        assert_eq!(CommunicatorKind::default(), CommunicatorKind::Ssh);
        for (input, expected, display_str) in [
            ("none", CommunicatorKind::None, "none"),
            ("ssh", CommunicatorKind::Ssh, "ssh"),
            ("winrm", CommunicatorKind::WinRm, "winrm"),
            ("docker", CommunicatorKind::Docker, "docker"),
            ("podman", CommunicatorKind::Podman, "podman"),
            ("chroot", CommunicatorKind::Chroot, "chroot"),
            ("ssm", CommunicatorKind::Ssm, "ssm"),
            ("grpc-proxy", CommunicatorKind::GrpcProxy, "grpc-proxy"),
            ("grpc_proxy", CommunicatorKind::GrpcProxy, "grpc-proxy"),
            ("mock", CommunicatorKind::Mock, "mock"),
        ] {
            let parsed = CommunicatorKind::from_str(input)?;
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), display_str);
        }
        assert!(CommunicatorKind::from_str("unknown").is_err());

        assert_eq!(ExportFormat::default(), ExportFormat::Qcow2);
        for (input, expected, display_str) in [
            ("qcow2", ExportFormat::Qcow2, "qcow2"),
            ("raw", ExportFormat::Raw, "raw"),
            ("img", ExportFormat::Raw, "raw"),
            ("vmdk", ExportFormat::Vmdk, "vmdk"),
            ("vdi", ExportFormat::Vdi, "vdi"),
            ("vhd", ExportFormat::Vhd, "vhd"),
            ("vhdx", ExportFormat::Vhdx, "vhdx"),
            ("ova", ExportFormat::Ova, "ova"),
            ("ovf", ExportFormat::Ovf, "ovf"),
            ("tar", ExportFormat::Tar, "tar"),
            ("box", ExportFormat::Box, "box"),
        ] {
            let parsed = ExportFormat::from_str(input)?;
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), display_str);
        }
        assert!(ExportFormat::from_str("unknown").is_err());
        Ok(())
    }

    #[test]
    fn test_timeout() -> Result<(), StampError> {
        let duration = Duration::from_secs(30);
        let timeout = Timeout::new(duration);
        assert_eq!(timeout.get(), duration);
        let timeout_clone = timeout;
        assert_eq!(timeout, timeout_clone);
        assert_eq!(timeout.to_string(), "30s");
        assert_eq!(Timeout::default().get(), Duration::from_secs(30));
        assert_eq!(
            Timeout::from_str("500ms")?.get(),
            Duration::from_millis(500)
        );
        assert_eq!(Timeout::from_str("10s")?.get(), Duration::from_secs(10));
        assert_eq!(Timeout::from_str("2m")?.get(), Duration::from_secs(120));
        assert_eq!(Timeout::from_str("1h")?.get(), Duration::from_secs(3600));
        assert_eq!(Timeout::from_str("45")?.get(), Duration::from_secs(45));
        assert!(Timeout::from_str("invalid").is_err());
        Ok(())
    }

    #[test]
    fn test_filepath() -> Result<(), StampError> {
        let p = PathBuf::from("/etc/hosts");
        let fp = FilePath::new(p.clone());
        assert_eq!(fp.get(), &p);
        assert_eq!(fp.as_path(), Path::new("/etc/hosts"));
        assert_eq!(fp.to_string(), "/etc/hosts");
        assert_eq!(FilePath::from_str("/var/log")?.to_string(), "/var/log");
        Ok(())
    }

    #[test]
    fn test_ami_id_valid() -> Result<(), StampError> {
        let valid8 = "ami-12345678";
        let valid17 = "ami-1234567890abcdef0";
        assert!(AmiId::parse(valid8).is_ok());
        assert!(AmiId::parse(valid17).is_ok());

        let ami = AmiId::from_str(valid8)?;
        assert_eq!(ami.as_str(), valid8);
        assert_eq!(ami.to_string(), valid8);
        Ok(())
    }

    #[test]
    fn test_ami_id_invalid() {
        assert!(AmiId::parse("not-an-ami").is_err());
        assert!(AmiId::parse("ami-123").is_err());
        assert!(AmiId::parse("ami-123456789012345678").is_err());
        assert!(AmiId::parse("ami-1234567z").is_err());
    }

    #[test]
    fn test_ipv4_cidr() -> Result<(), StampError> {
        let cidr = Ipv4Cidr::parse("192.168.1.0/24")?;
        assert_eq!(cidr.to_string(), "192.168.1.0/24");
        assert_eq!(Ipv4Cidr::from_str("10.0.0.0/8")?.get().prefix_len(), 8);
        assert!(Ipv4Cidr::parse("invalid").is_err());

        let json = serde_json::to_string(&cidr)?;
        let decoded: Ipv4Cidr = serde_json::from_str(&json)?;
        assert_eq!(cidr, decoded);
        Ok(())
    }

    #[test]
    fn test_ipv6_cidr() -> Result<(), StampError> {
        let cidr = Ipv6Cidr::parse("2001:db8::/32")?;
        assert_eq!(cidr.to_string(), "2001:db8::/32");
        assert_eq!(Ipv6Cidr::from_str("fe80::/64")?.get().prefix_len(), 64);
        assert!(Ipv6Cidr::parse("invalid").is_err());

        let json = serde_json::to_string(&cidr)?;
        let decoded: Ipv6Cidr = serde_json::from_str(&json)?;
        assert_eq!(cidr, decoded);
        Ok(())
    }

    #[test]
    fn test_mac_address() -> Result<(), StampError> {
        let valid_colon = "00:1a:2b:3c:4d:5e";
        let valid_hyphen = "00-1A-2B-3C-4D-5E";
        let mac1 = MacAddress::from_str(valid_colon)?;
        let mac2 = MacAddress::parse(valid_hyphen)?;
        assert_eq!(mac1.octets(), [0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]);
        assert_eq!(mac1, mac2);
        assert_eq!(mac1.to_string(), "00:1a:2b:3c:4d:5e");

        let constructed = MacAddress::new([1, 2, 3, 4, 5, 6]);
        assert_eq!(constructed.octets(), [1, 2, 3, 4, 5, 6]);

        assert!(MacAddress::parse("001122334455").is_err());
        assert!(MacAddress::parse("00:11:22:33:44").is_err());
        assert!(MacAddress::parse("00:11:22:33:44:55:66").is_err());
        assert!(MacAddress::parse("00:11:22:33:44:zz").is_err());
        assert!(MacAddress::parse("00:11:22:33:44:123").is_err());
        Ok(())
    }

    #[test]
    fn test_memory_mb_and_gb() {
        let mb = MemoryMb::new(2048);
        assert_eq!(mb.get(), 2048);
        assert_eq!(mb.to_gb().get(), 2);
        assert_eq!(mb.to_string(), "2048MB");

        let gb = MemoryGb::new(4);
        assert_eq!(gb.get(), 4);
        assert_eq!(gb.to_mb().get(), 4096);
        assert_eq!(gb.to_string(), "4GB");

        let converted: MemoryMb = gb.into();
        assert_eq!(converted.get(), 4096);
    }

    #[test]
    fn test_cpu_count() -> Result<(), StampError> {
        let cpu = CpuCount::new(4)?;
        assert_eq!(cpu.get(), 4);
        assert_eq!(cpu.to_string(), "4");

        assert_eq!(CpuCount::from_str("8")?.get(), 8);
        assert!(CpuCount::new(0).is_err());
        assert!(CpuCount::from_str("0").is_err());
        assert!(CpuCount::from_str("abc").is_err());
        Ok(())
    }

    #[test]
    fn test_disk_size_mb_and_gb() -> Result<(), StampError> {
        let disk_mb = DiskSizeMb::new(1024);
        assert_eq!(disk_mb.get(), 1024);
        assert_eq!(disk_mb.to_bytes(), 1024 * 1024 * 1024);
        assert_eq!(disk_mb.to_string(), "1024MB");
        assert_eq!(DiskSizeMb::from_str("2048")?.get(), 2048);
        assert!(DiskSizeMb::from_str("invalid").is_err());

        let disk = DiskSizeGb::new(50);
        assert_eq!(disk.get(), 50);
        assert_eq!(disk.to_bytes(), 50 * 1024 * 1024 * 1024);
        assert_eq!(disk.to_string(), "50GB");
        assert_eq!(DiskSizeGb::from_str("100")?.get(), 100);
        assert!(DiskSizeGb::from_str("invalid").is_err());
        Ok(())
    }

    #[test]
    fn test_plugin_address() -> Result<(), StampError> {
        let addr = PluginAddress::parse("github.com/hashicorp/amazon")?;
        assert_eq!(addr.hostname(), "github.com");
        assert_eq!(addr.namespace(), "hashicorp");
        assert_eq!(addr.plugin_type(), "amazon");
        assert_eq!(addr.full_repo_name(), "packer-plugin-amazon");
        assert_eq!(addr.as_str(), "github.com/hashicorp/amazon");
        assert_eq!(addr.to_string(), "github.com/hashicorp/amazon");

        let short_addr = PluginAddress::from_str("hashicorp/amazon")?;
        assert_eq!(short_addr.hostname(), "github.com");
        assert_eq!(short_addr.namespace(), "hashicorp");
        assert_eq!(short_addr.plugin_type(), "amazon");
        assert_eq!(short_addr.full_repo_name(), "packer-plugin-amazon");

        let explicit_plugin = PluginAddress::parse("github.com/custom/packer-plugin-foo")?;
        assert_eq!(explicit_plugin.full_repo_name(), "packer-plugin-foo");

        assert!(PluginAddress::parse("amazon").is_err());
        assert!(PluginAddress::parse("github.com//amazon").is_err());
        Ok(())
    }

    #[test]
    fn test_semver_constraint() -> Result<(), StampError> {
        let constraint = SemVerConstraint::from_str(">= 1.2.0, < 2.0.0")?;
        assert_eq!(constraint.to_string(), ">=1.2.0, <2.0.0");

        let v1 = semver::Version::parse("1.5.0")?;
        let v2 = semver::Version::parse("2.0.0")?;
        assert!(constraint.matches(&v1));
        assert!(!constraint.matches(&v2));

        let tilde_constraint = SemVerConstraint::parse("~> 1.2.0")?;
        let v_tilde = semver::Version::parse("1.2.5")?;
        assert!(tilde_constraint.matches(&v_tilde));
        assert!(!tilde_constraint.matches(&v1));

        assert!(SemVerConstraint::parse("invalid-semver-req").is_err());

        let json = serde_json::to_string(&constraint)?;
        let decoded: SemVerConstraint = serde_json::from_str(&json)?;
        assert_eq!(constraint, decoded);
        Ok(())
    }

    #[test]
    fn test_sha256_checksum() -> Result<(), StampError> {
        let hex64 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let c = Sha256Checksum::from_str(hex64)?;
        assert_eq!(c.as_str(), hex64);
        assert_eq!(c.to_string(), hex64);

        assert!(Sha256Checksum::parse("too-short").is_err());
        assert!(Sha256Checksum::parse(&format!("{hex64}0")).is_err());
        assert!(Sha256Checksum::parse(&hex64.replace('0', "z")).is_err());
        Ok(())
    }

    #[test]
    fn test_sha512_checksum() -> Result<(), StampError> {
        let hex128 = "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e";
        let c = Sha512Checksum::from_str(hex128)?;
        assert_eq!(c.as_str(), hex128);
        assert_eq!(c.to_string(), hex128);

        assert!(Sha512Checksum::parse("too-short").is_err());
        assert!(Sha512Checksum::parse(&format!("{hex128}0")).is_err());
        assert!(Sha512Checksum::parse(&hex128.replace('0', "z")).is_err());
        Ok(())
    }
}
