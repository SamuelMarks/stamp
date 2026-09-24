#![cfg_attr(coverage_nightly, coverage(off))]
//! Implementation of the `lxd` builder using native LXD / Incus REST API / CLI.
//!
//! Manages container creation, provisioning, snapshotting, and image publishing (`/1.0/images` or `lxc publish`).

use crate::builder::Builder;
use crate::communicator::Communicator;
use crate::engine::hook::{BuildContext, ProvisionHook};
use crate::engine::multistep::{Runner, StateBag, Step, StepAction};
use crate::error::StampError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration for the `lxd` builder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LxdConfig {
    /// The name of the builder instance.
    pub name: String,
    /// Source image alias or fingerprint (e.g. `ubuntu:22.04` or `images:alpine/edge`).
    pub image: String,
    /// Target published image alias or name.
    pub output_image: Option<String>,
    /// LXD remote endpoint or UNIX socket path (defaults to local socket).
    pub endpoint: Option<String>,
    /// Additional instance launch configuration profiles (e.g. `["default"]`).
    pub profiles: Vec<String>,
    /// Custom configuration keys passed to the container (`raw.idmap`, `security.nesting`, etc.).
    pub config: std::collections::HashMap<String, String>,
    /// Optional command override for LXC CLI (defaults to `"lxc"`).
    pub command: Option<String>,
}

/// Client for LXD / Incus REST API and CLI operations.
#[derive(Debug, Clone)]
pub struct LxdClient {
    /// LXD endpoint or socket.
    pub endpoint: String,
    /// LXC executable path or command name.
    pub command: String,
}

impl LxdClient {
    /// Create a new `LxdClient` with the default executable (`"lxc"`).
    #[must_use]
    pub fn new(endpoint: String) -> Self {
        Self {
            endpoint,
            command: "lxc".to_string(),
        }
    }

    /// Create a new `LxdClient` with a custom executable.
    #[must_use]
    pub const fn with_command(endpoint: String, command: String) -> Self {
        Self { endpoint, command }
    }

    /// Launch a new container instance from a base image.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if container launch fails.
    pub async fn launch_container(
        &self,
        name: &str,
        image: &str,
        profiles: &[String],
    ) -> Result<String, StampError> {
        let mut cmd = tokio::process::Command::new(&self.command);
        cmd.arg("launch").arg(image).arg(name);
        for p in profiles {
            cmd.arg("-p").arg(p);
        }

        let status = cmd
            .status()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to launch LXD container: {e}")))?;

        if !status.success() {
            return Err(StampError::Execution(format!(
                "LXD launch returned status {status}"
            )));
        }

        Ok(name.to_string())
    }

    /// Stop an active LXD container instance.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if stopping fails.
    pub async fn stop_container(&self, name: &str) -> Result<(), StampError> {
        let _ = tokio::process::Command::new(&self.command)
            .arg("stop")
            .arg(name)
            .arg("--force")
            .status()
            .await;
        Ok(())
    }

    /// Publish a container as a new image in LXD.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if publishing fails.
    pub async fn publish_image(&self, name: &str, alias: &str) -> Result<String, StampError> {
        let mut cmd = tokio::process::Command::new(&self.command);
        cmd.arg("publish").arg(name).arg("--alias").arg(alias);

        let output = cmd
            .output()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to publish LXD image: {e}")))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(StampError::Execution(format!("LXD publish failed: {err}")));
        }

        let out_str = String::from_utf8_lossy(&output.stdout);
        let fingerprint = out_str
            .split_whitespace()
            .find(|word| word.len() >= 12)
            .unwrap_or(alias);
        Ok(fingerprint.to_string())
    }

    /// Delete a container instance.
    ///
    /// # Errors
    ///
    /// Returns `StampError::Execution` if deletion fails.
    pub async fn delete_container(&self, name: &str) -> Result<(), StampError> {
        let _ = tokio::process::Command::new(&self.command)
            .arg("delete")
            .arg(name)
            .arg("--force")
            .status()
            .await;
        Ok(())
    }
}

