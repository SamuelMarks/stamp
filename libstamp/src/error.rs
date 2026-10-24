//! Core error types for the Stamp library.

use derive_more::derive::{Display, Error, From};

/// The unified error type for all operations in Stamp.
#[derive(Debug, Display, Error, From)]
pub enum StampError {
    /// An I/O error occurred.
    #[display("I/O error: {_0}")]
    Io(std::io::Error),
    /// A JSON parsing error occurred.
    #[display("JSON error: {_0}")]
    Json(serde_json::Error),
    /// A YAML parsing error occurred.
    #[display("YAML error: {_0}")]
    Yaml(serde_yaml::Error),
    /// An HTTP client error occurred.
    #[display("HTTP error: {_0}")]
    Http(reqwest::Error),
    /// A URL parsing error occurred.
    #[display("URL parse error: {_0}")]
    UrlParse(url::ParseError),
    /// A semantic version parsing error occurred.
    #[display("SemVer error: {_0}")]
    SemVer(semver::Error),
    /// A regular expression compilation error occurred.
    #[display("Regex error: {_0}")]
    Regex(regex::Error),
    /// A glob pattern error occurred.
    #[display("Glob pattern error: {_0}")]
    GlobPattern(glob::PatternError),
    /// A glob iteration error occurred.
    #[display("Glob error: {_0}")]
    Glob(glob::GlobError),
    /// A ZIP archive error occurred.
    #[display("ZIP error: {_0}")]
    Zip(zip::result::ZipError),
    /// A gRPC transport error occurred.
    #[display("gRPC transport error: {_0}")]
    TonicTransport(tonic::transport::Error),
    /// A gRPC status error occurred.
    #[display("gRPC status error: {_0}")]
    TonicStatus(Box<tonic::Status>),
    /// A protobuf decode error occurred.
    #[display("Protobuf decode error: {_0}")]
    ProstDecode(prost::DecodeError),
    /// A protobuf encode error occurred.
    #[display("Protobuf encode error: {_0}")]
    ProstEncode(prost::EncodeError),
    /// A UTF-8 string conversion error occurred.
    #[display("UTF-8 error: {_0}")]
    Utf8(std::string::FromUtf8Error),
    /// A UTF-8 slice conversion error occurred.
    #[display("UTF-8 slice error: {_0}")]
    StrUtf8(std::str::Utf8Error),
    /// An integer parsing error occurred.
    #[display("Integer parse error: {_0}")]
    ParseInt(std::num::ParseIntError),
    /// A floating-point parsing error occurred.
    #[display("Float parse error: {_0}")]
    ParseFloat(std::num::ParseFloatError),
    /// A boolean parsing error occurred.
    #[display("Boolean parse error: {_0}")]
    ParseBool(std::str::ParseBoolError),
    /// A socket address parsing error occurred.
    #[display("Address parse error: {_0}")]
    AddrParse(std::net::AddrParseError),
    /// An IP network CIDR parsing error occurred.
    #[display("IP network parse error: {_0}")]
    IpNetAddrParse(ipnet::AddrParseError),
    /// A TLS certificate generation error occurred.
    #[display("Certificate generation error: {_0}")]
    Rcgen(rcgen::Error),
    /// An RSA cryptographic error occurred.
    #[display("RSA error: {_0}")]
    Rsa(rsa::Error),
    /// A bcrypt hashing error occurred.
    #[display("Bcrypt error: {_0}")]
    Bcrypt(bcrypt::BcryptError),
    /// An SSH protocol error occurred.
    #[display("SSH error: {_0}")]
    Russh(Box<russh::Error>),
    /// An SSH key error occurred.
    #[display("SSH key error: {_0}")]
    RusshKeys(russh::keys::Error),
    /// An OpenSSH key format error occurred.
    #[display("SSH key format error: {_0}")]
    SshKey(russh::keys::ssh_key::Error),
    /// An SFTP protocol error occurred.
    #[display("SFTP error: {_0}")]
    RusshSftp(russh_sftp::client::error::Error),
    /// An HCL parsing error occurred.
    #[display("HCL error: {_0}")]
    Hcl(hashicorp_configuration_language_rs::error::HclError),
    /// A parsing error occurred.
    #[display("Parse error: {_0}")]
    #[from(ignore)]
    Parse(#[error(not(source))] String),
    /// A builder-specific error occurred.
    #[display("Builder error: {_0}")]
    #[from(ignore)]
    Builder(#[error(not(source))] String),
    /// A provisioner-specific error occurred.
    #[display("Provisioner error: {_0}")]
    #[from(ignore)]
    Provisioner(#[error(not(source))] String),
    /// A post-processor-specific error occurred.
    #[display("Post-processor error: {_0}")]
    #[from(ignore)]
    PostProcessor(#[error(not(source))] String),
    /// A communicator-specific error occurred.
    #[display("Communicator error: {_0}")]
    #[from(ignore)]
    Communicator(#[error(not(source))] String),
    /// A generic execution error.
    #[display("Execution error: {_0}")]
    #[from(ignore)]
    Execution(#[error(not(source))] String),
    /// A validation error occurred.
    #[display("Validation error: {_0}")]
    #[from(ignore)]
    Validation(#[error(not(source))] String),
    /// A structured field validation error occurred.
    #[display("Field validation error on '{field}': {reason}")]
    #[from(ignore)]
    FieldValidation {
        /// The configuration field name.
        field: String,
        /// The reason validation failed.
        reason: String,
    },
    /// A circular dependency was detected in DAG resolution.
    #[display("Circular dependency error: {_0}")]
    #[from(ignore)]
    CircularDependency(#[error(not(source))] String),
    /// A plugin resolution error occurred.
    #[display("Plugin resolution error: {_0}")]
    #[from(ignore)]
    PluginResolution(#[error(not(source))] String),
    /// A plugin handshake error occurred.
    #[display("Plugin handshake error: {_0}")]
    #[from(ignore)]
    PluginHandshake(#[error(not(source))] String),
    /// A protocol violation error occurred.
    #[display("Protocol violation error: {_0}")]
    #[from(ignore)]
    ProtocolViolation(#[error(not(source))] String),
    /// A schema mismatch error occurred.
    #[display("Schema mismatch error: {_0}")]
    #[from(ignore)]
    SchemaMismatch(#[error(not(source))] String),
    /// A test block parsing error occurred.
    #[display("Test block parse error: {_0}")]
    #[from(ignore)]
    ParseTestBlock(#[error(not(source))] String),
    /// A test failure occurred.
    #[display("Test failure in test '{}': {}", _0.test_name, _0.failed_condition)]
    #[from(ignore)]
    TestFailure(#[error(not(source))] crate::template::TestFailureDetails),
    /// A telemetry error occurred.
    #[display("Telemetry error: {_0}")]
    #[from(ignore)]
    Telemetry(#[error(not(source))] String),
    /// An HCP API error occurred.
    #[display("HCP API error: {_0}")]
    #[from(ignore)]
    HcpApi(#[error(not(source))] String),
    /// A plugin RPC error occurred.
    #[display("Plugin RPC error: {_0}")]
    #[from(ignore)]
    PluginRpc(#[error(not(source))] String),
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
    SignatureVerification(#[error(not(source))] String),
    /// A communicator operation timed out.
    #[display("Communicator timeout after {timeout_secs}s connecting to {target}")]
    #[from(ignore)]
    CommunicatorTimeout {
        /// Target address or host.
        target: String,
        /// Timeout duration in seconds.
        timeout_secs: u64,
    },
    /// A general operation timed out.
    #[display("Operation '{operation}' timed out after {}ms", duration.as_millis())]
    #[from(ignore)]
    Timeout {
        /// The name of the timed-out operation.
        operation: String,
        /// The elapsed timeout duration.
        duration: std::time::Duration,
    },
    /// A subprocess command failed with a non-zero exit code.
    #[display("Subprocess '{command}' failed with exit code {exit_code:?}: {stderr}")]
    #[from(ignore)]
    SubprocessFailed {
        /// The command that was executed.
        command: String,
        /// The exit code, if available.
        exit_code: Option<i32>,
        /// Captured standard error output.
        stderr: String,
    },
    /// A required key was missing from the multistep state bag.
    #[display("StateBag missing key '{key}' (expected type {expected_type})")]
    #[from(ignore)]
    StateBagMissingKey {
        /// The missing state key.
        key: String,
        /// The expected Rust type name.
        expected_type: &'static str,
    },
    /// A key in the multistep state bag had an unexpected type.
    #[display("StateBag type mismatch for key '{key}' (expected type {expected_type})")]
    #[from(ignore)]
    StateBagTypeMismatch {
        /// The state key.
        key: String,
        /// The expected Rust type name.
        expected_type: &'static str,
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
    TemplateValidation(#[error(not(source))] String),
    /// An invalid domain type was encountered.
    #[display("Invalid type error: {_0}")]
    #[from(ignore)]
    InvalidType(#[error(not(source))] String),
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

impl From<tonic::Status> for StampError {
    fn from(status: tonic::Status) -> Self {
        Self::TonicStatus(Box::new(status))
    }
}

impl From<russh::Error> for StampError {
    fn from(err: russh::Error) -> Self {
        Self::Russh(Box::new(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as StdError;

    fn assert_err_from<T, E>(res: Result<T, E>, expected_prefix: &str)
    where
        StampError: From<E>,
    {
        let mapped = res.map_err(|e| {
            let stamp_err = StampError::from(e);
            (
                stamp_err.to_string().starts_with(expected_prefix),
                stamp_err.source().is_some(),
            )
        });
        assert_eq!(mapped.err(), Some((true, true)));
    }

    #[test]
    fn test_stamp_error_display_and_sources() {
        let io_err = std::io::Error::other("io error");
        let stamp_io = StampError::from(io_err);
        assert_eq!(stamp_io.to_string(), "I/O error: io error");
        assert!(stamp_io.source().is_some());

        let json_res: Result<serde_json::Value, _> = serde_json::from_str("{ bad json");
        assert_err_from(json_res, "JSON error:");

        let yaml_res: Result<serde_yaml::Value, _> = serde_yaml::from_str("::::\n\tbad");
        assert_err_from(yaml_res, "YAML error:");

        let url_res = url::Url::parse("not a url");
        assert_err_from(url_res, "URL parse error:");

        let semver_res = semver::Version::parse("not-a-semver");
        assert_err_from(semver_res, "SemVer error:");

        let bad_regex_pattern = String::from("[unclosed");
        let regex_res = regex::Regex::new(&bad_regex_pattern);
        assert_err_from(regex_res, "Regex error:");

        let glob_res = glob::Pattern::new("[unclosed");
        assert_err_from(glob_res, "Glob pattern error:");

        let int_res = "abc".parse::<u32>();
        assert_err_from(int_res, "Integer parse error:");

        let float_res = "abc".parse::<f64>();
        assert_err_from(float_res, "Float parse error:");

        let bool_res = "notbool".parse::<bool>();
        assert_err_from(bool_res, "Boolean parse error:");

        let addr_res = "bad_addr".parse::<std::net::SocketAddr>();
        assert_err_from(addr_res, "Address parse error:");

        let ipnet_res = "bad_cidr".parse::<ipnet::Ipv4Net>();
        assert_err_from(ipnet_res, "IP network parse error:");

        let utf8_res = String::from_utf8(vec![0xff, 0xfe]);
        assert_err_from(utf8_res, "UTF-8 error:");

        let bad_bytes = vec![0xff_u8, 0xfe_u8];
        let str_utf8_res = std::str::from_utf8(&bad_bytes);
        assert_err_from(str_utf8_res, "UTF-8 slice error:");

        let status_err = StampError::from(tonic::Status::internal("grpc fail"));
        assert!(status_err.to_string().starts_with("gRPC status error:"));
        assert!(status_err.source().is_some());

        let zip_err = StampError::from(zip::result::ZipError::FileNotFound);
        assert!(zip_err.to_string().starts_with("ZIP error:"));
        assert!(zip_err.source().is_some());

        let russh_err = StampError::from(russh::Error::Disconnect);
        assert!(russh_err.to_string().starts_with("SSH error:"));
        assert!(russh_err.source().is_some());

        let russh_keys_err = StampError::from(russh::keys::Error::KeyIsCorrupt);
        assert!(russh_keys_err.to_string().starts_with("SSH key error:"));
        assert!(russh_keys_err.source().is_some());

        let ssh_key_err = StampError::from(russh::keys::ssh_key::Error::Version { number: 99 });
        assert!(ssh_key_err.to_string().starts_with("SSH key format error:"));
        assert!(ssh_key_err.source().is_some());

        let sftp_err = StampError::from(russh_sftp::client::error::Error::Timeout);
        assert!(sftp_err.to_string().starts_with("SFTP error:"));
        assert!(sftp_err.source().is_some());

        let rsa_err = StampError::from(rsa::Error::Verification);
        assert!(rsa_err.to_string().starts_with("RSA error:"));
        assert!(rsa_err.source().is_some());

        let bcrypt_err = StampError::from(bcrypt::BcryptError::CostNotAllowed(1));
        assert!(bcrypt_err.to_string().starts_with("Bcrypt error:"));
        assert!(bcrypt_err.source().is_some());

        assert_eq!(
            StampError::Parse("invalid syntax".to_string()).to_string(),
            "Parse error: invalid syntax"
        );
        assert!(
            StampError::Parse("invalid syntax".to_string())
                .source()
                .is_none()
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
            StampError::FieldValidation {
                field: "memory".to_string(),
                reason: "must be > 0".to_string(),
            }
            .to_string(),
            "Field validation error on 'memory': must be > 0"
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
            StampError::Timeout {
                operation: "ssh_connect".to_string(),
                duration: std::time::Duration::from_secs(5),
            }
            .to_string(),
            "Operation 'ssh_connect' timed out after 5000ms"
        );
        assert_eq!(
            StampError::SubprocessFailed {
                command: "qemu-img".to_string(),
                exit_code: Some(2),
                stderr: "disk full".to_string(),
            }
            .to_string(),
            "Subprocess 'qemu-img' failed with exit code Some(2): disk full"
        );
        assert_eq!(
            StampError::StateBagMissingKey {
                key: "vm_ip".to_string(),
                expected_type: "String",
            }
            .to_string(),
            "StateBag missing key 'vm_ip' (expected type String)"
        );
        assert_eq!(
            StampError::StateBagTypeMismatch {
                key: "vm_id".to_string(),
                expected_type: "u32",
            }
            .to_string(),
            "StateBag type mismatch for key 'vm_id' (expected type u32)"
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
    }
}
