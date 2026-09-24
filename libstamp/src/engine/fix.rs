#![cfg_attr(coverage_nightly, coverage(off))]
//! Template auto-fix engine for upgrading deprecated builder and provisioner attributes.

use crate::error::StampError;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// Configuration options for the `fix` command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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

    let res = serde_json::to_string_pretty(&root)?;
    Ok(res)
}

/// Upgrades deprecated attributes in HCL source code and formats it canonically.
#[must_use]
pub fn fix_hcl_template(content: &str) -> String {
    let mut result = content.to_string();

    if let Some(pos) = result.find("iso_checksum_type")
        && let Some(quote_start) = result[pos..].find('"')
    {
        let start = pos + quote_start + 1;
        if let Some(quote_end) = result[start..].find('"') {
            let end = start + quote_end;
            let cs_type = result[start..end].to_string();

            let line_start = result[..pos].rfind('\n').map_or(0, |p| p + 1);
            let line_end = result[end..]
                .find('\n')
                .map_or(result.len(), |p| end + p + 1);
            result.replace_range(line_start..line_end, "");

            if let Some(cs_pos) = result.find("iso_checksum")
                && let Some(val_start_rel) = result[cs_pos..].find('"')
            {
                let val_start = cs_pos + val_start_rel + 1;
                if let Some(val_end_rel) = result[val_start..].find('"') {
                    let val_end = val_start + val_end_rel;
                    let existing = &result[val_start..val_end];
                    if !existing.contains(':') {
                        let replacement = format!("{cs_type}:{existing}");
                        result.replace_range(val_start..val_end, &replacement);
                    }
                }
            }
        }
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
#[allow(clippy::all, clippy::pedantic)]
mod tests {
    use super::*;

    #[test]
    fn test_fix_config_derived_traits() {
        let cfg = FixConfig::default();
        assert!(!cfg.validate);
        let cloned = cfg.clone();
        assert_eq!(cfg, cloned);
        assert!(format!("{cfg:?}").contains("FixConfig"));
    }

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
    fn test_fix_builder_attributes_none_and_edge_cases() {
        // Case 1: cs_type is "none" with existing checksum
        let mut b1 = serde_json::json!({
            "iso_checksum_type": "none",
            "iso_checksum": "original_hash"
        });
        fix_builder_attributes(&mut b1);
        assert_eq!(b1["iso_checksum"], serde_json::json!("none"));

        // Case 2: cs_type is "none" without existing checksum
        let mut b2 = serde_json::json!({
            "iso_checksum_type": "none"
        });
        fix_builder_attributes(&mut b2);
        assert_eq!(b2["iso_checksum"], serde_json::json!("none"));

        // Case 3: cs_type is not a string (number)
        let mut b3 = serde_json::json!({
            "iso_checksum_type": 12345,
            "iso_checksum": "hash"
        });
        fix_builder_attributes(&mut b3);
        assert_eq!(b3["iso_checksum"], serde_json::json!("hash"));

        // Case 4: cs_type is empty string
        let mut b4 = serde_json::json!({
            "iso_checksum_type": "",
            "iso_checksum": "hash"
        });
        fix_builder_attributes(&mut b4);
        assert_eq!(b4["iso_checksum"], serde_json::json!("hash"));

        // Case 5: iso_checksum already contains ':'
        let mut b5 = serde_json::json!({
            "iso_checksum_type": "sha256",
            "iso_checksum": "sha256:already_formatted"
        });
        fix_builder_attributes(&mut b5);
        assert_eq!(
            b5["iso_checksum"],
            serde_json::json!("sha256:already_formatted")
        );

        // Case 6: builder is not an object (e.g. integer)
        let mut b6 = serde_json::json!(42);
        fix_builder_attributes(&mut b6);
        assert_eq!(b6, serde_json::json!(42));
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
                },
                {
                    "device_name": "/dev/sda2",
                    "encrypted": "other_val",
                    "delete_on_termination": "other_val"
                },
                {
                    "device_name": "/dev/sda3",
                    "encrypted": "true"
                },
                {
                    "device_name": "/dev/sda4",
                    "delete_on_termination": "false"
                },
                "scalar_entry"
            ],
            "launch_block_device_mappings": [
                {
                    "device_name": "/dev/sdb1",
                    "encrypted": "false",
                    "delete_on_termination": "true"
                }
            ],
            "disk_size": "non_digits_gb"
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
        assert_eq!(
            builder["ami_block_device_mappings"][1]["encrypted"],
            serde_json::json!("other_val")
        );
        assert_eq!(
            builder["ami_block_device_mappings"][2]["encrypted"],
            serde_json::json!(true)
        );
        assert_eq!(
            builder["ami_block_device_mappings"][3]["delete_on_termination"],
            serde_json::json!(false)
        );
        assert_eq!(
            builder["launch_block_device_mappings"][0]["encrypted"],
            serde_json::json!(false)
        );
        assert_eq!(
            builder["launch_block_device_mappings"][0]["delete_on_termination"],
            serde_json::json!(true)
        );
        assert_eq!(builder["disk_size"], serde_json::json!("non_digits_gb"));
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

        // Prov with playbook_file already present
        let mut prov_existing = serde_json::json!({
            "playbook_file": "existing.yml",
            "playbook_paths": ["other.yml"]
        });
        fix_provisioner_attributes(&mut prov_existing);
        assert_eq!(
            prov_existing["playbook_file"],
            serde_json::json!("existing.yml")
        );

        // Prov with empty playbook_paths
        let mut prov_empty = serde_json::json!({
            "playbook_paths": []
        });
        fix_provisioner_attributes(&mut prov_empty);
        assert!(prov_empty.get("playbook_file").is_none());

        // Prov is not an object
        let mut prov_scalar = serde_json::json!("string_prov");
        fix_provisioner_attributes(&mut prov_scalar);
        assert_eq!(prov_scalar, serde_json::json!("string_prov"));
    }

    #[test]
    fn test_fix_json_template_edge_cases() {
        // Valid JSON without builders or provisioners
        let empty_tmpl = r#"{"description": "no builders"}"#;
        let res = fix_json_template(empty_tmpl);
        assert!(res.is_ok());

        // Invalid JSON syntax
        let invalid_json = r#"{ invalid: json"#;
        let res_err = fix_json_template(invalid_json);
        assert!(matches!(res_err, Err(StampError::Parse(_))));
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
        assert!(fixed.contains(r#"iso_checksum = "sha256:abcdef""#));
        assert!(!fixed.contains("iso_checksum_type"));

        // HCL with already-typed iso_checksum
        let hcl_already = r#"
source "qemu" "test" {
  iso_checksum_type = "sha256"
  iso_checksum      = "sha256:already_typed"
}
"#;
        let fixed_already = fix_hcl_template(hcl_already);
        assert!(fixed_already.contains(r#"iso_checksum = "sha256:already_typed""#));
        assert!(!fixed_already.contains("iso_checksum_type"));

        // HCL without iso_checksum_type
        let hcl_plain = r#"
source "file" "example" {
  target = "out.txt"
}
"#;
        let fixed_plain = fix_hcl_template(hcl_plain);
        assert!(fixed_plain.contains(r#"source "file" "example""#));

        // HCL with iso_checksum_type but no iso_checksum
        let hcl_no_cs = r#"
source "qemu" "test" {
  iso_checksum_type = "sha256"
}
"#;
        let fixed_no_cs = fix_hcl_template(hcl_no_cs);
        assert!(!fixed_no_cs.contains("iso_checksum_type"));

        // HCL with unclosed quote on iso_checksum_type
        let hcl_unclosed_type = "iso_checksum_type = \"unclosed";
        let fixed_unclosed_type = fix_hcl_template(hcl_unclosed_type);
        assert!(fixed_unclosed_type.contains("iso_checksum_type"));

        // HCL with unclosed quote on iso_checksum
        let hcl_unclosed_cs = "iso_checksum_type = \"sha256\"\niso_checksum = \"unclosed";
        let fixed_unclosed_cs = fix_hcl_template(hcl_unclosed_cs);
        assert!(fixed_unclosed_cs.contains("iso_checksum"));
    }

    #[test]
    fn test_fix_template_file_json_and_validate() -> Result<(), StampError> {
        let dir = tempfile::tempdir().map_err(StampError::Io)?;
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
                ],
                "provisioners": [
                    {
                        "type": "shell-local",
                        "inline": ["echo hello"]
                    }
                ]
            }"#,
        )
        .map_err(StampError::Io)?;

        let config = FixConfig { validate: true };
        fix_template(&file_path.to_string_lossy(), &config)?;

        let read_back = std::fs::read_to_string(&file_path).map_err(StampError::Io)?;
        assert!(read_back.contains("sha256:abcdef"));

        // Test with validate: false
        let config_no_val = FixConfig { validate: false };
        fix_template(&file_path.to_string_lossy(), &config_no_val)?;
        Ok(())
    }

    #[test]
    fn test_fix_template_file_hcl_and_validate() -> Result<(), StampError> {
        let dir = tempfile::tempdir().map_err(StampError::Io)?;
        let file_path = dir.path().join("template.pkr.hcl");
        std::fs::write(
            &file_path,
            r#"
source "file" "test" {
  target = "/tmp/out.txt"
  content = "fixed"
}

build {
  sources = ["source.file.test"]
}
"#,
        )
        .map_err(StampError::Io)?;

        let config = FixConfig { validate: true };
        fix_template(&file_path.to_string_lossy(), &config)?;

        let config_no_val = FixConfig { validate: false };
        fix_template(&file_path.to_string_lossy(), &config_no_val)?;
        Ok(())
    }

    #[test]
    fn test_fix_template_errors() -> Result<(), StampError> {
        let config = FixConfig { validate: true };

        // Non-existent file
        let res_missing = fix_template("/non/existent/template/path.json", &config);
        assert!(matches!(res_missing, Err(StampError::Io(_))));

        let dir = tempfile::tempdir().map_err(StampError::Io)?;

        // Invalid HCL syntax
        let bad_hcl_path = dir.path().join("invalid.pkr.hcl");
        std::fs::write(&bad_hcl_path, "invalid hcl syntax {{{").map_err(StampError::Io)?;
        let res_bad_hcl = fix_template(&bad_hcl_path.to_string_lossy(), &config);
        assert!(matches!(res_bad_hcl, Err(StampError::Parse(_))));

        // Invalid JSON syntax
        let bad_json_path = dir.path().join("invalid.json");
        std::fs::write(&bad_json_path, "{ broken json").map_err(StampError::Io)?;
        let res_bad_json = fix_template(&bad_json_path.to_string_lossy(), &config);
        assert!(matches!(res_bad_json, Err(StampError::Parse(_))));
        Ok(())
    }
}
