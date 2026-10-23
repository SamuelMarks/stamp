//! Unified template model for Stamp.
//!
//! This module defines the `Template` struct and its components, which form the
//! internal representation of both Packer templates.

use serde::{Deserialize, Serialize};

/// The unified template structure.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    /// A generic description of the template.
    pub description: Option<String>,
    /// The root path of the template directory if loaded from disk.
    #[serde(skip)]
    pub root_path: Option<std::path::PathBuf>,
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
    /// Raw AST expressions for locals defined in the template.
    #[serde(skip)]
    pub local_expressions: std::collections::HashMap<
        String,
        hashicorp_configuration_language_rs::ast::expr::Expression,
    >,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
    /// Raw AST expressions for dynamic runtime evaluation.
    #[serde(skip)]
    pub expressions: std::collections::HashMap<
        String,
        hashicorp_configuration_language_rs::ast::expr::Expression,
    >,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
    /// Raw AST expressions for dynamic runtime evaluation.
    #[serde(skip)]
    pub expressions: std::collections::HashMap<
        String,
        hashicorp_configuration_language_rs::ast::expr::Expression,
    >,
}

/// A strongly-typed configuration for a provisioner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
    /// Raw AST expressions for dynamic runtime evaluation.
    #[serde(skip)]
    pub expressions: std::collections::HashMap<
        String,
        hashicorp_configuration_language_rs::ast::expr::Expression,
    >,
}

/// A strongly-typed configuration for a post-processor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
    /// Raw AST expressions for dynamic runtime evaluation.
    #[serde(skip)]
    pub expressions: std::collections::HashMap<
        String,
        hashicorp_configuration_language_rs::ast::expr::Expression,
    >,
}

/// A validation rule block inside an HCL2 variable definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VariableValidation {
    /// The boolean condition expression to validate.
    pub condition: String,
    /// The error message returned if validation fails.
    pub error_message: String,
    /// Raw condition expression AST if available.
    #[serde(skip)]
    pub condition_expr: Option<hashicorp_configuration_language_rs::ast::expr::Expression>,
    /// Raw error message expression AST if available.
    #[serde(skip)]
    pub error_message_expr: Option<hashicorp_configuration_language_rs::ast::expr::Expression>,
}

/// A strongly-typed configuration for a variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataSourceConfig {
    /// The type of the data source (e.g., "amazon-ami").
    pub source_type: String,
    /// A unique name for this data source instance.
    pub name: String,
    /// Data source configuration fields.
    #[serde(flatten)]
    pub config: std::collections::HashMap<String, String>,
    /// Raw AST expressions for dynamic runtime evaluation.
    #[serde(skip)]
    pub expressions: std::collections::HashMap<
        String,
        hashicorp_configuration_language_rs::ast::expr::Expression,
    >,
}

/// Trait providing strongly-typed domain accessors over raw template configuration maps.
pub trait TypedConfigAccess {
    /// Returns the underlying key-value configuration map.
    fn raw_config(&self) -> &std::collections::HashMap<String, String>;

    /// Retrieves a trimmed [`NonEmptyString`](crate::types::NonEmptyString) for `key`.
    fn get_non_empty(&self, key: &str) -> Option<crate::types::NonEmptyString> {
        self.raw_config()
            .get(key)
            .and_then(|v| crate::types::NonEmptyString::try_from(v.clone()).ok())
    }

    /// Retrieves a [`SecretString`](crate::types::SecretString) for `key` if present and non-empty.
    fn get_secret(&self, key: &str) -> Option<crate::types::SecretString> {
        self.raw_config()
            .get(key)
            .filter(|v| !v.is_empty())
            .cloned()
            .map(crate::types::SecretString::new)
    }

    /// Retrieves a [`FilePath`](crate::types::FilePath) for `key` if present and non-empty.
    fn get_file_path(&self, key: &str) -> Option<crate::types::FilePath> {
        self.raw_config()
            .get(key)
            .filter(|v| !v.trim().is_empty())
            .map(|v| crate::types::FilePath::new(std::path::PathBuf::from(v)))
    }

    /// Retrieves a [`Port`](crate::types::Port) for `key` if present and valid `u16`.
    fn get_port(&self, key: &str) -> Option<crate::types::Port> {
        self.raw_config()
            .get(key)
            .and_then(|v| v.parse::<u16>().ok())
            .map(crate::types::Port::new)
    }

