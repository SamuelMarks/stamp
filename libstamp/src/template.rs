#![cfg_attr(coverage_nightly, coverage(off))]
//! Unified template model for Stamp.
//!
//! This module defines the `Template` struct and its components, which form the
//! internal representation of both Packer templates.

use serde::{Deserialize, Serialize};

/// The unified template structure.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    /// A generic description of the template.
    pub description: Option<String>,
    /// The builders defined in the template.
    #[serde(default)]
    pub builders: Vec<BuilderConfig>,
    /// The provisioners defined in the template.
    #[serde(default)]
    pub provisioners: Vec<ProvisionerConfig>,
    /// The error cleanup provisioners defined in the template.
    #[serde(default)]
    pub error_cleanup_provisioners: Vec<ProvisionerConfig>,
    /// The post-processors defined in the template.
    #[serde(default)]
    pub post_processors: Vec<PostProcessorConfig>,
    /// The locals defined in the template.
    #[serde(default)]
    pub locals: std::collections::HashMap<String, String>,
    /// The variables defined in the template.
    #[serde(default)]
    pub variables: std::collections::HashMap<String, VariableConfig>,
    /// The data sources defined in the template.
    #[serde(default)]
    pub data_sources: Vec<DataSourceConfig>,
    /// Required plugins defined in the template.
    #[serde(default)]
    pub required_plugins: std::collections::HashMap<String, PluginConfig>,
    /// Packer core configuration.
    #[serde(default)]
    pub packer: Option<PackerConfig>,
    /// The test blocks defined in the template.
    #[serde(default)]
    pub tests: Vec<TestBlock>,
    /// The builds defined in the template.
    #[serde(default)]
    pub builds: Vec<BuildConfig>,
}

/// A strongly-typed configuration for a build block in HCL2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BuildConfig {
    /// The name of this build block if provided.
    pub name: Option<String>,
    /// The description of this build block.
    pub description: Option<String>,
    /// The source labels targeted by this build block.
    #[serde(default)]
    pub sources: Vec<String>,
    /// The provisioners defined for this build block.
    #[serde(default)]
    pub provisioners: Vec<ProvisionerConfig>,
    /// The error cleanup provisioners defined for this build block.
    #[serde(default)]
    pub error_cleanup_provisioners: Vec<ProvisionerConfig>,
    /// The post-processors defined for this build block.
    #[serde(default)]
    pub post_processors: Vec<PostProcessorConfig>,
}

/// A block representing a single test suite in HCL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestBlock {
    /// The name of the test suite.
    pub name: String,
    /// Assertions to evaluate in this test.
    #[serde(default)]
    pub assertions: Vec<TestAssertBlock>,
}

/// A block representing a single assertion inside a test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestAssertBlock {
    /// The assertion condition (usually an expression string).
    pub condition: String,
    /// An optional error message if the condition fails.
    pub error_message: Option<String>,
}

/// Details of a test failure, structured strongly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestFailureDetails {
    /// The name of the test that failed.
    pub test_name: String,
    /// The specific assertion condition that failed.
    pub failed_condition: String,
    /// Optional user-provided error message.
    pub error_message: Option<String>,
}

/// Configuration for the Packer block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PackerConfig {
    /// Required Core version constraint
    pub required_version: Option<String>,
    /// HCP Packer Registry configuration
    pub hcp_packer_registry: Option<HcpPackerRegistryConfig>,
}

/// A strongly-typed name for an HCP bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BucketName(pub String);

impl std::fmt::Display for BucketName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Configuration for HCP Packer Registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HcpPackerRegistryConfig {
    /// Bucket name
    pub bucket_name: BucketName,
    /// Description
    pub description: Option<String>,
    /// Labels
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
    /// Bucket-level labels
    #[serde(default)]
    pub bucket_labels: std::collections::HashMap<String, String>,
    /// Build-level labels
    #[serde(default)]
    pub build_labels: std::collections::HashMap<String, String>,
    /// Release channels to assign the iteration to upon completion
    #[serde(default)]
    pub channels: Vec<String>,
}

/// A strongly-typed configuration for a required plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginConfig {
    /// The plugin version constraint (e.g. ">= 1.0.0").
    pub version: String,
    /// The plugin source (e.g. "github.com/hashicorp/amazon").
    pub source: String,
}

