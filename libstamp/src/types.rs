#![cfg_attr(coverage_nightly, coverage(off))]
//! Strongly typed primitives for Stamp configuration and operations.

use crate::error::StampError;
use std::fmt;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

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
#[allow(clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_port() -> Result<(), StampError> {
        let port = Port::new(8080);
        assert_eq!(port.get(), 8080);
        let port_clone = port;
        assert_eq!(port, port_clone);
        assert_eq!(port.to_string(), "8080");
        assert_eq!(Port::default().get(), 22);
        assert_eq!(Port::from_str("9090")?.get(), 9090);
        assert!(Port::from_str("invalid").is_err());
        Ok(())
    }

    #[test]
    fn test_timeout() {
        let duration = Duration::from_secs(30);
        let timeout = Timeout::new(duration);
        assert_eq!(timeout.get(), duration);
        let timeout_clone = timeout;
        assert_eq!(timeout, timeout_clone);
        assert_eq!(timeout.to_string(), "30s");
        assert_eq!(Timeout::default().get(), Duration::from_secs(30));
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
        assert!(AmiId::parse("ami-123").is_err()); // too short
        assert!(AmiId::parse("ami-123456789012345678").is_err()); // too long
        assert!(AmiId::parse("ami-1234567z").is_err()); // non-hex
    }

    #[test]
    fn test_ipv4_cidr() -> Result<(), Box<dyn std::error::Error>> {
        let cidr = Ipv4Cidr::parse("192.168.1.0/24")?;
        assert_eq!(cidr.to_string(), "192.168.1.0/24");
        assert_eq!(Ipv4Cidr::from_str("10.0.0.0/8")?.get().prefix_len(), 8);
        assert!(Ipv4Cidr::parse("invalid").is_err());

        // Serde roundtrip
        let json = serde_json::to_string(&cidr)?;
        let decoded: Ipv4Cidr = serde_json::from_str(&json)?;
        assert_eq!(cidr, decoded);
        Ok(())
    }

    #[test]
    fn test_ipv6_cidr() -> Result<(), Box<dyn std::error::Error>> {
        let cidr = Ipv6Cidr::parse("2001:db8::/32")?;
        assert_eq!(cidr.to_string(), "2001:db8::/32");
        assert_eq!(Ipv6Cidr::from_str("fe80::/64")?.get().prefix_len(), 64);
        assert!(Ipv6Cidr::parse("invalid").is_err());

        // Serde roundtrip
        let json = serde_json::to_string(&cidr)?;
        let decoded: Ipv6Cidr = serde_json::from_str(&json)?;
        assert_eq!(cidr, decoded);
        Ok(())
    }

    #[test]
    fn test_mac_address() -> Result<(), StampError> {
        let valid_colon = "00:1a:2b:3c:4d:5e";
        let valid_hyphen = "00-1A-2B-3C-4D-5E";
        let mac1 = MacAddress::parse(valid_colon)?;
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
    fn test_disk_size_gb() -> Result<(), StampError> {
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
    fn test_semver_constraint() -> Result<(), Box<dyn std::error::Error>> {
        let constraint = SemVerConstraint::parse(">= 1.2.0, < 2.0.0")?;
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

        // Serde roundtrip
        let json = serde_json::to_string(&constraint)?;
        let decoded: SemVerConstraint = serde_json::from_str(&json)?;
        assert_eq!(constraint, decoded);
        Ok(())
    }

    #[test]
    fn test_sha256_checksum() -> Result<(), StampError> {
        let hex64 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let c = Sha256Checksum::parse(hex64)?;
        assert_eq!(c.as_str(), hex64);
        assert_eq!(c.to_string(), hex64);

        assert!(Sha256Checksum::parse("too-short").is_err());
        assert!(Sha256Checksum::parse(&format!("{hex64}0")).is_err()); // 65 chars
        assert!(Sha256Checksum::parse(&hex64.replace('0', "z")).is_err()); // non-hex
        Ok(())
    }

    #[test]
    fn test_sha512_checksum() -> Result<(), StampError> {
        let hex128 = "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e";
        let c = Sha512Checksum::parse(hex128)?;
        assert_eq!(c.as_str(), hex128);
        assert_eq!(c.to_string(), hex128);

        assert!(Sha512Checksum::parse("too-short").is_err());
        assert!(Sha512Checksum::parse(&format!("{hex128}0")).is_err()); // 129 chars
        assert!(Sha512Checksum::parse(&hex128.replace('0', "z")).is_err()); // non-hex
        Ok(())
    }
}
