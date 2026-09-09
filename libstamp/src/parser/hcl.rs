#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![allow(clippy::collapsible_if)]
//! HCL parser for Packer templates.

use crate::error::StampError;
use crate::template::Template;
use hashicorp_configuration_language_rs::ast::expr::Expression;
use hashicorp_configuration_language_rs::ast::structure::Block;
use hashicorp_configuration_language_rs::parse::parser::Parser;

/// Parses an HCL2 configuration string into a `Template`.
///
/// # Errors
///
/// Returns a `StampError::Parse` if the HCL cannot be parsed.
fn extract_packer(
    block: &Block,
    plugins: &mut std::collections::HashMap<String, crate::template::PluginConfig>,
    packer_config: &mut Option<crate::template::PackerConfig>,
) {
    let mut config = crate::template::PackerConfig::default();

    // Process attributes
    if let Some(req_ver) = block.body.attributes.get("required_version") {
        config.required_version = Some(expr_to_string(&req_ver.expr));
    }

    for pb in &block.body.blocks {
        if pb.block_type == "required_plugins" {
            for (attr_name, attr) in &pb.body.attributes {
                if let Expression::Object(obj, _) = &attr.expr {
                    let mut version = String::new();
                    let mut source = String::new();
                    for (k, v) in obj {
                        let id = expr_to_string(k);
                        if id == "version" {
                            version = expr_to_string(v);
                        } else if id == "source" {
                            source = expr_to_string(v);
                        }
                    }
                    plugins.insert(
                        attr_name.clone(),
                        crate::template::PluginConfig { version, source },
                    );
                }
            }
        } else if pb.block_type == "hcp_packer_registry" {
            let mut reg = crate::template::HcpPackerRegistryConfig::default();
            for (attr_name, attr) in &pb.body.attributes {
                if attr_name == "bucket_name" {
                    reg.bucket_name = crate::template::BucketName(expr_to_string(&attr.expr));
                } else if attr_name == "description" {
                    reg.description = Some(expr_to_string(&attr.expr));
                } else if attr_name == "channels" || attr_name == "channel" {
                    reg.channels.extend(expr_to_vec_string(&attr.expr));
                } else if attr_name == "bucket_labels" {
                    reg.bucket_labels.extend(expr_to_map_string(&attr.expr));
                } else if attr_name == "build_labels" {
                    reg.build_labels.extend(expr_to_map_string(&attr.expr));
                } else if attr_name == "labels" {
                    reg.labels.extend(expr_to_map_string(&attr.expr));
                }
            }

            for inner in &pb.body.blocks {
                if inner.block_type == "labels" {
                    for (k, v) in &inner.body.attributes {
                        reg.labels.insert(k.clone(), expr_to_string(&v.expr));
                    }
                } else if inner.block_type == "bucket_labels" {
                    for (k, v) in &inner.body.attributes {
                        reg.bucket_labels.insert(k.clone(), expr_to_string(&v.expr));
                    }
                } else if inner.block_type == "build_labels" {
                    for (k, v) in &inner.body.attributes {
                        reg.build_labels.insert(k.clone(), expr_to_string(&v.expr));
                    }
                }
            }

            config.hcp_packer_registry = Some(reg);
        }
    }

    *packer_config = Some(config);
}

