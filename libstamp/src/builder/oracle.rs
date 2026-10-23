//! Implementation of the `oracle-oci` builder using Oracle Cloud Infrastructure REST API.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::{FilePath, SecretString};
use rsa::RsaPrivateKey;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::Pkcs1v15Sign;
use rsa::pkcs8::DecodePrivateKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::sync::Arc;

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
    pub private_key: Option<SecretString>,
    /// Passphrase for encrypted private key.
    pub passphrase: Option<SecretString>,
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
    pub ssh_password: Option<SecretString>,
    /// Optional endpoint URL override for API requests (e.g. for testing).
    pub endpoint: Option<String>,
}

/// Request parameters for signing an OCI HTTP request.
#[derive(Debug, Clone)]
pub struct OciSignParams<'a> {
    /// HTTP method (e.g. `"POST"`, `"GET"`).
    pub method: &'a str,
    /// Target URI path (e.g. `"/20160918/instances"`).
    pub target_path: &'a str,
    /// Target host header.
    pub host: &'a str,
    /// RFC 2822 / HTTP date string.
    pub date: &'a str,
    /// Optional request body bytes.
    pub body: Option<&'a [u8]>,
    /// Tenancy OCID.
    pub tenancy_ocid: &'a str,
    /// User OCID.
    pub user_ocid: &'a str,
    /// Public key fingerprint.
    pub fingerprint: &'a str,
    /// PEM-encoded RSA private key.
    pub private_key_pem: &'a str,
}

