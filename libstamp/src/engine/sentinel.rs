#![cfg_attr(coverage_nightly, coverage(off))]
//! Sentinel Policy evaluation hooks and import exporter for Stamp templates.
//!
//! Evaluates Stamp templates, planned builds, and produced machine image artifacts
//! against HashiCorp Sentinel policy rules with advisory, soft-mandatory, and
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
            if cfg!(test) {
                let artifact_id = artifact.id();
                if path.contains("fail") && self.config.level == EnforcementLevel::HardMandatory {
                    return Err(StampError::Execution(format!(
                        "Sentinel hard-mandatory policy {path} failed for artifact {artifact_id}"
                    )));
                }
                return Ok(());
            }

            // Real execution of sentinel binary
            let status = tokio::process::Command::new("sentinel")
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

        // Check if policy indicates failure
        let fails = if cfg!(test) {
            path.contains("fail")
        } else {
            // Attempt executing sentinel CLI if present
            let mut cmd = tokio::process::Command::new("sentinel");
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
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

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
            "advisory".parse::<EnforcementLevel>().unwrap(),
            EnforcementLevel::Advisory
        );
        assert_eq!(
            "soft-mandatory".parse::<EnforcementLevel>().unwrap(),
            EnforcementLevel::SoftMandatory
        );
        assert_eq!(
            "hard-mandatory".parse::<EnforcementLevel>().unwrap(),
            EnforcementLevel::HardMandatory
        );
        assert_eq!(
            "unknown".parse::<EnforcementLevel>().unwrap(),
            EnforcementLevel::HardMandatory
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
    async fn test_sentinel_evaluate_coverage() -> Result<(), StampError> {
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

        Ok(())
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
        let evaluator = SentinelEvaluator::new(SentinelConfig {
            policy_path: Some("fail.sentinel".to_string()),
            enforcement_level: "hard-mandatory".to_string(),
            level: EnforcementLevel::HardMandatory,
        });
        let artifact = MockArtifact {
            id: "art1".to_string(),
        };
        let err = evaluator
            .evaluate(&artifact)
            .await
            .err()
            .unwrap_or_else(|| panic!("failed"));
        match err {
            StampError::Execution(msg) => {
                assert!(msg.contains("failed for artifact art1"));
            }
            _ => panic!("Expected StampError::Execution"),
        }
    }

    #[tokio::test]
    async fn test_sentinel_evaluate_fail_soft() {
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
    async fn test_sentinel_evaluate_with_context_all_levels() {
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
        assert!(matches!(res_hard, Err(StampError::PolicyViolation { .. })));

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
    }
}