/// Extracts a build block into active provisioners, post-processors, and builds.
fn extract_build(
    block: &Block,
    builds: &mut Vec<crate::template::BuildConfig>,
    provs: &mut Vec<crate::template::ProvisionerConfig>,
    err_provs: &mut Vec<crate::template::ProvisionerConfig>,
    posts: &mut Vec<crate::template::PostProcessorConfig>,
    description: &mut Option<String>,
    builders: &mut Vec<crate::template::BuilderConfig>,
) -> Result<(), crate::error::StampError> {
    let mut build_desc = None;
    let mut build_name = None;
    let mut build_sources = Vec::new();

    for (name, attr) in &block.body.attributes {
        if name == "description" {
            if let Expression::String(s, _) = &attr.expr {
                build_desc = Some(s.clone());
                if description.is_none() {
                    *description = Some(s.clone());
                }
            }
        } else if name == "name" {
            build_name = Some(expr_to_string(&attr.expr));
        } else if name == "sources" {
            build_sources = expr_to_vec_string(&attr.expr);
        }
    }

    let mut current_build_provs = Vec::new();
    let mut current_build_err_provs = Vec::new();
    let mut current_build_posts = Vec::new();

    for inner in &block.body.blocks {
        if inner.block_type == "source" {
            let (b_type, b_name) = if inner.labels.len() == 2 {
                (inner.labels[0].clone(), inner.labels[1].clone())
            } else if inner.labels.len() == 1 {
                let parts: Vec<&str> = inner.labels[0].split('.').collect();
                if parts.len() == 2 {
                    (parts[0].to_string(), parts[1].to_string())
                } else {
                    (inner.labels[0].clone(), inner.labels[0].clone())
                }
            } else {
                return Err(crate::error::StampError::Parse(
                    "source block inside build must have one or two labels".to_string(),
                ));
            };

            let mut config = std::collections::HashMap::new();
            for (k, attr) in &inner.body.attributes {
                config.insert(k.clone(), expr_to_string(&attr.expr));
            }
            builders.push(crate::template::BuilderConfig {
                builder_type: b_type,
                name: b_name,
                config,
                depends_on: vec![],
            });
        } else if inner.block_type == "provisioner"
            || inner.block_type == "error-cleanup-provisioner"
            || inner.block_type == "post-processor"
        {
            if inner.labels.len() == 1 {
                let type_name = inner.labels[0].clone();
                let mut config = std::collections::HashMap::new();
                let mut only = Vec::new();
                let mut except = Vec::new();
                let mut keep_input_artifact = false;

                for (attr_name, attr) in &inner.body.attributes {
                    if attr_name == "only" {
                        only = expr_to_vec_string(&attr.expr);
                    } else if attr_name == "except" {
                        except = expr_to_vec_string(&attr.expr);
                    } else if attr_name == "keep_input_artifact" {
                        keep_input_artifact = expr_to_string(&attr.expr) == "true";
                    } else {
                        config.insert(attr_name.clone(), expr_to_string(&attr.expr));
                    }
                }

                for nested in &inner.body.blocks {
                    for (k, attr) in &nested.body.attributes {
                        config.insert(
                            format!("{}.{}", nested.block_type, k),
                            expr_to_string(&attr.expr),
                        );
                    }
                }

                if inner.block_type == "provisioner" {
                    let prov = crate::template::ProvisionerConfig {
                        provisioner_type: type_name,
                        only,
                        except,
                        config,
                    };
                    current_build_provs.push(prov.clone());
                    provs.push(prov);
                } else if inner.block_type == "post-processor" {
                    let post = crate::template::PostProcessorConfig {
                        post_processor_type: type_name,
                        keep_input_artifact,
                        only,
                        except,
                        config,
                    };
                    current_build_posts.push(post.clone());
                    posts.push(post);
                } else if inner.block_type == "error-cleanup-provisioner" {
                    let err_prov = crate::template::ProvisionerConfig {
                        provisioner_type: type_name,
                        only,
                        except,
                        config,
                    };
                    current_build_err_provs.push(err_prov.clone());
                    err_provs.push(err_prov);
                }
            } else {
                return Err(crate::error::StampError::Parse(format!(
                    "{} block must have exactly one label",
                    inner.block_type
                )));
            }
        } else if inner.block_type == "post-processors" {
            for nested in &inner.body.blocks {
                if nested.block_type == "post-processor" && nested.labels.len() == 1 {
                    let type_name = nested.labels[0].clone();
                    let mut config = std::collections::HashMap::new();
                    let mut only = Vec::new();
                    let mut except = Vec::new();
                    let mut keep_input_artifact = false;

                    for (attr_name, attr) in &nested.body.attributes {
                        if attr_name == "only" {
                            only = expr_to_vec_string(&attr.expr);
                        } else if attr_name == "except" {
                            except = expr_to_vec_string(&attr.expr);
                        } else if attr_name == "keep_input_artifact" {
                            keep_input_artifact = expr_to_string(&attr.expr) == "true";
                        } else {
                            config.insert(attr_name.clone(), expr_to_string(&attr.expr));
                        }
                    }

                    for sub in &nested.body.blocks {
                        for (k, attr) in &sub.body.attributes {
                            config.insert(
                                format!("{}.{}", sub.block_type, k),
                                expr_to_string(&attr.expr),
                            );
                        }
                    }

                    let post = crate::template::PostProcessorConfig {
                        post_processor_type: type_name,
                        keep_input_artifact,
                        only,
                        except,
                        config,
                    };
                    current_build_posts.push(post.clone());
                    posts.push(post);
                }
            }
        }
    }

    builds.push(crate::template::BuildConfig {
        name: build_name,
        description: build_desc,
        sources: build_sources,
        provisioners: current_build_provs,
        error_cleanup_provisioners: current_build_err_provs,
        post_processors: current_build_posts,
    });

    Ok(())
}