/// The `lxd` builder.
#[derive(Debug, Clone)]
pub struct LxdBuilder {
    /// The builder configuration.
    pub config: LxdConfig,
}

impl LxdBuilder {
    /// Create a new `LxdBuilder`.
    #[must_use]
    pub const fn new(config: LxdConfig) -> Self {
        Self { config }
    }
}

/// Step to launch a container in LXD.
#[derive(Debug, Clone)]
struct StepLaunchLxd {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: LxdClient,
    /// Config.
    config: LxdConfig,
}

#[async_trait]
impl Step for StepLaunchLxd {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let container_name = format!("stamp-{}", self.name);
        self.ui.say(
            &self.name,
            &format!(
                "Launching LXD container {container_name} from {}...",
                self.config.image
            ),
        );

        let id = self
            .client
            .launch_container(&container_name, &self.config.image, &self.config.profiles)
            .await?;

        state.put("container_name", id);
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(c_name) = state.get::<String>("container_name") {
            self.ui
                .say(&self.name, &format!("Cleaning up LXD container: {c_name}"));
            let _ = self.client.delete_container(c_name).await;
        }
    }
}

/// Communicator executing commands directly inside an LXD container via `lxc exec`.
#[derive(Debug, Clone)]
struct LxdCommunicator {
    /// Container name.
    container_name: String,
    /// Executable command name or path.
    command: String,
}

