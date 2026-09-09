#![cfg_attr(coverage_nightly, coverage(off))]
//! Core error types for the Stamp library.

use derive_more::derive::{Display, From};

/// The unified error type for all operations in Stamp.
#[derive(Debug, Display, From)]
pub enum StampError {
    /// An I/O error occurred.
    #[display("I/O error: {_0}")]
    Io(std::io::Error),
    /// A JSON parsing error occurred.
    #[display("JSON error: {_0}")]
    Json(serde_json::Error),
    /// An HCL parsing error occurred.
    #[display("HCL error: {_0}")]
    Hcl(hashicorp_configuration_language_rs::error::HclError),
    /// A parsing error occurred.
    #[display("Parse error: {_0}")]
    #[from(ignore)]
    Parse(String),
    /// A builder-specific error occurred.
    #[display("Builder error: {_0}")]
    #[from(ignore)]
    Builder(String),
    /// A provisioner-specific error occurred.
    #[display("Provisioner error: {_0}")]
    #[from(ignore)]
    Provisioner(String),
    /// A post-processor-specific error occurred.
    #[display("Post-processor error: {_0}")]
    #[from(ignore)]
    PostProcessor(String),
    /// A communicator-specific error occurred.
    #[display("Communicator error: {_0}")]
    #[from(ignore)]
    Communicator(String),
    /// A generic execution error.
    #[display("Execution error: {_0}")]
    #[from(ignore)]
    Execution(String),
    /// A validation error occurred.
    #[display("Validation error: {_0}")]
    #[from(ignore)]
    Validation(String),
    /// A circular dependency was detected in DAG resolution.
    #[display("Circular dependency error: {_0}")]
    #[from(ignore)]
    CircularDependency(String),
    /// A plugin resolution error occurred.
    #[display("Plugin resolution error: {_0}")]
    #[from(ignore)]
    PluginResolution(String),
    /// A plugin handshake error occurred.
    #[display("Plugin handshake error: {_0}")]
    #[from(ignore)]
    PluginHandshake(String),
    /// A protocol violation error occurred.
    #[display("Protocol violation error: {_0}")]
    #[from(ignore)]
    ProtocolViolation(String),
    /// A schema mismatch error occurred.
    #[display("Schema mismatch error: {_0}")]
    #[from(ignore)]
    SchemaMismatch(String),
    /// A test block parsing error occurred.
    #[display("Test block parse error: {_0}")]
    #[from(ignore)]
    ParseTestBlock(String),
    /// A test failure occurred.
    #[display("Test failure in test '{}': {}", _0.test_name, _0.failed_condition)]
    #[from(ignore)]
    TestFailure(crate::template::TestFailureDetails),
    /// A telemetry error occurred.
    #[display("Telemetry error: {_0}")]
    #[from(ignore)]
    Telemetry(String),
    /// An HCP API error occurred.
    #[display("HCP API error: {_0}")]
    #[from(ignore)]
    HcpApi(String),
    /// A plugin RPC error occurred.
    #[display("Plugin RPC error: {_0}")]
    #[from(ignore)]
    PluginRpc(String),
    /// A cryptographic checksum mismatch occurred.
    #[display("Checksum mismatch: expected {expected}, got {actual}")]
    #[from(ignore)]
    ChecksumMismatch {
        /// Expected checksum.
        expected: String,
        /// Actual calculated checksum.
        actual: String,
    },
    /// A digital signature verification failed.
    #[display("Signature verification failed: {_0}")]
    #[from(ignore)]
    SignatureVerification(String),
    /// A communicator operation timed out.
    #[display("Communicator timeout after {timeout_secs}s connecting to {target}")]
    #[from(ignore)]
    CommunicatorTimeout {
        /// Target address or host.
        target: String,
        /// Timeout duration in seconds.
        timeout_secs: u64,
    },
    /// A policy enforcement check failed.
    #[display("Policy violation for '{policy}': {details}")]
    #[from(ignore)]
    PolicyViolation {
        /// The name or identifier of the violated policy.
        policy: String,
        /// Detailed reason for the failure.
        details: String,
    },
    /// A template validation constraint failed.
    #[display("Template validation error: {_0}")]
    #[from(ignore)]
    TemplateValidation(String),
    /// An invalid domain type was encountered.
    #[display("Invalid type error: {_0}")]
    #[from(ignore)]
    InvalidType(String),
    /// A plugin process crashed unexpectedly with captured stderr.
    #[display("Plugin process '{binary}' crashed with exit code {exit_code:?}: {stderr}")]
    #[from(ignore)]
    PluginCrashed {
        /// Plugin binary name or path.
        binary: String,
        /// Exit code of the terminated process, if known.
        exit_code: Option<i32>,
        /// Stderr captured prior to crash.
        stderr: String,
    },
}

