//! `custom-hook` provisioner.

use crate::communicator::Communicator;
use crate::engine::ui::Ui;
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::Timeout;
use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

/// Boxed future returned by a [`CustomHookCallback`].
pub type HookFuture<'a> = Pin<Box<dyn Future<Output = Result<(), StampError>> + Send + 'a>>;

/// Callback function type invoked by [`CustomHookProvisioner`].
pub type CustomHookCallback =
    Arc<dyn for<'a> Fn(&'a dyn Communicator, &'a Ui) -> HookFuture<'a> + Send + Sync>;

/// Configuration for `custom-hook`.
#[derive(Clone, Default)]
pub struct CustomHookConfig {
    /// Optional restart timeout duration string (e.g. `"5m"`).
    pub restart_timeout: String,
    /// Optional command to execute over the communicator during the custom hook.
    pub command: Option<String>,
    /// Optional in-process async callback invoked during provisioning.
    pub callback: Option<CustomHookCallback>,
}

impl std::fmt::Debug for CustomHookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomHookConfig")
            .field("restart_timeout", &self.restart_timeout)
            .field("command", &self.command)
            .field("has_callback", &self.callback.is_some())
            .finish()
    }
}

impl PartialEq for CustomHookConfig {
    fn eq(&self, other: &Self) -> bool {
        self.restart_timeout == other.restart_timeout
            && self.command == other.command
            && self.callback.is_some() == other.callback.is_some()
    }
}

impl Eq for CustomHookConfig {}

/// The `custom-hook` provisioner.
#[derive(Debug, Clone)]
pub struct CustomHookProvisioner {
    /// The config.
    pub config: CustomHookConfig,
}

impl CustomHookProvisioner {
    /// Create a new `CustomHookProvisioner`.
    #[must_use]
    pub const fn new(config: CustomHookConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provisioner for CustomHookProvisioner {
    async fn provision(&self, comm: &dyn Communicator, ui: Arc<Ui>) -> Result<(), StampError> {
        let timeout_opt = if self.config.restart_timeout.is_empty() {
            None
        } else {
            Some(Timeout::from_str(&self.config.restart_timeout)?)
        };

        let run_fut = async {
            if let Some(ref cb) = self.config.callback {
                cb(comm, &ui).await?;
            }

            if let Some(ref cmd_str) = self.config.command {
                ui.say("custom-hook", &format!("Executing hook command: {cmd_str}"));
                let res = comm
                    .execute(&crate::communicator::Command::new(cmd_str.clone()))
                    .await?;
                if res.exit_code != 0 {
                    return Err(StampError::Provisioner(format!(
                        "custom-hook command failed with exit code {}: {}",
                        res.exit_code, res.stderr
                    )));
                }
            }
            Ok(())
        };

        if let Some(to) = timeout_opt {
            tokio::time::timeout(to.0, run_fut)
                .await
                .map_err(|_| StampError::Timeout {
                    operation: "custom-hook".to_string(),
                    duration: to.0,
                })?
        } else {
            run_fut.await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_hook_derived_traits() {
        let config = CustomHookConfig {
            restart_timeout: "5m".to_string(),
            command: None,
            callback: None,
        };
        let cloned = config.clone();
        assert_eq!(cloned, config);
        assert!(format!("{config:?}").contains("CustomHookConfig"));
        assert_eq!(CustomHookConfig::default().restart_timeout, "");

        let p = CustomHookProvisioner::new(config);
        assert!(format!("{p:?}").contains("CustomHookProvisioner"));
        assert_eq!(p.clone().config, p.config);
    }

    #[tokio::test]
    async fn test_custom_hook_success_and_callback() -> Result<(), StampError> {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = called.clone();
        let cb: CustomHookCallback = Arc::new(move |_comm, ui| {
            let flag = called_clone.clone();
            Box::pin(async move {
                ui.say("custom-hook", "callback invoked");
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
        });

        let p = CustomHookProvisioner::new(CustomHookConfig {
            restart_timeout: "5m".to_string(),
            command: Some("echo hooked".to_string()),
            callback: Some(cb),
        });
        let mock_comm = crate::communicator::mock::MockCommunicator::new();
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        p.provision(&mock_comm, ui.clone()).await?;
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));

        let p_fail_cmd = CustomHookProvisioner::new(CustomHookConfig {
            restart_timeout: String::new(),
            command: Some("fail_cmd".to_string()),
            callback: None,
        });
        assert!(p_fail_cmd.provision(&mock_comm, ui).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_custom_hook_failure() {
        let p = CustomHookProvisioner::new(CustomHookConfig {
            restart_timeout: "fail".to_string(),
            command: None,
            callback: None,
        });
        let mock_comm = crate::communicator::mock::MockCommunicator::new();
        let ui = Arc::new(Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        assert!(p.provision(&mock_comm, ui).await.is_err());
    }
}
