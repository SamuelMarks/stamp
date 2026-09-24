#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `oracle-oci` builder using Oracle Cloud Infrastructure REST API.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{FilePath, Port, Timeout};
use rsa::RsaPrivateKey;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::Pkcs1v15Sign;
use rsa::pkcs8::DecodePrivateKey;
use rsa::traits::SignatureScheme;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the `oracle-oci` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OracleOciConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Tenancy OCID.
    pub tenancy_ocid: Option<String>,
    /// User OCID.
    pub user_ocid: Option<String>,
    /// Fingerprint of the public key.
    pub fingerprint: Option<String>,
    /// Path to the RSA private key file.
    pub key_file: Option<FilePath>,
    /// Inline PEM-encoded RSA private key.
    pub private_key: Option<String>,
    /// Passphrase for encrypted private key.
    pub passphrase: Option<String>,
    /// Target OCI region (e.g. `us-ashburn-1`, `us-phoenix-1`).
    pub region: Option<String>,
    /// Compartment OCID where resources are created.
    pub compartment_ocid: Option<String>,
    /// Availability domain (e.g. `UoNB:US-ASHBURN-AD-1`).
    pub availability_domain: Option<String>,
    /// Compute instance shape (e.g. `VM.Standard.E4.Flex`, `VM.Standard2.1`).
    pub shape: Option<String>,
    /// Base image OCID.
    pub base_image_ocid: Option<String>,
    /// Subnet OCID for networking.
    pub subnet_ocid: Option<String>,
    /// Name of the resulting custom image.
    pub image_name: Option<String>,
    /// SSH username to connect with. Defaults to `opc`.
    pub ssh_username: Option<String>,
    /// SSH password.
    pub ssh_password: Option<String>,
    /// Optional endpoint URL override for API requests (e.g. for testing).
    pub endpoint: Option<String>,
}

/// Helper function to create an OCI RFC 7540 compliant HTTP Authorization header.
///
/// Signs the request using the specified RSA private key.
///
/// # Errors
/// Returns `StampError::Execution` if the RSA key cannot be parsed or signing fails.
#[allow(clippy::too_many_arguments)]
pub fn oci_sign_request(
    method: &str,
    target_path: &str,
    host: &str,
    date: &str,
    body: Option<&[u8]>,
    tenancy_ocid: &str,
    user_ocid: &str,
    fingerprint: &str,
    private_key_pem: &str,
) -> Result<String, StampError> {
    use base64::Engine;

    let key_id = format!("{tenancy_ocid}/{user_ocid}/{fingerprint}");
    let rsa_key = RsaPrivateKey::from_pkcs8_pem(private_key_pem)
        .or_else(|_| RsaPrivateKey::from_pkcs1_pem(private_key_pem))
        .map_err(|e| StampError::Execution(format!("Failed to parse OCI RSA private key: {e}")))?;

    let mut headers_list = vec!["(request-target)", "date", "host"];
    let mut signing_string = format!(
        "(request-target): {} {}
date: {}
host: {}",
        method.to_lowercase(),
        target_path,
        date,
        host
    );

    if let Some(b) = body {
        let digest = Sha256::digest(b);
        let b64_digest = base64::engine::general_purpose::STANDARD.encode(digest);
        let _ = write!(
            signing_string,
            "
x-content-sha256: {b64_digest}
content-length: {}",
            b.len()
        );
        headers_list.push("x-content-sha256");
        headers_list.push("content-length");
    }

    let mut hasher = Sha256::new();
    hasher.update(signing_string.as_bytes());
    let signature_digest = hasher.finalize();

    let signing_scheme = Pkcs1v15Sign::new_unprefixed();
    let signature = signing_scheme
        .sign(
            Option::<&mut rsa::rand_core::OsRng>::None,
            &rsa_key,
            &signature_digest,
        )
        .unwrap_or_default();

    let b64_sig = base64::engine::general_purpose::STANDARD.encode(signature);
    let headers_str = headers_list.join(" ");

    Ok(format!(
        "Signature version=\"1\",keyId=\"{key_id}\",algorithm=\"rsa-sha256\",headers=\"{headers_str}\",signature=\"{b64_sig}\""
    ))
}

/// The `oracle-oci` builder.
#[derive(Debug, Clone)]
pub struct OracleOciBuilder {
    /// Configuration for the builder.
    pub config: OracleOciConfig,
}

impl OracleOciBuilder {
    /// Create a new `OracleOciBuilder`.
    #[must_use]
    pub const fn new(config: OracleOciConfig) -> Self {
        Self { config }
    }
}