impl std::error::Error for StampError {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_stamp_error_display() {
        assert_eq!(
            StampError::Io(std::io::Error::other("io error")).to_string(),
            "I/O error: io error"
        );
        assert_eq!(
            StampError::Parse("invalid syntax".to_string()).to_string(),
            "Parse error: invalid syntax"
        );
        assert_eq!(
            StampError::Builder("failed to build".to_string()).to_string(),
            "Builder error: failed to build"
        );
        assert_eq!(
            StampError::Provisioner("failed to provision".to_string()).to_string(),
            "Provisioner error: failed to provision"
        );
        assert_eq!(
            StampError::PostProcessor("failed to post-process".to_string()).to_string(),
            "Post-processor error: failed to post-process"
        );
        assert_eq!(
            StampError::Communicator("failed to communicate".to_string()).to_string(),
            "Communicator error: failed to communicate"
        );
        assert_eq!(
            StampError::Execution("failed to execute".to_string()).to_string(),
            "Execution error: failed to execute"
        );
        assert_eq!(
            StampError::Validation("invalid template".to_string()).to_string(),
            "Validation error: invalid template"
        );
        assert_eq!(
            StampError::CircularDependency("a -> b -> a".to_string()).to_string(),
            "Circular dependency error: a -> b -> a"
        );
        assert_eq!(
            StampError::PluginResolution("plugin not found".to_string()).to_string(),
            "Plugin resolution error: plugin not found"
        );
        assert_eq!(
            StampError::PluginHandshake("handshake failed".to_string()).to_string(),
            "Plugin handshake error: handshake failed"
        );
        assert_eq!(
            StampError::ProtocolViolation("invalid frame".to_string()).to_string(),
            "Protocol violation error: invalid frame"
        );
        assert_eq!(
            StampError::SchemaMismatch("invalid schema".to_string()).to_string(),
            "Schema mismatch error: invalid schema"
        );
        assert_eq!(
            StampError::ParseTestBlock("bad assert".to_string()).to_string(),
            "Test block parse error: bad assert"
        );
        let failure_details = crate::template::TestFailureDetails {
            test_name: "my_test".to_string(),
            failed_condition: "1 == 2".to_string(),
            error_message: None,
        };
        assert_eq!(
            StampError::TestFailure(failure_details).to_string(),
            "Test failure in test 'my_test': 1 == 2"
        );
        assert_eq!(
            StampError::Telemetry("network timeout".to_string()).to_string(),
            "Telemetry error: network timeout"
        );
        assert_eq!(
            StampError::HcpApi("bad token".to_string()).to_string(),
            "HCP API error: bad token"
        );
        assert_eq!(
            StampError::PluginRpc("rpc timeout".to_string()).to_string(),
            "Plugin RPC error: rpc timeout"
        );
        assert_eq!(
            StampError::ChecksumMismatch {
                expected: "abc".to_string(),
                actual: "def".to_string(),
            }
            .to_string(),
            "Checksum mismatch: expected abc, got def"
        );
        assert_eq!(
            StampError::SignatureVerification("invalid key".to_string()).to_string(),
            "Signature verification failed: invalid key"
        );
        assert_eq!(
            StampError::CommunicatorTimeout {
                target: "10.0.0.1".to_string(),
                timeout_secs: 30,
            }
            .to_string(),
            "Communicator timeout after 30s connecting to 10.0.0.1"
        );
        assert_eq!(
            StampError::PolicyViolation {
                policy: "deny-root".to_string(),
                details: "root user prohibited".to_string(),
            }
            .to_string(),
            "Policy violation for 'deny-root': root user prohibited"
        );
        assert_eq!(
            StampError::TemplateValidation("missing builders".to_string()).to_string(),
            "Template validation error: missing builders"
        );
        assert_eq!(
            StampError::InvalidType("unknown enum".to_string()).to_string(),
            "Invalid type error: unknown enum"
        );
        assert_eq!(
            StampError::PluginCrashed {
                binary: "packer-plugin-amazon".to_string(),
                exit_code: Some(1),
                stderr: "panic: nil pointer dereference".to_string(),
            }
            .to_string(),
            "Plugin process 'packer-plugin-amazon' crashed with exit code Some(1): panic: nil pointer dereference"
        );

        let json_err: Result<serde_json::Value, _> = serde_json::from_str("{ bad json");
        if let Err(e) = json_err {
            let stamp_err = StampError::from(e);
            assert!(stamp_err.to_string().starts_with("JSON error:"));
        }

        let io_err = std::io::Error::other("custom io err");
        let stamp_io = StampError::Io(io_err);
        use std::error::Error;
        assert!(stamp_io.source().is_none());
    }
}