/// Converts an HCL expression into a string representation.
fn expr_to_string(expr: &Expression) -> String {
    match expr {
        Expression::String(s, _) => s.clone(),
        Expression::Variable(v, _) => v.clone(),
        Expression::Number(n, _) => n.0.to_string(),
        Expression::Bool(b, _) => b.to_string(),
        Expression::Template(parts, _) => {
            let mut out = String::new();
            for p in parts {
                match p {
                    hashicorp_configuration_language_rs::ast::expr::TemplatePart::Literal(s, _) => {
                        out.push_str(s);
                    }
                    hashicorp_configuration_language_rs::ast::expr::TemplatePart::Interpolation(
                        i,
                        _,
                    ) => out.push_str(&expr_to_string(i)),
                    _ => {}
                }
            }
            out
        }
        Expression::Tuple(parts, _) => parts
            .iter()
            .map(expr_to_string)
            .collect::<Vec<_>>()
            .join(", "),
        Expression::Object(entries, _) => {
            let pairs: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", expr_to_string(k), expr_to_string(v)))
                .collect();
            format!("{{{}}}", pairs.join(", "))
        }
        _ => String::new(),
    }
}

/// Converts an HCL expression into a vector of strings.
fn expr_to_vec_string(expr: &Expression) -> Vec<String> {
    match expr {
        Expression::Tuple(parts, _) => parts.iter().map(expr_to_string).collect(),
        Expression::String(s, _) => vec![s.clone()],
        other => {
            let s = expr_to_string(other);
            if s.is_empty() { Vec::new() } else { vec![s] }
        }
    }
}

/// Converts an HCL object expression into a HashMap of key-value string pairs.
fn expr_to_map_string(expr: &Expression) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Expression::Object(entries, _) = expr {
        for (k, v) in entries {
            map.insert(expr_to_string(k), expr_to_string(v));
        }
    }
    map
}

/// Internal documentation missing.
fn extract_test(block: &Block, tests: &mut Vec<crate::template::TestBlock>) {
    if block.labels.len() != 1 {
        return;
    }
    let name = block.labels[0].clone();
    let mut assertions = Vec::new();

    for inner in &block.body.blocks {
        if inner.block_type == "assert" {
            let condition = inner
                .body
                .attributes
                .get("condition")
                .map(|a| expr_to_string(&a.expr))
                .unwrap_or_default();

            let error_message = inner
                .body
                .attributes
                .get("error_message")
                .map(|a| expr_to_string(&a.expr));

            assertions.push(crate::template::TestAssertBlock {
                condition,
                error_message,
            });
        }
    }

    tests.push(crate::template::TestBlock { name, assertions });
}

