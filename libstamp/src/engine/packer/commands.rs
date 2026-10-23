//! Template validation, formatting, inspection, upgrade, fix, init, and console commands.

use crate::error::StampError;

/// Undeclared variable warning options for template validation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UndeclaredVarOptions {
    /// Whether to warn or error on undeclared variables.
    pub warn_on_undeclared_var: bool,
    /// Disables warning on undeclared variables.
    pub no_warn_undeclared_var: bool,
}

/// Configuration for the `validate` command.
#[derive(Debug, Default, Clone)]
pub struct ValidateConfig {
    /// Skip builder preparation and evaluation of data sources. Only check syntax and schemas.
    pub syntax_only: bool,
    /// Evaluate data sources during validation.
    pub evaluate_datasources: bool,
    /// Undeclared variable warning options.
    pub undeclared_vars: UndeclaredVarOptions,
    /// Only validate the given builder/source names.
    pub only: Option<String>,
    /// Exclude the given builder/source names from validation.
    pub except: Option<String>,
    /// User variables passed via command-line (-var).
    pub vars: std::collections::HashMap<String, String>,
    /// Variable files (-var-file).
    pub var_files: Vec<String>,
}

/// Validates a template structure against component schemas and configuration.
///
/// # Errors
/// Returns `StampError` if validation fails.
pub fn validate_with_config(
    template: &crate::template::Template,
    config: &ValidateConfig,
) -> Result<(), crate::error::StampError> {
    if !config.syntax_only && template.builders.is_empty() {
        return Err(crate::error::StampError::Parse(
            "Template must contain at least one builder".to_string(),
        ));
    }

    // 1. Validate builder component schemas
    for b in &template.builders {
        let has_ssh = b
            .config
            .get("communicator")
            .map(std::string::String::as_str)
            == Some("ssh");
        if has_ssh && (b.builder_type == "hyperv-iso" || b.builder_type == "hyperv-vmcx") {
            return Err(crate::error::StampError::Parse(format!(
                "Builder '{}' of type '{}' only supports winrm communicator, but ssh was provided.",
                b.name.clone(),
                b.builder_type
            )));
        }

        if b.builder_type == "file" {
            if !b.config.contains_key("target") {
                return Err(crate::error::StampError::Validation(
                    "Builder 'file' requires 'target' parameter".to_string(),
                ));
            }
            if !b.config.contains_key("source") && !b.config.contains_key("content") {
                return Err(crate::error::StampError::Validation(
                    "Builder 'file' requires either 'source' or 'content' parameter".to_string(),
                ));
            }
        }
    }

    // 2. Validate provisioners
    for p in &template.provisioners {
        if p.provisioner_type.is_empty() {
            return Err(crate::error::StampError::Validation(
                "Provisioner type cannot be empty".to_string(),
            ));
        }
    }

    // 3. Undeclared variable checks
    let mut declared_vars = std::collections::HashSet::new();
    for k in template.variables.keys() {
        declared_vars.insert(k.clone());
    }
    for k in config.vars.keys() {
        declared_vars.insert(k.clone());
    }
    for (k, _) in std::env::vars() {
        if let Some(stripped) = k.strip_prefix("PKR_VAR_") {
            declared_vars.insert(stripped.to_string());
        }
    }

    let re_var = regex::Regex::new(r"(?:var\.|\$\{\s*var\.|\{\{\s*user\s+`)([\w-]+)")?;

    let check_str = |s: &str| -> Result<(), StampError> {
        for caps in re_var.captures_iter(s) {
            if let Some(v_name) = caps.get(1) {
                let name = v_name.as_str();
                if !declared_vars.contains(name) {
                    if config.undeclared_vars.warn_on_undeclared_var {
                        return Err(StampError::Validation(format!(
                            "Undeclared variable '{name}' referenced in configuration"
                        )));
                    } else if !config.undeclared_vars.no_warn_undeclared_var {
                        eprintln!(
                            "Warning: Undeclared variable '{name}' referenced in configuration"
                        );
                    }
                }
            }
        }
        Ok(())
    };

    for b in &template.builders {
        for v in b.config.values() {
            check_str(v)?;
        }
    }
    for p in &template.provisioners {
        for v in p.config.values() {
            check_str(v)?;
        }
    }
    for l in template.locals.values() {
        check_str(l)?;
    }

    Ok(())
}

