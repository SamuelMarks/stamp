#![cfg_attr(coverage_nightly, coverage(off))]
//! JSON parser for legacy Packer templates.

use crate::error::StampError;
use crate::template::{BuilderConfig, ProvisionerConfig, Template};

/// Parses a JSON configuration string into a `Template`.
///
/// # Errors
///
/// Returns a `StampError::Parse` if the JSON cannot be parsed.
pub fn parse_json<S: ::std::hash::BuildHasher>(
    input: &str,
    vars: &std::collections::HashMap<String, String, S>,
) -> Result<Template, StampError> {
    // Legacy packer templates have top-level "builders" and "provisioners" arrays.
    // They also sometimes have a "description" field.
    #[derive(serde::Deserialize)]
    struct LegacyPacker {
        description: Option<String>,
        #[serde(default)]
        builders: Vec<LegacyBuilder>,
        #[serde(default)]
        provisioners: Vec<LegacyProvisioner>,
        #[serde(default, rename = "error-cleanup-provisioner")]
        error_cleanup_provisioners: Vec<LegacyProvisioner>,
        #[serde(default, rename = "post-processors")]
        post_processors: Vec<LegacyPostProcessor>,
        #[serde(default)]
        variables: std::collections::HashMap<String, String>,
        #[serde(default)]
        packer: Option<crate::template::PackerConfig>,
        #[serde(default)]
        hcp_packer_registry: Option<crate::template::HcpPackerRegistryConfig>,
    }

    #[derive(serde::Deserialize)]
    struct LegacyBuilder {
        #[serde(rename = "type")]
        builder_type: String,
        name: Option<String>,
        #[serde(default)]
        depends_on: Vec<String>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    }

    #[derive(serde::Deserialize)]
    struct LegacyPostProcessor {
        #[serde(rename = "type")]
        post_processor_type: String,
        #[serde(default)]
        keep_input_artifact: bool,
        #[serde(default)]
        only: Vec<String>,
        #[serde(default)]
        except: Vec<String>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    }

    #[derive(serde::Deserialize)]
    struct LegacyProvisioner {
        #[serde(rename = "type")]
        provisioner_type: String,
        #[serde(default)]
        only: Vec<String>,
        #[serde(default)]
        except: Vec<String>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    }

    let mut raw_val: serde_json::Value = serde_json::from_str(input)?;

    let mut macro_ctx = crate::engine::legacy_macro::LegacyMacroContext::new();
    if let Some(var_obj) = raw_val
        .get("variables")
        .and_then(serde_json::Value::as_object)
    {
        for (k, v) in var_obj {
            let val_str = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            macro_ctx.set_user_var(k, val_str);
        }
    }
    for (k, v) in vars {
        macro_ctx.set_user_var(k, v);
    }
    macro_ctx.permissive = true;
    macro_ctx.preserve_unresolved = true;

    crate::engine::legacy_macro::interpolate_json_value(&mut raw_val, &macro_ctx)?;

    let parsed: LegacyPacker = serde_json::from_value(raw_val)?;

    let mut builders = Vec::new();
    for b in parsed.builders {
        let name = b.name.unwrap_or_else(|| b.builder_type.clone());
        let mut config = std::collections::HashMap::new();
        for (k, v) in b.extra {
            let val_str = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            config.insert(k, val_str);
        }
        builders.push(BuilderConfig {
            builder_type: b.builder_type,
            name,
            depends_on: b.depends_on,
            config,
        });
    }

    let mut provisioners = Vec::new();
    for p in parsed.provisioners {
        let mut config = std::collections::HashMap::new();
        for (k, v) in p.extra {
            let val_str = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            config.insert(k, val_str);
        }
        provisioners.push(ProvisionerConfig {
            provisioner_type: p.provisioner_type,
            only: p.only,
            except: p.except,
            config,
        });
    }

    let mut error_cleanup_provisioners = Vec::new();
    for p in parsed.error_cleanup_provisioners {
        let mut config = std::collections::HashMap::new();
        for (k, v) in p.extra {
            let val_str = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            config.insert(k, val_str);
        }
        error_cleanup_provisioners.push(ProvisionerConfig {
            provisioner_type: p.provisioner_type,
            only: p.only,
            except: p.except,
            config,
        });
    }

    let mut post_processors = Vec::new();
    for p in parsed.post_processors {
        let mut config = std::collections::HashMap::new();
        for (k, v) in p.extra {
            let val_str = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            config.insert(k, val_str);
        }
        post_processors.push(crate::template::PostProcessorConfig {
            post_processor_type: p.post_processor_type,
            keep_input_artifact: p.keep_input_artifact,
            only: p.only,
            except: p.except,
            config,
        });
    }

    let mut variables = std::collections::HashMap::new();
    for (k, v) in parsed.variables {
        let var = crate::template::VariableConfig {
            default: Some(v),
            ..crate::template::VariableConfig::default()
        };
        variables.insert(k, var);
    }

    let packer = parsed.packer.or_else(|| {
        parsed
            .hcp_packer_registry
            .map(|reg| crate::template::PackerConfig {
                required_version: None,
                hcp_packer_registry: Some(reg),
            })
    });

    Ok(Template {
        description: parsed.description,
        builders,
        provisioners,
        error_cleanup_provisioners,
        post_processors,
        variables,
        packer,
        ..Default::default()
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::match_wildcard_for_single_variants)]
    fn test_parse_json_valid() -> Result<(), crate::error::StampError> {
        let input = r#"{
            "description": "A legacy template",
            "builders": [
                {
                    "type": "amazon-ebs",
                    "name": "aws-builder"
                },
                {
                    "type": "docker"
                }
            ],
            "provisioners": [
                {
                    "type": "shell"
                }
            ],
            "error-cleanup-provisioner": [
                {
                    "type": "shell-local"
                }
            ],
            "post-processors": [
                {
                    "type": "docker-tag",
                    "keep_input_artifact": true
                }
            ],
            "variables": {
                "foo": "bar",
                "baz": "qux"
            }
        }"#;

        let tmpl = parse_json(input, &std::collections::HashMap::new())?;
        assert_eq!(tmpl.description.as_deref(), Some("A legacy template"));
        assert_eq!(tmpl.builders.len(), 2);
        assert_eq!(tmpl.builders[0].builder_type, "amazon-ebs");
        assert_eq!(tmpl.builders[0].name, "aws-builder");
        assert_eq!(tmpl.builders[1].builder_type, "docker");
        assert_eq!(tmpl.builders[1].name, "docker");
        assert_eq!(tmpl.provisioners.len(), 1);
        assert_eq!(tmpl.provisioners[0].provisioner_type, "shell");
        assert_eq!(tmpl.error_cleanup_provisioners.len(), 1);
        assert_eq!(
            tmpl.error_cleanup_provisioners[0].provisioner_type,
            "shell-local"
        );
        assert_eq!(tmpl.post_processors.len(), 1);
        assert_eq!(tmpl.post_processors[0].post_processor_type, "docker-tag");
        assert!(tmpl.post_processors[0].keep_input_artifact);

        assert_eq!(tmpl.variables.len(), 2);
        assert_eq!(
            tmpl.variables.get("foo").and_then(|v| v.default.as_deref()),
            Some("bar")
        );
        assert_eq!(
            tmpl.variables.get("baz").and_then(|v| v.default.as_deref()),
            Some("qux")
        );
        Ok(())
    }

    #[test]
    fn test_parse_json_invalid_syntax() {
        let input = r#"{ "builders": [ }"#;
        let err = parse_json(input, &std::collections::HashMap::new());
        assert!(matches!(err, Err(StampError::Json(_))));
    }

    #[test]
    fn test_parse_json_invalid_schema() {
        let input = r#"{ "builders": "not_an_array" }"#;
        let err = parse_json(input, &std::collections::HashMap::new());
        assert!(matches!(err, Err(StampError::Json(_))));
    }

    #[test]
    fn test_parse_json_advanced_blocks() -> Result<(), crate::error::StampError> {
        let input = r#"{
            "packer": {
                "required_version": ">= 1.8.0"
            },
            "builders": [
                {
                    "type": "null",
                    "name": "base-builder",
                    "depends_on": ["prev-builder"],
                    "count": 42,
                    "enabled": true
                }
            ],
            "provisioners": [
                {
                    "type": "shell",
                    "only": ["base-builder"],
                    "except": ["other-builder"],
                    "timeout_sec": 300
                }
            ],
            "error-cleanup-provisioner": [
                {
                    "type": "shell-local",
                    "only": ["base-builder"],
                    "except": ["other-builder"]
                }
            ],
            "post-processors": [
                {
                    "type": "manifest",
                    "only": ["base-builder"],
                    "except": ["other-builder"],
                    "output": "manifest.json"
                }
            ]
        }"#;

        let tmpl = parse_json(input, &std::collections::HashMap::new())?;
        assert_eq!(tmpl.builders.len(), 1);
        assert_eq!(tmpl.builders[0].depends_on, vec!["prev-builder"]);
        assert_eq!(
            tmpl.builders[0].config.get("count"),
            Some(&"42".to_string())
        );
        assert_eq!(
            tmpl.builders[0].config.get("enabled"),
            Some(&"true".to_string())
        );

        assert_eq!(tmpl.provisioners.len(), 1);
        assert_eq!(tmpl.provisioners[0].only, vec!["base-builder"]);
        assert_eq!(tmpl.provisioners[0].except, vec!["other-builder"]);

        assert_eq!(tmpl.error_cleanup_provisioners.len(), 1);
        assert_eq!(
            tmpl.error_cleanup_provisioners[0].only,
            vec!["base-builder"]
        );

        assert_eq!(tmpl.post_processors.len(), 1);
        assert_eq!(tmpl.post_processors[0].only, vec!["base-builder"]);
        assert_eq!(tmpl.post_processors[0].except, vec!["other-builder"]);

        let packer_cfg = tmpl
            .packer
            .ok_or_else(|| StampError::TemplateValidation("packer missing".to_string()))?;
        assert_eq!(packer_cfg.required_version.as_deref(), Some(">= 1.8.0"));
        Ok(())
    }

    #[test]
    fn test_parse_json_macro_interpolation() -> Result<(), crate::error::StampError> {
        let input = r#"{
            "variables": {
                "prefix": "test-box",
                "custom_clean": "{{ clean_resource_name `app:name.v1` }}"
            },
            "builders": [
                {
                    "type": "null",
                    "name": "{{ user `prefix` }}-builder",
                    "image_name": "img-{{ user `prefix` }}",
                    "runtime_id": "{{ build `ID` }}"
                }
            ]
        }"#;

        let mut cli_vars = std::collections::HashMap::new();
        cli_vars.insert("prefix".to_string(), "prod-box".to_string());

        let tmpl = parse_json(input, &cli_vars)?;
        assert_eq!(tmpl.builders.len(), 1);
        assert_eq!(tmpl.builders[0].name, "prod-box-builder");
        assert_eq!(
            tmpl.builders[0].config.get("image_name"),
            Some(&"img-prod-box".to_string())
        );
        // Runtime macro {{ build `ID` }} preserved for build step
        assert_eq!(
            tmpl.builders[0].config.get("runtime_id"),
            Some(&"{{ build `ID` }}".to_string())
        );
        assert_eq!(
            tmpl.variables
                .get("custom_clean")
                .and_then(|v| v.default.as_deref()),
            Some("app-name-v1")
        );
        Ok(())
    }

    #[test]
    fn test_parse_json_hcp_packer_registry() -> Result<(), crate::error::StampError> {
        let input = r#"{
            "hcp_packer_registry": {
                "bucket_name": "json-bucket",
                "description": "Bucket from JSON",
                "channels": ["prod"],
                "bucket_labels": { "tier": "gold" }
            },
            "builders": [
                {
                    "type": "null",
                    "name": "test-builder"
                }
            ]
        }"#;
        let vars = std::collections::HashMap::new();
        let tmpl = parse_json(input, &vars)?;
        let packer_cfg = tmpl
            .packer
            .as_ref()
            .ok_or_else(|| StampError::TemplateValidation("packer config missing".to_string()))?;
        let reg = packer_cfg.hcp_packer_registry.as_ref().ok_or_else(|| {
            StampError::TemplateValidation("hcp_packer_registry missing".to_string())
        })?;
        assert_eq!(reg.bucket_name.0, "json-bucket");
        assert_eq!(reg.description.as_deref(), Some("Bucket from JSON"));
        assert_eq!(reg.channels, vec!["prod"]);
        assert_eq!(
            reg.bucket_labels.get("tier").map(String::as_str),
            Some("gold")
        );
        Ok(())
    }
}
