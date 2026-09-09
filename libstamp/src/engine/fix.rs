#![cfg_attr(coverage_nightly, coverage(off))]
//! Template auto-fix engine for upgrading deprecated builder and provisioner attributes.

use crate::error::StampError;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// Configuration options for the `fix` command.
#[derive(Debug, Clone, Default)]
pub struct FixConfig {
    /// Whether to run template validation after applying fixes.
    pub validate: bool,
}

/// Upgrades deprecated builder attributes in a JSON builder object.
pub fn fix_builder_attributes(builder: &mut JsonValue) {
    if let Some(map) = builder.as_object_mut() {
        // 1. Upgrade iso_checksum_type and iso_checksum into iso_checksum: "type:hash"
        let checksum_type = map.remove("iso_checksum_type").and_then(|v| {
            if let JsonValue::String(s) = v {
                Some(s)
            } else {
                None
            }
        });

        if let Some(cs_type) = checksum_type {
            if let Some(cs_val) = map.get_mut("iso_checksum")
                && let JsonValue::String(cs_str) = cs_val
                && !cs_str.contains(':')
                && !cs_type.is_empty()
            {
                if cs_type.eq_ignore_ascii_case("none") {
                    *cs_str = "none".to_string();
                } else {
                    *cs_str = format!("{cs_type}:{cs_str}");
                }
            } else if cs_type.eq_ignore_ascii_case("none") {
                map.insert(
                    "iso_checksum".to_string(),
                    JsonValue::String("none".to_string()),
                );
            }
        }

        // 2. Normalize ami_block_device_mappings / launch_block_device_mappings
        for field in ["ami_block_device_mappings", "launch_block_device_mappings"] {
            if let Some(mappings) = map.get_mut(field).and_then(JsonValue::as_array_mut) {
                for dev in mappings {
                    if let Some(dev_map) = dev.as_object_mut() {
                        if let Some(enc) = dev_map.get_mut("encrypted")
                            && let JsonValue::String(s) = enc
                        {
                            if s.eq_ignore_ascii_case("true") {
                                *enc = JsonValue::Bool(true);
                            } else if s.eq_ignore_ascii_case("false") {
                                *enc = JsonValue::Bool(false);
                            }
                        }
                        if let Some(dot) = dev_map.get_mut("delete_on_termination")
                            && let JsonValue::String(s) = dot
                        {
                            if s.eq_ignore_ascii_case("true") {
                                *dot = JsonValue::Bool(true);
                            } else if s.eq_ignore_ascii_case("false") {
                                *dot = JsonValue::Bool(false);
                            }
                        }
                    }
                }
            }
        }

        // 3. Normalize disk_size strings to numbers if purely digits
        if let Some(ds) = map.get_mut("disk_size")
            && let JsonValue::String(s) = ds
            && let Ok(num) = s.parse::<u64>()
        {
            *ds = JsonValue::Number(num.into());
        }
    }
}

/// Upgrades deprecated provisioner attributes in a JSON provisioner object.
pub fn fix_provisioner_attributes(prov: &mut JsonValue) {
    if let Some(map) = prov.as_object_mut()
        && let Some(p_paths) = map.remove("playbook_paths")
        && !map.contains_key("playbook_file")
        && let JsonValue::Array(arr) = p_paths
        && let Some(first) = arr.into_iter().next()
    {
        map.insert("playbook_file".to_string(), first);
    }
}

/// Applies auto-fixes to a legacy JSON template string and returns the fixed JSON string.
///
/// # Errors
/// Returns `StampError::Parse` if parsing or serialization fails.
pub fn fix_json_template(content: &str) -> Result<String, StampError> {
    let mut root: JsonValue = serde_json::from_str(content)
        .map_err(|e| StampError::Parse(format!("Failed to parse JSON for fix: {e}")))?;

    if let Some(builders) = root.get_mut("builders").and_then(JsonValue::as_array_mut) {
        for b in builders {
            fix_builder_attributes(b);
        }
    }

    if let Some(provisioners) = root
        .get_mut("provisioners")
        .and_then(JsonValue::as_array_mut)
    {
        for p in provisioners {
            fix_provisioner_attributes(p);
        }
    }

    serde_json::to_string_pretty(&root).map_err(|e| StampError::Parse(e.to_string()))
}