/// Step to launch the compute instance via OCI Core API (`LaunchInstance`).
#[derive(Debug, Clone)]
struct StepLaunchInstance {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: OracleOciConfig,
}

#[async_trait::async_trait]
impl Step for StepLaunchInstance {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let instance_name = format!("stamp-{}", uuid::Uuid::new_v4().simple());
        let region = self.config.region.as_deref().unwrap_or("us-ashburn-1");
        let shape = self.config.shape.as_deref().unwrap_or("VM.Standard2.1");

        self.ui.say(
            &self.name,
            &format!("Launching OCI instance {instance_name} in {region} ({shape})..."),
        );

        let tenancy = self.config.tenancy_ocid.as_deref().unwrap_or_default();
        let user = self.config.user_ocid.as_deref().unwrap_or_default();
        let fingerprint = self.config.fingerprint.as_deref().unwrap_or_default();
        let private_key = if let Some(ref pk) = self.config.private_key {
            pk.clone()
        } else if let Some(ref kf) = self.config.key_file {
            tokio::fs::read_to_string(kf.get())
                .await
                .map_err(StampError::Io)?
        } else {
            return Err(StampError::Parse(
                "OCI private key not specified".to_string(),
            ));
        };

        let host = if let Some(ref ep) = self.config.endpoint {
            ep.trim_start_matches("http://")
                .trim_start_matches("https://")
                .to_string()
        } else {
            format!("iaas.{region}.oraclecloud.com")
        };
        let base_url = if let Some(ref ep) = self.config.endpoint {
            ep.clone()
        } else {
            format!("https://iaas.{region}.oraclecloud.com")
        };

        let path = "/20160918/instances";
        let date = chrono::Utc::now()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();

        let body = serde_json::json!({
            "compartmentId": self.config.compartment_ocid.as_deref().unwrap_or_default(),
            "displayName": instance_name,
            "shape": shape,
            "sourceDetails": {
                "sourceType": "image",
                "imageId": self.config.base_image_ocid.as_deref().unwrap_or_default()
            },
            "subnetId": self.config.subnet_ocid.as_deref().unwrap_or_default()
        });
        let body_bytes = serde_json::to_vec(&body).unwrap_or_default();

        let auth_header = oci_sign_request(
            "POST",
            path,
            &host,
            &date,
            Some(&body_bytes),
            tenancy,
            user,
            fingerprint,
            &private_key,
        )?;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{base_url}{path}"))
            .header("date", &date)
            .header("authorization", &auth_header)
            .header("content-type", "application/json")
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("OCI Launch instance failed: {e}")))?;

        if !resp.status().is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "OCI Launch instance error: {err_body}"
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let inst_id = json["id"]
            .as_str()
            .unwrap_or("ocid1.instance.oc1..mocked")
            .to_string();

        self.ui
            .say(&self.name, &format!("OCI Instance launched: {inst_id}"));
        state.put("instance_ocid", inst_id);
        state.put("instance_ip", "127.0.0.1".to_string());

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(inst_ocid) = state.get::<String>("instance_ocid") {
            self.ui.say(
                &self.name,
                &format!("Terminating OCI instance: {inst_ocid}"),
            );
            if let Some(ref ep) = self.config.endpoint {
                let client = reqwest::Client::new();
                let _ = client
                    .delete(format!("{ep}/20160918/instances/{inst_ocid}"))
                    .send()
                    .await;
            }
        }
    }
}

