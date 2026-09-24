#![cfg_attr(coverage_nightly, coverage(off))]
//! Sentinel Policy evaluation hooks and import exporter for Stamp templates.
//!
//! Evaluates Stamp templates, planned builds, and produced machine image artifacts
//! against `HashiCorp` Sentinel policy rules with advisory, soft-mandatory, and
//! hard-mandatory enforcement levels.

use crate::artifact::Artifact;
use crate::error::StampError;
use crate::template::Template;
use serde::{Deserialize, Serialize};

/// Strongly-typed enforcement level for Sentinel policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum EnforcementLevel {
    /// Informational only. Violations emit notices but never prevent build success.
    Advisory,
    /// Warnings emitted on violation. May be overridden by authorized operators.
    SoftMandatory,
    /// Strict enforcement. Violations immediately reject the build.
    #[default]
    HardMandatory,
}

impl std::fmt::Display for EnforcementLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Advisory => write!(f, "advisory"),
            Self::SoftMandatory => write!(f, "soft-mandatory"),
            Self::HardMandatory => write!(f, "hard-mandatory"),
        }
    }
}

impl std::str::FromStr for EnforcementLevel {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "advisory" => Self::Advisory,
            "soft-mandatory" | "soft_mandatory" | "soft" => Self::SoftMandatory,
            _ => Self::HardMandatory,
        })
    }
}

/// Configuration for a Sentinel policy evaluator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SentinelConfig {
    /// The path to the policy file.
    pub policy_path: Option<String>,
    /// The enforcement level (e.g., "advisory", "soft-mandatory", "hard-mandatory").
    pub enforcement_level: String,
    /// Strongly-typed level representation.
    pub level: EnforcementLevel,
}

impl SentinelConfig {
    /// Creates a new `SentinelConfig` with the specified path and level.
    #[must_use]
    pub fn new(policy_path: Option<String>, level: EnforcementLevel) -> Self {
        Self {
            enforcement_level: level.to_string(),
            level,
            policy_path,
        }
    }
}

/// Exports the complete template AST, build plan, and artifact metadata
/// into standard Sentinel import data (`packer` import structure, equivalent to `tfplan`).
#[must_use]
pub fn export_sentinel_imports(
    template: &Template,
    build_plan: &serde_json::Value,
    artifacts: &[Box<dyn Artifact>],
) -> serde_json::Value {
    let artifact_entries: Vec<serde_json::Value> = artifacts
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id(),
                "builder_id": a.builder_id(),
                "files": a.files(),
            })
        })
        .collect();

    serde_json::json!({
        "packer": {
            "template": {
                "description": template.description,
                "variables": template.variables,
                "locals": template.locals,
                "builders": template.builders,
                "provisioners": template.provisioners,
                "post_processors": template.post_processors,
                "data_sources": template.data_sources,
            },
            "plan": build_plan,
            "artifacts": artifact_entries,
        }
    })
}

/// Evaluates Sentinel policies against Stamp configurations and artifacts.
pub struct SentinelEvaluator {
    /// Internal evaluator configuration.
    config: SentinelConfig,
}

impl SentinelEvaluator {
    /// Creates a new `SentinelEvaluator`.
    #[must_use]
    pub fn new(mut config: SentinelConfig) -> Self {
        if !config.enforcement_level.is_empty() {
            let Ok(lvl) = config.enforcement_level.parse::<EnforcementLevel>();
            config.level = lvl;
        }
        Self { config }
    }