    /// Retrieves a [`MemoryMb`](crate::types::MemoryMb) for `key` if present and valid `u64`.
    fn get_memory_mb(&self, key: &str) -> Option<crate::types::MemoryMb> {
        self.raw_config()
            .get(key)
            .and_then(|v| v.parse::<u64>().ok())
            .map(crate::types::MemoryMb::new)
    }

    /// Retrieves a [`DiskSizeMb`](crate::types::DiskSizeMb) for `key` if present and valid `u64`.
    fn get_disk_size_mb(&self, key: &str) -> Option<crate::types::DiskSizeMb> {
        self.raw_config()
            .get(key)
            .and_then(|v| v.parse::<u64>().ok())
            .map(crate::types::DiskSizeMb::new)
    }

    /// Retrieves a [`CpuCount`](crate::types::CpuCount) for `key` if present and valid non-zero `u32`.
    fn get_cpu_count(&self, key: &str) -> Option<crate::types::CpuCount> {
        self.raw_config()
            .get(key)
            .and_then(|v| v.parse::<u32>().ok())
            .and_then(|n| crate::types::CpuCount::new(n).ok())
    }

    /// Retrieves a boolean flag for `key` (`"true"` or `"1"` -> `true`, `"false"` or `"0"` -> `false`).
    fn get_bool(&self, key: &str) -> Option<bool> {
        self.raw_config()
            .get(key)
            .and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" => Some(false),
                _ => None,
            })
    }
}

impl TypedConfigAccess for BuilderConfig {
    fn raw_config(&self) -> &std::collections::HashMap<String, String> {
        &self.config
    }
}

impl TypedConfigAccess for ProvisionerConfig {
    fn raw_config(&self) -> &std::collections::HashMap<String, String> {
        &self.config
    }
}

impl TypedConfigAccess for PostProcessorConfig {
    fn raw_config(&self) -> &std::collections::HashMap<String, String> {
        &self.config
    }
}

impl TypedConfigAccess for DataSourceConfig {
    fn raw_config(&self) -> &std::collections::HashMap<String, String> {
        &self.config
    }
}

impl Template {
    /// Merges another template into this one, combining all components.
    pub fn merge(&mut self, other: Self) {
        if other.description.is_some() {
            self.description = other.description;
        }
        if other.root_path.is_some() {
            self.root_path = other.root_path;
        }
        self.builders.extend(other.builders);
        self.provisioners.extend(other.provisioners);
        self.error_cleanup_provisioners
            .extend(other.error_cleanup_provisioners);
        self.post_processors.extend(other.post_processors);
        self.locals.extend(other.locals);
        self.local_expressions.extend(other.local_expressions);
        self.variables.extend(other.variables);
        self.data_sources.extend(other.data_sources);
        self.required_plugins.extend(other.required_plugins);
        if other.packer.is_some() {
            self.packer = other.packer;
        }
        self.tests.extend(other.tests);
        self.builds.extend(other.builds);
    }

    /// Loads and merges template files or directories into a single unified `Template`,
    /// performing multi-file HCL2 AST pre-merging across all `*.pkr.hcl` files before
    /// evaluating expressions.
    ///
    /// # Arguments
    /// * `paths` - Collection of file and/or directory paths to load.
    /// * `vars` - Map of variable overrides provided via CLI flags or var-files.
    ///
    /// # Errors
    /// Returns `StampError::Validation` if no paths are provided or a directory contains no valid templates.
    /// Returns `StampError::Io` if reading files or traversing directories fails.
    /// Returns `StampError::Parse` if HCL syntax diagnostics or AST merge collisions occur.
    pub fn from_files_or_dirs<P: AsRef<std::path::Path>, S: std::hash::BuildHasher>(
        paths: &[P],
        vars: &std::collections::HashMap<String, String, S>,
    ) -> Result<Self, crate::error::StampError> {
        if paths.is_empty() {
            return Err(crate::error::StampError::Validation(
                "At least one template file or directory must be specified".to_string(),
            ));
        }

        let mut unified = Self::default();
        let mut hcl_files: Vec<std::path::PathBuf> = Vec::new();
        let mut json_files: Vec<std::path::PathBuf> = Vec::new();
        let mut root_path_candidate: Option<std::path::PathBuf> = None;

        for p in paths {
            let path = p.as_ref();
            if !path.exists() {
                return Err(crate::error::StampError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Template path '{}' does not exist", path.display()),
                )));
            }

