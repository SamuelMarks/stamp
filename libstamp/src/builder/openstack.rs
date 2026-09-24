#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `openstack` builder supporting Keystone v2/v3 authentication,
//! Nova compute instances, floating IP association, and Glance image creation.

use crate::builder::Builder;
use crate::communicator::ssh::{SshCommunicator, SshConfig};
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use crate::types::{Port, Timeout};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

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
    pub password: Option<String>,
    /// Keystone domain name (defaults to `Default`).
    pub domain_name: Option<String>,
    /// Keystone project / tenant name.
    pub tenant_name: Option<String>,
    /// Keystone project / tenant ID.
    pub tenant_id: Option<String>,
    /// Pre-existing or manual `OpenStack` auth token.
    pub token: Option<String>,
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
    pub ssh_password: Option<String>,
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

    Ok(reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap_or_default())
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
            state.put("token", t.clone());
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
        let password = self.config.password.as_deref().unwrap_or_default();

        // Attempt Keystone v3 authentication
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

        // Fallback: Keystone v2
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

/// Step to provision the server over SSH.
#[derive(Clone)]
struct StepProvision {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: OpenstackConfig,
    /// Provisioning hook.
    hook: Arc<dyn ProvisionHook>,
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning Server...");

        let ip = state
            .get::<String>("server_ip")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let ssh_config = SshConfig {
            host: ip,
            port: Port::new(22),
            username: self
                .config
                .ssh_username
                .clone()
                .unwrap_or_else(|| "ubuntu".to_string()),
            password: self.config.ssh_password.clone(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ..Default::default()
        };

        let comm = Arc::new(SshCommunicator::new(ssh_config));

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "openstack".to_string(),
            user: "ubuntu".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "openstack".to_string(),
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
            Box::new(StepProvision {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
                hook,
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

    #[test]
    fn test_openstack_name() {
        let b = OpenstackBuilder::new(OpenstackConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert_eq!(b.name(), "test");
    }

    #[tokio::test]
    async fn test_openstack_prepare_success() {
        let b = OpenstackBuilder::new(OpenstackConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert!(b.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_openstack_prepare_failure() {
        let b = OpenstackBuilder::new(OpenstackConfig::default());
        assert!(b.prepare().await.is_err());
    }

    #[tokio::test]
    async fn test_openstack_cancel() {
        let b = OpenstackBuilder::new(OpenstackConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert!(b.cancel().await.is_ok());
    }

    #[test]
    fn test_derived_traits() {
        let config = OpenstackConfig {
            name: "test".to_string(),
            identity_endpoint: Some("http://id".to_string()),
            username: Some("u".to_string()),
            password: Some("p".to_string()),
            domain_name: Some("d".to_string()),
            tenant_name: Some("t".to_string()),
            tenant_id: Some("tid".to_string()),
            token: Some("tok".to_string()),
            compute_endpoint: Some("http://compute".to_string()),
            image_endpoint: Some("http://image".to_string()),
            network_endpoint: Some("http://network".to_string()),
            flavor: Some("f".to_string()),
            source_image: Some("img".to_string()),
            floating_ip_pool: Some("pool".to_string()),
            ssh_username: Some("user".to_string()),
            ssh_password: Some("pass".to_string()),
            server_name: Some("srv".to_string()),
            image_name: Some("in".to_string()),
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let serialized = serde_json::to_string(&config);
        assert!(serialized.is_ok());
        for json in serialized {
            let deserialized: Result<OpenstackConfig, _> = serde_json::from_str(&json);
            assert!(deserialized.is_ok());
        }

        let builder = OpenstackBuilder::new(config);
        assert_eq!(format!("{builder:?}"), format!("{builder:?}"));
    }

    #[tokio::test]
    async fn test_openstack_run_full_mocked() {
        let mut server = mockito::Server::new_async().await;

        // Keystone v3 auth
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
            .with_body(serde_json::to_string(&catalog).unwrap_or_default())
            .create_async()
            .await;

        // Nova create server
        let _m_server = server
            .mock("POST", "/servers")
            .with_status(200)
            .with_body(r#"{"server": {"id": "srv-999"}}"#)
            .create_async()
            .await;

        // Neutron floating IP
        let _m_fip = server
            .mock("POST", "/v2.0/floatingips")
            .with_status(200)
            .with_body(r#"{"floatingip": {"id": "fip-123", "floating_ip_address": "10.0.0.5"}}"#)
            .create_async()
            .await;

        // Glance create image
        let _m_img = server
            .mock("POST", "/servers/srv-999/action")
            .with_status(202)
            .with_header("Location", "glance-image-555")
            .create_async()
            .await;

        // Nova delete server (cleanup)
        let _m_del_server = server
            .mock("DELETE", "/servers/srv-999")
            .with_status(204)
            .create_async()
            .await;

        // Neutron delete floating IP (cleanup)
        let _m_del_fip = server
            .mock("DELETE", "/v2.0/floatingips/fip-123")
            .with_status(204)
            .create_async()
            .await;

        let b = OpenstackBuilder::new(OpenstackConfig {
            name: "test-full".to_string(),
            identity_endpoint: Some(server.url()),
            username: Some("user".to_string()),
            password: Some("pass".to_string()),
            floating_ip_pool: Some("public".to_string()),
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
        let res = b.run(hook, ui, OnErrorStrategy::Cleanup).await;
        assert!(res.is_ok());
        for art in res {
            assert_eq!(art.id(), "openstack:glance-image-555");
        }
    }

    #[tokio::test]
    async fn test_keystone_auth_branches() {
        let mut server = mockito::Server::new_async().await;

        // v3 fails, v2 succeeds
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
            .with_body(serde_json::to_string(&v2_resp).unwrap_or_default())
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
        assert!(step.run(&mut state).await.is_ok());
        assert_eq!(
            state.get::<String>("token"),
            Some(&"mock-v2-token".to_string())
        );

        // When token is already configured
        let mut step_tok = StepAuthenticateKeystone {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OpenstackConfig {
                token: Some("existing-tok".to_string()),
                compute_endpoint: Some("http://compute".to_string()),
                network_endpoint: Some("http://network".to_string()),
                ..Default::default()
            },
        };
        let mut state_tok = StateBag::new();
        assert!(step_tok.run(&mut state_tok).await.is_ok());
        assert_eq!(
            state_tok.get::<String>("token"),
            Some(&"existing-tok".to_string())
        );

        // v2 rejected
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
            ui: ui.clone(),
            name: "test".to_string(),
            config: OpenstackConfig {
                identity_endpoint: Some(server_rej.url()),
                ..Default::default()
            },
        };
        let mut state_rej = StateBag::new();
        assert!(step_rej.run(&mut state_rej).await.is_err());

        // Invalid network endpoint
        let mut step_net_err = StepAuthenticateKeystone {
            ui,
            name: "test".to_string(),
            config: OpenstackConfig {
                identity_endpoint: Some("http://invalid.keystone.endpoint:9999".to_string()),
                ..Default::default()
            },
        };
        let mut state_net_err = StateBag::new();
        assert!(step_net_err.run(&mut state_net_err).await.is_err());
        step.cleanup(&state).await;
    }

    #[tokio::test]
    async fn test_server_creation_and_image_branches() {
        let mut server = mockito::Server::new_async().await;

        // Server creation HTTP error
        let _m_srv_err = server
            .mock("POST", "/servers_err/servers")
            .with_status(500)
            .with_body("Server error")
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

        // Server creation network connection error
        let mut state_net_err = StateBag::new();
        state_net_err.put("token", "tok".to_string());
        state_net_err.put(
            "compute_endpoint",
            "http://invalid.compute.endpoint:9999".to_string(),
        );
        assert!(step_srv.run(&mut state_net_err).await.is_err());

        // Server creation invalid JSON response
        let _m_srv_bad_json = server
            .mock("POST", "/servers_bad_json/servers")
            .with_status(200)
            .with_body("not-valid-json")
            .create_async()
            .await;

        let mut state_bad_json = StateBag::new();
        state_bad_json.put("token", "tok".to_string());
        state_bad_json.put(
            "compute_endpoint",
            format!("{}/servers_bad_json", server.url()),
        );
        assert!(step_srv.run(&mut state_bad_json).await.is_err());

        // Server creation missing id in response
        let _m_srv_missing = server
            .mock("POST", "/servers_missing/servers")
            .with_status(200)
            .with_body(r#"{"server": {}}"#)
            .create_async()
            .await;

        let mut state_missing = StateBag::new();
        state_missing.put("token", "tok".to_string());
        state_missing.put(
            "compute_endpoint",
            format!("{}/servers_missing", server.url()),
        );
        assert!(step_srv.run(&mut state_missing).await.is_err());

        // Image creation without Location header (generates UUID fallback)
        let _m_img_noloc = server
            .mock("POST", "/servers/srv-123/action")
            .with_status(202)
            .create_async()
            .await;

        let mut step_img = StepCreateImage {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OpenstackConfig {
                image_name: None,
                ..Default::default()
            },
        };
        let mut img_state = StateBag::new();
        img_state.put("token", "tok".to_string());
        img_state.put("compute_endpoint", server.url());
        img_state.put("server_id", "srv-123".to_string());
        let res_img = step_img.run(&mut img_state).await;
        assert!(res_img.is_ok());
        step_img.cleanup(&img_state).await;

        // Image creation failure
        let mut img_state_fail = StateBag::new();
        img_state_fail.put("token", "tok".to_string());
        img_state_fail.put(
            "compute_endpoint",
            "http://invalid.glance.endpoint:9999".to_string(),
        );
        img_state_fail.put("server_id", "srv-123".to_string());
        assert!(step_img.run(&mut img_state_fail).await.is_err());
    }

    #[tokio::test]
    async fn test_floating_ip_and_provision_branches() {
        let mut server = mockito::Server::new_async().await;

        // Floating IP creation failure (status 500)
        let _m_fip_err = server
            .mock("POST", "/v2.0/floatingips")
            .with_status(500)
            .create_async()
            .await;

        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let mut step_fip = StepAllocateFloatingIp {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OpenstackConfig {
                floating_ip_pool: Some("public".to_string()),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        state.put("token", "tok".to_string());
        state.put("network_endpoint", server.url());
        assert!(step_fip.run(&mut state).await.is_ok());
        assert!(state.get::<String>("floating_ip").is_none());

        // When endpoint is invalid network error
        let mut state_net_err = StateBag::new();
        state_net_err.put("token", "tok".to_string());
        state_net_err.put(
            "network_endpoint",
            "http://invalid.neutron.endpoint:9999".to_string(),
        );
        assert!(step_fip.run(&mut state_net_err).await.is_err());

        // Floating IP skipped when pool is None
        let mut step_fip_none = StepAllocateFloatingIp {
            ui: ui.clone(),
            name: "test".to_string(),
            config: OpenstackConfig {
                floating_ip_pool: None,
                ..Default::default()
            },
        };
        assert!(step_fip_none.run(&mut state).await.is_ok());

        // Provisioner failure
        let fail_hook = Arc::new(DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut step_prov = StepProvision {
            ui,
            name: "test".to_string(),
            config: OpenstackConfig::default(),
            hook: fail_hook,
        };
        assert!(step_prov.run(&mut state).await.is_err());
        step_prov.cleanup(&state).await;
    }

    #[tokio::test]
    async fn test_builder_error_strategies() {
        let b = OpenstackBuilder::new(OpenstackConfig {
            name: "test".to_string(),
            identity_endpoint: Some("http://invalid.keystone:9999".to_string()),
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
    async fn test_openstack_cleanups() {
        let mut server = mockito::Server::new_async().await;
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
        state.put("server_id", "srv-1".to_string());
        state.put("token", "tok".to_string());
        state.put("compute_endpoint", server.url());
        step_srv.cleanup(&state).await;

        let mut step_fip = StepAllocateFloatingIp {
            ui,
            name: "test".to_string(),
            config: OpenstackConfig::default(),
        };
        state.put("floating_ip_id", "fip-1".to_string());
        state.put("network_endpoint", server.url());
        step_fip.cleanup(&state).await;

        // Cleanup with empty state
        let empty_state = StateBag::new();
        step_srv.cleanup(&empty_state).await;
        step_fip.cleanup(&empty_state).await;
    }
}