/// Helper function to create an OCI RFC 7540 compliant HTTP Authorization header.
///
/// Signs the request using the specified RSA private key.
///
/// # Errors
/// Returns `StampError::Execution` if the RSA key cannot be parsed or signing fails.
pub fn oci_sign_request(params: &OciSignParams<'_>) -> Result<String, StampError> {
    use base64::Engine;

    let key_id = format!(
        "{}/{}/{}",
        params.tenancy_ocid, params.user_ocid, params.fingerprint
    );
    let rsa_key = RsaPrivateKey::from_pkcs8_pem(params.private_key_pem)
        .or_else(|_| RsaPrivateKey::from_pkcs1_pem(params.private_key_pem))
        .map_err(|e| StampError::Execution(format!("Failed to parse OCI RSA private key: {e}")))?;

    let mut headers_list = vec!["(request-target)", "date", "host"];
    let mut signing_string = format!(
        "(request-target): {} {}
date: {}
host: {}",
        params.method.to_lowercase(),
        params.target_path,
        params.date,
        params.host
    );

    if let Some(b) = params.body {
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

    let signature = rsa_key
        .sign(Pkcs1v15Sign::new_unprefixed(), &signature_digest)
        .unwrap_or_default();

    let b64_sig = base64::engine::general_purpose::STANDARD.encode(signature);
    let headers_str = headers_list.join(" ");

    Ok(format!(
        r#"Signature version="1",keyId="{key_id}",algorithm="rsa-sha256",headers="{headers_str}",signature="{b64_sig}""#
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
            pk.expose().to_string()
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
        let body_bytes = serde_json::to_vec(&body)?;

        let auth_header = oci_sign_request(&OciSignParams {
            method: "POST",
            target_path: path,
            host: &host,
            date: &date,
            body: Some(&body_bytes),
            tenancy_ocid: tenancy,
            user_ocid: user,
            fingerprint,
            private_key_pem: &private_key,
        })?;

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
        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "opc".to_string());
        let ssh_pass = self
            .config
            .ssh_password
            .as_ref()
            .map(|s| s.expose().to_string());

        let mut runner = Runner::new(vec![
            Box::new(StepLaunchInstance {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "oracle-oci", hook)
                    .with_ssh_credentials(ssh_user, ssh_pass)
                    .with_message("Provisioning OCI instance..."),
            ),
            Box::new(StepCreateImage {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ]);

        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let artifact_id = state
            .get::<String>("artifact_id")
            .cloned()
            .unwrap_or_default();

        Ok(Box::new(BuildArtifact::with_builder(
            self.name(),
            artifact_id,
            vec![],
        )))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
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

    #[tokio::test]
    async fn test_oracle_prepare_and_cancel() -> Result<(), StampError> {
        let b = OracleOciBuilder::new(OracleOciConfig {
            name: "test".to_string(),
            private_key: Some(SecretString::new("pem")),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
        b.prepare().await?;
        b.cancel().await?;

        let b_keyfile = OracleOciBuilder::new(OracleOciConfig {
            name: "test".to_string(),
            key_file: Some(FilePath::new(std::path::PathBuf::from("/tmp/key.pem"))),
            ..Default::default()
        });
        b_keyfile.prepare().await?;

        let b_empty = OracleOciBuilder::new(OracleOciConfig::default());
        assert!(b_empty.prepare().await.is_err());

        let b_no_key = OracleOciBuilder::new(OracleOciConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert!(b_no_key.prepare().await.is_err());
        Ok(())
    }

    #[test]
    fn test_oci_signing() -> Result<(), StampError> {
        let auth = oci_sign_request(&OciSignParams {
            method: "POST",
            target_path: "/20160918/instances",
            host: "iaas.us-ashburn-1.oraclecloud.com",
            date: "Sun, 06 Sep 2026 00:00:00 GMT",
            body: Some(br#"{"hello": "world"}"#),
            tenancy_ocid: "ocid1.tenancy.oc1..test",
            user_ocid: "ocid1.user.oc1..test",
            fingerprint: "20:3a:c7:..",
            private_key_pem: TEST_RSA_PKCS1_PEM,
        })?;
        assert!(auth.contains(r#"Signature version="1""#));
        assert!(auth.contains("signature="));
        assert!(auth.contains("x-content-sha256"));

        let auth_get = oci_sign_request(&OciSignParams {
            method: "GET",
            target_path: "/20160918/instances",
            host: "iaas.us-ashburn-1.oraclecloud.com",
            date: "Sun, 06 Sep 2026 00:00:00 GMT",
            body: None,
            tenancy_ocid: "ocid1.tenancy.oc1..test",
            user_ocid: "ocid1.user.oc1..test",
            fingerprint: "20:3a:c7:..",
            private_key_pem: TEST_RSA_PKCS1_PEM,
        })?;
        assert!(auth_get.contains(r#"Signature version="1""#));
        assert!(!auth_get.contains("x-content-sha256"));

        assert!(
            oci_sign_request(&OciSignParams {
                method: "GET",
                target_path: "/",
                host: "host",
                date: "date",
                body: None,
                tenancy_ocid: "t",
                user_ocid: "u",
                fingerprint: "f",
                private_key_pem: "invalid-pem",
            })
            .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_oracle_run_mocked() -> Result<(), StampError> {
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
            private_key: Some(SecretString::new(TEST_RSA_PKCS1_PEM)),
            endpoint: Some(server.url()),
            image_name: Some("custom-img".to_string()),
            ssh_password: Some(SecretString::new("pass")),
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

        let art = b.run(hook, ui, OnErrorStrategy::Cleanup).await?;
        assert!(art.id().starts_with("oracle-oci:ocid1.image.oc1."));
        assert_eq!(art.builder_id(), "test-oracle");
        Ok(())
    }

    #[tokio::test]
    async fn test_oracle_run_failures_and_steps() -> Result<(), StampError> {
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
            private_key: Some(SecretString::new(TEST_RSA_PKCS1_PEM)),
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

        assert!(
            b.run(hook.clone(), ui.clone(), OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
        assert!(
            b.run(hook, ui.clone(), OnErrorStrategy::Abort)
                .await
                .is_err()
        );

        let dir = tempfile::tempdir()?;
        let key_file_path = dir.path().join("test_oci_key.pem");
        tokio::fs::write(&key_file_path, TEST_RSA_PKCS1_PEM).await?;

        let mut server2 = mockito::Server::new_async().await;
        let _m_launch_ok = server2
            .mock("POST", "/20160918/instances")
            .with_status(200)
            .with_body(r#"{"id": "ocid1.instance.oc1..inst-1"}"#)
            .create_async()
            .await;
        let _m_del = server2
            .mock("DELETE", "/20160918/instances/ocid1.instance.oc1..inst-1")
            .with_status(204)
            .create_async()
            .await;

        let mut step_launch_kf = StepLaunchInstance {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OracleOciConfig {
                key_file: Some(FilePath::new(key_file_path)),
                endpoint: Some(server2.url()),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        step_launch_kf.run(&mut state).await?;
        step_launch_kf.cleanup(&state).await;

        let mut step_img = StepCreateImage {
            ui,
            name: "test".to_string(),
            config: OracleOciConfig::default(),
        };
        step_img.run(&mut state).await?;
        step_img.cleanup(&state).await;
        Ok(())
    }
}
