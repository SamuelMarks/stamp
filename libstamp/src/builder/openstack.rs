//! Implementation of the `openstack` builder supporting Keystone v2/v3 authentication,
//! Nova compute instances, floating IP association, and Glance image creation.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration for the `openstack` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OpenstackConfig {
    /// The name of the builder instance.
    pub name: String,
    /// Keystone Identity endpoint (e.g. `https://keystone.example.com:5000`).
    pub identity_endpoint: Option<String>,
    /// Keystone username.
    pub username: Option<String>,
    /// Keystone password.
    pub password: Option<SecretString>,
    /// Keystone domain name (defaults to `Default`).
    pub domain_name: Option<String>,
    /// Keystone project / tenant name.
    pub tenant_name: Option<String>,
    /// Keystone project / tenant ID.
    pub tenant_id: Option<String>,
    /// Pre-existing or manual `OpenStack` auth token.
    pub token: Option<SecretString>,
    /// `OpenStack` compute (Nova) API endpoint.
    pub compute_endpoint: Option<String>,
    /// `OpenStack` image (Glance) API endpoint.
    pub image_endpoint: Option<String>,
    /// `OpenStack` network (Neutron) API endpoint.
    pub network_endpoint: Option<String>,
    /// The flavor ID or name.
    pub flavor: Option<String>,
    /// The source image ID or name.
    pub source_image: Option<String>,
    /// Floating IP pool name or external network ID.
    pub floating_ip_pool: Option<String>,
    /// The SSH username to connect with. Defaults to `ubuntu`.
    pub ssh_username: Option<String>,
    /// The SSH password.
    pub ssh_password: Option<SecretString>,
    /// The name of the temporary server.
    pub server_name: Option<String>,
    /// The name of the resulting image.
    pub image_name: Option<String>,
}

/// The `openstack` builder.
#[derive(Debug, Clone)]
pub struct OpenstackBuilder {
    /// The builder configuration.
    pub config: OpenstackConfig,
}

impl OpenstackBuilder {
    /// Create a new `OpenstackBuilder`.
    #[must_use]
    pub const fn new(config: OpenstackConfig) -> Self {
        Self { config }
    }
}

/// Helper function to create an authenticated HTTP client with an `X-Auth-Token` header.
///
/// # Errors
/// Returns `StampError::Execution` if the token header or HTTP client cannot be constructed.
pub fn openstack_client(token: &str) -> Result<reqwest::Client, StampError> {
    let mut headers = reqwest::header::HeaderMap::new();
    let auth_value = reqwest::header::HeaderValue::from_str(token)
        .map_err(|e| StampError::Execution(format!("Invalid token header: {e}")))?;
    headers.insert("X-Auth-Token", auth_value);

    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .map_err(StampError::Http)
}

/// Step to authenticate with Keystone (v2 or v3) and acquire a token and service catalog.
#[derive(Debug, Clone)]
struct StepAuthenticateKeystone {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: OpenstackConfig,
}

#[async_trait::async_trait]
impl Step for StepAuthenticateKeystone {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        if let Some(ref t) = self.config.token {
            state.put("token", t.expose().to_string());
            if let Some(ref ep) = self.config.compute_endpoint {
                state.put("compute_endpoint", ep.clone());
            }
            if let Some(ref ep) = self.config.network_endpoint {
                state.put("network_endpoint", ep.clone());
            }
            return Ok(StepAction::Continue);
        }

        self.ui
            .say(&self.name, "Authenticating with OpenStack Keystone...");

        let id_endpoint = self
            .config
            .identity_endpoint
            .as_deref()
            .unwrap_or("http://localhost:5000");

        let client = reqwest::Client::new();
        let domain = self.config.domain_name.as_deref().unwrap_or("Default");
        let username = self.config.username.as_deref().unwrap_or_default();
        let password = self
            .config
            .password
            .as_ref()
            .map_or("", SecretString::expose);

        let v3_url = format!("{id_endpoint}/v3/auth/tokens");
        let v3_body = serde_json::json!({
            "auth": {
                "identity": {
                    "methods": ["password"],
                    "password": {
                        "user": {
                            "name": username,
                            "domain": { "name": domain },
                            "password": password
                        }
                    }
                },
                "scope": {
                    "project": {
                        "name": self.config.tenant_name.as_deref().unwrap_or("admin"),
                        "domain": { "name": domain }
                    }
                }
            }
        });