/// Validates a template structure.
///
/// # Errors
/// Returns `StampError::Parse` if validation fails.
pub fn validate(template: &crate::template::Template) -> Result<(), crate::error::StampError> {
    validate_with_config(template, &ValidateConfig::default())
}

/// Output and write options for formatting.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FmtOutputOptions {
    /// Check if input is formatted. Return error if not.
    pub check: bool,
    /// Display diffs of formatting changes.
    pub diff: bool,
    /// Write result to source file.
    pub write: bool,
}

/// Configuration for the `fmt` command.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FmtConfig {
    /// Process directories recursively.
    pub recursive: bool,
    /// Output and write mode options.
    pub options: FmtOutputOptions,
}

/// Formats a single template.
///
/// # Errors
/// Returns `StampError` if formatting fails.
pub fn fmt_file(template_path: &str, config: &FmtConfig) -> Result<(), StampError> {
    let content = std::fs::read_to_string(template_path)?;
    let formatted = if std::path::Path::new(template_path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        let parsed: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| StampError::Parse(e.to_string()))?;
        serde_json::to_string_pretty(&parsed).map_err(|e| StampError::Parse(e.to_string()))?
    } else {
        hashicorp_configuration_language_rs::serde::from_str::<serde_json::Value>(&content)
            .map_err(|e| StampError::Parse(e.to_string()))?;
        crate::engine::fmt::format_hcl_canonical(&content)
    };

    if content != formatted {
        if config.options.diff {
            println!(
                "{}",
                crate::engine::fmt::generate_diff(&content, &formatted)
            );
        }
        if config.options.write {
            std::fs::write(template_path, &formatted)?;
        }
        if config.options.check {
            return Err(StampError::Validation(format!(
                "Formatting needed for {template_path}"
            )));
        }
    }
    Ok(())
}

/// Formats templates according to configuration.
///
/// # Errors
/// Returns `StampError` if formatting fails.
pub fn fmt(template_path: &str, config: &FmtConfig) -> Result<(), StampError> {
    let path = std::path::Path::new(template_path);
    if path.is_dir() {
        if !config.recursive {
            return Err(StampError::Io(std::io::Error::new(
                std::io::ErrorKind::IsADirectory,
                "Path is a directory, use recursive flag",
            )));
        }
        let mut queue = vec![path.to_path_buf()];
        while let Some(dir) = queue.pop() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let meta = entry.metadata()?;
                if meta.is_dir() {
                    queue.push(entry.path());
                } else if meta.is_file() {
                    let path = entry.path();
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if (ext.eq_ignore_ascii_case("json")
                        || ext.eq_ignore_ascii_case("hcl")
                        || ext.eq_ignore_ascii_case("pkr.hcl"))
                        && let Some(p) = path.to_str()
                    {
                        fmt_file(p, config)?;
                    }
                }
            }
        }
    } else {
        fmt_file(template_path, config)?;
    }
    Ok(())
}

/// Configuration for the `inspect` command.
#[derive(Debug, Default, Clone)]
pub struct InspectConfig {
    /// Output in machine-readable format
    pub machine_readable: bool,
    /// Use sequential evaluation for data sources and blocks
    pub use_sequential_evaluation: bool,
    /// Output in JSON format
    pub json: bool,
}