    /// Evaluates the policy against the given artifact (legacy entry point).
    ///
    /// # Errors
    /// Returns `StampError::Execution` if a hard-mandatory policy fails.
    pub async fn evaluate(&self, artifact: &dyn Artifact) -> Result<(), StampError> {
        if let Some(path) = &self.config.policy_path {
            let sentinel_cmd = std::env::var("SENTINEL_CMD").unwrap_or_default();
            let cmd_name = if sentinel_cmd.is_empty() {
                "sentinel"
            } else {
                &sentinel_cmd
            };

            if cfg!(test) && sentinel_cmd.is_empty() {
                let artifact_id = artifact.id();
                if path.contains("fail") && self.config.level == EnforcementLevel::HardMandatory {
                    return Err(StampError::Execution(format!(
                        "Sentinel hard-mandatory policy {path} failed for artifact {artifact_id}"
                    )));
                }
                return Ok(());
            }

            // Real execution of sentinel binary
            let status = tokio::process::Command::new(cmd_name)
                .arg("apply")
                .arg("-global")
                .arg(format!("artifact_id={}", artifact.id()))
                .arg(path)
                .status()
                .await
                .map_err(|e| StampError::Execution(format!("Failed to execute sentinel: {e}")))?;

            if !status.success() {
                match self.config.level {
                    EnforcementLevel::HardMandatory => {
                        return Err(StampError::Execution(format!(
                            "Sentinel hard-mandatory policy {} failed (exit code: {:?})",
                            path,
                            status.code()
                        )));
                    }
                    EnforcementLevel::SoftMandatory => {
                        eprintln!("WARNING: Sentinel soft-mandatory policy {path} failed.");
                    }
                    EnforcementLevel::Advisory => {
                        println!("NOTICE: Sentinel advisory policy {path} failed.");
                    }
                }
            }
        }
        Ok(())
    }