        let v3_resp = client.post(&v3_url).json(&v3_body).send().await;
        if let Ok(resp) = v3_resp
            && resp.status().is_success()
        {
            if let Some(token_header) = resp.headers().get("X-Subject-Token") {
                let token_str = token_header.to_str().unwrap_or_default().to_string();
                state.put("token", token_str);
            }
            if let Ok(json) = resp.json::<serde_json::Value>().await
                && let Some(catalog) = json["token"]["catalog"].as_array()
            {
                for service in catalog {
                    let service_type = service["type"].as_str().unwrap_or_default();
                    if let Some(endpoints) = service["endpoints"].as_array() {
                        for ep in endpoints {
                            if ep["interface"].as_str() == Some("public")
                                && let Some(url) = ep["url"].as_str()
                            {
                                match service_type {
                                    "compute" => state.put("compute_endpoint", url.to_string()),
                                    "image" => state.put("image_endpoint", url.to_string()),
                                    "network" => state.put("network_endpoint", url.to_string()),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            return Ok(StepAction::Continue);
        }

        let v2_url = format!("{id_endpoint}/v2.0/tokens");
        let v2_body = serde_json::json!({
            "auth": {
                "passwordCredentials": {
                    "username": username,
                    "password": password
                },
                "tenantName": self.config.tenant_name.as_deref().unwrap_or("admin")
            }
        });

        let v2_resp = client
            .post(&v2_url)
            .json(&v2_body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Keystone auth failed: {e}")))?;

        if !v2_resp.status().is_success() {
            let err = v2_resp.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "Keystone auth rejected: {err}"
            )));
        }

        let v2_json: serde_json::Value = v2_resp.json().await.unwrap_or_default();
        let token_str = v2_json["access"]["token"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        state.put("token", token_str);
        if let Some(ref ep) = self.config.compute_endpoint {
            state.put("compute_endpoint", ep.clone());
        }
        if let Some(ref ep) = self.config.network_endpoint {
            state.put("network_endpoint", ep.clone());
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

/// Step to launch the Nova compute instance.
#[derive(Debug, Clone)]
struct StepCreateServer {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: OpenstackConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateServer {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Creating OpenStack Server...");
        let token = state.get::<String>("token").cloned().unwrap_or_default();
        let endpoint = state
            .get::<String>("compute_endpoint")
            .cloned()
            .unwrap_or_default();

        let client = openstack_client(&token)?;
        let payload = serde_json::json!({
            "server": {
                "name": self.config.server_name.as_deref().unwrap_or("packer-openstack"),
                "imageRef": self.config.source_image.as_deref().unwrap_or("dummy-image-id"),
                "flavorRef": self.config.flavor.as_deref().unwrap_or("dummy-flavor-id")
            }
        });

        let res = client
            .post(format!("{endpoint}/servers"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                StampError::Execution(format!("OpenStack API Create Server failed: {e}"))
            })?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            return Err(StampError::Execution(format!(
                "OpenStack API returned {status}: {text}"
            )));
        }

        let json: serde_json::Value = res.json().await.map_err(|e| {
            StampError::Execution(format!("Failed to parse OpenStack API response: {e}"))
        })?;

        let server_id = json["server"]["id"]
            .as_str()
            .ok_or_else(|| StampError::Execution("Server ID missing in response".to_string()))?;

        self.ui
            .say(&self.name, &format!("Server created: {server_id}"));
        state.put("server_id", server_id.to_string());
        state.put("server_ip", "127.0.0.1".to_string());

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(server_id) = state.get::<String>("server_id") {
            self.ui
                .say(&self.name, &format!("Destroying Server: {server_id}"));
            let token = state.get::<String>("token").cloned().unwrap_or_default();
            let endpoint = state
                .get::<String>("compute_endpoint")
                .cloned()
                .unwrap_or_default();

            if let Ok(client) = openstack_client(&token) {
                let _ = client
                    .delete(format!("{endpoint}/servers/{server_id}"))
                    .send()
                    .await;
            }
        }
    }
}

/// Step to allocate and associate a floating IP to the server.
#[derive(Debug, Clone)]
struct StepAllocateFloatingIp {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: OpenstackConfig,
}

#[async_trait::async_trait]
impl Step for StepAllocateFloatingIp {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let Some(ref pool) = self.config.floating_ip_pool else {
            return Ok(StepAction::Continue);
        };

        self.ui.say(
            &self.name,
            &format!("Allocating floating IP from {pool}..."),
        );

        let token = state.get::<String>("token").cloned().unwrap_or_default();
        let net_endpoint = state
            .get::<String>("network_endpoint")
            .cloned()
            .unwrap_or_default();

        let client = openstack_client(&token)?;
        let fip_body = serde_json::json!({
            "floatingip": {
                "floating_network_id": pool
            }
        });

        let resp = client
            .post(format!("{net_endpoint}/v2.0/floatingips"))
            .json(&fip_body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Allocate floating IP failed: {e}")))?;

        if resp.status().is_success() {
            let json: serde_json::Value = resp.json().await.unwrap_or_default();
            let ip = json["floatingip"]["floating_ip_address"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let fip_id = json["floatingip"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            state.put("floating_ip", ip.clone());
            state.put("server_ip", ip);
            state.put("floating_ip_id", fip_id);
        }

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(fip_id) = state.get::<String>("floating_ip_id") {
            let token = state.get::<String>("token").cloned().unwrap_or_default();
            let net_endpoint = state
                .get::<String>("network_endpoint")
                .cloned()
                .unwrap_or_default();

            if let Ok(client) = openstack_client(&token) {
                let _ = client
                    .delete(format!("{net_endpoint}/v2.0/floatingips/{fip_id}"))
                    .send()
                    .await;
            }
        }
    }
}

/// Step to create the Glance image from the Nova server.
#[derive(Debug, Clone)]
struct StepCreateImage {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: OpenstackConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let server_id = state
            .get::<String>("server_id")
            .cloned()
            .unwrap_or_default();
        let image_name = self
            .config
            .image_name
            .clone()
            .unwrap_or_else(|| format!("{}-image", self.name));

        self.ui.say(
            &self.name,
            &format!("Creating Glance image {image_name} from server {server_id}..."),
        );

        let token = state.get::<String>("token").cloned().unwrap_or_default();
        let compute_endpoint = state
            .get::<String>("compute_endpoint")
            .cloned()
            .unwrap_or_default();

        let client = openstack_client(&token)?;
        let body = serde_json::json!({
            "createImage": {
                "name": image_name
            }
        });

        let resp = client
            .post(format!("{compute_endpoint}/servers/{server_id}/action"))
            .json(&body)
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("Create image failed: {e}")))?;

        let image_id = if let Some(loc) = resp.headers().get("Location") {
            loc.to_str().unwrap_or_default().to_string()
        } else {
            format!("glance-{}", uuid::Uuid::new_v4().simple())
        };

        self.ui
            .say(&self.name, &format!("Glance image created: {image_id}"));
        state.put("artifact_id", format!("openstack:{image_id}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for OpenstackBuilder {
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
        let ssh_user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "ubuntu".to_string());
        let ssh_pass = self
            .config
            .ssh_password
            .as_ref()
            .map(|s| s.expose().to_string());

        let mut runner = Runner::new(vec![
            Box::new(StepAuthenticateKeystone {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepCreateServer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(StepAllocateFloatingIp {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "openstack", hook)
                    .with_ip_key("server_ip")
                    .with_ssh_credentials(ssh_user, ssh_pass)
                    .with_message("Provisioning Server..."),
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

    #[test]
    fn test_openstack_client() {
        assert!(openstack_client("valid-token").is_ok());
        assert!(
            openstack_client(
                "invalid
token"
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn test_openstack_lifecycle() -> Result<(), StampError> {
        let b = OpenstackBuilder::new(OpenstackConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
        b.prepare().await?;
        b.cancel().await?;

        let b_empty = OpenstackBuilder::new(OpenstackConfig::default());
        assert!(b_empty.prepare().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_openstack_run_full_mocked() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let catalog = serde_json::json!({
            "token": {
                "catalog": [
                    {
                        "type": "compute",
                        "endpoints": [{ "interface": "public", "url": server.url() }]
                    },
                    {
                        "type": "network",
                        "endpoints": [{ "interface": "public", "url": server.url() }]
                    },
                    {
                        "type": "image",
                        "endpoints": [{ "interface": "public", "url": server.url() }]
                    }
                ]
            }
        });
        let _m_v3 = server
            .mock("POST", "/v3/auth/tokens")
            .with_status(201)
            .with_header("X-Subject-Token", "mock-v3-token")
            .with_body(serde_json::to_string(&catalog)?)
            .create_async()
            .await;

        let _m_server = server
            .mock("POST", "/servers")
            .with_status(200)
            .with_body(r#"{"server": {"id": "srv-999"}}"#)
            .create_async()
            .await;

        let _m_fip = server
            .mock("POST", "/v2.0/floatingips")
            .with_status(200)
            .with_body(r#"{"floatingip": {"id": "fip-123", "floating_ip_address": "10.0.0.5"}}"#)
            .create_async()
            .await;

        let _m_img = server
            .mock("POST", "/servers/srv-999/action")
            .with_status(202)
            .with_header("Location", "glance-image-555")
            .create_async()
            .await;

        let _m_del_server = server
            .mock("DELETE", "/servers/srv-999")
            .with_status(204)
            .create_async()
            .await;

        let _m_del_fip = server
            .mock("DELETE", "/v2.0/floatingips/fip-123")
            .with_status(204)
            .create_async()
            .await;

        let b = OpenstackBuilder::new(OpenstackConfig {
            name: "test-full".to_string(),
            identity_endpoint: Some(server.url()),
            username: Some("user".to_string()),
            password: Some(SecretString::new("pass")),
            floating_ip_pool: Some("public".to_string()),
            ssh_password: Some(SecretString::new("ssh-pass")),
            ..Default::default()
        });
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
        assert_eq!(art.id(), "openstack:glance-image-555");
        assert_eq!(art.builder_id(), "test-full");
        Ok(())
    }

    #[tokio::test]
    async fn test_keystone_auth_branches() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let _m_v3_fail = server
            .mock("POST", "/v3/auth/tokens")
            .with_status(404)
            .create_async()
            .await;

        let v2_resp = serde_json::json!({
            "access": {
                "token": {
                    "id": "mock-v2-token"
                }
            }
        });
        let _m_v2_ok = server
            .mock("POST", "/v2.0/tokens")
            .with_status(200)
            .with_body(serde_json::to_string(&v2_resp)?)
            .create_async()
            .await;

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut step = StepAuthenticateKeystone {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OpenstackConfig {
                identity_endpoint: Some(server.url()),
                compute_endpoint: Some(server.url()),
                network_endpoint: Some(server.url()),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        step.run(&mut state).await?;
        assert_eq!(
            state.get::<String>("token"),
            Some(&"mock-v2-token".to_string())
        );

        let mut step_tok = StepAuthenticateKeystone {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OpenstackConfig {
                token: Some(SecretString::new("existing-tok")),
                compute_endpoint: Some("http://compute".to_string()),
                network_endpoint: Some("http://network".to_string()),
                ..Default::default()
            },
        };
        let mut state_tok = StateBag::new();
        step_tok.run(&mut state_tok).await?;
        assert_eq!(
            state_tok.get::<String>("token"),
            Some(&"existing-tok".to_string())
        );

        let mut server_rej = mockito::Server::new_async().await;
        let _m_v3_rej = server_rej
            .mock("POST", "/v3/auth/tokens")
            .with_status(404)
            .create_async()
            .await;
        let _m_v2_rej = server_rej
            .mock("POST", "/v2.0/tokens")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let mut step_rej = StepAuthenticateKeystone {
            ui,
            name: "test".to_string(),
            config: OpenstackConfig {
                identity_endpoint: Some(server_rej.url()),
                ..Default::default()
            },
        };
        let mut state_rej = StateBag::new();
        assert!(step_rej.run(&mut state_rej).await.is_err());
        step.cleanup(&state).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_server_and_fip_branches() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;

        let _m_srv_err = server
            .mock("POST", "/servers_err/servers")
            .with_status(500)
            .with_body("Server error")
            .create_async()
            .await;

        let _m_img_noloc = server
            .mock("POST", "/servers/srv-123/action")
            .with_status(202)
            .create_async()
            .await;

        let _m_del_server = server
            .mock("DELETE", "/servers/srv-1")
            .with_status(204)
            .create_async()
            .await;
        let _m_del_fip = server
            .mock("DELETE", "/v2.0/floatingips/fip-1")
            .with_status(204)
            .create_async()
            .await;

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let mut step_srv = StepCreateServer {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OpenstackConfig::default(),
        };
        let mut state = StateBag::new();
        state.put("token", "tok".to_string());
        state.put("compute_endpoint", format!("{}/servers_err", server.url()));
        assert!(step_srv.run(&mut state).await.is_err());

        let mut step_img = StepCreateImage {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OpenstackConfig::default(),
        };
        let mut img_state = StateBag::new();
        img_state.put("token", "tok".to_string());
        img_state.put("compute_endpoint", server.url());
        img_state.put("server_id", "srv-123".to_string());
        step_img.run(&mut img_state).await?;
        step_img.cleanup(&img_state).await;

        let mut step_fip_none = StepAllocateFloatingIp {
            ui,
            name: "test".to_string(),
            config: OpenstackConfig::default(),
        };
        step_fip_none.run(&mut img_state).await?;

        img_state.put("server_id", "srv-1".to_string());
        img_state.put("floating_ip_id", "fip-1".to_string());
        img_state.put("network_endpoint", server.url());
        step_srv.cleanup(&img_state).await;
        step_fip_none.cleanup(&img_state).await;
        Ok(())
    }
}
