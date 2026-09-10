#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
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
    pub endpoint: Url,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        let checkpoint_disabled = std::env::var("CHECKPOINT_DISABLE")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let telemetry_disabled = std::env::var("PACKER_TELEMETRY")
            .is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("false"));

        Self {
            enabled: !checkpoint_disabled && !telemetry_disabled,
            endpoint: Url::parse("https://checkpoint-api.hashicorp.com/v1/telemetry")
                .unwrap_or_else(|_| {
                    Url::parse("http://localhost").unwrap_or_else(|_| unreachable!())
                }),
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
    /// # Errors
    /// Returns `StampError::Telemetry` if an unrecoverable network failure occurs during dispatch.
    pub async fn dispatch(&self, event: Event) -> Result<(), StampError> {
        if !self.config.enabled {
            return Ok(());
        }

        let res = self
            .http
            .post(self.config.endpoint.clone())
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

        // We check the specific checkpoint endpoint
        let endpoint = if self.config.endpoint.as_str().contains("localhost")
            || self.config.endpoint.as_str().contains("127.0.0.1")
        {
            // Re-use endpoint domain for tests if testing
            self.config
                .endpoint
                .join("/v1/check/packer")
                .unwrap_or_else(|_| self.config.endpoint.clone())
        } else {
            Url::parse("https://checkpoint-api.hashicorp.com/v1/check/packer")
                .unwrap_or_else(|_| unreachable!())
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
    #[must_use]
    pub fn format_human(&self, verbose: bool) -> String {
        if verbose {
            format!(
                "Stamp v{}\n\nPlatform: {}/{}\nOS: {}\nArch: {}\nRevision: {}",
                self.version, self.os, self.arch, self.os, self.arch, self.revision
            )
        } else {
            format!("Stamp v{}", self.version)
        }
    }

    /// Formats the version information in Packer-compatible machine-readable CSV format.
    #[must_use]
    pub fn format_machine_readable(&self, timestamp: u64) -> String {
        format!(
            "{timestamp},,version,{version}\n{timestamp},,version-prerelease,{pre}\n{timestamp},,version-metadata,{meta}",
            timestamp = timestamp,
            version = self.version,
            pre = self.prerelease.as_deref().unwrap_or(""),
            meta = self.metadata.as_deref().unwrap_or("")
        )
    }
}

/// Checks Checkpoint for newer releases of Stamp/Packer.
///
/// Returns `Ok(Some(new_version))` if a newer release is available, or `Ok(None)` otherwise.
///
/// # Errors
/// Returns `StampError::Telemetry` if the check fails.
pub async fn check_for_updates(current_version: &str) -> Result<Option<Version>, StampError> {
    let client = Client::new(TelemetryConfig::default());
    client.checkpoint_check(current_version).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_telemetry_config_default() {
        let _guard = match ENV_MUTEX.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        unsafe {
            std::env::set_var("PACKER_TELEMETRY", "0");
        }
        let cfg = TelemetryConfig::default();
        assert!(!cfg.enabled);

        unsafe {
            std::env::set_var("PACKER_TELEMETRY", "1");
        }
        let cfg2 = TelemetryConfig::default();
        assert!(cfg2.enabled);

        unsafe {
            std::env::remove_var("PACKER_TELEMETRY");
        }
    }

    #[tokio::test]
    async fn test_client_dispatch_disabled() {
        let cfg = TelemetryConfig {
            enabled: false,
            endpoint: Url::parse("http://localhost").unwrap_or_else(|_| unreachable!()),
        };
        let client = Client::new(cfg);
        let res = client
            .dispatch(Event::BuildStart {
                builder: "test".to_string(),
            })
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_client_checkpoint_disabled() {
        let cfg = TelemetryConfig {
            enabled: false,
            endpoint: Url::parse("http://localhost").unwrap_or_else(|_| unreachable!()),
        };
        let client = Client::new(cfg);
        let res = client.checkpoint_check("1.0.0").await;
        assert!(res.is_ok());
        assert_eq!(res.unwrap_or_else(|_| unreachable!()), None);
    }

    #[tokio::test]
    async fn test_client_dispatch_enabled_success() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .create_async()
            .await;

        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: Url::parse(&server.url()).unwrap(),
        };
        let client = Client::new(cfg);
        let res = client
            .dispatch(Event::BuildStart {
                builder: "test".to_string(),
            })
            .await;
        assert!(res.is_ok());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_dispatch_enabled_failure() {
        // use an invalid port to simulate error
        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: Url::parse("http://127.0.0.1:0").unwrap(),
        };
        let client = Client::new(cfg);
        let res = client
            .dispatch(Event::BuildStart {
                builder: "test".to_string(),
            })
            .await;
        assert!(matches!(res, Err(StampError::Telemetry(_))));
    }

    #[tokio::test]
    async fn test_client_checkpoint_enabled_success_upgrade() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/check/packer")
            .with_status(200)
            .with_body("{\"current_version\":\"2.0.0\"}")
            .create_async()
            .await;

        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: Url::parse(&server.url()).unwrap(),
        };
        let client = Client::new(cfg);
        let res = client.checkpoint_check("1.0.0").await.unwrap();
        assert_eq!(res, Some(Version::parse("2.0.0").unwrap()));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_checkpoint_enabled_success_no_upgrade() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/check/packer")
            .with_status(200)
            .with_body("{\"current_version\":\"1.0.0\"}")
            .create_async()
            .await;

        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: Url::parse(&server.url()).unwrap(),
        };
        let client = Client::new(cfg);
        let res = client.checkpoint_check("1.0.0").await.unwrap();
        assert_eq!(res, None);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_checkpoint_enabled_failure_network() {
        let cfg = TelemetryConfig {
            enabled: true,
            endpoint: Url::parse("http://127.0.0.1:0").unwrap(),
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
            endpoint: Url::parse(&server.url()).unwrap(),
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
            endpoint: Url::parse(&server.url()).unwrap(),
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
            endpoint: Url::parse("http://localhost").unwrap(),
        };
        let client = Client::new(cfg);
        let res = client.checkpoint_check("invalid").await;
        assert!(matches!(res, Err(StampError::Telemetry(_))));
    }

    #[test]
    fn test_checkpoint_disable_env() {
        let _guard = match ENV_MUTEX.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        unsafe {
            std::env::set_var("CHECKPOINT_DISABLE", "1");
        }
        let cfg = TelemetryConfig::default();
        assert!(!cfg.enabled);

        unsafe {
            std::env::set_var("CHECKPOINT_DISABLE", "true");
        }
        let cfg2 = TelemetryConfig::default();
        assert!(!cfg2.enabled);

        unsafe {
            std::env::remove_var("CHECKPOINT_DISABLE");
        }
    }

    #[test]
    fn test_version_info_formatting() {
        let info = VersionInfo::current();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        let human_short = info.format_human(false);
        assert!(human_short.contains(&info.version));
        let human_verbose = info.format_human(true);
        assert!(human_verbose.contains("Platform:"));
        assert!(human_verbose.contains(&info.os));
        assert!(human_verbose.contains(&info.arch));

        let mr = info.format_machine_readable(1_700_000_000);
        assert!(mr.contains("1700000000,,version,"));
        assert!(mr.contains("1700000000,,version-prerelease,"));
        assert!(mr.contains("1700000000,,version-metadata,"));
    }

    #[tokio::test]
    async fn test_check_for_updates_disabled() {
        let _guard = match ENV_MUTEX.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        unsafe {
            std::env::set_var("CHECKPOINT_DISABLE", "1");
        }
        let res = check_for_updates("0.0.1").await;
        assert!(res.is_ok());
        if let Ok(update_opt) = res {
            assert_eq!(update_opt, None);
        }
        unsafe {
            std::env::remove_var("CHECKPOINT_DISABLE");
        }
    }
}
