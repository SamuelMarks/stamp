//! Telemetry and version checking capabilities.
//!
//! Provides opt-in telemetry for usage data and version checking against Checkpoint.

use crate::error::StampError;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

/// Configuration for telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryConfig {
    /// Whether telemetry is enabled.
    pub enabled: bool,
    /// The endpoint URL to send telemetry data to.
    pub endpoint: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        let checkpoint_disabled = std::env::var("CHECKPOINT_DISABLE")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let telemetry_disabled = std::env::var("PACKER_TELEMETRY")
            .is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("false"));

        Self {
            enabled: !checkpoint_disabled && !telemetry_disabled,
            endpoint: "https://checkpoint-api.hashicorp.com/v1/telemetry".to_string(),
        }
    }
}

/// Identifiers for specific telemetry events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Dispatched when a build starts.
    BuildStart {
        /// The builder type.
        builder: String,
    },
    /// Dispatched when a build successfully completes.
    BuildComplete {
        /// The builder type.
        builder: String,
        /// The duration of the build in milliseconds.
        duration_ms: u64,
    },
    /// Dispatched when a build fails.
    BuildError {
        /// The builder type.
        builder: String,
        /// The error message.
        error: String,
    },
}

/// An asynchronous client for dispatching telemetry and checking versions.
pub struct Client {
    /// Configuration settings controlling endpoint and telemetry enablement.
    config: TelemetryConfig,
    /// HTTP client used to perform outbound requests.
    http: reqwest::Client,
}

/// Checkpoint API response payload.
#[derive(Deserialize)]
struct CheckpointResponse {
    /// Upstream version string.
    current_version: String,
}

impl Client {
    /// Creates a new `Client` with the given configuration.
    ///
    /// # Arguments
    /// * `config` - The [`TelemetryConfig`] governing endpoint URL and enablement.
    #[must_use]
    pub fn new(config: TelemetryConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    /// Dispatches a telemetry event if enabled.
    ///
    /// Fails gracefully, meaning it won't panic or return errors that halt execution
    /// if the endpoint is unreachable. Returns Ok immediately if disabled.
    ///
    /// # Arguments
    /// * `event` - The [`Event`] payload to dispatch.
    ///
    /// # Errors
    /// Returns `StampError::Telemetry` if an unrecoverable network failure occurs during dispatch.
    pub async fn dispatch(&self, event: Event) -> Result<(), StampError> {
        if !self.config.enabled {
            return Ok(());
        }

        let endpoint_url = Url::parse(&self.config.endpoint)
            .map_err(|e| StampError::Telemetry(format!("Invalid telemetry endpoint URL: {e}")))?;

        let res = self
            .http
            .post(endpoint_url)
            .json(&event)
            .timeout(Duration::from_secs(3))
            .send()
            .await;

        if let Err(e) = res {
            return Err(StampError::Telemetry(e.to_string()));
        }

        Ok(())
    }

    /// Checks the given current version against the upstream checkpoint API.
    ///
    /// # Arguments
    /// * `current_version` - Semantic version string of the running binary.
    ///
    /// # Errors
    /// Returns `StampError::Telemetry` if the request fails or parsing fails.
    pub async fn checkpoint_check(
        &self,
        current_version: &str,
    ) -> Result<Option<Version>, StampError> {
        let current = Version::parse(current_version)
            .map_err(|e| StampError::Telemetry(format!("Invalid current version: {e}")))?;

        if !self.config.enabled {
            return Ok(None);
        }

        let base_url = Url::parse(&self.config.endpoint)
            .map_err(|e| StampError::Telemetry(format!("Invalid telemetry endpoint URL: {e}")))?;

        // We check the specific checkpoint endpoint
        let endpoint =
            if base_url.as_str().contains("localhost") || base_url.as_str().contains("127.0.0.1") {
                base_url
                    .join("/v1/check/packer")
                    .map_err(|e| StampError::Telemetry(e.to_string()))?
            } else {
                Url::parse("https://checkpoint-api.hashicorp.com/v1/check/packer")
                    .map_err(|e| StampError::Telemetry(e.to_string()))?
            };

        let res = self
            .http
            .get(endpoint)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map_err(|e| StampError::Telemetry(e.to_string()))?;

        let body = res
            .json::<CheckpointResponse>()
            .await
            .map_err(|e| StampError::Telemetry(e.to_string()))?;
        let latest = Version::parse(&body.current_version)
            .map_err(|e| StampError::Telemetry(format!("Invalid upstream version: {e}")))?;

        if latest > current {
            Ok(Some(latest))
        } else {
            Ok(None)
        }
    }
}

/// Version information containing semver, commit, platform target, and OS/architecture details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// Semantic version string (e.g. `0.1.0`).
    pub version: String,
    /// Prerelease identifier (e.g. `dev`, `beta.1`), if any.
    pub prerelease: Option<String>,
    /// Build metadata, if any.
    pub metadata: Option<String>,
    /// Target operating system string.
    pub os: String,
    /// Target CPU architecture string.
    pub arch: String,
    /// Git commit revision SHA, or "release".
    pub revision: String,
}

