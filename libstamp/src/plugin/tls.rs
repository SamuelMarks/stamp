//! Ephemeral mutual TLS (mTLS) certificate generation and exchange for Go-plugin.
//!
//! Generates ephemeral in-memory X.509 certificates and keys using `rcgen` to secure
//! gRPC channels between Stamp and out-of-process plugins without requiring persistent disk files.

use crate::error::StampError;
use rcgen::{CertificateParams, DnType, KeyPair, KeyUsagePurpose};
use tonic::transport::{Certificate, ClientTlsConfig, Identity, ServerTlsConfig};

/// Container for an ephemeral mutual TLS certificate authority and key pairs.
#[derive(Clone)]
pub struct MtlsCertificates {
    /// Certificate Authority PEM string.
    pub ca_cert_pem: String,
    /// Server public certificate PEM string.
    pub server_cert_pem: String,
    /// Server private key PEM string.
    pub server_key_pem: String,
    /// Client public certificate PEM string.
    pub client_cert_pem: String,
    /// Client private key PEM string.
    pub client_key_pem: String,
}

impl std::fmt::Debug for MtlsCertificates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtlsCertificates")
            .field("ca_cert_pem", &"<PEM redacted>")
            .field("server_cert_pem", &"<PEM redacted>")
            .field("server_key_pem", &"<REDACTED>")
            .field("client_cert_pem", &"<PEM redacted>")
            .field("client_key_pem", &"<REDACTED>")
            .finish()
    }
}

impl MtlsCertificates {
    /// Generates a new ephemeral CA and signed server/client keypairs for mTLS.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if cryptographic generation or signing fails.
    pub fn generate() -> Result<Self, StampError> {
        // 1. Generate Ephemeral CA
        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "Stamp Ephemeral Plugin CA");
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let ca_key = KeyPair::generate()
            .map_err(|e| StampError::Execution(format!("CA keypair generation failed: {e}")))?;
        let ca_cert = ca_params
            .self_signed(&ca_key)
            .map_err(|e| StampError::Execution(format!("CA self-signing failed: {e}")))?;
        let ca_cert_pem = ca_cert.pem();

        // 2. Generate Server Certificate signed by CA
        let mut server_params = CertificateParams::new(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ])
        .map_err(|e| StampError::Execution(format!("Server SAN setup failed: {e}")))?;
        server_params
            .distinguished_name
            .push(DnType::CommonName, "Stamp Plugin Server");
        server_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        let server_key = KeyPair::generate()
            .map_err(|e| StampError::Execution(format!("Server keypair generation failed: {e}")))?;
        let server_cert = server_params
            .signed_by(&server_key, &ca_cert, &ca_key)
            .map_err(|e| StampError::Execution(format!("Server signing failed: {e}")))?;
        let server_cert_pem = server_cert.pem();
        let server_key_pem = server_key.serialize_pem();

        // 3. Generate Client Certificate signed by CA
        let mut client_params = CertificateParams::new(vec!["localhost".to_string()])
            .map_err(|e| StampError::Execution(format!("Client SAN setup failed: {e}")))?;
        client_params
            .distinguished_name
            .push(DnType::CommonName, "Stamp Plugin Client");
        client_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        let client_key = KeyPair::generate()
            .map_err(|e| StampError::Execution(format!("Client keypair generation failed: {e}")))?;
        let client_cert = client_params
            .signed_by(&client_key, &ca_cert, &ca_key)
            .map_err(|e| StampError::Execution(format!("Client signing failed: {e}")))?;
        let client_cert_pem = client_cert.pem();
        let client_key_pem = client_key.serialize_pem();

        Ok(Self {
            ca_cert_pem,
            server_cert_pem,
            server_key_pem,
            client_cert_pem,
            client_key_pem,
        })
    }

    /// Exports the server certificate encoded in base64 for handshake exchange.
    #[must_use]
    pub fn server_cert_base64(&self) -> String {
        use base64::Engine;
        base64::prelude::BASE64_STANDARD.encode(self.server_cert_pem.as_bytes())
    }

    /// Exports the client certificate encoded in base64 for environment variable exchange.
    #[must_use]
    pub fn client_cert_base64(&self) -> String {
        use base64::Engine;
        base64::prelude::BASE64_STANDARD.encode(self.client_cert_pem.as_bytes())
    }

    /// Constructs a tonic `ServerTlsConfig` requiring client certificate verification.
    #[must_use]
    pub fn server_tls_config(&self) -> ServerTlsConfig {
        let server_identity = Identity::from_pem(
            self.server_cert_pem.as_bytes(),
            self.server_key_pem.as_bytes(),
        );
        let client_ca = Certificate::from_pem(self.ca_cert_pem.as_bytes());

        ServerTlsConfig::new()
            .identity(server_identity)
            .client_ca_root(client_ca)
    }

    /// Constructs a tonic `ClientTlsConfig` presenting the client identity and verifying server certificate.
    #[must_use]
    pub fn client_tls_config(&self) -> ClientTlsConfig {
        let client_identity = Identity::from_pem(
            self.client_cert_pem.as_bytes(),
            self.client_key_pem.as_bytes(),
        );
        let server_ca = Certificate::from_pem(self.ca_cert_pem.as_bytes());

        ClientTlsConfig::new()
            .identity(client_identity)
            .ca_certificate(server_ca)
            .domain_name("localhost")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_ephemeral_mtls_generation_and_encoding() {
        let certs = MtlsCertificates::generate().unwrap();
        assert!(certs.ca_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(certs.server_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(certs.server_key_pem.contains("BEGIN PRIVATE KEY"));
        assert!(certs.client_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(certs.client_key_pem.contains("BEGIN PRIVATE KEY"));

        let b64_server = certs.server_cert_base64();
        assert!(!b64_server.is_empty());

        let b64_client = certs.client_cert_base64();
        assert!(!b64_client.is_empty());

        let _server_tls = certs.server_tls_config();
        let _client_tls = certs.client_tls_config();

        let debug_str = format!("{certs:?}");
        assert!(debug_str.contains("REDACTED"));
    }
}