/// Inspects a template.
///
/// # Errors
/// Returns `StampError` if inspection fails.
pub fn inspect(
    template: &crate::template::Template,
    config: &InspectConfig,
) -> Result<String, StampError> {
    if config.json {
        let val = serde_json::json!({
            "description": template.description,
            "variables": template.variables,
            "data_sources": template.data_sources,
            "builders": template.builders,
            "provisioners": template.provisioners,
        });
        return serde_json::to_string_pretty(&val).map_err(|e| StampError::Parse(e.to_string()));
    }

    if config.machine_readable {
        let mut lines = Vec::new();
        let ts = "0";

        if let Some(desc) = &template.description {
            lines.push(format!(
                "{},,template-description,{}",
                ts,
                desc.replace(',', "%!(PACKER_COMMA)")
            ));
        }

        let mut var_keys: Vec<_> = template.variables.keys().collect();
        var_keys.sort();
        for k in var_keys {
            let var = &template.variables[k];
            let v_type = var.variable_type.as_deref().unwrap_or("string");
            let v_def = var.default.as_deref().unwrap_or("");
            lines.push(format!("{ts},,template-variable,{k},{v_type},{v_def}"));
        }

        for ds in &template.data_sources {
            lines.push(format!(
                "{},,template-data-source,{},{}",
                ts, ds.name, ds.source_type
            ));
        }

        for b in &template.builders {
            lines.push(format!(
                "{},,template-builder,{},{}",
                ts, b.name, b.builder_type
            ));
        }

        for p in &template.provisioners {
            lines.push(format!(
                "{},,template-provisioner,{}",
                ts, p.provisioner_type
            ));
        }

        for pp in &template.post_processors {
            lines.push(format!(
                "{},,template-post-processor,{}",
                ts, pp.post_processor_type
            ));
        }

        return Ok(lines.join(
            "
",
        ));
    }

    let mut lines = Vec::new();

    if let Some(desc) = &template.description {
        lines.push("Description:".to_string());
        lines.push(format!("  {desc}"));
        lines.push(String::new());
    }

    if !template.variables.is_empty() {
        lines.push("Variables:".to_string());
        let mut keys: Vec<_> = template.variables.keys().collect();
        keys.sort();
        for k in keys {
            let var = &template.variables[k];
            let v_type = var.variable_type.as_deref().unwrap_or("string");
            let v_def = var.default.as_deref().unwrap_or("<required>");
            lines.push(format!("  {k}: {v_type} (default: {v_def})"));
        }
        lines.push(String::new());
    }

    if !template.data_sources.is_empty() {
        lines.push("Data Sources:".to_string());
        for ds in &template.data_sources {
            lines.push(format!("  {} ({})", ds.name, ds.source_type));
        }
        lines.push(String::new());
    }

    if !template.builders.is_empty() {
        lines.push("Builders:".to_string());
        for b in &template.builders {
            lines.push(format!("  {} ({})", b.name, b.builder_type));
        }
        lines.push(String::new());
    }

    if !template.provisioners.is_empty() {
        lines.push("Provisioners:".to_string());
        for p in &template.provisioners {
            lines.push(format!("  {}", p.provisioner_type));
        }
        lines.push(String::new());
    }

    if !template.post_processors.is_empty() {
        lines.push("Post-processors:".to_string());
        for pp in &template.post_processors {
            lines.push(format!("  {}", pp.post_processor_type));
        }
        lines.push(String::new());
    }

    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }

    Ok(lines.join(
        "
",
    ))
}

/// Upgrades legacy JSON to HCL2.
///
/// # Errors
/// Returns `StampError` if upgrading fails.
pub fn hcl2_upgrade(template_path: &str, output_path: Option<&str>) -> Result<(), StampError> {
    let content = std::fs::read_to_string(template_path)?;

    if let Some(out) = output_path {
        let out_path = std::path::Path::new(out);
        if out_path.is_dir() || out.ends_with('/') || out.ends_with(char::from(92u8)) {
            std::fs::create_dir_all(out_path)?;
            let structured = crate::engine::upgrade::upgrade_json_to_structured_hcl2(&content)?;
            std::fs::write(out_path.join("variables.pkr.hcl"), structured.variables)?;
            std::fs::write(out_path.join("sources.pkr.hcl"), structured.sources)?;
            std::fs::write(out_path.join("build.pkr.hcl"), structured.build)?;
            return Ok(());
        }
    }

    let hcl_out = crate::engine::upgrade::upgrade_json_to_hcl2(&content)?;

    let out_file = output_path.map_or_else(
        || format!("{}.pkr.hcl", template_path.trim_end_matches(".json")),
        std::string::ToString::to_string,
    );

    std::fs::write(&out_file, hcl_out)?;
    Ok(())
}

/// Fixes a template.
///
/// # Errors
/// Returns `StampError` if fixing fails.
pub fn fix(template_path: &str) -> Result<String, StampError> {
    crate::engine::fix::fix_template(template_path, &crate::engine::fix::FixConfig::default())
}

/// Initializes a template, downloading/installing required plugins.
///
/// # Errors
/// Returns `StampError` if initialization fails.
pub async fn init(template: &str) -> Result<(), StampError> {
    crate::engine::packer_init::init(template, false).await
}

/// Consoles a template.
///
/// # Errors
/// Returns `StampError` if console fails.
pub fn console(template_path: &str) -> Result<(), StampError> {
    crate::engine::console::run_console(
        Some(template_path),
        &crate::engine::console::ConsoleConfig::default(),
    )
}
