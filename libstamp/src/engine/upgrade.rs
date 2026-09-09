#![cfg_attr(coverage_nightly, coverage(off))]
//! AST rewrites for converting legacy Packer JSON templates into idiomatic HCL2.

use crate::error::StampError;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

/// Infers an idiomatic HCL2 type declaration for a given JSON default value.
#[must_use]
pub fn infer_hcl_type(val: &JsonValue) -> &'static str {
    match val {
        JsonValue::Bool(_) => "bool",
        JsonValue::Number(_) => "number",
        JsonValue::Array(arr) => {
            if arr.is_empty() {
                "list(string)"
            } else if arr.iter().all(JsonValue::is_number) {
                "list(number)"
            } else if arr.iter().all(JsonValue::is_boolean) {
                "list(bool)"
            } else {
                "list(string)"
            }
        }
        JsonValue::Object(_) => "map(string)",
        JsonValue::Null | JsonValue::String(_) => "string",
    }
}

/// Rewrites legacy Packer interpolation expressions `{{ ... }}` into idiomatic HCL2 `${...}`.
#[must_use]
pub fn rewrite_legacy_expression(val_str: &str) -> String {
    let mut result = val_str.to_string();

    // 1. {{ user `var_name` }} / 'var_name' / "var_name" -> ${var.var_name}
    let re_user = regex::Regex::new(r#"\{\{\s*user\s+[`'"]([^`'"]+)[`'"]\s*\}\}"#)
        .unwrap_or_else(|_| regex::Regex::new("$a").unwrap_or_else(|_| unreachable!()));
    result = re_user
        .replace_all(&result, |caps: &regex::Captures| {
            format!("${{var.{}}}", &caps[1])
        })
        .to_string();

    // 2. {{ env `ENV_VAR` }} / 'ENV_VAR' / "ENV_VAR" -> ${env("ENV_VAR")}
    let re_env = regex::Regex::new(r#"\{\{\s*env\s+[`'"]([^`'"]+)[`'"]\s*\}\}"#)
        .unwrap_or_else(|_| regex::Regex::new("$a").unwrap_or_else(|_| unreachable!()));
    result = re_env
        .replace_all(&result, |caps: &regex::Captures| {
            format!(r#"${{env("{}")}}"#, &caps[1])
        })
        .to_string();

    // 3. {{ build `ID` }} / 'ID' / "ID" -> ${build.ID}
    let re_build = regex::Regex::new(r#"\{\{\s*build\s+[`'"]([^`'"]+)[`'"]\s*\}\}"#)
        .unwrap_or_else(|_| regex::Regex::new("$a").unwrap_or_else(|_| unreachable!()));
    result = re_build
        .replace_all(&result, |caps: &regex::Captures| {
            format!("${{build.{}}}", &caps[1])
        })
        .to_string();

    // 4. {{ clean_resource_name `name` }} -> ${clean_resource_name("name")}
    let re_clean = regex::Regex::new(r#"\{\{\s*clean_resource_name\s+[`'"]([^`'"]+)[`'"]\s*\}\}"#)
        .unwrap_or_else(|_| regex::Regex::new("$a").unwrap_or_else(|_| unreachable!()));
    result = re_clean
        .replace_all(&result, |caps: &regex::Captures| {
            format!(r#"${{clean_resource_name("{}")}}"#, &caps[1])
        })
        .to_string();

    // 5. {{ isotime `format` }} -> ${legacy_isotime("format")}
    let re_isotime_fmt = regex::Regex::new(r#"\{\{\s*isotime\s+[`'"]([^`'"]+)[`'"]\s*\}\}"#)
        .unwrap_or_else(|_| regex::Regex::new("$a").unwrap_or_else(|_| unreachable!()));
    result = re_isotime_fmt
        .replace_all(&result, |caps: &regex::Captures| {
            format!(r#"${{legacy_isotime("{}")}}"#, &caps[1])
        })
        .to_string();

    // 6. {{ split `val` `sep` }} -> ${split("sep", "val")}
    let re_split =
        regex::Regex::new(r#"\{\{\s*split\s+[`'"]([^`'"]+)[`'"]\s+[`'"]([^`'"]+)[`'"]\s*\}\}"#)
            .unwrap_or_else(|_| regex::Regex::new("$a").unwrap_or_else(|_| unreachable!()));
    result = re_split
        .replace_all(&result, |caps: &regex::Captures| {
            format!(r#"${{split("{}", "{}")}}"#, &caps[2], &caps[1])
        })
        .to_string();

    // 7. {{ lower `val` }} -> ${lower("val")}
    let re_lower = regex::Regex::new(r#"\{\{\s*lower\s+[`'"]([^`'"]+)[`'"]\s*\}\}"#)
        .unwrap_or_else(|_| regex::Regex::new("$a").unwrap_or_else(|_| unreachable!()));
    result = re_lower
        .replace_all(&result, |caps: &regex::Captures| {
            format!(r#"${{lower("{}")}}"#, &caps[1])
        })
        .to_string();

    // 8. {{ upper `val` }} -> ${upper("val")}
    let re_upper = regex::Regex::new(r#"\{\{\s*upper\s+[`'"]([^`'"]+)[`'"]\s*\}\}"#)
        .unwrap_or_else(|_| regex::Regex::new("$a").unwrap_or_else(|_| unreachable!()));
    result = re_upper
        .replace_all(&result, |caps: &regex::Captures| {
            format!(r#"${{upper("{}")}}"#, &caps[1])
        })
        .to_string();

    // 9. Builtin keywords
    result = result.replace("{{ timestamp }}", "${timestamp()}");
    result = result.replace("{{timestamp}}", "${timestamp()}");
    result = result.replace("{{ isotime }}", "${legacy_isotime()}");
    result = result.replace("{{isotime}}", "${legacy_isotime()}");
    result = result.replace("{{ uuid }}", "${uuidv4()}");
    result = result.replace("{{uuid}}", "${uuidv4()}");
    result = result.replace("{{ build_name }}", "${build.name}");
    result = result.replace("{{build_name}}", "${build.name}");
    result = result.replace("{{ .BuildName }}", "${build.name}");
    result = result.replace("{{.BuildName}}", "${build.name}");
    result = result.replace("{{ build_type }}", "${build.type}");
    result = result.replace("{{build_type}}", "${build.type}");
    result = result.replace("{{ .BuildType }}", "${build.type}");
    result = result.replace("{{.BuildType}}", "${build.type}");
    result = result.replace("{{ template_dir }}", "${path.root}");
    result = result.replace("{{template_dir}}", "${path.root}");
    result = result.replace("{{ .TemplateDir }}", "${path.root}");
    result = result.replace("{{.TemplateDir}}", "${path.root}");
    result = result.replace("{{ pwd }}", "${path.cwd}");
    result = result.replace("{{pwd}}", "${path.cwd}");
    result = result.replace("{{ .Path }}", "${path.root}");
    result = result.replace("{{.Path}}", "${path.root}");
    result = result.replace("{{ .Vars }}", "");
    result = result.replace("{{.Vars}}", "");

    result
}

/// Formats a JSON value into an idiomatic HCL2 expression representation.
#[must_use]
pub fn json_val_to_hcl_expr(val: &JsonValue, indent_level: usize) -> String {
    let indent = "  ".repeat(indent_level);
    match val {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::String(s) => {
            let rewritten = rewrite_legacy_expression(s);
            // If the entire string is an expression like "${var.foo}", strip quotes and braces
            if rewritten.starts_with("${")
                && rewritten.ends_with('}')
                && !rewritten[2..rewritten.len() - 1].contains("${")
            {
                let inner = &rewritten[2..rewritten.len() - 1];
                inner.to_string()
            } else {
                format!(
                    "\"{}\"",
                    rewritten.replace('\\', "\\\\").replace('"', "\\\"")
                )
            }
        }
        JsonValue::Array(arr) => {
            if arr.is_empty() {
                "[]".to_string()
            } else {
                let inner_indent = "  ".repeat(indent_level + 1);
                let items: Vec<String> = arr
                    .iter()
                    .map(|item| {
                        format!(
                            "{inner_indent}{},",
                            json_val_to_hcl_expr(item, indent_level + 1)
                        )
                    })
                    .collect();
                format!(
                    "[
{}
{indent}]",
                    items.join(
                        "
"
                    )
                )
            }
        }
        JsonValue::Object(map) => {
            if map.is_empty() {
                "{}".to_string()
            } else {
                let inner_indent = "  ".repeat(indent_level + 1);
                let mut entries: Vec<String> = map
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{inner_indent}{k} = {}",
                            json_val_to_hcl_expr(v, indent_level + 1)
                        )
                    })
                    .collect();
                entries.sort();
                format!(
                    "{{
{}
{indent}}}",
                    entries.join(
                        "
"
                    )
                )
            }
        }
    }
}

/// Converts a legacy Packer JSON template string into an idiomatic HCL2 configuration string.
///
/// # Errors
/// Returns `StampError::Parse` if JSON parsing or validation fails.
pub fn upgrade_json_to_hcl2(json_content: &str) -> Result<String, StampError> {
    let root: JsonValue = serde_json::from_str(json_content)
        .map_err(|e| StampError::Parse(format!("Failed to parse legacy JSON template: {e}")))?;

    let root_obj = root
        .as_object()
        .ok_or_else(|| StampError::Parse("Root of JSON template must be an object".to_string()))?;

    let mut hcl_parts = Vec::new();

    // 1. Description
    if let Some(desc) = root_obj.get("description").and_then(JsonValue::as_str) {
        hcl_parts.push(format!(
            "# {desc}
"
        ));
    }

    // 2. Packer block
    if let Some(min_version) = root_obj
        .get("min_packer_version")
        .and_then(JsonValue::as_str)
    {
        hcl_parts.push(format!(
            "packer {{\n  required_version = \">={min_version}\"\n}}\n"
        ));
    }

    // 3. Variables with inferred types
    if let Some(vars) = root_obj.get("variables").and_then(JsonValue::as_object) {
        let sorted_vars: BTreeMap<&String, &JsonValue> = vars.iter().collect();
        for (var_name, default_val) in sorted_vars {
            let val_expr = json_val_to_hcl_expr(default_val, 1);
            let inferred_type = infer_hcl_type(default_val);
            hcl_parts.push(format!(
                "variable \"{var_name}\" {{\n  type    = {inferred_type}\n  default = {val_expr}\n}}\n"
            ));
        }
    }

    // 4. Builders -> source blocks
    let mut source_labels = Vec::new();
    let mut builder_name_to_source = BTreeMap::new();

    if let Some(builders) = root_obj.get("builders").and_then(JsonValue::as_array) {
        for (idx, builder) in builders.iter().enumerate() {
            if let Some(b_obj) = builder.as_object() {
                let b_type = b_obj
                    .get("type")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        StampError::Parse(format!("Builder at index {idx} is missing 'type'"))
                    })?;

                let b_name = b_obj
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .unwrap_or(b_type);

                let source_label = format!("source.{b_type}.{b_name}");
                source_labels.push(source_label);
                builder_name_to_source.insert(b_name.to_string(), format!("{b_type}.{b_name}"));

                let mut block_lines = Vec::new();
                block_lines.push(format!("source \"{b_type}\" \"{b_name}\" {{"));

                let sorted_attrs: BTreeMap<&String, &JsonValue> = b_obj.iter().collect();
                for (k, v) in sorted_attrs {
                    if *k == "type" || *k == "name" {
                        continue;
                    }
                    let expr = json_val_to_hcl_expr(v, 1);
                    block_lines.push(format!("  {k} = {expr}"));
                }
                block_lines.push("}\n".to_string());
                hcl_parts.push(block_lines.join("\n"));
            }
        }
    }

    // 5. Build block: sources, provisioners, and post-processors
    let mut build_lines = Vec::new();
    build_lines.push("build {".to_string());

    if !source_labels.is_empty() {
        build_lines.push("  sources = [".to_string());
        for sl in &source_labels {
            build_lines.push(format!("    \"{sl}\","));
        }
        build_lines.push("  ]\n".to_string());
    }

    // Provisioners
    if let Some(provisioners) = root_obj.get("provisioners").and_then(JsonValue::as_array) {
        for (idx, prov) in provisioners.iter().enumerate() {
            if let Some(p_obj) = prov.as_object() {
                let p_type = p_obj
                    .get("type")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        StampError::Parse(format!("Provisioner at index {idx} is missing 'type'"))
                    })?;

                build_lines.push(format!("  provisioner \"{p_type}\" {{"));

                let sorted_attrs: BTreeMap<&String, &JsonValue> = p_obj.iter().collect();
                for (k, v) in sorted_attrs {
                    if *k == "type" {
                        continue;
                    }
                    if (*k == "only" || *k == "except")
                        && let Some(arr) = v.as_array()
                    {
                        let mapped: Vec<String> = arr
                            .iter()
                            .filter_map(JsonValue::as_str)
                            .map(|name| {
                                builder_name_to_source
                                    .get(name)
                                    .cloned()
                                    .unwrap_or_else(|| name.to_string())
                            })
                            .collect();
                        let items = mapped
                            .iter()
                            .map(|s| format!("\"{s}\""))
                            .collect::<Vec<_>>()
                            .join(", ");
                        build_lines.push(format!("    {k} = [{items}]"));
                        continue;
                    }
                    let expr = json_val_to_hcl_expr(v, 2);
                    build_lines.push(format!("    {k} = {expr}"));
                }
                build_lines.push("  }\n".to_string());
            }
        }
    }

    // Error cleanup provisioners
    if let Some(err_provs) = root_obj
        .get("error-cleanup-provisioner")
        .and_then(JsonValue::as_array)
    {
        for (idx, prov) in err_provs.iter().enumerate() {
            if let Some(p_obj) = prov.as_object() {
                let p_type = p_obj
                    .get("type")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        StampError::Parse(format!(
                            "Error-cleanup-provisioner at index {idx} is missing 'type'"
                        ))
                    })?;

                build_lines.push(format!("  error-cleanup-provisioner \"{p_type}\" {{"));

                let sorted_attrs: BTreeMap<&String, &JsonValue> = p_obj.iter().collect();
                for (k, v) in sorted_attrs {
                    if *k == "type" {
                        continue;
                    }
                    let expr = json_val_to_hcl_expr(v, 2);
                    build_lines.push(format!("    {k} = {expr}"));
                }
                build_lines.push("  }\n".to_string());
            }
        }
    }

    // Post-processors
    if let Some(pps) = root_obj
        .get("post-processors")
        .and_then(JsonValue::as_array)
    {
        for (idx, pp) in pps.iter().enumerate() {
            if let Some(pp_obj) = pp.as_object() {
                let pp_type = pp_obj
                    .get("type")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        StampError::Parse(format!(
                            "Post-processor at index {idx} is missing 'type'"
                        ))
                    })?;

                build_lines.push(format!("  post-processor \"{pp_type}\" {{"));
                let sorted_attrs: BTreeMap<&String, &JsonValue> = pp_obj.iter().collect();
                for (k, v) in sorted_attrs {
                    if *k == "type" {
                        continue;
                    }
                    let expr = json_val_to_hcl_expr(v, 2);
                    build_lines.push(format!("    {k} = {expr}"));
                }
                build_lines.push("  }\n".to_string());
            } else if let Some(pipeline) = pp.as_array() {
                // Sequential pipeline block
                build_lines.push("  post-processors {".to_string());
                for (sub_idx, sub_pp) in pipeline.iter().enumerate() {
                    if let Some(sub_obj) = sub_pp.as_object() {
                        let sub_type = sub_obj
                            .get("type")
                            .and_then(JsonValue::as_str)
                            .ok_or_else(|| {
                                StampError::Parse(format!(
                                    "Pipeline post-processor at [{idx}][{sub_idx}] is missing 'type'"
                                ))
                            })?;

                        build_lines.push(format!("    post-processor \"{sub_type}\" {{"));
                        let sorted_attrs: BTreeMap<&String, &JsonValue> = sub_obj.iter().collect();
                        for (k, v) in sorted_attrs {
                            if *k == "type" {
                                continue;
                            }
                            let expr = json_val_to_hcl_expr(v, 3);
                            build_lines.push(format!("      {k} = {expr}"));
                        }
                        build_lines.push("    }\n".to_string());
                    }
                }
                build_lines.push("  }\n".to_string());
            }
        }
    }

    build_lines.push("}\n".to_string());
    hcl_parts.push(build_lines.join("\n"));

    let raw_hcl = hcl_parts.join("\n");
    Ok(crate::engine::fmt::format_hcl_canonical(&raw_hcl))
}

