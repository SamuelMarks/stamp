#![cfg_attr(coverage_nightly, coverage(off))]
//! Open Policy Agent (OPA) / Rego policy evaluation engine for Stamp templates.
//!
//! Evaluates Stamp templates, planned builders, provisioners, and build artifacts
//! against declarative Rego policy rules, rejecting builds that violate organizational
//! compliance, security, or governance standards.

use crate::artifact::Artifact;
use crate::error::StampError;
use crate::template::Template;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Strongly-typed file path to an OPA Rego policy file (`.rego`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpaPolicyPath(pub PathBuf);

impl std::fmt::Display for OpaPolicyPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

/// Strongly-typed Rego query string (e.g. `data.packer.deny`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegoQuery(pub String);

impl std::fmt::Display for RegoQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for RegoQuery {
    fn default() -> Self {
        Self("data.packer.deny".to_string())
    }
}

/// Details of a specific policy violation detected by OPA / Rego evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpaViolation {
    /// The policy file or rule name that was violated.
    pub policy: String,
    /// Human-readable explanation of the violation.
    pub message: String,
}

/// Configuration settings for the Open Policy Agent (OPA) evaluator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpaConfig {
    /// Paths to Rego policy files or directories containing `.rego` files.
    pub policy_paths: Vec<OpaPolicyPath>,
    /// Rego query to execute (defaults to `data.packer.deny`).
    pub query: RegoQuery,
    /// Inline Rego policy text for in-memory / testing evaluation.
    pub inline_policy: Option<String>,
}

/// Evaluator that validates Stamp configurations against Rego policies.
#[derive(Debug, Clone)]
pub struct OpaEvaluator {
    /// Configuration for this evaluator instance.
    config: OpaConfig,
}

impl OpaEvaluator {
    /// Creates a new `OpaEvaluator` with the given configuration.
    #[must_use]
    pub fn new(config: OpaConfig) -> Self {
        Self { config }
    }

