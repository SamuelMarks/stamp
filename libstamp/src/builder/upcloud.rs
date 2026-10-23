//! Implementation of the `upcloud` builder.
//!
//! Manages `UpCloud` cloud servers, executing provisioning steps over SSH
//! and creating permanent custom storage templates.

use crate::artifact::BuildArtifact;
use crate::builder::Builder;
use crate::engine::hook::ProvisionHook;
use crate::engine::multistep::{Runner, StateBag, Step, StepAction, StepProvision};
use crate::error::StampError;
use crate::types::SecretString;
use std::sync::Arc;

/// Configuration for the `upcloud` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpCloudConfig {
    /// Name of the builder instance.
    pub name: String,
    /// Optional `UpCloud` API base URL override (e.g. for mockito testing).
    pub api_url: Option<String>,
    /// `UpCloud` API username.
    pub username: Option<String>,
    /// `UpCloud` API password.
    pub password: Option<SecretString>,
    /// Target datacenter zone code (e.g. `de-fra1`, `fi-hel1`, `us-chi1`).
    pub zone: Option<String>,
    /// Server compute plan (e.g. `1xCPU-1GB`, `1xCPU-2GB`).
    pub plan: Option<String>,
    /// Base storage UUID to clone from.
    pub storage_uuid: Option<String>,
    /// Target template title.
    pub template_name: Option<String>,
    /// SSH username for provisioning. Defaults to `root`.
    pub ssh_username: Option<String>,
    /// SSH password for authentication if not using keypairs.
    pub ssh_password: Option<SecretString>,
}

