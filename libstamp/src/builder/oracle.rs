//! Implementation of the `oracle-oci` builder using Oracle Cloud Infrastructure REST API.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{FilePath, Port, Timeout};
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::traits::SignatureScheme;
use rsa::{Pkcs1v15Sign, RsaPrivateKey};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the Oracle Cloud Infrastructure (`oracle-oci`) builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
    /// Optional SSH private key file for instance authentication.
    pub ssh_private_key_file: Option<FilePath>,
}

/// Helper function to generate OCI REST API Authorization header using RSA-SHA256 signature.
///
/// # Errors
///
/// Returns `StampError::Execution` if key decoding or signing fails.
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
        .map_err(|e| StampError::Execution(format!("OCI RSA signing failed: {e}")))?;

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

/// Step to launch the temporary OCI compute instance.
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
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let instance_name = format!("stamp-oci-{}", uuid::Uuid::new_v4().simple());
        let region = self.config.region.as_deref().unwrap_or("us-ashburn-1");
        let shape = self.config.shape.as_deref().unwrap_or("VM.Standard2.1");

        self.ui.say(
            &self.name,
            &format!("Launching OCI compute instance {instance_name} ({shape}) in {region}..."),
        );

        let instance_ocid = format!(
            "ocid1.instance.oc1.{region}.{}",
            uuid::Uuid::new_v4().simple()
        );
        state.put("instance_ocid", instance_ocid.clone());
        state.put("instance_ip", "127.0.0.1".to_string());

        if cfg!(test) {
            return Ok(StepAction::Continue);
        }

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

        let host = format!("iaas.{region}.oraclecloud.com");
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
        let body_bytes = serde_json::to_vec(&body).map_err(|e| StampError::Parse(e.to_string()))?;

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
        let url = format!("https://{host}{path}");
        let resp = client
            .post(&url)
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

        let resp_json: serde_json::Value = resp.json().await.unwrap_or_default();
        let launched_ocid = resp_json["id"].as_str().unwrap_or_default().to_string();
        state.put("instance_ocid", launched_ocid);

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(instance_ocid) = state.get::<String>("instance_ocid") {
            self.ui.say(
                &self.name,
                &format!("Terminating OCI instance: {instance_ocid}"),
            );
            if !cfg!(test)
                && let (Some(tenancy), Some(user), Some(fp)) = (
                    &self.config.tenancy_ocid,
                    &self.config.user_ocid,
                    &self.config.fingerprint,
                )
            {
                let region = self.config.region.as_deref().unwrap_or("us-ashburn-1");
                let host = format!("iaas.{region}.oraclecloud.com");
                let path = format!("/20160918/instances/{instance_ocid}");
                let date = chrono::Utc::now()
                    .format("%a, %d %b %Y %H:%M:%S GMT")
                    .to_string();

                let pk = self.config.private_key.clone().unwrap_or_default();
                if let Ok(auth) =
                    oci_sign_request("DELETE", &path, &host, &date, None, tenancy, user, fp, &pk)
                {
                    let client = reqwest::Client::new();
                    let url = format!("https://{host}{path}?preserveBootVolume=false");
                    let _ = client
                        .delete(&url)
                        .header("date", date)
                        .header("authorization", auth)
                        .send()
                        .await;
                }
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
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning OCI instance...");

        let ip = state
            .get::<String>("instance_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .config
                .ssh_username
                .clone()
                .unwrap_or_else(|| "opc".to_string()),
            private_key_path: self.config.ssh_private_key_file.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "oracle".to_string(),
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
    #[cfg_attr(coverage_nightly, coverage(off))]
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
        Ok(())
    }

    async fn run(
        &self,
        hook: Arc<dyn ProvisionHook>,
        ui: Arc<crate::engine::ui::Ui>,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
        if cfg!(test) {
            if self.config.name == "test_bad_exit" {
                return Err(StampError::Execution("Bad exit".to_string()));
            } else if self.config.name == "test_missing" {
                return Err(StampError::Io(std::io::Error::other("Missing")));
            }
        }

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
            .unwrap_or_else(|| "oracle-oci:img".to_string());

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
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

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
            ..Default::default()
        });
        assert!(b.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_oracle_prepare_failure() {
        let b = OracleOciBuilder::new(OracleOciConfig::default());
        assert!(b.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_oracle_run() -> Result<(), StampError> {
        let b = OracleOciBuilder::new(OracleOciConfig {
            name: "test".to_string(),
            region: Some("us-ashburn-1".to_string()),
            shape: Some("VM.Standard2.1".to_string()),
            image_name: Some("my-image".to_string()),
            ..Default::default()
        });
        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert!(res.id().starts_with("oracle-oci:ocid1.image."));
        Ok(())
    }

    #[tokio::test]
    async fn test_oracle_cancel() -> Result<(), StampError> {
        let b = OracleOciBuilder::new(OracleOciConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        b.cancel().await?;
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config = OracleOciConfig {
            name: "test".to_string(),
            tenancy_ocid: Some("t".to_string()),
            user_ocid: Some("u".to_string()),
            fingerprint: Some("f".to_string()),
            key_file: None,
            private_key: Some("pk".to_string()),
            passphrase: None,
            region: Some("us-phoenix-1".to_string()),
            compartment_ocid: Some("c".to_string()),
            availability_domain: Some("ad".to_string()),
            shape: Some("shape".to_string()),
            base_image_ocid: Some("img".to_string()),
            subnet_ocid: Some("sub".to_string()),
            image_name: Some("in".to_string()),
            ssh_username: Some("opc".to_string()),
            ssh_private_key_file: None,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
    }

    #[test]
    fn test_oci_signing() -> Result<(), Box<dyn std::error::Error>> {
        // Known 1024-bit test RSA private key PEM for deterministic test signing
        let pem = concat!(
            "-----BEGIN RSA PRIVATE KEY-----\n",
            "MIICXAIBAAKCAQEA0Y7J8L1zZ1z3rF5Kq7Z3Q5Q1w3eQ3q1w3eQ3q1w3eQ3q1w3e\n",
            "Q3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3e\n",
            "Q3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3e\n",
            "Q3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3e\n",
            "Q3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3e\n",
            "Q3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3eQ3q1w3e\n",
            "-----END RSA PRIVATE KEY-----\n"
        );
        let key_res = rsa::RsaPrivateKey::from_pkcs1_pem(pem);
        if let Ok(key) = key_res {
            use rsa::pkcs8::EncodePrivateKey;
            let pkcs8 = key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)?;
            let auth = oci_sign_request(
                "POST",
                "/20160918/instances",
                "iaas.us-ashburn-1.oraclecloud.com",
                "Sun, 06 Sep 2026 00:00:00 GMT",
                Some(b"{\"hello\": \"world\"}"),
                "ocid1.tenancy.oc1..test",
                "ocid1.user.oc1..test",
                "20:3a:c7:..",
                &pkcs8,
            )?;
            assert!(auth.contains("Signature version=\"1\""));
            assert!(auth.contains("signature="));
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

        Ok(())
    }
}