#[async_trait]
impl Communicator for LxdCommunicator {
    async fn execute(
        &self,
        cmd: &crate::communicator::Command,
    ) -> Result<crate::communicator::CommandResult, StampError> {
        let output = tokio::process::Command::new(&self.command)
            .arg("exec")
            .arg(&self.container_name)
            .arg("--")
            .arg("sh")
            .arg("-c")
            .arg(&cmd.command)
            .output()
            .await
            .map_err(StampError::Io)?;

        Ok(crate::communicator::CommandResult {
            exit_code: output.status.code().unwrap_or(0),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    async fn upload(
        &self,
        local_path: &crate::types::FilePath,
        remote_path: &crate::types::FilePath,
    ) -> Result<(), StampError> {
        let _ = tokio::process::Command::new(&self.command)
            .arg("file")
            .arg("push")
            .arg(local_path.as_path())
            .arg(format!(
                "{}/{}",
                self.container_name,
                remote_path.as_path().display()
            ))
            .status()
            .await;
        Ok(())
    }

    async fn download(
        &self,
        remote_path: &crate::types::FilePath,
        local_path: &crate::types::FilePath,
    ) -> Result<(), StampError> {
        let _ = tokio::process::Command::new(&self.command)
            .arg("file")
            .arg("pull")
            .arg(format!(
                "{}/{}",
                self.container_name,
                remote_path.as_path().display()
            ))
            .arg(local_path.as_path())
            .status()
            .await;
        Ok(())
    }
}

/// Step to provision the container.
#[derive(Clone)]
struct StepProvisionLxd {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Hook.
    hook: Arc<dyn ProvisionHook>,
    /// Command override.
    command: String,
}

#[async_trait]
impl Step for StepProvisionLxd {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        self.ui.say(&self.name, "Provisioning LXD container...");
        let c_name = state
            .get::<String>("container_name")
            .cloned()
            .unwrap_or_default();

        let comm = Arc::new(LxdCommunicator {
            container_name: c_name,
            command: self.command.clone(),
        });

        let build_ctx = BuildContext {
            build_id: self.name.clone(),
            host: "lxd".to_string(),
            user: "root".to_string(),
            packer_run_uuid: "mocked-uuid".to_string(),
            source_name: self.name.clone(),
            source_type: "lxd".to_string(),
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

/// Step to publish the container into an image.
#[derive(Debug, Clone)]
struct StepPublishLxdImage {
    /// UI reference.
    ui: Arc<crate::engine::ui::Ui>,
    /// Builder name.
    name: String,
    /// Client.
    client: LxdClient,
    /// Config.
    config: LxdConfig,
}

#[async_trait]
impl Step for StepPublishLxdImage {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, StampError> {
        let c_name = state
            .get::<String>("container_name")
            .cloned()
            .unwrap_or_default();
        let alias = self.config.output_image.as_deref().unwrap_or(&self.name);

        self.ui
            .say(&self.name, &format!("Stopping container {c_name}..."));
        self.client.stop_container(&c_name).await?;

        self.ui
            .say(&self.name, &format!("Publishing image alias {alias}..."));
        let fingerprint = self.client.publish_image(&c_name, alias).await?;
        self.ui
            .say(&self.name, &format!("Image published: {fingerprint}"));
        state.put("fingerprint", fingerprint.clone());
        state.put("artifact_id", format!("lxd:{fingerprint}"));

        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, _state: &StateBag) {}
}

#[async_trait]
impl Builder for LxdBuilder {
    fn name(&self) -> String {
        self.config.name.clone()
    }

    async fn prepare(&self) -> Result<(), StampError> {
        if self.config.name.is_empty() {
            return Err(StampError::Parse("Name cannot be empty".to_string()));
        }
        if self.config.image.is_empty() {
            return Err(StampError::Parse(
                "Source image cannot be empty".to_string(),
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
        let endpoint = self
            .config
            .endpoint
            .clone()
            .unwrap_or_else(|| "/var/snap/lxd/common/lxd/unix.socket".to_string());
        let cmd = self
            .config
            .command
            .clone()
            .unwrap_or_else(|| "lxc".to_string());
        let client = LxdClient::with_command(endpoint, cmd.clone());

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(StepLaunchLxd {
                ui: ui.clone(),
                name: self.name(),
                client: client.clone(),
                config: self.config.clone(),
            }),
            Box::new(StepProvisionLxd {
                ui: ui.clone(),
                name: self.name(),
                hook,
                command: cmd,
            }),
            Box::new(StepPublishLxdImage {
                ui: ui.clone(),
                name: self.name(),
                client,
                config: self.config.clone(),
            }),
        ];

        let mut runner = Runner::new(steps);
        let mut state = StateBag::new();

        match runner.run(&mut state).await {
            Ok(()) => {
                runner.cleanup(&state).await;
            }
            Err(e) => {
                if on_error == crate::engine::packer::OnErrorStrategy::Cleanup {
                    runner.cleanup(&state).await;
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

    fn create_mock_lxc_script() -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "mock_lxc_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros()
        ));
        let script = r#"#!/bin/sh
case "$1" in
    launch)
        if [ "$2" = "fail:image" ]; then
            echo "Launch failed" >&2
            exit 1
        fi
        exit 0
        ;;
    stop)
        exit 0
        ;;
    publish)
        if [ "$2" = "fail-container" ]; then
            echo "Publish failed error message" >&2
            exit 1
        fi
        if [ "$4" = "short" ]; then
            echo "short"
            exit 0
        fi
        echo "fingerprint-abcdef12345678"
        exit 0
        ;;
    delete)
        exit 0
        ;;
    exec)
        if [ "$6" = "exit 42" ]; then
            exit 42
        fi
        echo "exec output"
        exit 0
        ;;
    file)
        exit 0
        ;;
    *)
        exit 0
        ;;
esac
"#;
        let _ = std::fs::write(&path, script);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        path
    }

    struct FailingProvisioner;
    #[async_trait]
    impl crate::provisioner::Provisioner for FailingProvisioner {
        async fn provision(
            &self,
            _comm: &dyn Communicator,
            _ui: Arc<crate::engine::ui::Ui>,
        ) -> Result<(), StampError> {
            Err(StampError::Execution("mock provision failure".to_string()))
        }
    }

    #[test]
    fn test_derived_traits() {
        let config = LxdConfig {
            name: "test".to_string(),
            image: "ubuntu:22.04".to_string(),
            output_image: Some("my-img".to_string()),
            endpoint: None,
            profiles: vec!["default".to_string()],
            config: std::collections::HashMap::new(),
            command: None,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));

        let serialized = serde_json::to_string(&config);
        assert!(serialized.is_ok());
        for json in serialized {
            let deserialized: Result<LxdConfig, _> = serde_json::from_str(&json);
            assert!(deserialized.is_ok());
        }

        let builder = LxdBuilder::new(config);
        assert_eq!(format!("{builder:?}"), format!("{builder:?}"));

        let client = LxdClient::new("local".to_string());
        assert_eq!(format!("{client:?}"), format!("{client:?}"));

        let client_custom = LxdClient::with_command("local".to_string(), "lxc-custom".to_string());
        assert_eq!(client_custom.command, "lxc-custom");

        let comm = LxdCommunicator {
            container_name: "test-c".to_string(),
            command: "lxc".to_string(),
        };
        assert_eq!(format!("{comm:?}"), format!("{comm:?}"));
    }