impl VersionInfo {
    /// Upstream Packer compatibility version reported to tools like Bento.
    pub const PACKER_COMPAT_VERSION: &'static str = "1.11.2";

    /// Creates a new `VersionInfo` initialized from compile-time and runtime platform constants.
    #[must_use]
    pub fn current() -> Self {
        let full_version = env!("CARGO_PKG_VERSION");
        let parsed = semver::Version::parse(full_version).ok();
        let prerelease = parsed.as_ref().and_then(|v| {
            if v.pre.is_empty() {
                None
            } else {
                Some(v.pre.to_string())
            }
        });
        let metadata = parsed.as_ref().and_then(|v| {
            if v.build.is_empty() {
                None
            } else {
                Some(v.build.to_string())
            }
        });

        Self {
            version: full_version.to_string(),
            prerelease,
            metadata,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            revision: option_env!("STAMP_GIT_REVISION")
                .unwrap_or("release")
                .to_string(),
        }
    }

    /// Formats the version information in human-readable or verbose format matching Packer output.
    ///
    /// # Arguments
    /// * `verbose` - Whether to include extended platform, OS, arch, and revision details.
    #[must_use]
    pub fn format_human(&self, verbose: bool) -> String {
        if verbose {
            format!(
                "Packer v{} (Stamp drop-in replacement v{})\n\nPlatform: {}/{}\nOS: {}\nArch: {}\nRevision: {}",
                Self::PACKER_COMPAT_VERSION,
                self.version,
                self.os,
                self.arch,
                self.os,
                self.arch,
                self.revision
            )
        } else {
            format!(
                "Packer v{} (Stamp drop-in replacement)",
                Self::PACKER_COMPAT_VERSION
            )
        }
    }

    /// Formats the version information in Packer-compatible machine-readable CSV format.
    ///
    /// # Arguments
    /// * `timestamp` - Unix epoch timestamp in seconds.
    #[must_use]
    pub fn format_machine_readable(&self, timestamp: u64) -> String {
        format!(
            "{timestamp},,version,{version}\n{timestamp},,version-prerelease,{pre}\n{timestamp},,version-metadata,{meta}",
            timestamp = timestamp,
            version = Self::PACKER_COMPAT_VERSION,
            pre = self.prerelease.as_deref().unwrap_or(""),
            meta = self.metadata.as_deref().unwrap_or("")
        )
    }
}

/// Checks Checkpoint for newer releases of Stamp/Packer using an explicit [`TelemetryConfig`].
///
/// # Arguments
/// * `current_version` - Semantic version string of the running binary.
/// * `config` - Explicit telemetry configuration.
///
/// # Errors
/// Returns `StampError::Telemetry` if the check fails.
pub async fn check_for_updates_with_config(
    current_version: &str,
    config: TelemetryConfig,
) -> Result<Option<Version>, StampError> {
    let client = Client::new(config);
    client.checkpoint_check(current_version).await
}