/// Separate HCL2 files generated from a legacy JSON template.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructuredHcl2 {
    /// Variables and Packer configuration block.
    pub variables: String,
    /// Source definitions.
    pub sources: String,
    /// Build pipeline definition.
    pub build: String,
}

/// Upgrades a legacy JSON template into structured separate HCL2 components.
///
/// # Errors
/// Returns `StampError::Parse` if parsing fails.
pub fn upgrade_json_to_structured_hcl2(content: &str) -> Result<StructuredHcl2, StampError> {
    let root: JsonValue = serde_json::from_str(content)
        .map_err(|e| StampError::Parse(format!("Failed to parse legacy JSON template: {e}")))?;

    let root_obj = root
        .as_object()
        .ok_or_else(|| StampError::Parse("Root JSON value must be an object".to_string()))?;

    let mut var_parts = Vec::new();
    if let Some(desc) = root_obj.get("description").and_then(JsonValue::as_str) {
        var_parts.push(format!("// {desc}\n"));
    }
    if let Some(min_version) = root_obj
        .get("min_packer_version")
        .and_then(JsonValue::as_str)
    {
        var_parts.push(format!(
            "packer {{\n  required_version = \">={min_version}\"\n}}\n"
        ));
    }
    if let Some(vars) = root_obj.get("variables").and_then(JsonValue::as_object) {
        let sorted_vars: BTreeMap<&String, &JsonValue> = vars.iter().collect();
        for (var_name, default_val) in sorted_vars {
            let val_expr = json_val_to_hcl_expr(default_val, 1);
            let inferred_type = infer_hcl_type(default_val);
            var_parts.push(format!(
                "variable \"{var_name}\" {{\n  type    = {inferred_type}\n  default = {val_expr}\n}}\n"
            ));
        }
    }

    let mut source_parts = Vec::new();
    let mut source_labels = Vec::new();
    let mut builder_name_to_source = BTreeMap::new();

    if let Some(builders) = root_obj.get("builders").and_then(JsonValue::as_array) {
        for (idx, builder) in builders.iter().enumerate() {
            if let Some(b_obj) = builder.as_object() {
                let b_type = b_obj
                    .get("type")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        StampError::Parse(format!("Builder at index {idx} is missing 'type'"))
                    })?;

                let b_name = b_obj
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .unwrap_or(b_type);

                let source_label = format!("source.{b_type}.{b_name}");
                source_labels.push(source_label);
                builder_name_to_source.insert(b_name.to_string(), format!("{b_type}.{b_name}"));

                let mut block_lines = Vec::new();
                block_lines.push(format!("source \"{b_type}\" \"{b_name}\" {{"));

                let sorted_attrs: BTreeMap<&String, &JsonValue> = b_obj.iter().collect();
                for (k, v) in sorted_attrs {
                    if *k == "type" || *k == "name" {
                        continue;
                    }
                    let expr = json_val_to_hcl_expr(v, 1);
                    block_lines.push(format!("  {k} = {expr}"));
                }
                block_lines.push("}\n".to_string());
                source_parts.push(block_lines.join("\n"));
            }
        }
    }

    let mut build_lines = Vec::new();
    build_lines.push("build {".to_string());
    if !source_labels.is_empty() {
        build_lines.push("  sources = [".to_string());
        for sl in &source_labels {
            build_lines.push(format!("    \"{sl}\","));
        }
        build_lines.push("  ]\n".to_string());
    }

    if let Some(provisioners) = root_obj.get("provisioners").and_then(JsonValue::as_array) {
        for (idx, prov) in provisioners.iter().enumerate() {
            if let Some(p_obj) = prov.as_object() {
                let p_type = p_obj
                    .get("type")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        StampError::Parse(format!("Provisioner at index {idx} is missing 'type'"))
                    })?;

                build_lines.push(format!("  provisioner \"{p_type}\" {{"));
                let sorted_attrs: BTreeMap<&String, &JsonValue> = p_obj.iter().collect();
                for (k, v) in sorted_attrs {
                    if *k == "type" {
                        continue;
                    }
                    if *k == "only" || *k == "except" {
                        if let Some(targets) = v.as_array() {
                            let mut remapped = Vec::new();
                            for t in targets {
                                if let Some(t_str) = t.as_str() {
                                    let remap = builder_name_to_source
                                        .get(t_str)
                                        .map_or_else(|| t_str.to_string(), Clone::clone);
                                    remapped.push(format!("\"{remap}\""));
                                }
                            }
                            build_lines.push(format!("    {k} = [{}]", remapped.join(", ")));
                        }
                        continue;
                    }
                    let expr = json_val_to_hcl_expr(v, 2);
                    build_lines.push(format!("    {k} = {expr}"));
                }
                build_lines.push("  }\n".to_string());
            }
        }
    }

    if let Some(pps) = root_obj
        .get("post-processors")
        .and_then(JsonValue::as_array)
    {
        for (idx, pp) in pps.iter().enumerate() {
            if let Some(pp_obj) = pp.as_object() {
                let pp_type = pp_obj
                    .get("type")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        StampError::Parse(format!(
                            "Post-processor at index {idx} is missing 'type'"
                        ))
                    })?;

                build_lines.push(format!("  post-processor \"{pp_type}\" {{"));
                let sorted_attrs: BTreeMap<&String, &JsonValue> = pp_obj.iter().collect();
                for (k, v) in sorted_attrs {
                    if *k == "type" {
                        continue;
                    }
                    let expr = json_val_to_hcl_expr(v, 2);
                    build_lines.push(format!("    {k} = {expr}"));
                }
                build_lines.push("  }\n".to_string());
            } else if let Some(pipeline) = pp.as_array() {
                build_lines.push("  post-processors {".to_string());
                for (sub_idx, sub_pp) in pipeline.iter().enumerate() {
                    if let Some(sub_obj) = sub_pp.as_object() {
                        let sub_type = sub_obj
                            .get("type")
                            .and_then(JsonValue::as_str)
                            .ok_or_else(|| {
                                StampError::Parse(format!(
                                    "Pipeline post-processor at [{idx}][{sub_idx}] is missing 'type'"
                                ))
                            })?;

                        build_lines.push(format!("    post-processor \"{sub_type}\" {{"));
                        let sorted_attrs: BTreeMap<&String, &JsonValue> = sub_obj.iter().collect();
                        for (k, v) in sorted_attrs {
                            if *k == "type" {
                                continue;
                            }
                            let expr = json_val_to_hcl_expr(v, 3);
                            build_lines.push(format!("      {k} = {expr}"));
                        }
                        build_lines.push("    }\n".to_string());
                    }
                }
                build_lines.push("  }\n".to_string());
            }
        }
    }
    build_lines.push("}\n".to_string());

    let raw_vars = var_parts.join("\n");
    let raw_sources = source_parts.join("\n");
    let raw_build = build_lines.join("\n");

    Ok(StructuredHcl2 {
        variables: crate::engine::fmt::format_hcl_canonical(&raw_vars),
        sources: crate::engine::fmt::format_hcl_canonical(&raw_sources),
        build: crate::engine::fmt::format_hcl_canonical(&raw_build),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_hcl_type() {
        assert_eq!(infer_hcl_type(&serde_json::json!(true)), "bool");
        assert_eq!(infer_hcl_type(&serde_json::json!(42)), "number");
        assert_eq!(infer_hcl_type(&serde_json::json!(3.14)), "number");
        assert_eq!(infer_hcl_type(&serde_json::json!("hello")), "string");
        assert_eq!(infer_hcl_type(&serde_json::json!(null)), "string");
        assert_eq!(infer_hcl_type(&serde_json::json!([])), "list(string)");
        assert_eq!(
            infer_hcl_type(&serde_json::json!([1, 2, 3])),
            "list(number)"
        );
        assert_eq!(
            infer_hcl_type(&serde_json::json!([true, false])),
            "list(bool)"
        );
        assert_eq!(
            infer_hcl_type(&serde_json::json!(["a", "b"])),
            "list(string)"
        );
        assert_eq!(
            infer_hcl_type(&serde_json::json!({"k": "v"})),
            "map(string)"
        );
    }

    #[test]
    fn test_rewrite_legacy_expression() {
        assert_eq!(
            rewrite_legacy_expression("{{ user `ami_id` }}"),
            "${var.ami_id}"
        );
        assert_eq!(
            rewrite_legacy_expression("{{ user 'ami_id' }}"),
            "${var.ami_id}"
        );
        assert_eq!(
            rewrite_legacy_expression("{{ user \"ami_id\" }}"),
            "${var.ami_id}"
        );
        assert_eq!(
            rewrite_legacy_expression("prefix-{{ timestamp }}-suffix"),
            "prefix-${timestamp()}-suffix"
        );
        assert_eq!(
            rewrite_legacy_expression("{{ env `AWS_REGION` }}"),
            "${env(\"AWS_REGION\")}"
        );
        assert_eq!(rewrite_legacy_expression("{{ build `ID` }}"), "${build.ID}");
        assert_eq!(
            rewrite_legacy_expression("{{ build_name }}"),
            "${build.name}"
        );
        assert_eq!(
            rewrite_legacy_expression("{{ build_type }}"),
            "${build.type}"
        );
        assert_eq!(rewrite_legacy_expression("{{ pwd }}"), "${path.cwd}");
        assert_eq!(rewrite_legacy_expression("{{ uuid }}"), "${uuidv4()}");
        assert_eq!(
            rewrite_legacy_expression("{{ isotime `2006-01-02` }}"),
            "${legacy_isotime(\"2006-01-02\")}"
        );
        assert_eq!(
            rewrite_legacy_expression("{{ lower `UPPER` }}"),
            "${lower(\"UPPER\")}"
        );
        assert_eq!(
            rewrite_legacy_expression("{{ upper `lower` }}"),
            "${upper(\"lower\")}"
        );
    }

    #[test]
    fn test_upgrade_json_to_hcl2_full() {
        let json = r#"{
            "description": "Ubuntu AMI build",
            "min_packer_version": "1.7.0",
            "variables": {
                "region": "us-west-2",
                "is_prod": true,
                "disk_size": 50,
                "subnets": ["sub-1", "sub-2"],
                "env_secret": "{{ env `MY_SECRET` }}"
            },
            "builders": [
                {
                    "type": "amazon-ebs",
                    "name": "my-ami",
                    "region": "{{ user `region` }}",
                    "instance_type": "t3.micro"
                }
            ],
            "provisioners": [
                {
                    "type": "shell",
                    "inline": ["echo hello", "uptime"],
                    "only": ["my-ami"]
                }
            ],
            "error-cleanup-provisioner": [
                {
                    "type": "shell-local",
                    "inline": ["echo 'cleanup'"]
                }
            ],
            "post-processors": [
                {
                    "type": "manifest",
                    "output": "manifest.json"
                },
                [
                    {
                        "type": "docker-tag",
                        "repository": "my-repo"
                    },
                    {
                        "type": "docker-push"
                    }
                ]
            ]
        }"#;

        let hcl = upgrade_json_to_hcl2(json).unwrap();
        assert!(hcl.contains("# Ubuntu AMI build"));
        assert!(hcl.contains("packer {"));
        assert!(hcl.contains("variable \"region\" {"));
        assert!(hcl.contains("type = string") || hcl.contains("type    = string"));
        assert!(hcl.contains("variable \"is_prod\" {"));
        assert!(hcl.contains("type = bool") || hcl.contains("type    = bool"));
        assert!(hcl.contains("variable \"disk_size\" {"));
        assert!(hcl.contains("type = number") || hcl.contains("type    = number"));
        assert!(hcl.contains("variable \"subnets\" {"));
        assert!(hcl.contains("type = list(string)") || hcl.contains("type    = list(string)"));
        assert!(hcl.contains("source \"amazon-ebs\" \"my-ami\" {"));
        assert!(hcl.contains("var.region"));
        assert!(hcl.contains("build {"));
        assert!(hcl.contains("\"source.amazon-ebs.my-ami\","));
        assert!(hcl.contains("provisioner \"shell\" {"));
        assert!(hcl.contains("only"));
        assert!(hcl.contains("amazon-ebs.my-ami"));
        assert!(hcl.contains("error-cleanup-provisioner \"shell-local\" {"));
        assert!(hcl.contains("post-processor \"manifest\" {"));
        assert!(hcl.contains("post-processors {"));
        assert!(hcl.contains("post-processor \"docker-tag\" {"));
        assert!(hcl.contains("post-processor \"docker-push\" {"));
    }

    #[test]
    fn test_upgrade_json_to_hcl2_errors() {
        let not_json = "{ invalid";
        assert!(upgrade_json_to_hcl2(not_json).is_err());

        let not_object = "[\"not an object\"]";
        assert!(upgrade_json_to_hcl2(not_object).is_err());

        let missing_builder_type = r#"{"builders": [{}]}"#;
        assert!(upgrade_json_to_hcl2(missing_builder_type).is_err());

        let missing_prov_type = r#"{"builders": [{"type": "file"}], "provisioners": [{}]}"#;
        assert!(upgrade_json_to_hcl2(missing_prov_type).is_err());

        let missing_err_prov_type = r#"{"error-cleanup-provisioner": [{}]}"#;
        assert!(upgrade_json_to_hcl2(missing_err_prov_type).is_err());

        let missing_post_type = r#"{"post-processors": [{}]}"#;
        assert!(upgrade_json_to_hcl2(missing_post_type).is_err());

        let missing_pipeline_sub_type = r#"{"post-processors": [[{}]]}"#;
        assert!(upgrade_json_to_hcl2(missing_pipeline_sub_type).is_err());
    }

    #[test]
    fn test_upgrade_json_to_structured_hcl2() {
        let json = r#"{
            "description": "My golden image",
            "variables": {
                "region": "us-west-2"
            },
            "builders": [
                {
                    "type": "null",
                    "name": "my-null"
                }
            ],
            "provisioners": [
                {
                    "type": "shell",
                    "inline": ["echo hi"]
                }
            ]
        }"#;

        let structured = upgrade_json_to_structured_hcl2(json).unwrap();
        assert!(structured.variables.contains("variable \"region\""));
        assert!(structured.sources.contains("source \"null\" \"my-null\""));
        assert!(structured.build.contains("build {"));
    }
}