/// A strongly-typed configuration for a builder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BuilderConfig {
    /// The type of the builder (e.g., "amazon-ebs", "virtualbox-iso").
    pub builder_type: String,
    /// A unique name for this builder instance.
    pub name: String,
    /// Other builders this builder depends on.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Builder configuration fields.
    #[serde(flatten)]
    pub config: std::collections::HashMap<String, String>,
}

/// A strongly-typed configuration for a provisioner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProvisionerConfig {
    /// The type of the provisioner (e.g., "shell", "ansible").
    pub provisioner_type: String,
    /// Optional list of builders this provisioner only applies to.
    #[serde(default)]
    pub only: Vec<String>,
    /// Optional list of builders this provisioner does not apply to.
    #[serde(default)]
    pub except: Vec<String>,
    /// Provisioner configuration fields.
    #[serde(flatten)]
    pub config: std::collections::HashMap<String, String>,
}

/// A strongly-typed configuration for a post-processor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PostProcessorConfig {
    /// The type of the post-processor.
    pub post_processor_type: String,
    /// Keep input artifacts.
    #[serde(default)]
    pub keep_input_artifact: bool,
    /// Optional list of builders this post-processor only applies to.
    #[serde(default)]
    pub only: Vec<String>,
    /// Optional list of builders this post-processor does not apply to.
    #[serde(default)]
    pub except: Vec<String>,
    /// Post-processor configuration fields.
    #[serde(flatten)]
    pub config: std::collections::HashMap<String, String>,
}

/// A validation rule block inside an HCL2 variable definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VariableValidation {
    /// The boolean condition expression to validate.
    pub condition: String,
    /// The error message returned if validation fails.
    pub error_message: String,
}

/// A strongly-typed configuration for a variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VariableConfig {
    /// The type of the variable (e.g., "string", "bool").
    #[serde(rename = "type")]
    pub variable_type: Option<String>,
    /// The description of the variable.
    pub description: Option<String>,
    /// The default value of the variable.
    pub default: Option<String>,
    /// Whether the variable is sensitive and should be redacted.
    pub sensitive: Option<bool>,
    /// Validation rules specified for this variable.
    #[serde(default)]
    pub validations: Vec<VariableValidation>,
}

/// A strongly-typed configuration for a data source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DataSourceConfig {
    /// The type of the data source (e.g., "amazon-ami").
    pub source_type: String,
    /// A unique name for this data source instance.
    pub name: String,
    /// Data source configuration fields.
    #[serde(flatten)]
    pub config: std::collections::HashMap<String, String>,
}

impl Template {
    /// Merges another template into this one, combining all components.
    pub fn merge(&mut self, other: Self) {
        if other.description.is_some() {
            self.description = other.description;
        }
        self.builders.extend(other.builders);
        self.provisioners.extend(other.provisioners);
        self.error_cleanup_provisioners
            .extend(other.error_cleanup_provisioners);
        self.post_processors.extend(other.post_processors);
        self.locals.extend(other.locals);
        self.variables.extend(other.variables);
        self.data_sources.extend(other.data_sources);
        self.required_plugins.extend(other.required_plugins);
        if other.packer.is_some() {
            self.packer = other.packer;
        }
        self.tests.extend(other.tests);
        self.builds.extend(other.builds);
    }
}