/// Checks Checkpoint for newer releases of Stamp/Packer.
///
/// Returns `Ok(Some(new_version))` if a newer release is available, or `Ok(None)` otherwise.
///
/// # Arguments
/// * `current_version` - Semantic version string of the running binary.
///
/// # Errors
/// Returns `StampError::Telemetry` if the check fails.
pub async fn check_for_updates(current_version: &str) -> Result<Option<Version>, StampError> {
    check_for_updates_with_config(current_version, TelemetryConfig::default()).await
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::utils::{ENV_MUTEX, ScopedEnvVar};

    #[test]
    fn test_telemetry_config_default() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = ScopedEnvVar::remove("CHECKPOINT_DISABLE");
        let _t0 = ScopedEnvVar::set("PACKER_TELEMETRY", "0");
        let cfg = TelemetryConfig::default();
        assert!(!cfg.enabled);

        let _t1 = ScopedEnvVar::set("PACKER_TELEMETRY", "1");
        let cfg2 = TelemetryConfig::default();
        assert!(cfg2.enabled);
    }

    #[tokio::test]
    async fn test_client_dispatch_disabled() -> Result<(), StampError> {
        let cfg = TelemetryConfig {
            enabled: false,
            endpoint: "http://localhost".to_string(),
        };
        let client = Client::new(cfg);
        client
            .dispatch(Event::BuildStart {
                builder: "test".to_string(),
            })
            .await
    }

    #[tokio::test]
    async fn test_client_checkpoint_disabled() -> Result<(), StampError> {
        let cfg = TelemetryConfig {
            enabled: false,
            endpoint: "http://localhost".to_string(),
        };
        let client = Client::new(cfg);
        let res = client.checkpoint_check("1.0.0").await?;
        assert_eq!(res, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_client_dispatch_enabled_success() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .create_async()
            .await;

        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: server.url(),
        };
        let client = Client::new(cfg);
        client
            .dispatch(Event::BuildStart {
                builder: "test".to_string(),
            })
            .await?;
        mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_client_dispatch_enabled_failure() {
        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: "http://127.0.0.1:0".to_string(),
        };
        let client = Client::new(cfg);
        let res = client
            .dispatch(Event::BuildStart {
                builder: "test".to_string(),
            })
            .await;
        assert!(matches!(res, Err(StampError::Telemetry(_))));

        let bad_url_cfg = TelemetryConfig {
            enabled: true,
            endpoint: "not a url".to_string(),
        };
        let bad_client = Client::new(bad_url_cfg);
        let bad_res = bad_client
            .dispatch(Event::BuildStart {
                builder: "test".to_string(),
            })
            .await;
        assert!(matches!(bad_res, Err(StampError::Telemetry(_))));
    }

    #[tokio::test]
    async fn test_client_checkpoint_enabled_success_upgrade() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/check/packer")
            .with_status(200)
            .with_body("{\"current_version\":\"2.0.0\"}")
            .create_async()
            .await;

        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: server.url(),
        };
        let res = check_for_updates_with_config("1.0.0", cfg).await?;
        assert_eq!(res, Some(Version::parse("2.0.0")?));
        mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_client_checkpoint_enabled_success_no_upgrade() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/check/packer")
            .with_status(200)
            .with_body("{\"current_version\":\"1.0.0\"}")
            .create_async()
            .await;

        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: server.url(),
        };
        let client = Client::new(cfg);
        let res = client.checkpoint_check("1.0.0").await?;
        assert_eq!(res, None);
        mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_client_checkpoint_enabled_failure_network() {
        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: "http://127.0.0.1:0".to_string(),
        };
        let client = Client::new(cfg);
        let res = client.checkpoint_check("1.0.0").await;
        assert!(matches!(res, Err(StampError::Telemetry(_))));
    }

    #[tokio::test]
    async fn test_client_checkpoint_enabled_failure_invalid_json() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/check/packer")
            .with_status(200)
            .with_body("invalid json")
            .create_async()
            .await;

        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: server.url(),
        };
        let client = Client::new(cfg);
        let res = client.checkpoint_check("1.0.0").await;
        assert!(matches!(res, Err(StampError::Telemetry(_))));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_checkpoint_enabled_failure_invalid_version() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/check/packer")
            .with_status(200)
            .with_body("{\"current_version\":\"invalid\"}")
            .create_async()
            .await;

        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: server.url(),
        };
        let client = Client::new(cfg);
        let res = client.checkpoint_check("1.0.0").await;
        assert!(matches!(res, Err(StampError::Telemetry(_))));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_checkpoint_invalid_current_version() {
        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: "http://localhost".to_string(),
        };
        let client = Client::new(cfg);
        let res = client.checkpoint_check("invalid").await;
        assert!(matches!(res, Err(StampError::Telemetry(_))));
    }

    #[test]
    fn test_checkpoint_disable_env() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c1 = ScopedEnvVar::set("CHECKPOINT_DISABLE", "1");
        let cfg = TelemetryConfig::default();
        assert!(!cfg.enabled);

        let _c2 = ScopedEnvVar::set("CHECKPOINT_DISABLE", "true");
        let cfg2 = TelemetryConfig::default();
        assert!(!cfg2.enabled);
    }

    #[test]
    fn test_version_info_formatting() {
        let info = VersionInfo::current();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        let human_short = info.format_human(false);
        assert!(human_short.contains(VersionInfo::PACKER_COMPAT_VERSION));
        assert!(human_short.contains("Stamp drop-in replacement"));
        let human_verbose = info.format_human(true);
        assert!(human_verbose.contains("Platform:"));
        assert!(human_verbose.contains(&info.os));
        assert!(human_verbose.contains(&info.arch));
        assert!(human_verbose.contains(&info.version));

        let mr = info.format_machine_readable(1_700_000_000);
        assert!(mr.contains(&format!(
            "1700000000,,version,{}",
            VersionInfo::PACKER_COMPAT_VERSION
        )));
        assert!(mr.contains("1700000000,,version-prerelease,"));
        assert!(mr.contains("1700000000,,version-metadata,"));
    }

    #[tokio::test]
    async fn test_check_for_updates_disabled() -> Result<(), StampError> {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = ScopedEnvVar::set("CHECKPOINT_DISABLE", "1");
        let res = check_for_updates("0.0.1").await?;
        assert_eq!(res, None);
        Ok(())
    }
}