/// Upgrades deprecated attributes in HCL source code and formats it canonically.
#[must_use]
pub fn fix_hcl_template(content: &str) -> String {
    let mut result = content.to_string();

    // Regex to fix iso_checksum_type + iso_checksum in HCL
    let re_cs_type = regex::Regex::new(r#"iso_checksum_type\s*=\s*"([^"]+)""#)
        .unwrap_or_else(|_| regex::Regex::new("").unwrap_or_else(|_| unreachable!()));
    let mut detected_type = None;
    if let Some(caps) = re_cs_type.captures(&result)
        && let Some(m) = caps.get(1)
    {
        detected_type = Some(m.as_str().to_string());
    }

    if let Some(cs_type) = detected_type {
        result = re_cs_type.replace_all(&result, "").to_string();
        let re_cs = regex::Regex::new(r#"iso_checksum\s*=\s*"([^":]+)""#)
            .unwrap_or_else(|_| regex::Regex::new("").unwrap_or_else(|_| unreachable!()));
        let replacement = format!("iso_checksum = \"{cs_type}:$1\"");
        result = re_cs.replace_all(&result, replacement.as_str()).to_string();
    }

    crate::engine::fmt::format_hcl_canonical(&result)
}

/// Fixes a template file in place according to configuration.
///
/// # Errors
/// Returns `StampError` if reading, fixing, writing, or validation fails.
pub fn fix_template(template_path: &str, config: &FixConfig) -> Result<(), StampError> {
    let content = std::fs::read_to_string(template_path).map_err(StampError::Io)?;
    let is_json = std::path::Path::new(template_path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));

    let fixed_content = if is_json {
        fix_json_template(&content)?
    } else {
        crate::parser::hcl::parse_hcl(&content, &HashMap::new())
            .map_err(|e| StampError::Parse(e.to_string()))?;
        fix_hcl_template(&content)
    };

    std::fs::write(template_path, &fixed_content).map_err(StampError::Io)?;

    if config.validate {
        let tmpl = if is_json {
            crate::parser::json::parse_json(&fixed_content, &HashMap::new())?
        } else {
            crate::parser::hcl::parse_hcl(&fixed_content, &HashMap::new())?
        };
        crate::engine::packer::validate(&tmpl)?;
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_fix_builder_attributes_checksum() {
        let mut builder = serde_json::json!({
            "type": "qemu",
            "iso_checksum_type": "sha256",
            "iso_checksum": "abcdef123456",
            "disk_size": "20000"
        });

        fix_builder_attributes(&mut builder);

        assert_eq!(
            builder["iso_checksum"],
            serde_json::json!("sha256:abcdef123456")
        );
        assert!(builder.get("iso_checksum_type").is_none());
        assert_eq!(builder["disk_size"], serde_json::json!(20000));
    }

    #[test]
    fn test_fix_builder_attributes_block_devices() {
        let mut builder = serde_json::json!({
            "type": "amazon-ebs",
            "ami_block_device_mappings": [
                {
                    "device_name": "/dev/sda1",
                    "encrypted": "true",
                    "delete_on_termination": "false"
                }
            ]
        });

        fix_builder_attributes(&mut builder);

        assert_eq!(
            builder["ami_block_device_mappings"][0]["encrypted"],
            serde_json::json!(true)
        );
        assert_eq!(
            builder["ami_block_device_mappings"][0]["delete_on_termination"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn test_fix_provisioner_attributes() {
        let mut prov = serde_json::json!({
            "type": "ansible",
            "playbook_paths": ["playbook.yml"]
        });

        fix_provisioner_attributes(&mut prov);

        assert_eq!(prov["playbook_file"], serde_json::json!("playbook.yml"));
        assert!(prov.get("playbook_paths").is_none());
    }

    #[test]
    fn test_fix_hcl_template() {
        let hcl = r#"
source "qemu" "test" {
  iso_checksum_type = "sha256"
  iso_checksum      = "abcdef"
}
"#;
        let fixed = fix_hcl_template(hcl);
        assert!(fixed.contains("iso_checksum = \"sha256:abcdef\""));
        assert!(!fixed.contains("iso_checksum_type"));
    }

    #[test]
    fn test_fix_template_file_json_and_validate() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("template.json");
        std::fs::write(
            &file_path,
            r#"{
                "builders": [
                    {
                        "type": "file",
                        "name": "my-file",
                        "target": "/tmp/out.txt",
                        "content": "hello",
                        "iso_checksum_type": "sha256",
                        "iso_checksum": "abcdef"
                    }
                ]
            }"#,
        )
        .unwrap();

        let config = FixConfig { validate: true };
        let res = fix_template(&file_path.to_string_lossy(), &config);
        assert!(res.is_ok());

        let read_back = std::fs::read_to_string(&file_path).unwrap();
        assert!(read_back.contains("sha256:abcdef"));
    }
}