/// Loads and merges template files or directories into a single unified `Template`.
///
/// If a path is a directory, it scans for `*.pkr.hcl` and `*.pkr.json` files (falling back to `*.hcl` and `*.json`)
/// in alphabetical order, parsing and merging them all.
///
/// # Errors
/// Returns `StampError` if reading or parsing any template file fails, or if a path does not exist.
pub fn load_templates<P: AsRef<std::path::Path>, S: std::hash::BuildHasher>(
    paths: &[P],
    vars: &std::collections::HashMap<String, String, S>,
) -> Result<Template, crate::error::StampError> {
    if paths.is_empty() {
        return Err(crate::error::StampError::Validation(
            "At least one template file or directory must be specified".to_string(),
        ));
    }

    let mut unified = Template::default();
    let mut files_to_load = Vec::new();

    for p in paths {
        let path = p.as_ref();
        if !path.exists() {
            return Err(crate::error::StampError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Template path '{}' does not exist", path.display()),
            )));
        }

        if path.is_dir() {
            let mut dir_entries = Vec::new();
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let file_path = entry.path();
                if file_path.is_file() {
                    let is_template =
                        file_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|name| {
                                let lower = name.to_ascii_lowercase();
                                lower.ends_with(".pkr.hcl")
                                    || lower.ends_with(".pkr.json")
                                    || file_path.extension().is_some_and(|ext| {
                                        ext.eq_ignore_ascii_case("hcl")
                                            || ext.eq_ignore_ascii_case("json")
                                    })
                            });
                    if is_template {
                        dir_entries.push(file_path);
                    }
                }
            }
            dir_entries.sort();
            if dir_entries.is_empty() {
                return Err(crate::error::StampError::Validation(format!(
                    "No valid template files (*.pkr.hcl, *.pkr.json, *.hcl, *.json) found in directory '{}'",
                    path.display()
                )));
            }
            files_to_load.extend(dir_entries);
        } else {
            files_to_load.push(path.to_path_buf());
        }
    }

    for file in files_to_load {
        let content = std::fs::read_to_string(&file)?;
        let tmpl = if file
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            crate::parser::json::parse_json(&content, vars)?
        } else {
            crate::parser::hcl::parse_hcl(&content, vars)?
        };
        unified.merge(tmpl);
    }

    Ok(unified)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_traits() {
        let tmpl = Template::default();
        assert_eq!(tmpl.clone(), tmpl);
        assert_eq!(format!("{:?}", tmpl), format!("{:?}", tmpl));

        let test_block = TestBlock {
            name: "n".into(),
            assertions: vec![],
        };
        assert_eq!(test_block.clone(), test_block);
        assert_eq!(format!("{:?}", test_block), format!("{:?}", test_block));

        let assert_block = TestAssertBlock {
            condition: "c".into(),
            error_message: None,
        };
        assert_eq!(assert_block.clone(), assert_block);
        assert_eq!(format!("{:?}", assert_block), format!("{:?}", assert_block));

        let failure = TestFailureDetails {
            test_name: "t".into(),
            failed_condition: "c".into(),
            error_message: None,
        };
        assert_eq!(failure.clone(), failure);
        assert_eq!(format!("{:?}", failure), format!("{:?}", failure));

        let packer = PackerConfig::default();
        assert_eq!(packer.clone(), packer);
        assert_eq!(format!("{:?}", packer), format!("{:?}", packer));

        let bucket = BucketName("b".into());
        assert_eq!(bucket.clone(), bucket);
        assert_eq!(format!("{:?}", bucket), format!("{:?}", bucket));

        let hcp = HcpPackerRegistryConfig::default();
        assert_eq!(hcp.clone(), hcp);
        assert_eq!(format!("{:?}", hcp), format!("{:?}", hcp));

        let plugin = PluginConfig::default();
        assert_eq!(plugin.clone(), plugin);
        assert_eq!(format!("{:?}", plugin), format!("{:?}", plugin));

        let builder = BuilderConfig::default();
        assert_eq!(builder.clone(), builder);
        assert_eq!(format!("{:?}", builder), format!("{:?}", builder));

        let prov = ProvisionerConfig::default();
        assert_eq!(prov.clone(), prov);
        assert_eq!(format!("{:?}", prov), format!("{:?}", prov));

        let post = PostProcessorConfig::default();
        assert_eq!(post.clone(), post);
        assert_eq!(format!("{:?}", post), format!("{:?}", post));

        let var = VariableConfig::default();
        assert_eq!(var.clone(), var);
        assert_eq!(format!("{:?}", var), format!("{:?}", var));

        let ds = DataSourceConfig::default();
        assert_eq!(ds.clone(), ds);
        assert_eq!(format!("{:?}", ds), format!("{:?}", ds));

        let s = serde_json::to_string(&tmpl).unwrap_or_default();
        let _d: Template = serde_json::from_str(&s).unwrap_or_default();
    }

    #[test]
    fn test_template_default() {
        let tmpl = Template::default();
        assert_eq!(tmpl.description, None);
        assert!(tmpl.builders.is_empty());
        assert!(tmpl.provisioners.is_empty());
        assert!(tmpl.error_cleanup_provisioners.is_empty());
        assert!(tmpl.post_processors.is_empty());
        assert!(tmpl.locals.is_empty());
    }

    #[test]
    fn test_bucket_name_display() {
        let b = crate::template::BucketName("test-bucket".to_string());
        assert_eq!(b.to_string(), "test-bucket");
    }

    #[test]
    fn test_packer_config() {
        let registry = HcpPackerRegistryConfig {
            bucket_name: crate::template::BucketName("my-bucket".to_string()),
            description: Some("test description".to_string()),
            labels: std::collections::HashMap::new(),
            bucket_labels: std::collections::HashMap::from([(
                "team".to_string(),
                "infra".to_string(),
            )]),
            build_labels: std::collections::HashMap::from([(
                "os".to_string(),
                "linux".to_string(),
            )]),
            channels: vec!["production".to_string()],
        };
        let packer = PackerConfig {
            required_version: Some(">= 1.0.0".to_string()),
            hcp_packer_registry: Some(registry),
        };
        assert_eq!(packer.required_version.as_deref(), Some(">= 1.0.0"));
        let empty_packer = PackerConfig::default();
        for opt in [
            &packer.hcp_packer_registry,
            &empty_packer.hcp_packer_registry,
        ] {
            let val = match opt {
                Some(r) => r.bucket_name.to_string(),
                None => "none".to_string(),
            };
            if opt.is_some() {
                assert_eq!(val, "my-bucket");
            } else {
                assert_eq!(val, "none");
            }
        }
        let _ = HcpPackerRegistryConfig::default();
        let _ = PluginConfig::default();
        let _ = BucketName::default();
    }

    #[test]
    fn test_builder_config() {
        let builder = BuilderConfig {
            builder_type: "null".to_string(),
            name: "my-builder".to_string(),
            ..Default::default()
        };
        assert_eq!(builder.builder_type, "null");
        assert_eq!(builder.name, "my-builder");
    }

    #[test]
    fn test_provisioner_config() {
        let provisioner = ProvisionerConfig {
            provisioner_type: "shell".to_string(),
            only: vec!["amazon-ebs.example".to_string()],
            except: vec!["docker.ubuntu".to_string()],
            config: std::collections::HashMap::new(),
        };
        assert_eq!(provisioner.provisioner_type, "shell");
        assert_eq!(provisioner.only.len(), 1);
        assert_eq!(provisioner.except.len(), 1);

        let post = PostProcessorConfig {
            post_processor_type: "manifest".to_string(),
            keep_input_artifact: true,
            only: vec!["amazon-ebs.example".to_string()],
            except: vec![],
            config: std::collections::HashMap::new(),
        };
        assert_eq!(post.post_processor_type, "manifest");
        assert!(post.keep_input_artifact);

        let build = BuildConfig {
            name: Some("test-build".to_string()),
            description: Some("desc".to_string()),
            sources: vec!["source.amazon-ebs.example".to_string()],
            provisioners: vec![provisioner],
            error_cleanup_provisioners: vec![],
            post_processors: vec![post],
        };
        assert_eq!(build.name.as_deref(), Some("test-build"));
        assert_eq!(build.sources.len(), 1);
        assert_eq!(build.provisioners.len(), 1);
        assert_eq!(build.post_processors.len(), 1);
    }

    #[test]
    fn test_template_merge() {
        let mut t1 = Template {
            description: Some("t1".to_string()),
            locals: [("k1".to_string(), "v1".to_string())].into(),
            ..Default::default()
        };
        let t2 = Template {
            description: Some("t2".to_string()),
            locals: [("k2".to_string(), "v2".to_string())].into(),
            ..Default::default()
        };
        t1.merge(t2);
        assert_eq!(t1.description.as_deref(), Some("t2"));
        assert_eq!(t1.locals.len(), 2);
    }

    #[test]
    fn test_load_templates_directory_and_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let f1 = temp_dir.path().join("base.pkr.json");
        std::fs::write(&f1, r#"{"builders": [{"type": "null"}]}"#).unwrap();

        let f2 = temp_dir.path().join("extra.pkr.hcl");
        std::fs::write(&f2, "locals {\n  env = \"dev\"\n}\n").unwrap();

        let vars = std::collections::HashMap::new();

        // 1. Directory scanning
        if let Ok(tmpl) = load_templates(&[temp_dir.path()], &vars) {
            assert_eq!(tmpl.builders.len(), 1);
            assert_eq!(tmpl.locals.get("env").map(|s| s.as_str()), Some("dev"));
        }

        // 2. Multi-file loading
        if let Ok(tmpl_files) = load_templates(&[&f1, &f2], &vars) {
            assert_eq!(tmpl_files.builders.len(), 1);
            assert_eq!(
                tmpl_files.locals.get("env").map(|s| s.as_str()),
                Some("dev")
            );
        }

        // 3. Error cases
        assert!(load_templates::<std::path::PathBuf, _>(&[], &vars).is_err());
        assert!(load_templates(&[temp_dir.path().join("nonexistent")], &vars).is_err());

        if let Ok(empty_dir) = tempfile::tempdir() {
            assert!(load_templates(&[empty_dir.path()], &vars).is_err());
        }
    }
}