/// Parses an HCL template string.
/// # Errors
/// Returns `StampError` if parsing fails.
pub fn parse_hcl<S: ::std::hash::BuildHasher>(
    input: &str,
    _vars: &std::collections::HashMap<String, String, S>,
) -> Result<Template, StampError> {
    let mut parser = Parser::new(input);
    let body = parser.parse_body();
    if parser.errors().has_errors() {
        return Err(crate::error::StampError::Parse(format!(
            "{:?}",
            parser.errors()
        )));
    }

    let mut builders = Vec::new();
    let mut provisioners = Vec::new();
    let mut error_cleanup_provisioners = Vec::new();
    let mut description = None;
    let mut required_plugins = std::collections::HashMap::new();
    let mut variables = std::collections::HashMap::new();
    let mut post_processors = Vec::new();
    let mut locals = std::collections::HashMap::new();
    let mut tests = Vec::new();
    let mut data_sources = Vec::new();
    let mut builds = Vec::new();
    let mut packer = None;

    for block in &body.blocks {
        if block.block_type == "packer" {
            extract_packer(block, &mut required_plugins, &mut packer);
        } else if block.block_type == "test" {
            extract_test(block, &mut tests);
        } else if block.block_type == "data" {
            if block.labels.len() >= 2 {
                let mut config = std::collections::HashMap::new();
                for (k, attr) in &block.body.attributes {
                    config.insert(k.clone(), expr_to_string(&attr.expr));
                }
                for inner_block in &block.body.blocks {
                    for (k, attr) in &inner_block.body.attributes {
                        config.insert(
                            format!("{}.{}", inner_block.block_type, k),
                            expr_to_string(&attr.expr),
                        );
                    }
                }
                data_sources.push(crate::template::DataSourceConfig {
                    source_type: block.labels[0].clone(),
                    name: block.labels[1].clone(),
                    config,
                });
            } else {
                return Err(crate::error::StampError::Parse(
                    "data block must have exactly two labels".to_string(),
                ));
            }
        } else if block.block_type == "source" {
            if block.labels.len() >= 2 {
                let config = block
                    .body
                    .attributes
                    .iter()
                    .map(|(k, attr)| (k.clone(), expr_to_string(&attr.expr)))
                    .collect();
                builders.push(crate::template::BuilderConfig {
                    builder_type: block.labels[0].clone(),
                    name: block.labels[1].clone(),
                    config,
                    depends_on: vec![],
                });
            } else {
                return Err(crate::error::StampError::Parse(
                    "source block must have exactly two labels".to_string(),
                ));
            }
        } else if block.block_type == "build" {
            extract_build(
                block,
                &mut builds,
                &mut provisioners,
                &mut error_cleanup_provisioners,
                &mut post_processors,
                &mut description,
                &mut builders,
            )?;
        } else if block.block_type == "variable" {
            if let Some(label) = block.labels.first() {
                let mut var = crate::template::VariableConfig::default();
                for (k, attr) in &block.body.attributes {
                    let v = expr_to_string(&attr.expr);
                    if k == "default" {
                        var.default = Some(v);
                    } else if k == "description" {
                        var.description = Some(v);
                    } else if k == "type" {
                        var.variable_type = Some(v);
                    } else if k == "sensitive" {
                        if v == "true" {
                            var.sensitive = Some(true);
                        } else if v == "false" {
                            var.sensitive = Some(false);
                        }
                    }
                }
                for v_block in &block.body.validations {
                    var.validations.push(crate::template::VariableValidation {
                        condition: expr_to_string(&v_block.condition),
                        error_message: expr_to_string(&v_block.error_message),
                    });
                }
                for inner_block in &block.body.blocks {
                    if inner_block.block_type == "validation" {
                        let mut val = crate::template::VariableValidation::default();
                        for (k, attr) in &inner_block.body.attributes {
                            let v = expr_to_string(&attr.expr);
                            if k == "condition" {
                                val.condition = v;
                            } else if k == "error_message" {
                                val.error_message = v;
                            }
                        }
                        var.validations.push(val);
                    }
                }
                variables.insert(label.clone(), var);
            }
        } else if block.block_type == "locals" {
            for (k, attr) in &block.body.attributes {
                locals.insert(k.clone(), expr_to_string(&attr.expr));
            }
        }
    }

    Ok(Template {
        description,
        builders,
        provisioners,
        error_cleanup_provisioners,
        required_plugins,
        variables,
        post_processors,
        locals,
        data_sources,
        packer,
        tests,
        builds,
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::match_wildcard_for_single_variants)]
    fn test_parse_hcl_valid() -> Result<(), crate::error::StampError> {
        let input = r#"
            source "amazon-ebs" "example" {
                ami_name = "test"
            }

            variable "foo" {
                default = "bar"
                description = "foo desc"
                type = string
                unknown = "val"
            }

            locals {
                bar = "baz"
            }

            build {
                description = "My template"
                sources = ["source.amazon-ebs.example"]

                provisioner "shell" {
                    inline = ["echo 'hello'"]
                }

                error-cleanup-provisioner "shell-local" {
                    inline = ["echo 'cleanup'"]
                }

                post-processor "docker-tag" {
                    repository = "foo"
                }
            }
        "#;

        let tmpl = parse_hcl(input, &std::collections::HashMap::new())?;
        assert_eq!(tmpl.description.as_deref(), Some("My template"));
        assert_eq!(tmpl.builders.len(), 1);
        assert_eq!(tmpl.builders[0].builder_type, "amazon-ebs");
        assert_eq!(tmpl.builders[0].name, "example");
        assert_eq!(tmpl.provisioners.len(), 1);
        assert_eq!(tmpl.provisioners[0].provisioner_type, "shell");
        assert_eq!(tmpl.error_cleanup_provisioners.len(), 1);
        assert_eq!(
            tmpl.error_cleanup_provisioners[0].provisioner_type,
            "shell-local"
        );
        assert_eq!(tmpl.post_processors.len(), 1);
        assert_eq!(tmpl.post_processors[0].post_processor_type, "docker-tag");

        assert_eq!(
            tmpl.variables
                .get("foo")
                .unwrap_or_else(|| panic!("failed"))
                .default
                .as_deref(),
            Some("bar")
        );
        assert_eq!(
            tmpl.variables
                .get("foo")
                .unwrap_or_else(|| panic!("failed"))
                .description
                .as_deref(),
            Some("foo desc")
        );
        assert_eq!(
            tmpl.variables
                .get("foo")
                .unwrap_or_else(|| panic!("failed"))
                .variable_type
                .as_deref(),
            Some("string")
        );

        assert_eq!(
            tmpl.locals.get("bar").unwrap_or_else(|| panic!("failed")),
            "baz"
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::match_wildcard_for_single_variants)]
    fn test_parse_hcl_invalid_syntax() -> Result<(), crate::error::StampError> {
        let input = r#"source "amazon-ebs" {"#;
        let Err(err) = parse_hcl(input, &std::collections::HashMap::new()) else {
            return Err(crate::error::StampError::Parse(
                "Expected error on invalid HCL".to_string(),
            ));
        };
        assert!(matches!(err, StampError::Parse(_)));
        Ok(())
    }

    #[test]
    #[allow(clippy::match_wildcard_for_single_variants)]
    fn test_parse_hcl_invalid_source_labels() -> Result<(), crate::error::StampError> {
        let input = r#"source "amazon-ebs" {}"#; // Only one label
        let Err(err) = parse_hcl(input, &std::collections::HashMap::new()) else {
            return Err(crate::error::StampError::Parse(
                "Expected error on invalid source labels".to_string(),
            ));
        };
        assert!(
            matches!(err, StampError::Parse(msg) if msg.contains("source block must have exactly two labels"))
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::match_wildcard_for_single_variants)]
    fn test_parse_hcl_invalid_provisioner_labels() -> Result<(), crate::error::StampError> {
        let input = r#"
            build {
                provisioner "shell" "extra" {}
            }
        "#;
        let Err(err) = parse_hcl(input, &std::collections::HashMap::new()) else {
            return Err(crate::error::StampError::Parse(
                "Expected error on invalid provisioner labels".to_string(),
            ));
        };
        assert!(
            matches!(err, StampError::Parse(msg) if msg.contains("provisioner block must have exactly one label"))
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::match_wildcard_for_single_variants)]
    fn test_parse_hcl_ignored_blocks() -> Result<(), crate::error::StampError> {
        let input = r#"
            build {
                unknown "test" {}
                description = 123
            }
            build {
                unknown "test2" {}
            }
        "#;
        let tmpl = parse_hcl(input, &std::collections::HashMap::new())?;
        assert_eq!(tmpl.provisioners.len(), 0);
        assert_eq!(tmpl.description.as_deref(), None); // Wait, description is parsed as "123"
        Ok(())
    }

    #[test]
    fn test_parse_hcl_description_as_string() -> Result<(), crate::error::StampError> {
        let input = r#"
            build {
                description = "this is a test description"
            }
        "#;
        let tmpl = parse_hcl(input, &std::collections::HashMap::new())?;
        assert_eq!(
            tmpl.description.as_deref(),
            Some("this is a test description")
        );
        Ok(())
    }

    #[test]
    fn test_parse_hcl_plugins() -> Result<(), crate::error::StampError> {
        let input = r#"
            packer {
                required_plugins {
                    my_plugin = {
                        version = "1.0"
                        source = "github.com/my/plugin"
                    }
                    my_plugin2 = {
                        version = 2
                        source = 3
                        unknown = "val"
                    }
                    my_plugin3 = "not_an_object"
                }
                unknown_block {}
            }
            variable "no_label" {}
        "#;
        let vars = std::collections::HashMap::new();
        let tpl = crate::parser::hcl::parse_hcl(input, &vars)?;
        assert_eq!(tpl.required_plugins.len(), 2); // Only my_plugin has string values for version/source
        assert_eq!(
            tpl.required_plugins
                .get("my_plugin")
                .unwrap_or_else(|| panic!("failed"))
                .version,
            "1.0"
        );
        assert_eq!(
            tpl.required_plugins
                .get("my_plugin")
                .unwrap_or_else(|| panic!("failed"))
                .source,
            "github.com/my/plugin"
        );
        Ok(())
    }

    #[test]
    fn test_parse_hcl_provisioner_no_labels() -> Result<(), crate::error::StampError> {
        let input = r"
            build {
                provisioner {
                }
            }
        ";
        let vars = std::collections::HashMap::new();
        let res = crate::parser::hcl::parse_hcl(input, &vars);
        assert!(res.is_err());
        Ok(())
    }

    #[test]
    fn test_parse_hcl_test_block() -> Result<(), crate::error::StampError> {
        let input = r#"
            test "my_test_suite" {
                assert {
                    condition = "1 == 1"
                    error_message = "Math is broken"
                }
                assert {
                    condition = "2 == 2"
                }
            }
            test {
                // missing label
                assert {
                    condition = "3 == 3"
                }
            }
        "#;
        let vars = std::collections::HashMap::new();
        let tpl = crate::parser::hcl::parse_hcl(input, &vars)?;
        assert_eq!(tpl.tests.len(), 1);
        let test = &tpl.tests[0];
        assert_eq!(test.name, "my_test_suite");
        assert_eq!(test.assertions.len(), 2);
        assert_eq!(test.assertions[0].condition, "1 == 1");
        assert_eq!(
            test.assertions[0].error_message.as_deref(),
            Some("Math is broken")
        );
        assert_eq!(test.assertions[1].condition, "2 == 2");
        assert_eq!(test.assertions[1].error_message.as_deref(), None);
        Ok(())
    }

    #[test]
    fn test_parse_hcl_data_sources() -> Result<(), crate::error::StampError> {
        let input = r#"
            data "amazon-ami" "ubuntu" {
                most_recent = true
                filters {
                    name = "ubuntu/images/*"
                }
            }
        "#;
        let vars = std::collections::HashMap::new();
        let tpl = crate::parser::hcl::parse_hcl(input, &vars)?;
        assert_eq!(tpl.data_sources.len(), 1);
        let ds = &tpl.data_sources[0];
        assert_eq!(ds.source_type, "amazon-ami");
        assert_eq!(ds.name, "ubuntu");
        assert_eq!(ds.config.get("most_recent"), Some(&"true".to_string()));
        assert_eq!(
            ds.config.get("filters.name"),
            Some(&"ubuntu/images/*".to_string())
        );
        Ok(())
    }

    #[test]
    fn test_parse_hcl_data_sources_invalid_label() {
        let input = r#"
            data "single_label" {}
        "#;
        let vars = std::collections::HashMap::new();
        let res = crate::parser::hcl::parse_hcl(input, &vars);
        assert!(res.is_err());
    }

    #[test]
    fn test_parse_hcl_multi_build_and_filters() -> Result<(), crate::error::StampError> {
        let input = r#"
            source "null" "first" {}
            source "null" "second" {}

            build {
                name = "build-a"
                sources = ["source.null.first"]

                provisioner "shell" {
                    only = ["null.first"]
                    except = ["null.second"]
                    inline = ["echo 'build-a'"]
                }

                post-processor "manifest" {
                    keep_input_artifact = true
                    only = ["null.first"]
                }
            }

            build {
                name = "build-b"
                sources = ["source.null.second"]

                source "null.inline_builder" {
                    custom_flag = "1"
                }

                provisioner "shell" {
                    inline = ["echo 'build-b'"]
                }
            }
        "#;
        let vars = std::collections::HashMap::new();
        let tpl = crate::parser::hcl::parse_hcl(input, &vars)?;
        assert_eq!(tpl.builds.len(), 2);
        assert_eq!(tpl.builds[0].name.as_deref(), Some("build-a"));
        assert_eq!(tpl.builds[0].sources, vec!["source.null.first"]);
        assert_eq!(tpl.builds[0].provisioners.len(), 1);
        assert_eq!(tpl.builds[0].provisioners[0].only, vec!["null.first"]);
        assert_eq!(tpl.builds[0].provisioners[0].except, vec!["null.second"]);
        assert_eq!(
            tpl.builds[0].provisioners[0].config.get("inline"),
            Some(&"echo 'build-a'".to_string())
        );
        assert_eq!(tpl.builds[0].post_processors.len(), 1);
        assert!(tpl.builds[0].post_processors[0].keep_input_artifact);

        assert_eq!(tpl.builds[1].name.as_deref(), Some("build-b"));
        assert_eq!(tpl.builds[1].sources, vec!["source.null.second"]);
        assert_eq!(tpl.builders.len(), 3);
        assert_eq!(tpl.builders[2].builder_type, "null");
        assert_eq!(tpl.builders[2].name, "inline_builder");
        assert_eq!(
            tpl.builders[2].config.get("custom_flag"),
            Some(&"1".to_string())
        );
        Ok(())
    }

    #[test]
    fn test_parse_hcl_variable_validation() -> Result<(), StampError> {
        let input = r#"
            variable "image_name" {
                type = "string"
                default = "my-image"
                description = "Name of image"
                sensitive = false
                validation {
                    condition = "length(var.image_name) > 4"
                    error_message = "The image_name value must be greater than 4 characters."
                }
            }
        "#;
        let vars = std::collections::HashMap::new();
        let tpl = crate::parser::hcl::parse_hcl(input, &vars)?;
        assert_eq!(tpl.variables.len(), 1);
        let var = tpl.variables.get("image_name").unwrap();
        assert_eq!(var.variable_type.as_deref(), Some("string"));
        assert_eq!(var.default.as_deref(), Some("my-image"));
        assert_eq!(var.description.as_deref(), Some("Name of image"));
        assert_eq!(var.sensitive, Some(false));
        assert_eq!(var.validations.len(), 1);
        assert_eq!(var.validations[0].condition, "length(var.image_name) > 4");
        assert_eq!(
            var.validations[0].error_message,
            "The image_name value must be greater than 4 characters."
        );
        Ok(())
    }

    #[test]
    fn test_parse_hcl_post_processors_sequence() -> Result<(), StampError> {
        let input = r#"
            source "null" "example" {}

            build {
                sources = ["source.null.example"]

                post-processors {
                    post-processor "checksum" {
                        checksum_types = ["sha256"]
                        keep_input_artifact = true
                    }
                    post-processor "compress" {
                        format = "tar.gz"
                        keep_input_artifact = false
                    }
                }
            }
        "#;
        let vars = std::collections::HashMap::new();
        let tpl = crate::parser::hcl::parse_hcl(input, &vars)?;
        assert_eq!(tpl.builds.len(), 1);
        let posts = &tpl.builds[0].post_processors;
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].post_processor_type, "checksum");
        assert!(posts[0].keep_input_artifact);
        assert_eq!(posts[1].post_processor_type, "compress");
        assert!(!posts[1].keep_input_artifact);
        Ok(())
    }

    #[test]
    fn test_parse_hcl_build_source_invalid_labels() {
        let input = r#"
            build {
                source {}
            }
        "#;
        let vars = std::collections::HashMap::new();
        let res = crate::parser::hcl::parse_hcl(input, &vars);
        assert!(res.is_err());
    }

    #[test]
    fn test_parse_hcl_hcp_packer_registry() -> Result<(), StampError> {
        let input = r#"
            packer {
                hcp_packer_registry {
                    bucket_name = "learn-packer-ubuntu"
                    description = "Ubuntu base image"
                    channels = ["production", "staging"]
                    bucket_labels = {
                        "owner" = "platform-team"
                    }
                    build_labels {
                        os = "ubuntu"
                    }
                    labels = {
                        "tier" = "base"
                    }
                }
            }
        "#;
        let vars = std::collections::HashMap::new();
        let tpl = crate::parser::hcl::parse_hcl(input, &vars)?;
        let packer_cfg = tpl.packer.as_ref().ok_or_else(|| {
            StampError::TemplateValidation("packer config should be present".to_string())
        })?;
        let reg = packer_cfg.hcp_packer_registry.as_ref().ok_or_else(|| {
            StampError::TemplateValidation("hcp_packer_registry should be present".to_string())
        })?;
        assert_eq!(reg.bucket_name.0, "learn-packer-ubuntu");
        assert_eq!(reg.description.as_deref(), Some("Ubuntu base image"));
        assert_eq!(reg.channels, vec!["production", "staging"]);
        assert_eq!(
            reg.bucket_labels.get("owner").map(String::as_str),
            Some("platform-team")
        );
        assert_eq!(
            reg.build_labels.get("os").map(String::as_str),
            Some("ubuntu")
        );
        assert_eq!(reg.labels.get("tier").map(String::as_str), Some("base"));
        Ok(())
    }
}