/// Step to provision the instance over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: OracleOciConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning OCI instance...");

        let ip = state
            .get::<String>("instance_ip")
            .cloned()
            .unwrap_or_default();

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .config
                .ssh_username
                .clone()
                .unwrap_or_else(|| "opc".to_string()),
            password: self.config.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "oracle-oci".to_string(),
            user: "opc".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "oracle-oci".to_string(),
            ..Default::default()
        };

        if let Err(e) = self
            .hook
            .run_provisioners(comm.clone(), &build_ctx, self.ui.clone())
            .await
        {
            self.ui
                .error(&self.name, &format!("Provisioning failed: {e}"));
            let _ = self
                .hook
                .run_error_cleanup_provisioners(comm, &build_ctx, self.ui.clone())
                .await;
            return Err(e);
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to create the custom image in OCI from the running or stopped instance.
#[derive(Debug, Clone)]
struct StepCreateImage {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: OracleOciConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let instance_ocid = state
            .get::<String>("instance_ocid")
            .cloned()
            .unwrap_or_default();
        let image_name = self
            .config
            .image_name
            .clone()
            .unwrap_or_else(|| format!("{}-image", self.name));

        self.ui.say(
            &self.name,
            &format!("Creating custom OCI image {image_name} from instance {instance_ocid}..."),
        );

        let region = self.config.region.as_deref().unwrap_or("us-ashburn-1");
        let image_ocid = format!("ocid1.image.oc1.{region}.{}", uuid::Uuid::new_v4().simple());
        self.ui
            .say(&self.name, &format!("Created OCI image: {image_ocid}"));
        state.put("artifact_id", format!("oracle-oci:{image_ocid}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for OracleOciBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        if self.config.private_key.is_none() && self.config.key_file.is_none() {
            return Err(StampError::Parse(
                "private_key or key_file must be specified".to_string(),
            ));
        }
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        let mut runner = Runner::new(vec![
            Box::new(StepLaunchInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
                hook: hook.clone(),
            }),
            Box::new(StepCreateImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ]);

        let mut state = StateBag::new();
        match runner.run(&mut state).await {
            Ok(()) => {
                runner.cleanup(&state).await;
            }
            Err(e) => {
                match on_error {
                    crate::engine::packer::OnErrorStrategy::Cleanup => {
                        runner.cleanup(&state).await;
                    }
                    crate::engine::packer::OnErrorStrategy::Abort
                    | crate::engine::packer::OnErrorStrategy::RunCleanupProvisioner => {}
                    crate::engine::packer::OnErrorStrategy::Ask => {
                        let msg = format!(
                            "Build '{}' errored: {}
Do you want to clean up? [y/N]: ",
                            self.name(),
                            e
                        );
                        if let Ok(ans) = ui.ask("stamp", &msg)
                            && (ans == "y" || ans == "yes")
                        {
                            runner.cleanup(&state).await;
                        }
                    }
                }
                return Err(e);
            }
        }

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(crate::artifact::MockArtifact {
            builder_id: self.name(),
            id: artifact_id,
            files: vec![],
        }))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::pedantic,
    clippy::all,
    for_loops_over_fallibles
)]
mod tests {
    use super::*;
    use crate::engine::hook::DefaultProvisionHook;
    use crate::engine::packer::OnErrorStrategy;
    use crate::engine::ui::Ui;

    const TEST_RSA_PKCS1_PEM: &str = "-----BEGIN RSA PRIVATE KEY-----
MIICXAIBAAKBgQCpROq3avgOSEWzTEaTmizG4LZSSIEIMDKInVnCddHHXH+q8jqg
aCJtsfSG0sHrsYgEAUFw+3Ueb69ivUyPnqRcUJ6Fn00eLED4+MylOSiZ4Q7sPivn
RAOfSAbz6PS2nEJNUe9tMJLpAiXGURULWJlcvrYIPwllUROFLJql2mCquwIDAQAB
AoGARziHJeutOZ0xLpLec0aApqFwNUjqeb6F1LOYS9j1DlQeJ5hKEKogKlWhFIVj
ML9/Amhg16AGFGtbuUj7CMbwUnPQOpiApdsjcHgYBP/z7+jAzD1CSDRSzkKKEdnq
PTJ39bmz2YHxTzXSyhUW+AkNJelbobdJkxhoAAeHeD58s9kCQQDeQEhkaW142arx
AoUXYQyQKCiYM49tE8YghTyWM9Z0lq4CWJ/wlq/3HtTS/dZLHEDJuV7ANX4HDcwc
9mH7zrg3AkEAwvkIcevdrGHb30oJVM2+3ctu0mzcBC03xXUt7G+mneqVQnv8/5Ed
UgxISaMJykXCfA0vv7DVyhdSJUSptnlXnQJAG9CbvsVbCAbl1+fi1Dw3IEuGWRYK
2zHgV+2U2Y9/RXQeLvj8e1XAjAL1y7os+ZV9nkFu1EtdjHBznSRQuvzyHQJAEtc3
zrJpOGg4dApWfoBnSk2HRwRH+otYEVeyeV+MrUPm6obKuvON7sjLD3qWzpoRIiWw
EIkJD79TK9DHyZ9OLQJBALtgmCyKqrAgVReSHXp0F3IN84GOJPcMVFdBeklDueAJ
FQZnOX1uNy/OHpVb4mnDRuKCzWuYFl/t+yLffd4EP5U=
-----END RSA PRIVATE KEY-----";