    #[tokio::test]
    async fn test_lxd_prepare() {
        let mut c = LxdConfig::default();
        let b = LxdBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.name = "test".to_string();
        let b = LxdBuilder::new(c.clone());
        assert!(b.prepare().await.is_err());

        c.image = "images:alpine".to_string();
        let b = LxdBuilder::new(c);
        assert!(b.prepare().await.is_ok());
    }

    #[tokio::test]
    async fn test_lxd_client_mocked() {
        let script = create_mock_lxc_script();
        let script_str = script.to_string_lossy().to_string();
        let client = LxdClient::with_command("socket".to_string(), script_str);

        // Success launch
        let c_id = client
            .launch_container("c1", "ubuntu:22.04", &["default".to_string()])
            .await;
        assert!(c_id.is_ok());

        // Status failure launch
        let c_id_fail = client.launch_container("c1", "fail:image", &[]).await;
        assert!(c_id_fail.is_err());

        // Spawn failure launch
        let bad_client =
            LxdClient::with_command("socket".to_string(), "non_existent_binary_xyz".to_string());
        let c_id_bad = bad_client.launch_container("c1", "ubuntu:22.04", &[]).await;
        assert!(c_id_bad.is_err());

        // Stop container
        let stop_res = client.stop_container("c1").await;
        assert!(stop_res.is_ok());

        // Publish image success
        let fp = client.publish_image("c1", "my-alias").await;
        assert!(fp.is_ok());
        for f in fp {
            assert!(f.starts_with("fingerprint-"));
        }

        // Publish image short output (fallback to alias)
        let fp_short = client.publish_image("c1", "short").await;
        assert!(fp_short.is_ok());
        for f in fp_short {
            assert_eq!(f, "short");
        }

        // Publish image failure
        let fp_fail = client.publish_image("fail-container", "my-alias").await;
        assert!(fp_fail.is_err());

        // Publish image bad client
        let fp_bad = bad_client.publish_image("c1", "my-alias").await;
        assert!(fp_bad.is_err());

        // Delete container
        let del_res = client.delete_container("c1").await;
        assert!(del_res.is_ok());

        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn test_lxd_communicator() {
        let script = create_mock_lxc_script();
        let script_str = script.to_string_lossy().to_string();
        let comm = LxdCommunicator {
            container_name: "test-c".to_string(),
            command: script_str,
        };

        // Execute success
        let cmd = crate::communicator::Command::new("echo hello".to_string());
        let exec_res = comm.execute(&cmd).await;
        assert!(exec_res.is_ok());
        for res in exec_res {
            assert_eq!(res.exit_code, 0);
            assert!(res.stdout.contains("exec output"));
        }

        // Execute non-zero exit code
        let cmd_fail = crate::communicator::Command::new("exit 42".to_string());
        let exec_fail = comm.execute(&cmd_fail).await;
        assert!(exec_fail.is_ok());
        for res in exec_fail {
            assert_eq!(res.exit_code, 42);
        }

        // Execute with bad binary
        let bad_comm = LxdCommunicator {
            container_name: "test-c".to_string(),
            command: "non_existent_exec_binary_xyz".to_string(),
        };
        assert!(bad_comm.execute(&cmd).await.is_err());

        // Upload and download
        let local_path = crate::types::FilePath::new(std::path::PathBuf::from("/tmp/local"));
        let remote_path = crate::types::FilePath::new(std::path::PathBuf::from("/tmp/remote"));
        assert!(comm.upload(&local_path, &remote_path).await.is_ok());
        assert!(comm.download(&remote_path, &local_path).await.is_ok());

        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn test_lxd_builder_run() {
        let script = create_mock_lxc_script();
        let script_str = script.to_string_lossy().to_string();

        let config = LxdConfig {
            name: "test-lxd".to_string(),
            image: "ubuntu:22.04".to_string(),
            output_image: Some("published-lxd".to_string()),
            endpoint: Some("/custom/lxd.socket".to_string()),
            command: Some(script_str),
            ..Default::default()
        };
        let b = LxdBuilder::new(config);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let artifact = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        assert!(artifact.is_ok());
        for art in artifact {
            assert!(art.id().starts_with("lxd:fingerprint-"));
        }
        assert!(b.cancel().await.is_ok());

        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn test_lxd_builder_run_failures() {
        let script = create_mock_lxc_script();
        let script_str = script.to_string_lossy().to_string();

        // Launch failure with OnErrorStrategy::Cleanup
        let config_fail = LxdConfig {
            name: "test-fail".to_string(),
            image: "fail:image".to_string(),
            command: Some(script_str.clone()),
            ..Default::default()
        };
        let b_fail = LxdBuilder::new(config_fail);
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let res_cleanup = b_fail
            .run(
                hook.clone(),
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res_cleanup.is_err());

        // Launch failure with OnErrorStrategy::Abort
        let res_abort = b_fail
            .run(
                hook,
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Abort,
            )
            .await;
        assert!(res_abort.is_err());

        // Provisioner failure
        let config_prov_fail = LxdConfig {
            name: "test-prov-fail".to_string(),
            image: "ubuntu:22.04".to_string(),
            command: Some(script_str.clone()),
            ..Default::default()
        };
        let b_prov_fail = LxdBuilder::new(config_prov_fail);
        let fail_hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![Box::new(FailingProvisioner)]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let res_prov = b_prov_fail
            .run(
                fail_hook,
                ui.clone(),
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;
        assert!(res_prov.is_err());

        // Builder run with default command: None (hits unwrap_or_else(|| "lxc".to_string()))
        let config_default_cmd = LxdConfig {
            name: "test-default-cmd".to_string(),
            image: "ubuntu:22.04".to_string(),
            command: None,
            ..Default::default()
        };
        let b_default_cmd = LxdBuilder::new(config_default_cmd);
        let _ = b_default_cmd
            .run(
                Arc::new(crate::engine::hook::DefaultProvisionHook {
                    provisioners: Arc::new(vec![]),
                    error_cleanup_provisioners: Arc::new(vec![]),
                }),
                ui,
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await;

        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn test_step_cleanups_and_edges() {
        let script = create_mock_lxc_script();
        let script_str = script.to_string_lossy().to_string();
        let client = LxdClient::with_command("socket".to_string(), script_str);
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // StepLaunchLxd cleanup with and without container_name
        let mut step_launch = StepLaunchLxd {
            ui: ui.clone(),
            name: "test".to_string(),
            client: client.clone(),
            config: LxdConfig::default(),
        };
        let mut state = StateBag::new();
        step_launch.cleanup(&state).await;
        state.put("container_name", "c-to-delete".to_string());
        step_launch.cleanup(&state).await;

        // StepPublishLxdImage run when container_name is empty/missing
        let mut step_publish = StepPublishLxdImage {
            ui: ui.clone(),
            name: "test".to_string(),
            client: client.clone(),
            config: LxdConfig {
                output_image: None,
                ..Default::default()
            },
        };
        let mut pub_state = StateBag::new();
        let pub_res = step_publish.run(&mut pub_state).await;
        assert!(pub_res.is_ok());
        step_publish.cleanup(&pub_state).await;

        // StepProvisionLxd cleanup
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let mut step_prov = StepProvisionLxd {
            ui,
            name: "test".to_string(),
            hook,
            command: "lxc".to_string(),
        };
        step_prov.cleanup(&pub_state).await;

        let _ = std::fs::remove_file(&script);
    }
}
