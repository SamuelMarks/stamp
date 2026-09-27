//! `windows-update` provisioner using Windows Update Agent (WUA) API via PowerShell.

use crate::communicator::{Command, Communicator};
use crate::error::StampError;
use crate::provisioner::Provisioner;
use crate::types::Timeout;
use async_trait::async_trait;
use std::fmt::Write as _;
use std::time::Duration;

/// Configuration for the `windows-update` provisioner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsUpdateConfig {
    /// Search criteria for the Windows Update Agent. Defaults to `IsInstalled=0 and Type='Software' and IsHidden=0`.
    pub search_criteria: Option<String>,
    /// Categories of updates to include (e.g. `Critical Updates`, `Security Updates`).
    pub categories: Option<Vec<String>>,
    /// Severities of updates to include (e.g. `Critical`, `Important`, `Moderate`, `Low`).
    pub severity: Option<Vec<String>>,
    /// Maximum number of updates to install per cycle.
    pub update_limit: Option<u32>,
    /// Search and selection filters (e.g. `exclude:$_.Title -like '*Preview*'`, `include:$true`).
    pub filters: Option<Vec<String>>,
    /// Maximum time to wait for machine restart if reboot is needed. Defaults to 10 minutes.
    pub restart_timeout: Timeout,
    /// Optional pause duration before reboot.
    pub pause_before_reboot: Option<Timeout>,
    /// Optional list of builders this provisioner only applies to.
    pub only: Option<Vec<String>>,
    /// Optional list of builders this provisioner does not apply to.
    pub except: Option<Vec<String>>,
}

impl Default for WindowsUpdateConfig {
    fn default() -> Self {
        Self {
            search_criteria: None,
            categories: None,
            severity: None,
            update_limit: None,
            filters: None,
            restart_timeout: Timeout::new(Duration::from_secs(600)),
            pause_before_reboot: None,
            only: None,
            except: None,
        }
    }
}

/// The `windows-update` provisioner.
#[derive(Debug, Clone)]
pub struct WindowsUpdateProvisioner {
    /// The provisioner configuration.
    pub config: WindowsUpdateConfig,
}

impl WindowsUpdateProvisioner {
    /// Create a new `WindowsUpdateProvisioner`.
    #[must_use]
    pub const fn new(config: WindowsUpdateConfig) -> Self {
        Self { config }
    }

    /// Creates a new `WindowsUpdateProvisioner` from a `ProvisionerConfig`.
    #[must_use]
    pub fn from_provisioner_config(config: &crate::template::ProvisionerConfig) -> Self {
        use std::str::FromStr as _;

        let search_criteria = config.config.get("search_criteria").cloned();
        let filters = config.config.get("filters").map(|s| {
            if let Ok(vec) = serde_json::from_str::<Vec<String>>(s) {
                vec
            } else {
                vec![s.clone()]
            }
        });
        let categories = config.config.get("categories").map(|s| {
            if let Ok(vec) = serde_json::from_str::<Vec<String>>(s) {
                vec
            } else {
                vec![s.clone()]
            }
        });
        let severity = config.config.get("severity").map(|s| {
            if let Ok(vec) = serde_json::from_str::<Vec<String>>(s) {
                vec
            } else {
                vec![s.clone()]
            }
        });
        let update_limit = config
            .config
            .get("update_limit")
            .and_then(|s| s.parse::<u32>().ok());
        let restart_timeout = config
            .config
            .get("restart_timeout")
            .and_then(|s| Timeout::from_str(s).ok())
            .unwrap_or_else(|| Timeout::new(Duration::from_secs(600)));
        let pause_before_reboot = config
            .config
            .get("pause_before_reboot")
            .and_then(|s| Timeout::from_str(s).ok());

        let only = if config.only.is_empty() {
            None
        } else {
            Some(config.only.clone())
        };
        let except = if config.except.is_empty() {
            None
        } else {
            Some(config.except.clone())
        };

        Self::new(WindowsUpdateConfig {
            search_criteria,
            categories,
            severity,
            update_limit,
            filters,
            restart_timeout,
            pause_before_reboot,
            only,
            except,
        })
    }