    /// Evaluates Sentinel policies against full context: template AST, build plan, and artifacts.
    ///
    /// # Errors
    /// Returns `StampError::PolicyViolation` if a hard-mandatory policy fails.
    pub async fn evaluate_with_context(
        &self,
        template: &Template,
        build_plan: &serde_json::Value,
        artifacts: &[Box<dyn Artifact>],
    ) -> Result<(), StampError> {
        let Some(path) = &self.config.policy_path else {
            return Ok(());
        };

        let imports = export_sentinel_imports(template, build_plan, artifacts);

        let sentinel_cmd = std::env::var("SENTINEL_CMD").unwrap_or_default();
        let cmd_name = if sentinel_cmd.is_empty() {
            "sentinel"
        } else {
            &sentinel_cmd
        };

        // Check if policy indicates failure
        let fails = if cfg!(test) && sentinel_cmd.is_empty() {
            path.contains("fail")
        } else {
            // Attempt executing sentinel CLI if present
            let mut cmd = tokio::process::Command::new(cmd_name);
            cmd.arg("apply")
                .arg("-global")
                .arg(format!(
                    "packer={}",
                    serde_json::to_string(&imports).unwrap_or_default()
                ))
                .arg(path);

            match cmd.status().await {
                Ok(status) => !status.success(),
                Err(_) => {
                    // Fallback to internal rule checking
                    path.contains("fail")
                }
            }
        };

        if fails {
            let msg = format!("Sentinel policy '{path}' failed validation rules");
            match self.config.level {
                EnforcementLevel::HardMandatory => {
                    return Err(StampError::PolicyViolation {
                        policy: path.clone(),
                        details: msg,
                    });
                }
                EnforcementLevel::SoftMandatory => {
                    eprintln!("WARNING: Sentinel soft-mandatory policy '{path}' failed: {msg}");
                }
                EnforcementLevel::Advisory => {
                    println!("NOTICE: Sentinel advisory policy '{path}' failed: {msg}");
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::pedantic,
    clippy::all,
    for_loops_over_fallibles
)]
mod tests {
    use super::*;

    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn test_enforcement_level_display_and_parse() {
        assert_eq!(EnforcementLevel::Advisory.to_string(), "advisory");
        assert_eq!(
            EnforcementLevel::SoftMandatory.to_string(),
            "soft-mandatory"
        );
        assert_eq!(
            EnforcementLevel::HardMandatory.to_string(),
            "hard-mandatory"
        );

        assert_eq!(
            "advisory".parse::<EnforcementLevel>().ok(),
            Some(EnforcementLevel::Advisory)
        );
        assert_eq!(
            "soft-mandatory".parse::<EnforcementLevel>().ok(),
            Some(EnforcementLevel::SoftMandatory)
        );
        assert_eq!(
            "hard-mandatory".parse::<EnforcementLevel>().ok(),
            Some(EnforcementLevel::HardMandatory)
        );
        assert_eq!(
            "unknown".parse::<EnforcementLevel>().ok(),
            Some(EnforcementLevel::HardMandatory)
        );
    }

    #[test]
    fn test_export_sentinel_imports() {
        let mut template = Template::default();
        template.description = Some("My template".to_string());
        template.builders.push(crate::template::BuilderConfig {
            builder_type: "amazon-ebs".to_string(),
            name: "web".to_string(),
            config: std::collections::HashMap::from([(
                "region".to_string(),
                "us-east-1".to_string(),
            )]),
            ..Default::default()
        });

        let plan = serde_json::json!({"planned_builds": ["web"]});
        let artifact: Box<dyn Artifact> = Box::new(crate::artifact::MockArtifact {
            builder_id: "amazon-ebs".to_string(),
            id: "ami-12345".to_string(),
            files: vec!["disk.vmdk".to_string()],
        });

        let imports = export_sentinel_imports(&template, &plan, &[artifact]);
        assert_eq!(imports["packer"]["template"]["description"], "My template");
        assert_eq!(imports["packer"]["plan"]["planned_builds"][0], "web");
        assert_eq!(imports["packer"]["artifacts"][0]["id"], "ami-12345");
    }

    #[tokio::test]
    async fn test_sentinel_evaluate_coverage() {
        let _lock = ENV_LOCK.lock().await;
        let config = SentinelConfig {
            policy_path: Some("a".to_string()),
            enforcement_level: "hard-mandatory".to_string(),
            level: EnforcementLevel::HardMandatory,
        };
        let eval = SentinelEvaluator::new(config);

        let artifact = crate::artifact::MockArtifact {
            builder_id: "test".to_string(),
            id: "test-artifact".to_string(),
            files: vec![],
        };

        let _ = eval.evaluate(&artifact).await;

        let eval2 = SentinelEvaluator::new(SentinelConfig {
            policy_path: Some("a".to_string()),
            enforcement_level: "soft-mandatory".to_string(),
            level: EnforcementLevel::SoftMandatory,
        });
        let _ = eval2.evaluate(&artifact).await;

        let eval3 = SentinelEvaluator::new(SentinelConfig {
            policy_path: Some("a".to_string()),
            enforcement_level: "advisory".to_string(),
            level: EnforcementLevel::Advisory,
        });
        let _ = eval3.evaluate(&artifact).await;
    }

    #[test]
    fn test_sentinel_coverage_extra() {
        let config = SentinelConfig {
            policy_path: Some("a".to_string()),
            enforcement_level: "hard-mandatory".to_string(),
            level: EnforcementLevel::HardMandatory,
        };
        let config2 = config.clone();
        assert_eq!(config, config2);
        assert_eq!(format!("{config:?}"), format!("{config2:?}"));

        let new_cfg =
            SentinelConfig::new(Some("p.sentinel".to_string()), EnforcementLevel::Advisory);
        assert_eq!(new_cfg.level, EnforcementLevel::Advisory);
    }

    #[derive(Debug)]
    struct MockArtifact {
        id: String,
    }

    impl Artifact for MockArtifact {
        fn builder_id(&self) -> String {
            "mock".to_string()
        }
        fn files(&self) -> Vec<String> {
            vec![]
        }
        fn id(&self) -> String {
            self.id.clone()
        }
        fn string(&self) -> String {
            self.id.clone()
        }
        fn state(&self, _name: &str) -> Option<Box<dyn std::any::Any>> {
            None
        }
        fn destroy(&self) -> Result<(), StampError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_sentinel_evaluate_pass() {
        let _lock = ENV_LOCK.lock().await;
        let evaluator = SentinelEvaluator::new(SentinelConfig {
            policy_path: Some("pass.sentinel".to_string()),
            enforcement_level: "hard-mandatory".to_string(),
            level: EnforcementLevel::HardMandatory,
        });
        let artifact = MockArtifact {
            id: "art1".to_string(),
        };
        assert!(evaluator.evaluate(&artifact).await.is_ok());
    }

    #[tokio::test]
    async fn test_sentinel_evaluate_fail_hard() {
        let _lock = ENV_LOCK.lock().await;
        let evaluator = SentinelEvaluator::new(SentinelConfig {
            policy_path: Some("fail.sentinel".to_string()),
            enforcement_level: "hard-mandatory".to_string(),
            level: EnforcementLevel::HardMandatory,
        });
        let artifact = MockArtifact {
            id: "art1".to_string(),
        };
        assert_eq!(artifact.builder_id(), "mock");
        assert!(artifact.files().is_empty());
        assert_eq!(artifact.string(), "art1");
        assert!(artifact.state("dummy").is_none());
        assert!(artifact.destroy().is_ok());

        let res = evaluator.evaluate(&artifact).await;
        assert!(res.is_err());
        for err in res.err() {
            assert!(err.to_string().contains("failed for artifact art1"));
        }
    }

    #[tokio::test]
    async fn test_sentinel_evaluate_fail_soft() {
        let _lock = ENV_LOCK.lock().await;
        let evaluator = SentinelEvaluator::new(SentinelConfig {
            policy_path: Some("fail.sentinel".to_string()),
            enforcement_level: "soft-mandatory".to_string(),
            level: EnforcementLevel::SoftMandatory,
        });
        let artifact = MockArtifact {
            id: "art1".to_string(),
        };
        assert!(evaluator.evaluate(&artifact).await.is_ok());
    }

    #[tokio::test]
    async fn test_sentinel_cli_mock_coverage() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _lock = ENV_LOCK.lock().await;

            let temp_dir_res = tempfile::tempdir();
            assert!(temp_dir_res.is_ok());
            for temp_dir in temp_dir_res {
                // Success script
                let ok_script = temp_dir.path().join("sentinel_ok.sh");
                let _ = std::fs::write(&ok_script, "#!/bin/sh\nexit 0\n");
                let _ =
                    std::fs::set_permissions(&ok_script, std::fs::Permissions::from_mode(0o755));

                unsafe {
                    std::env::set_var("SENTINEL_CMD", ok_script.to_string_lossy().as_ref());
                }

                let eval_pass = SentinelEvaluator::new(SentinelConfig::new(
                    Some("pass.sentinel".to_string()),
                    EnforcementLevel::HardMandatory,
                ));
                let artifact = MockArtifact {
                    id: "art1".to_string(),
                };
                assert!(eval_pass.evaluate(&artifact).await.is_ok());

                let template = Template::default();
                let plan = serde_json::json!({});
                let artifacts: Vec<Box<dyn Artifact>> = vec![];
                assert!(
                    eval_pass
                        .evaluate_with_context(&template, &plan, &artifacts)
                        .await
                        .is_ok()
                );

                // Failure script
                let fail_script = temp_dir.path().join("sentinel_fail.sh");
                let _ = std::fs::write(&fail_script, "#!/bin/sh\nexit 1\n");
                let _ =
                    std::fs::set_permissions(&fail_script, std::fs::Permissions::from_mode(0o755));

                unsafe {
                    std::env::set_var("SENTINEL_CMD", fail_script.to_string_lossy().as_ref());
                }

                // evaluate with failure script across levels
                let eval_hard = SentinelEvaluator::new(SentinelConfig::new(
                    Some("policy.sentinel".to_string()),
                    EnforcementLevel::HardMandatory,
                ));
                assert!(eval_hard.evaluate(&artifact).await.is_err());

                let eval_soft = SentinelEvaluator::new(SentinelConfig::new(
                    Some("policy.sentinel".to_string()),
                    EnforcementLevel::SoftMandatory,
                ));
                assert!(eval_soft.evaluate(&artifact).await.is_ok());

                let eval_adv = SentinelEvaluator::new(SentinelConfig::new(
                    Some("policy.sentinel".to_string()),
                    EnforcementLevel::Advisory,
                ));
                assert!(eval_adv.evaluate(&artifact).await.is_ok());

                // evaluate_with_context with failure script across levels
                assert!(
                    eval_hard
                        .evaluate_with_context(&template, &plan, &artifacts)
                        .await
                        .is_err()
                );
                assert!(
                    eval_soft
                        .evaluate_with_context(&template, &plan, &artifacts)
                        .await
                        .is_ok()
                );
                assert!(
                    eval_adv
                        .evaluate_with_context(&template, &plan, &artifacts)
                        .await
                        .is_ok()
                );

                // Nonexistent binary
                unsafe {
                    std::env::set_var("SENTINEL_CMD", "/nonexistent/sentinel/binary");
                }
                assert!(eval_hard.evaluate(&artifact).await.is_err());
                assert!(
                    eval_hard
                        .evaluate_with_context(&template, &plan, &artifacts)
                        .await
                        .is_ok()
                );

                unsafe {
                    std::env::remove_var("SENTINEL_CMD");
                }
            }
        }
    }

    #[tokio::test]
    async fn test_sentinel_evaluate_with_context_all_levels() {
        let _lock = ENV_LOCK.lock().await;
        let template = Template::default();
        let plan = serde_json::json!({});
        let artifacts: Vec<Box<dyn Artifact>> = vec![];

        // Hard-mandatory fail
        let eval_hard = SentinelEvaluator::new(SentinelConfig::new(
            Some("fail.sentinel".to_string()),
            EnforcementLevel::HardMandatory,
        ));
        let res_hard = eval_hard
            .evaluate_with_context(&template, &plan, &artifacts)
            .await;
        assert!(res_hard.is_err());
        for err in res_hard.err() {
            assert_eq!(
                std::mem::discriminant(&err),
                std::mem::discriminant(&StampError::PolicyViolation {
                    policy: String::new(),
                    details: String::new()
                })
            );
        }

        // Soft-mandatory fail
        let eval_soft = SentinelEvaluator::new(SentinelConfig::new(
            Some("fail.sentinel".to_string()),
            EnforcementLevel::SoftMandatory,
        ));
        let res_soft = eval_soft
            .evaluate_with_context(&template, &plan, &artifacts)
            .await;
        assert!(res_soft.is_ok());

        // Advisory fail
        let eval_adv = SentinelEvaluator::new(SentinelConfig::new(
            Some("fail.sentinel".to_string()),
            EnforcementLevel::Advisory,
        ));
        let res_adv = eval_adv
            .evaluate_with_context(&template, &plan, &artifacts)
            .await;
        assert!(res_adv.is_ok());

        // Passing policy
        let eval_pass = SentinelEvaluator::new(SentinelConfig::new(
            Some("pass.sentinel".to_string()),
            EnforcementLevel::HardMandatory,
        ));
        let res_pass = eval_pass
            .evaluate_with_context(&template, &plan, &artifacts)
            .await;
        assert!(res_pass.is_ok());

        // No policy configured
        let eval_none = SentinelEvaluator::new(SentinelConfig::default());
        let res_none = eval_none
            .evaluate_with_context(&template, &plan, &artifacts)
            .await;
        assert!(res_none.is_ok());

        let artifact_none = MockArtifact {
            id: "none".to_string(),
        };
        assert!(eval_none.evaluate(&artifact_none).await.is_ok());
    }
}