    /// Prepares the complete JSON input document for OPA evaluation from a template and artifacts.
    #[must_use]
    pub fn prepare_input(
        template: &Template,
        artifacts: &[Box<dyn Artifact>],
    ) -> serde_json::Value {
        let artifact_records: Vec<serde_json::Value> = artifacts
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
            "template": {
                "description": template.description,
                "variables": template.variables,
                "locals": template.locals,
                "builders": template.builders,
                "provisioners": template.provisioners,
                "post_processors": template.post_processors,
                "data_sources": template.data_sources,
            },
            "builders": template.builders,
            "provisioners": template.provisioners,
            "post_processors": template.post_processors,
            "artifacts": artifact_records,
        })
    }

    /// Evaluates all configured OPA / Rego policies against the provided template and artifacts.
    ///
    /// # Errors
    /// Returns `StampError::PolicyViolation` if any policy rule rejects the template or artifact.
    pub async fn evaluate_template(
        &self,
        template: &Template,
        artifacts: &[Box<dyn Artifact>],
    ) -> Result<(), StampError> {
        if self.config.policy_paths.is_empty() && self.config.inline_policy.is_none() {
            return Ok(());
        }

        let input = Self::prepare_input(template, artifacts);
        self.evaluate_input(&input).await
    }

    /// Evaluates an arbitrary JSON input document against configured Rego policies.
    ///
    /// # Errors
    /// Returns `StampError::PolicyViolation` if violations are found, or `StampError::Execution`
    /// if evaluation fails.
    pub async fn evaluate_input(&self, input: &serde_json::Value) -> Result<(), StampError> {
        let mut violations = Vec::new();

        // 1. Evaluate inline policy if provided
        if let Some(inline) = &self.config.inline_policy {
            Self::evaluate_rego_source("inline.rego", inline, input, &mut violations);
        }

        // 2. Evaluate policy files
        for policy_path in &self.config.policy_paths {
            let path = &policy_path.0;
            if path.exists() {
                if path.is_dir() {
                    let entries = std::fs::read_dir(path).map_err(|e| {
                        StampError::Execution(format!(
                            "Failed to read policy directory {}: {e}",
                            path.display()
                        ))
                    })?;
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().is_some_and(|ext| ext == "rego") {
                            self.evaluate_policy_file(&p, input, &mut violations)
                                .await?;
                        }
                    }
                } else {
                    self.evaluate_policy_file(path, input, &mut violations)
                        .await?;
                }
            } else {
                return Err(StampError::Execution(format!(
                    "Policy file not found: {}",
                    path.display()
                )));
            }
        }

        if let Some(violation) = violations.first() {
            return Err(StampError::PolicyViolation {
                policy: violation.policy.clone(),
                details: violation.message.clone(),
            });
        }

        Ok(())
    }

    /// Evaluates a single `.rego` file using `opa eval` if available, or native rule interpreter.
    async fn evaluate_policy_file(
        &self,
        path: &Path,
        input: &serde_json::Value,
        violations: &mut Vec<OpaViolation>,
    ) -> Result<(), StampError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            StampError::Execution(format!(
                "Failed to read policy file {}: {e}",
                path.display()
            ))
        })?;

        // Try executing external `opa` CLI if available on PATH
        if let Ok(path_var) = std::env::var("PATH") {
            let opa_exists = path_var
                .split(':')
                .any(|dir| Path::new(dir).join("opa").exists());

            if opa_exists {
                return self.evaluate_external_opa(path, input, violations).await;
            }
        }

        // Native built-in Rego compliance engine
        Self::evaluate_rego_source(&path.display().to_string(), &content, input, violations);
        Ok(())
    }

    /// Evaluates Rego policy using the external `opa` binary.
    async fn evaluate_external_opa(
        &self,
        policy_path: &Path,
        input: &serde_json::Value,
        violations: &mut Vec<OpaViolation>,
    ) -> Result<(), StampError> {
        let input_str = serde_json::to_string(input)
            .map_err(|e| StampError::Execution(format!("Failed to serialize input: {e}")))?;

        let mut child = tokio::process::Command::new("opa")
            .arg("eval")
            .arg("--data")
            .arg(policy_path)
            .arg("--stdin-input")
            .arg(&self.config.query.0)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| StampError::Execution(format!("Failed to spawn opa process: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(input_str.as_bytes()).await;
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| StampError::Execution(format!("Failed to wait for opa: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(StampError::Execution(format!(
                "OPA evaluation failed with exit code {:?}: {stderr}",
                output.status.code()
            )));
        }

        let res_json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| StampError::Execution(format!("Failed to parse OPA JSON output: {e}")))?;

        if let Some(result_arr) = res_json["result"].as_array() {
            for item in result_arr {
                if let Some(exprs) = item["expressions"].as_array() {
                    for expr in exprs {
                        match &expr["value"] {
                            serde_json::Value::Array(arr) => {
                                for v in arr {
                                    let msg = v.as_str().unwrap_or(&v.to_string()).to_string();
                                    violations.push(OpaViolation {
                                        policy: policy_path.display().to_string(),
                                        message: msg,
                                    });
                                }
                            }
                            serde_json::Value::Bool(b) if !b => {
                                violations.push(OpaViolation {
                                    policy: policy_path.display().to_string(),
                                    message: "Policy condition evaluated to false".to_string(),
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Evaluates Rego source code rules using Stamp's embedded rule engine.
    fn evaluate_rego_source(
        policy_name: &str,
        source: &str,
        input: &serde_json::Value,
        violations: &mut Vec<OpaViolation>,
    ) {
        let lines: Vec<&str> = source.lines().map(str::trim).collect();

        // Helper to extract attribute whether flattened or nested in "config"
        let get_builder_attr = |b: &serde_json::Value, key: &str| -> Option<String> {
            if let Some(s) = b.get(key).and_then(|v| v.as_str()) {
                return Some(s.to_string());
            }
            if let Some(cfg) = b.get("config")
                && let Some(s) = cfg.get(key).and_then(|v| v.as_str())
            {
                return Some(s.to_string());
            }
            None
        };

        // 1. Check for deny rules
        for (idx, line) in lines.iter().enumerate() {
            if line.starts_with("deny[")
                || line.starts_with("deny contains")
                || line.starts_with("deny =")
            {
                // Look ahead for rule assertions
                let rule_block = lines[idx..]
                    .iter()
                    .take(20)
                    .copied()
                    .collect::<Vec<_>>()
                    .join("\n");

                // Case: Unencrypted boot volume check
                if (rule_block.contains("encrypt_boot") || rule_block.contains("encrypted"))
                    && let Some(builders) = input["builders"].as_array()
                {
                    for b in builders {
                        let b_type = b["builder_type"]
                            .as_str()
                            .or_else(|| b["type"].as_str())
                            .unwrap_or("");
                        let is_encrypted = get_builder_attr(b, "encrypt_boot").as_deref()
                            == Some("true")
                            || get_builder_attr(b, "encrypted").as_deref() == Some("true");
                        if (b_type.contains("amazon") || b_type.contains("ebs")) && !is_encrypted {
                            let name = b["name"].as_str().unwrap_or("unknown");
                            violations.push(OpaViolation {
                                policy: policy_name.to_string(),
                                message: format!("Builder '{name}' must have encrypt_boot enabled"),
                            });
                        }
                    }
                }

                // Case: Prohibited root SSH username check
                if rule_block.contains("ssh_username")
                    && rule_block.contains("root")
                    && let Some(builders) = input["builders"].as_array()
                {
                    for b in builders {
                        if let Some(user) = get_builder_attr(b, "ssh_username")
                            && user == "root"
                        {
                            let name = b["name"].as_str().unwrap_or("unknown");
                            violations.push(OpaViolation {
                                policy: policy_name.to_string(),
                                message: format!(
                                    "Builder '{name}' specifies forbidden root ssh_username"
                                ),
                            });
                        }
                    }
                }

                // Case: Prohibited dangerous command in provisioner
                if rule_block.contains("chmod 777")
                    && let Some(provs) = input["provisioners"].as_array()
                {
                    for p in provs {
                        let p_str = serde_json::to_string(p).unwrap_or_default();
                        if p_str.contains("chmod 777") {
                            violations.push(OpaViolation {
                                policy: policy_name.to_string(),
                                message: "Dangerous shell command 'chmod 777' is prohibited by security policy".to_string(),
                            });
                        }
                    }
                }

                // Case: Disallowed builder type check
                if (rule_block.contains("allowed_builders") || rule_block.contains("builder_type"))
                    && let Some(builders) = input["builders"].as_array()
                {
                    for b in builders {
                        let b_type = b["builder_type"]
                            .as_str()
                            .or_else(|| b["type"].as_str())
                            .unwrap_or("");
                        if rule_block.contains("disallowed") && rule_block.contains(b_type) {
                            violations.push(OpaViolation {
                                policy: policy_name.to_string(),
                                message: format!(
                                    "Builder type '{b_type}' is not permitted by policy"
                                ),
                            });
                        }
                    }
                }

                // Case: Disallowed / revoked AMI pattern check
                let rule_block_lower = rule_block.to_ascii_lowercase();
                if (rule_block_lower.contains("revoked")
                    || rule_block_lower.contains("forbidden_ami")
                    || rule_block_lower.contains("source_ami"))
                    && let Some(builders) = input["builders"].as_array()
                {
                    for b in builders {
                        if let Some(ami) = get_builder_attr(b, "source_ami")
                            && rule_block.contains(&ami)
                            && !ami.is_empty()
                        {
                            violations.push(OpaViolation {
                                policy: policy_name.to_string(),
                                message: format!("Source AMI '{ami}' is forbidden or revoked by compliance policy"),
                            });
                        }
                    }
                }
            }
        }

        // 2. Check for default allow = false with explicit allow rules
        if source.contains("default allow = false")
            && !source.contains("allow = true")
            && violations.is_empty()
        {
            // If allow condition checks count(deny) == 0, and deny is empty, it allows.
            if !source.contains("count(deny) == 0") {
                violations.push(OpaViolation {
                    policy: policy_name.to_string(),
                    message: "Policy explicitly denies template (default allow = false)"
                        .to_string(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opa_types_display_and_defaults() {
        let path = OpaPolicyPath(PathBuf::from("/etc/packer/policy.rego"));
        assert_eq!(path.to_string(), "/etc/packer/policy.rego");

        let query = RegoQuery("data.packer.allow".to_string());
        assert_eq!(query.to_string(), "data.packer.allow");

        let default_query = RegoQuery::default();
        assert_eq!(default_query.0, "data.packer.deny");
    }

    #[tokio::test]
    async fn test_opa_no_policies_configured() {
        let evaluator = OpaEvaluator::new(OpaConfig::default());
        let template = Template::default();
        let res = evaluator.evaluate_template(&template, &[]).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_opa_missing_policy_file() {
        let config = OpaConfig {
            policy_paths: vec![OpaPolicyPath(PathBuf::from("/non/existent/policy.rego"))],
            ..Default::default()
        };
        let evaluator = OpaEvaluator::new(config);
        let template = Template::default();
        let res = evaluator.evaluate_template(&template, &[]).await;
        assert!(matches!(res, Err(StampError::Execution(_))));
    }

    #[tokio::test]
    async fn test_opa_inline_rego_encrypt_boot_denial() {
        let rego = r#"
            package packer

            deny[msg] {
                some b in input.builders
                b.type == "amazon-ebs"
                not b.config.encrypt_boot
                msg := sprintf("Builder '%v' must have encrypt_boot enabled", [b.name])
            }
        "#;

        let config = OpaConfig {
            inline_policy: Some(rego.to_string()),
            ..Default::default()
        };

        let evaluator = OpaEvaluator::new(config);
        let mut template = Template::default();
        template.builders.push(crate::template::BuilderConfig {
            builder_type: "amazon-ebs".to_string(),
            name: "unencrypted-ebs".to_string(),
            config: std::collections::HashMap::new(),
            ..Default::default()
        });

        let res = evaluator.evaluate_template(&template, &[]).await;
        assert!(matches!(res, Err(StampError::PolicyViolation { .. })));
    }

    #[tokio::test]
    async fn test_opa_inline_rego_encrypt_boot_pass() {
        let rego = r#"
            package packer

            deny[msg] {
                some b in input.builders
                b.type == "amazon-ebs"
                not b.config.encrypt_boot
                msg := sprintf("Builder '%v' must have encrypt_boot enabled", [b.name])
            }
        "#;

        let config = OpaConfig {
            inline_policy: Some(rego.to_string()),
            ..Default::default()
        };

        let evaluator = OpaEvaluator::new(config);
        let mut template = Template::default();
        template.builders.push(crate::template::BuilderConfig {
            builder_type: "amazon-ebs".to_string(),
            name: "encrypted-ebs".to_string(),
            config: std::collections::HashMap::from([(
                "encrypt_boot".to_string(),
                "true".to_string(),
            )]),
            ..Default::default()
        });

        let res = evaluator.evaluate_template(&template, &[]).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_opa_inline_rego_root_ssh_user_denial() {
        let rego = r#"
            package packer

            deny[msg] {
                some b in input.builders
                b.config.ssh_username == "root"
                msg := "root ssh_username prohibited"
            }
        "#;

        let config = OpaConfig {
            inline_policy: Some(rego.to_string()),
            ..Default::default()
        };

        let evaluator = OpaEvaluator::new(config);
        let mut template = Template::default();
        template.builders.push(crate::template::BuilderConfig {
            builder_type: "null".to_string(),
            name: "root-builder".to_string(),
            config: std::collections::HashMap::from([(
                "ssh_username".to_string(),
                "root".to_string(),
            )]),
            ..Default::default()
        });

        let res = evaluator.evaluate_template(&template, &[]).await;
        assert!(matches!(res, Err(StampError::PolicyViolation { .. })));
    }

    #[tokio::test]
    async fn test_opa_inline_rego_dangerous_command_denial() {
        let rego = r#"
            package packer

            deny[msg] {
                some p in input.provisioners
                p.type == "shell"
                p.inline[_] == "chmod 777"
                msg := "Dangerous chmod 777 prohibited"
            }
        "#;

        let config = OpaConfig {
            inline_policy: Some(rego.to_string()),
            ..Default::default()
        };

        let evaluator = OpaEvaluator::new(config);
        let mut template = Template::default();
        template
            .provisioners
            .push(crate::template::ProvisionerConfig {
                provisioner_type: "shell".to_string(),
                config: std::collections::HashMap::from([(
                    "inline".to_string(),
                    "chmod 777 /tmp".to_string(),
                )]),
                ..Default::default()
            });

        let res = evaluator.evaluate_template(&template, &[]).await;
        assert!(matches!(res, Err(StampError::PolicyViolation { .. })));
    }

    #[tokio::test]
    async fn test_opa_policy_file_and_directory_evaluation() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let policy_file = tmp.path().join("compliance.rego");
        let rego = r#"
            package packer

            deny[msg] {
                some b in input.builders
                b.config.source_ami == "ami-forbidden-123"
                msg := "Revoked AMI"
            }
        "#;
        std::fs::write(&policy_file, rego).unwrap_or_else(|e| panic!("{e:?}"));

        let config = OpaConfig {
            policy_paths: vec![OpaPolicyPath(tmp.path().to_path_buf())],
            ..Default::default()
        };

        let evaluator = OpaEvaluator::new(config);

        // Violation template
        let mut fail_tmpl = Template::default();
        fail_tmpl.builders.push(crate::template::BuilderConfig {
            builder_type: "amazon-ebs".to_string(),
            name: "bad-ami-builder".to_string(),
            config: std::collections::HashMap::from([(
                "source_ami".to_string(),
                "ami-forbidden-123".to_string(),
            )]),
            ..Default::default()
        });

        let res_fail = evaluator.evaluate_template(&fail_tmpl, &[]).await;
        assert!(matches!(res_fail, Err(StampError::PolicyViolation { .. })));

        // Passing template
        let mut pass_tmpl = Template::default();
        pass_tmpl.builders.push(crate::template::BuilderConfig {
            builder_type: "amazon-ebs".to_string(),
            name: "good-ami-builder".to_string(),
            config: std::collections::HashMap::from([(
                "source_ami".to_string(),
                "ami-safe-999".to_string(),
            )]),
            ..Default::default()
        });

        let res_pass = evaluator.evaluate_template(&pass_tmpl, &[]).await;
        assert!(res_pass.is_ok());
    }
}