            if root_path_candidate.is_none() {
                if path.is_dir() {
                    root_path_candidate = Some(path.to_path_buf());
                } else if let Some(parent) = path.parent() {
                    root_path_candidate = Some(parent.to_path_buf());
                }
            }

            if path.is_dir() {
                let mut dir_entries = Vec::new();
                for entry in std::fs::read_dir(path)? {
                    let entry = entry?;
                    let file_path = entry.path();
                    if file_path.is_file() {
                        let is_template = file_path
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
                for f in dir_entries {
                    if f.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                    {
                        json_files.push(f);
                    } else {
                        hcl_files.push(f);
                    }
                }
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            {
                json_files.push(path.to_path_buf());
            } else {
                hcl_files.push(path.to_path_buf());
            }
        }

        unified.root_path = root_path_candidate;

        // Multi-file HCL AST pre-merging across all *.pkr.hcl and *.hcl files
        if !hcl_files.is_empty() {
            let mut file_tuples = Vec::with_capacity(hcl_files.len());
            for hcl_file in &hcl_files {
                let content = std::fs::read_to_string(hcl_file)?;
                file_tuples.push((hcl_file.to_string_lossy().to_string(), content));
            }
            let files_ref: Vec<(&str, &str)> = file_tuples
                .iter()
                .map(|(name, content)| (name.as_str(), content.as_str()))
                .collect();

            let merged_body =
                hashicorp_configuration_language_rs::parse::merge::merge_files(&files_ref)
                    .map_err(|diags| crate::error::StampError::Parse(diags.to_string()))?;

            let hcl_template = crate::parser::hcl::parse_body_to_template(&merged_body)?;
            unified.merge(hcl_template);
        }

        // Process any legacy JSON templates
        for json_file in json_files {
            let content = std::fs::read_to_string(&json_file)?;
            let json_tmpl = crate::parser::json::parse_json(&content, vars)?;
            unified.merge(json_tmpl);
        }

        Ok(unified)
    }
}

/// Loads and merges template files or directories into a single unified `Template`.
///
/// If a path is a directory, it scans for `*.pkr.hcl` and `*.pkr.json` files (falling back to `*.hcl` and `*.json`)
/// in alphabetical order, parsing and merging them all.
///
/// # Arguments
/// * `paths` - Collection of file and/or directory paths to load.
/// * `vars` - Map of variable overrides provided via CLI flags or var-files.
///
/// # Errors
/// Returns `StampError` if reading or parsing any template file fails, or if a path does not exist.
pub fn load_templates<P: AsRef<std::path::Path>, S: std::hash::BuildHasher>(
    paths: &[P],
    vars: &std::collections::HashMap<String, String, S>,
) -> Result<Template, crate::error::StampError> {
    Template::from_files_or_dirs(paths, vars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StampError;

    #[test]
    fn test_derived_traits() -> Result<(), StampError> {
        let tmpl = Template {
            description: Some("sample".to_string()),
            ..Default::default()
        };
        let cloned = tmpl.clone();
        assert_eq!(cloned.description.as_deref(), Some("sample"));
        assert!(format!("{tmpl:?}").contains("sample"));

        let test_block = TestBlock {
            name: "n".into(),
            assertions: vec![TestAssertBlock {
                condition: "c".into(),
                error_message: Some("err".into()),
            }],
        };
        let test_block_json = serde_json::to_string(&test_block)?;
        let decoded_test_block: TestBlock = serde_json::from_str(&test_block_json)?;
        assert_eq!(decoded_test_block, test_block);

        let failure = TestFailureDetails {
            test_name: "t".into(),
            failed_condition: "c".into(),
            error_message: None,
        };
        let failure_json = serde_json::to_string(&failure)?;
        let decoded_failure: TestFailureDetails = serde_json::from_str(&failure_json)?;
        assert_eq!(decoded_failure, failure);

        let packer = PackerConfig::default();
        assert!(packer.required_version.is_none());

        let bucket = BucketName("b".into());
        assert_eq!(bucket.0, "b");

        let hcp = HcpPackerRegistryConfig::default();
        assert_eq!(hcp.channels.len(), 0);

        let plugin = PluginConfig::default();
        assert_eq!(plugin.version, "");

        let builder = BuilderConfig::default();
        assert_eq!(builder.name, "");

        let prov = ProvisionerConfig::default();
        assert_eq!(prov.provisioner_type, "");

        let post = PostProcessorConfig::default();
        assert!(!post.keep_input_artifact);

        let var = VariableConfig::default();
        assert!(var.default.is_none());

        let ds = DataSourceConfig::default();
        assert_eq!(ds.source_type, "");

        let s = serde_json::to_string(&tmpl)?;
        let decoded: Template = serde_json::from_str(&s)?;
        assert_eq!(decoded, tmpl);
        Ok(())
    }

    #[test]
    fn test_template_default() {
        let tmpl = Template::default();
        assert_eq!(tmpl.description, None);
        assert_eq!(tmpl.builders.len(), 0);
        assert_eq!(tmpl.provisioners.len(), 0);
        assert_eq!(tmpl.error_cleanup_provisioners.len(), 0);
        assert_eq!(tmpl.post_processors.len(), 0);
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
        let mut cfg = std::collections::HashMap::new();
        cfg.insert("vm_name".to_string(), "ubuntu".to_string());
        cfg.insert("api_token".to_string(), "tok123".to_string());
        cfg.insert("iso_path".to_string(), "/tmp/ubuntu.iso".to_string());
        cfg.insert("ssh_port".to_string(), "2222".to_string());
        cfg.insert("memory".to_string(), "2048".to_string());
        cfg.insert("disk_size".to_string(), "40960".to_string());
        cfg.insert("cpus".to_string(), "4".to_string());
        cfg.insert("headless".to_string(), "true".to_string());
        cfg.insert("disabled".to_string(), "false".to_string());
        cfg.insert("invalid_bool".to_string(), "maybe".to_string());

        let builder = BuilderConfig {
            builder_type: "null".to_string(),
            name: "my-builder".to_string(),
            config: cfg.clone(),
            ..Default::default()
        };
        assert_eq!(builder.builder_type, "null");
        assert_eq!(builder.name, "my-builder");
        assert_eq!(
            builder
                .get_non_empty("vm_name")
                .as_ref()
                .map(crate::types::NonEmptyString::as_str),
            Some("ubuntu")
        );
        assert_eq!(
            builder
                .get_secret("api_token")
                .as_ref()
                .map(crate::types::SecretString::expose_secret),
            Some("tok123")
        );
        assert_eq!(
            builder
                .get_file_path("iso_path")
                .as_ref()
                .and_then(|p| p.as_path().to_str()),
            Some("/tmp/ubuntu.iso")
        );
        assert_eq!(builder.get_port("ssh_port").map(|p| p.get()), Some(2222));
        assert_eq!(builder.get_memory_mb("memory").map(|m| m.get()), Some(2048));
        assert_eq!(
            builder.get_disk_size_mb("disk_size").map(|d| d.get()),
            Some(40960)
        );
        assert_eq!(builder.get_cpu_count("cpus").map(|c| c.get()), Some(4));
        assert_eq!(builder.get_bool("headless"), Some(true));
        assert_eq!(builder.get_bool("disabled"), Some(false));
        assert_eq!(builder.get_bool("invalid_bool"), None);

        let prov = ProvisionerConfig {
            provisioner_type: "shell".to_string(),
            config: cfg.clone(),
            ..Default::default()
        };
        assert_eq!(prov.get_bool("headless"), Some(true));

        let pp = PostProcessorConfig {
            post_processor_type: "manifest".to_string(),
            config: cfg.clone(),
            ..Default::default()
        };
        assert_eq!(pp.get_bool("headless"), Some(true));

        let ds = DataSourceConfig {
            source_type: "http".to_string(),
            name: "test".to_string(),
            config: cfg,
            ..Default::default()
        };
        assert_eq!(ds.get_bool("headless"), Some(true));
    }

    #[test]
    fn test_provisioner_config() {
        let provisioner = ProvisionerConfig {
            provisioner_type: "shell".to_string(),
            only: vec!["amazon-ebs.example".to_string()],
            except: vec!["docker.ubuntu".to_string()],
            config: std::collections::HashMap::new(),
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
    fn test_load_templates_directory_and_files() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let f1 = temp_dir.path().join("base.pkr.json");
        std::fs::write(&f1, r#"{"builders": [{"type": "null"}]}"#)?;

        let f2 = temp_dir.path().join("extra.pkr.hcl");
        std::fs::write(&f2, "locals {\n  env = \"dev\"\n}\n")?;

        let vars = std::collections::HashMap::new();

        // 1. Directory scanning
        let tmpl = load_templates(&[temp_dir.path()], &vars)?;
        assert_eq!(tmpl.builders.len(), 1);
        assert_eq!(tmpl.locals.get("env").map(String::as_str), Some("dev"));

        // 2. Multi-file loading
        let tmpl_files = load_templates(&[&f1, &f2], &vars)?;
        assert_eq!(tmpl_files.builders.len(), 1);
        assert_eq!(
            tmpl_files.locals.get("env").map(String::as_str),
            Some("dev")
        );

        // 3. Error cases
        assert!(load_templates::<std::path::PathBuf, _>(&[], &vars).is_err());
        assert!(load_templates(&[temp_dir.path().join("nonexistent")], &vars).is_err());

        let empty_dir = tempfile::tempdir()?;
        assert!(load_templates(&[empty_dir.path()], &vars).is_err());
        Ok(())
    }

    #[test]
    fn test_bento_multi_file_pre_merge() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let dir = temp_dir.path();

        let pkr_vars = dir.join("pkr-variables.pkr.hcl");
        std::fs::write(
            &pkr_vars,
            r#"
            variable "os_name" {
                type = string
                default = "ubuntu"
            }
            variable "os_arch" {
                type = string
                default = "x86_64"
            }
            "#,
        )?;

        let pkr_sources = dir.join("pkr-sources.pkr.hcl");
        std::fs::write(
            &pkr_sources,
            r#"
            locals {
                vm_name = "${var.os_name}-${var.os_arch}"
            }

            source "qemu" "vm" {
                vm_name = local.vm_name
                memory = 2048
            }
            "#,
        )?;

        let pkr_plugins = dir.join("pkr-plugins.pkr.hcl");
        std::fs::write(
            &pkr_plugins,
            r#"
            packer {
                required_version = ">= 1.7.0"
                required_plugins {
                    qemu = {
                        version = "~> 1.0"
                        source = "github.com/hashicorp/qemu"
                    }
                }
            }
            "#,
        )?;

        let pkr_builder = dir.join("pkr-builder.pkr.hcl");
        std::fs::write(
            &pkr_builder,
            r#"
            build {
                sources = ["source.qemu.vm"]
                provisioner "shell" {
                    inline = ["echo 'hello from bento'"]
                }
            }
            "#,
        )?;

        let vars = std::collections::HashMap::new();
        let tmpl = Template::from_files_or_dirs(&[dir], &vars)?;

        assert_eq!(tmpl.variables.len(), 2);
        assert!(tmpl.variables.contains_key("os_name"));
        assert!(tmpl.variables.contains_key("os_arch"));
        assert_eq!(tmpl.locals.len(), 1);
        assert!(tmpl.locals.contains_key("vm_name"));
        assert_eq!(tmpl.builders.len(), 1);
        assert_eq!(tmpl.builders[0].builder_type, "qemu");
        assert_eq!(tmpl.builders[0].name, "vm");
        assert_eq!(tmpl.builds.len(), 1);
        assert_eq!(tmpl.builds[0].sources, vec!["source.qemu.vm"]);
        assert_eq!(tmpl.builds[0].provisioners.len(), 1);
        assert_eq!(tmpl.builds[0].provisioners[0].provisioner_type, "shell");
        assert_eq!(
            tmpl.packer
                .as_ref()
                .and_then(|p| p.required_version.as_deref()),
            Some(">= 1.7.0")
        );
        assert_eq!(tmpl.required_plugins.len(), 1);
        assert_eq!(
            tmpl.required_plugins.get("qemu").map(|q| q.source.as_str()),
            Some("github.com/hashicorp/qemu")
        );
        assert_eq!(tmpl.root_path.as_deref(), Some(dir));
        Ok(())
    }

    #[test]
    fn test_duplicate_definition_diagnostics_forwarding() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir()?;
        let dir = temp_dir.path();

        let f1 = dir.join("file1.pkr.hcl");
        std::fs::write(
            &f1,
            r#"
            variable "dup_var" {
                default = "first"
            }
            "#,
        )?;

        let f2 = dir.join("file2.pkr.hcl");
        std::fs::write(
            &f2,
            r#"
            variable "dup_var" {
                default = "second"
            }
            "#,
        )?;

        let vars = std::collections::HashMap::new();
        let res = Template::from_files_or_dirs(&[dir], &vars);
        assert!(
            matches!(res, Err(StampError::Parse(ref msg)) if msg.contains("Variable 'dup_var' defined multiple times")),
            "Expected duplicate variable error, got {res:?}"
        );
        Ok(())
    }
}