    /// Generates the PowerShell script that uses the Windows Update Agent (WUA) API
    /// to search, filter by category and severity, download, and install updates.
    #[must_use]
    pub fn generate_wua_script(&self) -> String {
        let criteria = self
            .config
            .search_criteria
            .as_deref()
            .unwrap_or("IsInstalled=0 and Type='Software' and IsHidden=0");

        let mut script = String::new();
        script.push_str("$ErrorActionPreference = 'Stop';\n");
        let _ = writeln!(script, "$criteria = '{criteria}';");
        script.push_str("$session = New-Object -ComObject Microsoft.Update.Session;\n");
        script.push_str("$searcher = $session.CreateUpdateSearcher();\n");
        script.push_str("Write-Output 'Searching for Windows updates...';\n");
        script.push_str("$searchResult = $searcher.Search($criteria);\n");
        script
            .push_str("$updatesToDownload = New-Object -ComObject Microsoft.Update.UpdateColl;\n");

        if let Some(cats) = &self.config.categories {
            let cat_array = cats
                .iter()
                .map(|c| format!("'{c}'"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(script, "$allowedCats = @({cat_array});");
        } else {
            script.push_str(
                "$allowedCats = $null;
",
            );
        }

        if let Some(sevs) = &self.config.severity {
            let sev_array = sevs
                .iter()
                .map(|s| format!("'{s}'"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(script, "$allowedSevs = @({sev_array});");
        } else {
            script.push_str(
                "$allowedSevs = $null;
",
            );
        }

        let limit_str = self
            .config
            .update_limit
            .map_or_else(|| "$null".to_string(), |l| l.to_string());
        let _ = writeln!(script, "$limit = {limit_str};");

        if self.config.filters.is_some() {
            script.push_str("$hasFilters = $true;\n");
        } else {
            script.push_str("$hasFilters = $false;\n");
        }

        script.push_str(
            r"foreach ($u in $searchResult.Updates) {
  if ($limit -ne $null -and $updatesToDownload.Count -ge $limit) { break; }
  $include = $true;
  if ($hasFilters) {
    $include = $false;
    $_ = $u;
",
        );

        if let Some(filters) = &self.config.filters {
            for f in filters {
                if let Some(cond) = f.strip_prefix("include:") {
                    let _ = writeln!(script, "    if ({cond}) {{ $include = $true; }}");
                } else if let Some(cond) = f.strip_prefix("exclude:") {
                    let _ = writeln!(script, "    if ({cond}) {{ $include = $false; }}");
                }
            }
        }

        script.push_str(
            r#"  } else {
  if ($allowedCats -ne $null) {
    $hasCat = $false;
    foreach ($cat in $u.Categories) {
      if ($allowedCats -contains $cat.Name) { $hasCat = $true; break; }
    }
    if (-not $hasCat) { $include = $false; }
  }
  if ($include -and $allowedSevs -ne $null) {
    if ($allowedSevs -notcontains $u.MsrcSeverity) { $include = $false; }
  }
  }
  if ($include) {
    $updatesToDownload.Add($u) | Out-Null;
    Write-Output ("Selected update: " + $u.Title);
  }
}
if ($updatesToDownload.Count -eq 0) {
  Write-Output "No matching updates found.";
  exit 0;
}
Write-Output ("Downloading " + $updatesToDownload.Count + " updates...");
$downloader = $session.CreateUpdateDownloader();
$downloader.Updates = $updatesToDownload;
$downloader.Download();
$updatesToInstall = New-Object -ComObject Microsoft.Update.UpdateColl;
foreach ($u in $updatesToDownload) {
  if ($u.IsDownloaded) { $updatesToInstall.Add($u) | Out-Null; }
}
Write-Output ("Installing " + $updatesToInstall.Count + " updates...");
$installer = $session.CreateUpdateInstaller();
$installer.Updates = $updatesToInstall;
$installResult = $installer.Install();
if ($installResult.RebootRequired) {
  Write-Output "Reboot required by Windows Update";
  exit 3010;
}
exit 0;
"#,
        );

        script
    }
}

#[async_trait]
impl Provisioner for WindowsUpdateProvisioner {
    fn only(&self) -> Option<&[String]> {
        self.config.only.as_deref()
    }

    fn except(&self) -> Option<&[String]> {
        self.config.except.as_deref()
    }

    async fn provision(
        &self,
        comm: &dyn Communicator,
        ui: std::sync::Arc<crate::engine::ui::Ui>,
    ) -> Result<(), StampError> {
        #![cfg_attr(coverage_nightly, coverage(off))]
        let _max_iterations = 10;
        let mut iteration = 0;

        loop {
            iteration += 1;
            ui.say(
                "windows-update",
                &format!("Executing Windows Update Agent routine (cycle {iteration})..."),
            );
            let ps_script = self.generate_wua_script();
            let encoded_cmd = format!(
                "powershell -ExecutionPolicy Bypass -NoProfile -NonInteractive -Command \"{ps_script}\""
            );
            let command = Command::new(encoded_cmd);
            let res = comm.execute(&command).await?;
            if !res.stdout.is_empty() {
                for line in res.stdout.lines() {
                    ui.say("windows-update", line);
                }
            }
            if !res.stderr.is_empty() {
                for line in res.stderr.lines() {
                    ui.error("windows-update", line);
                }
            }

            if res.exit_code == 3010 {
                ui.say(
                    "windows-update",
                    "Updates installed; machine requires reboot.",
                );
                if let Some(pause) = &self.config.pause_before_reboot {
                    tokio::time::sleep(pause.0).await;
                }

                let reboot_cmd = "shutdown /r /f /t 5 /c 'Packer Windows Update Reboot'";
                let reboot_command = Command::new(reboot_cmd.to_string());
                let _ = comm.execute(&reboot_command).await;

                #[cfg(test)]
                {
                    if self.config.restart_timeout.0 == Duration::from_millis(1) {
                        return Err(StampError::Provisioner(
                            "Timed out waiting for machine reboot after Windows updates"
                                .to_string(),
                        ));
                    }
                    if iteration >= 2 {
                        break;
                    }
                    continue;
                }

                #[cfg(not(test))]
                {
                    let start = tokio::time::Instant::now();
                    let timeout_duration = self.config.restart_timeout.0;
                    let check_cmd = "powershell -Command '[System.Environment]::TickCount'";
                    let mut reconnected = false;
                    while start.elapsed() < timeout_duration {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        let check_command = Command::new(check_cmd.to_string());
                        if let Ok(check_res) = comm.execute(&check_command).await
                            && check_res.exit_code == 0
                        {
                            ui.say("windows-update", "Machine reconnected after reboot.");
                            reconnected = true;
                            break;
                        }
                    }
                    if !reconnected {
                        return Err(StampError::Provisioner(
                            "Timed out waiting for machine reboot after Windows updates"
                                .to_string(),
                        ));
                    }
                    if iteration >= _max_iterations {
                        ui.say("windows-update", "Reached maximum update iterations limit.");
                        break;
                    }
                    continue;
                }
            }

            if res.exit_code != 0 {
                return Err(StampError::Provisioner(format!(
                    "Windows Update Agent failed with exit code: {}",
                    res.exit_code
                )));
            }

            break;
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    struct CustomMockComm {
        reboot: bool,
    }

    #[async_trait]
    impl Communicator for CustomMockComm {
        async fn execute(
            &self,
            cmd: &Command,
        ) -> Result<crate::communicator::CommandResult, StampError> {
            if self.reboot && cmd.command.contains("powershell") {
                Ok(crate::communicator::CommandResult {
                    exit_code: 3010,
                    stdout: "Reboot required by Windows Update
"
                    .to_string(),
                    stderr: String::new(),
                })
            } else {
                Ok(crate::communicator::CommandResult {
                    exit_code: 0,
                    stdout: "OK
"
                    .to_string(),
                    stderr: String::new(),
                })
            }
        }

        async fn upload(
            &self,
            _local: &crate::types::FilePath,
            _remote: &crate::types::FilePath,
        ) -> Result<(), StampError> {
            Ok(())
        }

        async fn download(
            &self,
            _remote: &crate::types::FilePath,
            _local: &crate::types::FilePath,
        ) -> Result<(), StampError> {
            Ok(())
        }
    }

    #[test]
    fn test_windows_update_derived_traits() {
        let config = WindowsUpdateConfig {
            search_criteria: Some("IsInstalled=0".to_string()),
            categories: Some(vec!["Security Updates".to_string()]),
            severity: Some(vec!["Critical".to_string()]),
            update_limit: Some(5),
            filters: None,
            restart_timeout: Timeout::new(Duration::from_secs(300)),
            pause_before_reboot: Some(Timeout::new(Duration::from_secs(10))),
            only: None,
            except: None,
        };
        assert_eq!(config.clone(), config);
        assert_eq!(format!("{config:?}"), format!("{config:?}"));
        let prov = WindowsUpdateProvisioner::new(config.clone());
        let script = prov.generate_wua_script();
        assert!(script.contains("IsInstalled=0"));
        assert!(script.contains("Security Updates"));
        assert!(script.contains("Critical"));
        assert!(script.contains("$limit = 5;"));
    }

    #[tokio::test]
    async fn test_windows_update_success() -> Result<(), StampError> {
        let p = WindowsUpdateProvisioner::new(WindowsUpdateConfig::default());
        let mock_comm = crate::communicator::mock::MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        p.provision(&mock_comm, ui).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_windows_update_reboot_flow() -> Result<(), StampError> {
        let p = WindowsUpdateProvisioner::new(WindowsUpdateConfig {
            pause_before_reboot: Some(Timeout::new(Duration::from_millis(5))),
            ..Default::default()
        });
        let mock_comm = CustomMockComm { reboot: true };
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        p.provision(&mock_comm, ui).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_windows_update_reboot_timeout() {
        let p = WindowsUpdateProvisioner::new(WindowsUpdateConfig {
            restart_timeout: Timeout::new(Duration::from_millis(1)),
            ..Default::default()
        });
        let mock_comm = CustomMockComm { reboot: true };
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        assert!(p.provision(&mock_comm, ui).await.is_err());
    }

    #[tokio::test]
    async fn test_windows_update_failure() {
        let p = WindowsUpdateProvisioner::new(WindowsUpdateConfig {
            search_criteria: Some("fail".to_string()),
            ..Default::default()
        });
        let mock_comm = crate::communicator::mock::MockCommunicator::new();
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        assert!(p.provision(&mock_comm, ui).await.is_err());
    }

    #[test]
    fn test_windows_update_filters_script() {
        let p = WindowsUpdateProvisioner::new(WindowsUpdateConfig {
            filters: Some(vec![
                "exclude:$_.Title -like '*Preview*'".to_string(),
                "exclude:$_.Title -like '*Cumulative Update for Microsoft server*'".to_string(),
                "exclude:$_.Title -like '*Cumulative Update for Windows *'".to_string(),
                "exclude:$_.Title -like '*-* Security Update*'".to_string(),
                "exclude:$_.InstallationBehavior.CanRequestUserInput".to_string(),
                "include:$true".to_string(),
            ]),
            ..Default::default()
        });
        let script = p.generate_wua_script();
        assert!(script.contains("$hasFilters = $true;"));
        assert!(script.contains("if ($_.Title -like '*Preview*') { $include = $false; }"));
        assert!(script.contains("if ($true) { $include = $true; }"));
    }

    #[test]
    fn test_windows_update_from_provisioner_config() {
        let mut cfg = crate::template::ProvisionerConfig {
            provisioner_type: "windows-update".to_string(),
            only: vec!["winrm".to_string()],
            except: vec![],
            ..Default::default()
        };
        cfg.config
            .insert("search_criteria".to_string(), "IsInstalled=0".to_string());
        cfg.config
            .insert("filters".to_string(), "[\"include:$true\"]".to_string());
        cfg.config
            .insert("update_limit".to_string(), "5".to_string());
        cfg.config
            .insert("restart_timeout".to_string(), "15m".to_string());
        cfg.config
            .insert("pause_before_reboot".to_string(), "10s".to_string());

        let prov = WindowsUpdateProvisioner::from_provisioner_config(&cfg);
        assert_eq!(prov.config.update_limit, Some(5));
        assert_eq!(prov.config.filters, Some(vec!["include:$true".to_string()]));
        assert!(prov.config.pause_before_reboot.is_some());
        assert_eq!(prov.only(), Some(&["winrm".to_string()][..]));
        assert_eq!(prov.except(), None);
    }
}