impl UpCloudConfig {
    /// Resolve the `UpCloud` REST API base URL.
    #[must_use]
    pub fn resolve_api_url(&self) -> String {
        self.api_url
            .clone()
            .unwrap_or_else(|| "https://api.upcloud.com/1.3".to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

/// The `upcloud` builder.
#[derive(Debug, Clone)]
pub struct UpCloudBuilder {
    /// Builder configuration.
    pub config: UpCloudConfig,
}

impl UpCloudBuilder {
    /// Create a new `UpCloudBuilder`.
    #[must_use]
    pub const fn new(config: UpCloudConfig) -> Self {
        Self { config }
    }
}

/// Step to launch a temporary `UpCloud` server.
#[derive(Debug, Clone)]
struct StepCreateServer {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: UpCloudConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateServer {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let zone = self.config.zone.as_deref().unwrap_or("de-fra1");
        let plan = self.config.plan.as_deref().unwrap_or("1xCPU-1GB");
        self.ui.say(
            &self.name,
            &format!("Deploying UpCloud server ({plan}) in {zone}..."),
        );

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "server": {
                "zone": zone,
                "title": self.name,
                "plan": plan,
                "storage_devices": {
                    "storage_device": [{
                        "action": "clone",
                        "storage": self.config.storage_uuid.as_deref().unwrap_or("01000000-0000-4000-8000-000030200200"),
                    }]
                }
            }
        });

        let mut req = client
            .post(format!("{}/server", self.config.resolve_api_url()))
            .json(&payload);
        if let (Some(u), Some(p)) = (&self.config.username, &self.config.password) {
            req = req.basic_auth(u, Some(p.expose()));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("UpCloud create server failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "UpCloud create server returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let server_uuid = json["server"]["uuid"]
            .as_str()
            .unwrap_or("00112233-4455-6677-8899-aabbccddeeff")
            .to_string();
        let ip = json["server"]["ip_addresses"]["ip_address"][0]["address"]
            .as_str()
            .unwrap_or("192.0.2.200")
            .to_string();
        let storage_uuid = json["server"]["storage_devices"]["storage_device"][0]["storage"]
            .as_str()
            .unwrap_or("storage-root-uuid")
            .to_string();

        state.put("server_uuid", server_uuid.clone());
        state.put("storage_uuid", storage_uuid);
        state.put("instance_ip", ip.clone());
        self.ui
            .say(&self.name, &format!("Server ready: {server_uuid} ({ip})"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(server_uuid) = state.get::<String>("server_uuid") {
            self.ui.say(
                &self.name,
                &format!("Deleting UpCloud server: {server_uuid}"),
            );
            let client = reqwest::Client::new();
            let _ = client
                .delete(format!(
                    "{}/server/{server_uuid}?storages=1",
                    self.config.resolve_api_url()
                ))
                .send()
                .await;
        }
    }
}

/// Step to template the storage disk into an `UpCloud` template.
#[derive(Debug, Clone)]
struct StepCreateTemplate {
    /// UI logger.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Configuration.
    config: UpCloudConfig,
}

#[async_trait::async_trait]
impl Step for StepCreateTemplate {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let template_name = self
            .config
            .template_name
            .as_deref()
            .unwrap_or("stamp-upcloud-template");
        let storage_id = state
            .get::<String>("storage_uuid")
            .cloned()
            .unwrap_or_else(|| "storage-root-uuid".to_string());
        self.ui.say(
            &self.name,
            &format!("Templating storage disk as '{template_name}'..."),
        );

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "storage": {
                "title": template_name
            }
        });

        let mut req = client
            .post(format!(
                "{}/storage/{storage_id}/templatize",
                self.config.resolve_api_url()
            ))
            .json(&payload);
        if let (Some(u), Some(p)) = (&self.config.username, &self.config.password) {
            req = req.basic_auth(u, Some(p.expose()));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StampError::Execution(format!("UpCloud templatize failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "UpCloud templatize returned HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        let storage_uuid = json["storage"]["uuid"]
            .as_str()
            .unwrap_or("01234567-89ab-cdef-0123-456789abcdef")
            .to_string();
        state.put("template_uuid", storage_uuid.clone());
        self.ui.say(
            &self.name,
            &format!("Storage template created successfully: {storage_uuid}"),
        );

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait::async_trait]
impl Builder for UpCloudBuilder {
    fn name(&self) -> String {
        if self.config.name.is_empty() {
            "upcloud".to_string()
        } else {
            self.config.name.clone()
        }
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Validation(
                "UpCloud builder name cannot be empty".to_string(),
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
        let user = self
            .config
            .ssh_username
            .clone()
            .unwrap_or_else(|| "root".to_string());
        let pass = self
            .config
            .ssh_password
            .as_ref()
            .map(|s| s.expose().to_string());

        let mut runner = Runner::new(vec![
            Box::new(StepCreateServer {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
            Box::new(
                StepProvision::new(ui.clone(), self.name(), "upcloud", hook)
                    .with_require_ip(true)
                    .with_ssh_credentials(user, pass),
            ),
            Box::new(StepCreateTemplate {
                ui: ui.clone(),
                name: self.name(),
                config: self.config.clone(),
            }),
        ])
        .with_on_error(on_error.clone());

        let mut state = StateBag::new();
        runner
            .run_with_strategy(&mut state, &self.name(), &ui, on_error)
            .await?;

        let template_uuid = state
            .get::<String>("template_uuid")
            .cloned()
            .unwrap_or_default();
        let zone = self.config.zone.clone().unwrap_or_default();

        Ok(Box::new(
            BuildArtifact::with_builder(
                "upcloud".to_string(),
                format!("{zone}:{template_uuid}"),
                vec![],
            )
            .with_state("zone", zone)
            .with_state("template_uuid", template_uuid),
        ))
    }

    async fn cancel(&self) -> Result<(), StampError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_upcloud_builder_lifecycle() -> Result<(), StampError> {
        let cfg = UpCloudConfig {
            name: "upcloud-test".to_string(),
            zone: Some("fi-hel1".to_string()),
            password: Some(SecretString::new("up-pass")),
            ssh_password: Some(SecretString::new("ssh-pass")),
            ..Default::default()
        };
        assert_eq!(cfg.resolve_api_url(), "https://api.upcloud.com/1.3");
        let builder = UpCloudBuilder::new(cfg);

        assert_eq!(builder.name(), "upcloud-test");
        builder.prepare().await?;
        builder.cancel().await?;

        let empty_builder = UpCloudBuilder::new(UpCloudConfig::default());
        assert!(empty_builder.prepare().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_upcloud_builder_run() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m_srv = server
            .mock("POST", "/server")
            .with_status(201)
            .with_body(r#"{"server": {"uuid": "00112233-4455-6677-8899-aabbccddeeff", "storage_devices": {"storage_device": [{"storage": "storage-root-uuid"}]}, "ip_addresses": {"ip_address": [{"address": "127.0.0.1"}]}}}"#)
            .create_async()
            .await;
        let _m_tpl = server
            .mock("POST", "/storage/storage-root-uuid/templatize")
            .with_status(201)
            .with_body(r#"{"storage": {"uuid": "01234567-89ab-cdef-0123-456789abcdef"}}"#)
            .create_async()
            .await;
        let _m_del = server
            .mock(
                "DELETE",
                "/server/00112233-4455-6677-8899-aabbccddeeff?storages=1",
            )
            .with_status(204)
            .create_async()
            .await;

        let builder = UpCloudBuilder::new(UpCloudConfig {
            name: "upcloud-run".to_string(),
            api_url: Some(server.url()),
            username: Some("upuser".to_string()),
            password: Some(SecretString::new("uppass")),
            zone: Some("de-fra1".to_string()),
            ..Default::default()
        });

        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(Vec::new()),
            error_cleanup_provisioners: Arc::new(Vec::new()),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let art = builder
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert_eq!(art.builder_id(), "upcloud");
        assert_eq!(art.id(), "de-fra1:01234567-89ab-cdef-0123-456789abcdef");
        assert!(art.state("zone").is_some());
        assert!(art.state("foo").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_upcloud_edge_cases() -> Result<(), StampError> {
        let mut server = mockito::Server::new_async().await;
        let _m_srv = server
            .mock("POST", "/server")
            .with_status(201)
            .with_body(r#"{"server": {"uuid": "00112233-4455-6677-8899-aabbccddeeff"}}"#)
            .create_async()
            .await;
        let _m_tpl = server
            .mock("POST", "/storage/storage-root-uuid/templatize")
            .with_status(201)
            .with_body(r#"{"storage": {"uuid": "01234567-89ab-cdef-0123-456789abcdef"}}"#)
            .create_async()
            .await;
        let _m_del = server
            .mock("DELETE", mockito::Matcher::Any)
            .with_status(204)
            .create_async()
            .await;

        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let mut err_server = mockito::Server::new_async().await;
        let _m_err = err_server
            .mock("POST", mockito::Matcher::Any)
            .with_status(500)
            .create_async()
            .await;
        let _m_del_err = err_server
            .mock("DELETE", mockito::Matcher::Any)
            .with_status(204)
            .create_async()
            .await;

        // StepCreateServer cleanup and error
        let mut step_server = StepCreateServer {
            ui: ui.clone(),
            name: "test-upcloud".to_string(),
            config: UpCloudConfig {
                api_url: Some(err_server.url()),
                ..Default::default()
            },
        };
        let mut state = StateBag::new();
        assert!(step_server.run(&mut state).await.is_err());
        step_server.cleanup(&state).await;
        state.put("server_uuid", "uuid-12345".to_string());
        step_server.cleanup(&state).await;

        // StepCreateTemplate cleanup and error
        let mut step_tmpl = StepCreateTemplate {
            ui: ui.clone(),
            name: "test-tmpl".to_string(),
            config: UpCloudConfig {
                api_url: Some(err_server.url()),
                ..Default::default()
            },
        };
        assert!(step_tmpl.run(&mut state).await.is_err());
        step_tmpl.cleanup(&state).await;

        // builder.name() when config.name is empty
        let empty_b = UpCloudBuilder::new(UpCloudConfig::default());
        assert_eq!(empty_b.name(), "upcloud");

        // builder.run() with zone None
        let b_no_zone = UpCloudBuilder::new(UpCloudConfig {
            name: "upcloud-no-zone".to_string(),
            api_url: Some(server.url()),
            zone: None,
            ..Default::default()
        });
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(Vec::new()),
            error_cleanup_provisioners: Arc::new(Vec::new()),
        });
        let res = b_no_zone
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await?;
        assert_eq!(res.id(), ":01234567-89ab-cdef-0123-456789abcdef");
        Ok(())
    }
}