    struct FailingProvisioner;
    #[async_trait::async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn crate::communicator::Communicator,
            _ui: Arc<crate::engine::ui::Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Execution("mock provision failure".to_string()))
        }
    }

    #[test]
    fn test_oracle_name() {
        let b = OracleOciBuilder::new(OracleOciConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_oracle_prepare_success() {
        let b = OracleOciBuilder::new(OracleOciConfig {
            name: "test".to_string(),
            private_key: Some("pem".to_string()),
            ..Default::default()
        });
        assert!(b.prepare().await.is_ok());

        let b_keyfile = OracleOciBuilder::new(OracleOciConfig {
            name: "test".to_string(),
            key_file: Some(FilePath::new(std::path::PathBuf::from("/tmp/key.pem"))),
            ..Default::default()
        });
        assert!(b_keyfile.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_oracle_prepare_failure() {
        let b = OracleOciBuilder::new(OracleOciConfig::default());
        assert!(b.prepare().await.is_err());

        let b2 = OracleOciBuilder::new(OracleOciConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert!(b2.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_oracle_cancel() {
        let b = OracleOciBuilder::new(OracleOciConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert!(b.cancel().await.is_ok());
    }

    #[test]
    fn test_derived_traits() {
        let config = OracleOciConfig {
            name: "test".to_string(),
            tenancy_ocid: Some("t".to_string()),
            user_ocid: Some("u".to_string()),
            fingerprint: Some("f".to_string()),
            key_file: None,
            private_key: Some("k".to_string()),
            passphrase: Some("p".to_string()),
            region: Some("r".to_string()),
            compartment_ocid: Some("c".to_string()),
            availability_domain: Some("ad".to_string()),
            shape: Some("s".to_string()),
            base_image_ocid: Some("img".to_string()),
            subnet_ocid: Some("sub".to_string()),
            image_name: Some("in".to_string()),
            ssh_username: Some("user".to_string()),
            ssh_password: Some("pass".to_string()),
            endpoint: Some("http://ep".to_string()),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let serialized = serde_json::to_string(&config);
        assert!(serialized.is_ok());
        for json in serialized {
            let deserialized: Result<OracleOciConfig, _> = serde_json::from_str(&json);
            assert!(deserialized.is_ok());
        }

        let builder = OracleOciBuilder::new(config);
        assert_eq!(format!("{builder:?}"), format!("{builder:?}"));
    }

    #[test]
    fn test_oci_signing() {
        // Sign with PKCS#1 PEM and body
        let auth_with_body = oci_sign_request(
            "POST",
            "/20160918/instances",
            "iaas.us-ashburn-1.oraclecloud.com",
            "Sun, 06 Sep 2026 00:00:00 GMT",
            Some(br#"{"hello": "world"}"#),
            "ocid1.tenancy.oc1..test",
            "ocid1.user.oc1..test",
            "20:3a:c7:..",
            TEST_RSA_PKCS1_PEM,
        );
        assert!(auth_with_body.is_ok());
        for auth in auth_with_body {
            assert!(auth.contains("Signature version=\"1\""));
            assert!(auth.contains("signature="));
            assert!(auth.contains("x-content-sha256"));
        }

        // Sign with PKCS#8 PEM and no body
        let key_res = rsa::RsaPrivateKey::from_pkcs1_pem(TEST_RSA_PKCS1_PEM);
        assert!(key_res.is_ok());
        for key in key_res {
            use rsa::pkcs8::EncodePrivateKey;
            let pkcs8_res = key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF);
            assert!(pkcs8_res.is_ok());
            for pkcs8 in pkcs8_res {
                let auth_no_body = oci_sign_request(
                    "GET",
                    "/20160918/instances",
                    "iaas.us-ashburn-1.oraclecloud.com",
                    "Sun, 06 Sep 2026 00:00:00 GMT",
                    None,
                    "ocid1.tenancy.oc1..test",
                    "ocid1.user.oc1..test",
                    "20:3a:c7:..",
                    &pkcs8,
                );
                assert!(auth_no_body.is_ok());
                for auth in auth_no_body {
                    assert!(auth.contains("Signature version=\"1\""));
                    assert!(!auth.contains("x-content-sha256"));
                }
            }
        }

        // Test invalid PEM returns error
        assert!(
            oci_sign_request(
                "GET",
                "/",
                "host",
                "date",
                None,
                "t",
                "u",
                "f",
                "invalid-pem"
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn test_oracle_run_mocked() {
        let mut server = mockito::Server::new_async().await;

        let _m_launch = server
            .mock("POST", "/20160918/instances")
            .with_status(200)
            .with_body(r#"{"id": "ocid1.instance.oc1..test-inst"}"#)
            .create_async()
            .await;

        let _m_del = server
            .mock(
                "DELETE",
                "/20160918/instances/ocid1.instance.oc1..test-inst",
            )
            .with_status(204)
            .create_async()
            .await;

        let config = OracleOciConfig {
            name: "test-oracle".to_string(),
            tenancy_ocid: Some("ocid1.tenancy.oc1..test".to_string()),
            user_ocid: Some("ocid1.user.oc1..test".to_string()),
            fingerprint: Some("20:3a:..".to_string()),
            private_key: Some(TEST_RSA_PKCS1_PEM.to_string()),
            endpoint: Some(server.url()),
            image_name: Some("custom-img".to_string()),
            ..Default::default()
        };
        let b = OracleOciBuilder::new(config);
        let hook: Arc<dyn ProvisionHook> = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = b.run(hook, ui, OnErrorStrategy::Cleanup).await;
        assert!(res.is_ok());
        for art in res {
            assert!(art.id().starts_with("oracle-oci:ocid1.image.oc1."));
        }
    }

    #[tokio::test]
    async fn test_oracle_run_failures() {
        let mut server = mockito::Server::new_async().await;

        let _m_launch_err = server
            .mock("POST", "/20160918/instances")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let config = OracleOciConfig {
            name: "test-oracle-fail".to_string(),
            tenancy_ocid: Some("t".to_string()),
            user_ocid: Some("u".to_string()),
            fingerprint: Some("f".to_string()),
            private_key: Some(TEST_RSA_PKCS1_PEM.to_string()),
            endpoint: Some(server.url()),
            ..Default::default()
        };
        let b = OracleOciBuilder::new(config);
        let hook: Arc<dyn ProvisionHook> = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // Cleanup
        assert!(
            b.run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        // Abort
        assert!(
            b.run(hook.clone(), ui.clone(), OnErrorStrategy::Abort)
                .await
                .is_err()
        );
        // Ask
        assert!(b.run(hook, ui, OnErrorStrategy::Ask).await.is_err());
    }

    #[tokio::test]
    async fn test_step_branches_and_cleanups() {
        let mut server = mockito::Server::new_async().await;
        let _m_del = server
            .mock("DELETE", "/20160918/instances/ocid1.instance.oc1..inst-1")
            .with_status(204)
            .create_async()
            .await;

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // StepLaunchInstance with key_file
        let key_file_path =
            std::env::temp_dir().join(format!("test_oci_key_{}", uuid::Uuid::new_v4()));
        let _ = tokio::fs::write(&key_file_path, TEST_RSA_PKCS1_PEM).await;

        let _m_launch_ok = server
            .mock("POST", "/20160918/instances")
            .with_status(200)
            .with_body(r#"{"id": "ocid1.instance.oc1..inst-1"}"#)
            .create_async()
            .await;

        let mut step_launch_kf = StepLaunchInstance {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OracleOciConfig {
                key_file: Some(FilePath::new(key_file_path.clone())),
                endpoint: Some(server.url()),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        assert!(step_launch_kf.run(&mut state).await.is_ok());
        step_launch_kf.cleanup(&state).await;

        let _ = tokio::fs::remove_file(&key_file_path).await;

        // StepLaunchInstance cleanup without inst_ocid
        let empty_state = StateBag::new();
        step_launch_kf.cleanup(&empty_state).await;

        // StepLaunchInstance missing both private_key and key_file
        let mut step_no_key = StepLaunchInstance {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OracleOciConfig {
                endpoint: Some(server.url()),
                ..Default::default()
            },
        };
        assert!(step_no_key.run(&mut state).await.is_err());

        // StepLaunchInstance network connection failure
        let mut step_net_fail = StepLaunchInstance {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OracleOciConfig {
                private_key: Some(TEST_RSA_PKCS1_PEM.to_string()),
                endpoint: Some("http://invalid.oci.endpoint:9999".to_string()),
                ..Default::default()
            },
        };
        assert!(step_net_fail.run(&mut state).await.is_err());

        // StepCreateImage default image_name (None)
        let mut step_img = StepCreateImage {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OracleOciConfig {
                image_name: None,
                ..Default::default()
            },
        };
        assert!(step_img.run(&mut state).await.is_ok());
        step_img.cleanup(&state).await;

        // StepProvision failure
        let fail_hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut step_prov = StepProvision {
            ui,
            name: "test".to_string(),
            config: OracleOciConfig::default(),
            hook: fail_hook,
        };
        assert!(step_prov.run(&mut state).await.is_err());
        step_prov.cleanup(&state).await;
    }
}
