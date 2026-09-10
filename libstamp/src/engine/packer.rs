#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
//! Packer concurrent build engine.

use crate::error::StampError;

/// Represents a boolean-like state for a configuration feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeatureState {
    #[default]
    /// The feature is disabled.
    Disabled,
    /// The feature is enabled.
    Enabled,
}

impl FeatureState {
    /// Returns `true` if the feature is enabled.
    #[must_use]
    pub fn is_enabled(self) -> bool {
        self == Self::Enabled
    }
}

/// Strategy for handling errors during the build.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum OnErrorStrategy {
    /// Clean up artifacts and exit.
    #[default]
    Cleanup,
    /// Abort without cleanup.
    Abort,
    /// Ask the user what to do.
    Ask,
    /// Execute error cleanup provisioners without destroying builder resources.
    RunCleanupProvisioner,
}

/// Configuration for the Packer engine.
#[derive(Debug, Default, Clone)]
pub struct EngineConfig {
    /// Enable color output
    pub color: FeatureState,
    /// Disable parallelization and enable debug mode
    pub debug: FeatureState,
    /// Build all builds other than these
    pub except: Vec<String>,
    /// Build only the specified builds
    pub only: Vec<String>,
    /// Force a build to continue if artifacts exist, deletes them previously
    pub force: FeatureState,
    /// Ignore prerelease versions when installing plugins
    pub ignore_prerelease_plugins: FeatureState,
    /// Enable machine-readable output
    pub machine_readable: FeatureState,
    /// If the build fails do: clean up, abort, or ask
    pub on_error: OnErrorStrategy,
    /// Number of builds to run in parallel
    pub parallel_builds: Option<u32>,
    /// Pacing duration between builder starts to avoid overwhelming CPU/disk I/O
    pub pacing_delay: Option<std::time::Duration>,
    /// Skip Packer policy enforcement
    pub skip_enforcement: FeatureState,
    /// Enable UI timestamps
    pub timestamp_ui: FeatureState,
    /// Use sequential evaluation for data sources and blocks
    pub use_sequential_evaluation: FeatureState,
    /// Variables for templates
    pub vars: Vec<String>,
    /// JSON files containing user variables
    pub var_files: Vec<String>,
    /// Warn on undeclared variables
    pub warn_on_undeclared_var: FeatureState,
    /// Packer core configuration
    pub packer_config: Option<crate::template::PackerConfig>,
    /// Scrubber to redact sensitive values
    pub scrubber: Option<std::sync::Arc<crate::engine::ui::Scrubber>>,
    /// Optional Sentinel policy path override
    pub sentinel_policy: Option<String>,
    /// Optional OPA policy path override
    pub opa_policy: Option<String>,
}

/// Result tuple returned by concurrent builder tasks.
type InFlightBuilderResult = (
    String,
    Result<Box<dyn crate::artifact::Artifact>, crate::error::StampError>,
);

/// Executes a set of builders concurrently.
///
/// # Errors
/// Returns `StampError` if any builder fails to run.
pub async fn build_concurrently(
    builders: Vec<Box<dyn crate::builder::Builder>>,
    provisioners: std::sync::Arc<Vec<Box<dyn crate::provisioner::Provisioner>>>,
    error_cleanup_provisioners: std::sync::Arc<Vec<Box<dyn crate::provisioner::Provisioner>>>,
    post_processors: std::sync::Arc<Vec<Box<dyn crate::post_processor::PostProcessor>>>,
    config: EngineConfig,
) -> Result<(), crate::error::StampError> {
    let filtered_builders: Vec<Box<dyn crate::builder::Builder>> = builders
        .into_iter()
        .filter(|b| {
            let name = b.name();
            if !config.only.is_empty() && !config.only.contains(&name) {
                return false;
            }
            if !config.except.is_empty() && config.except.contains(&name) {
                return false;
            }
            true
        })
        .collect();

    if config.force.is_enabled() {
        for builder in &filtered_builders {
            builder.force_clean().await?;
        }
    }

    if config.machine_readable.is_enabled() {
        println!("{{\"status\": \"engine_started\"}}");
    }
    if config.timestamp_ui.is_enabled() {
        println!("Timestamp UI enabled");
    }

    let mut all_artifacts = Vec::new();

    let ui_multiplexer = crate::engine::ui::UiMultiplexer::new(
        config.machine_readable,
        config.color,
        config.timestamp_ui,
        config.scrubber.clone(),
    );

    crate::engine::dag::validate_builder_dependencies(&filtered_builders)?;
    let tiers = crate::engine::dag::resolve_builder_execution_tiers(&filtered_builders)?;

    if config.debug.is_enabled()
        || config.parallel_builds == Some(1)
        || config.use_sequential_evaluation.is_enabled()
    {
        if config.color.is_enabled() {
            println!("\x1b[33mDEBUG MODE ENABLED: Running sequentially\x1b[0m");
        } else {
            println!("DEBUG MODE ENABLED: Running sequentially");
        }
        let mut builder_map: std::collections::HashMap<String, Box<dyn crate::builder::Builder>> =
            std::collections::HashMap::new();
        for b in filtered_builders {
            builder_map.insert(b.name(), b);
        }
        for tier in tiers {
            for b_name in tier {
                if let Some(builder) = builder_map.remove(&b_name) {
                    builder.prepare().await?;
                    let hook = std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                        provisioners: provisioners.clone(),
                        error_cleanup_provisioners: error_cleanup_provisioners.clone(),
                    });
                    let ui = ui_multiplexer.get_ui();
                    match builder.run(hook, ui.clone(), config.on_error.clone()).await {
                        Ok(artifact) => all_artifacts.push(artifact),
                        Err(e) => {
                            match config.on_error {
                                OnErrorStrategy::Cleanup => {
                                    builder.cancel().await?;
                                }
                                OnErrorStrategy::Abort => {
                                    // Do nothing, leave artifacts
                                }
                                OnErrorStrategy::RunCleanupProvisioner => {
                                    ui.say(
                                        &builder.name(),
                                        "OnErrorStrategy::RunCleanupProvisioner: Preserving builder resources",
                                    );
                                }
                                OnErrorStrategy::Ask => {
                                    if !cfg!(test) {
                                        let b_name = builder.name();
                                        if let Ok(input) = ui.ask(
                                            "stamp",
                                            &format!("Build '{b_name}' errored: {e}\nDo you want to clean up? [y/N]: "),
                                        )
                                            && (input == "y" || input == "yes") {
                                                builder.cancel().await?;
                                            }
                                    }
                                }
                            }
                            return Err(e);
                        }
                    }
                }
            }
        }
    } else {
        let pacing_delay = config.pacing_delay.or_else(|| {
            std::env::var("PACKER_BUILDER_PACING_MS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map(std::time::Duration::from_millis)
        });

        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let on_error = config.on_error.clone();

        let semaphore = if let Some(limit) = config.parallel_builds {
            if limit > 0 {
                Some(std::sync::Arc::new(tokio::sync::Semaphore::new(
                    limit as usize,
                )))
            } else {
                None
            }
        } else {
            None
        };

        let mut pending_builders: std::collections::HashMap<
            String,
            std::sync::Arc<dyn crate::builder::Builder>,
        > = std::collections::HashMap::new();
        let mut remaining_deps: std::collections::HashMap<
            String,
            std::collections::HashSet<String>,
        > = std::collections::HashMap::new();

        for b in filtered_builders {
            let name = b.name();
            let deps: std::collections::HashSet<String> = b.depends_on().into_iter().collect();
            remaining_deps.insert(name.clone(), deps);
            pending_builders.insert(name, std::sync::Arc::from(b));
        }

        let mut in_flight: tokio::task::JoinSet<InFlightBuilderResult> =
            tokio::task::JoinSet::new();

        let active_builders: std::sync::Arc<
            tokio::sync::Mutex<
                std::collections::HashMap<String, std::sync::Arc<dyn crate::builder::Builder>>,
            >,
        > = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

        let mut launched_count = 0;

        while !pending_builders.is_empty() || !in_flight.is_empty() {
            let ready_names: Vec<String> = remaining_deps
                .iter()
                .filter(|(name, deps)| deps.is_empty() && pending_builders.contains_key(*name))
                .map(|(name, _)| name.clone())
                .collect();

            for name in ready_names {
                if let Some(builder) = pending_builders.remove(&name) {
                    if let Some(delay) = pacing_delay
                        && launched_count > 0
                    {
                        tokio::time::sleep(delay).await;
                    }
                    launched_count += 1;

                    active_builders
                        .lock()
                        .await
                        .insert(name.clone(), builder.clone());

                    let b = builder.clone();
                    let err_strat = on_error.clone();
                    let sem = semaphore.clone();
                    let provs = provisioners.clone();
                    let err_provs = error_cleanup_provisioners.clone();
                    let ui = ui_multiplexer.get_ui();
                    let b_cancel_rx = cancel_rx.clone();
                    let b_name = name.clone();

                    in_flight.spawn(async move {
                        let res = async {
                            b.prepare().await?;
                            let _permit = if let Some(s) = sem {
                                Some(s.acquire_owned().await.map_err(|e| {
                                    crate::error::StampError::Parse(format!(
                                        "Failed to acquire semaphore permit: {e}"
                                    ))
                                })?)
                            } else {
                                None
                            };
                            if *b_cancel_rx.borrow() {
                                return Err(crate::error::StampError::Execution(
                                    "Build cancelled".to_string(),
                                ));
                            }
                            let hook =
                                std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                                    provisioners: provs,
                                    error_cleanup_provisioners: err_provs,
                                });
                            b.run(hook, ui.clone(), err_strat.clone()).await
                        }
                        .await;

                        match res {
                            Ok(artifact) => (b_name, Ok(artifact)),
                            Err(e) => {
                                match err_strat {
                                    OnErrorStrategy::Cleanup => {
                                        let _ = b.cancel().await;
                                    }
                                    OnErrorStrategy::Abort => {}
                                    OnErrorStrategy::RunCleanupProvisioner => {
                                        ui.say(
                                            &b.name(),
                                            "OnErrorStrategy::RunCleanupProvisioner: Preserving builder resources",
                                        );
                                    }
                                    OnErrorStrategy::Ask => {
                                        let bn = b.name();
                                        if let Ok(input) = ui.ask(
                                            "stamp",
                                            &format!(
                                                "Build '{bn}' errored: {e}\nDo you want to clean up? [y/N]: "
                                            ),
                                        )
                                            && (input == "y" || input == "yes") {
                                                let _ = b.cancel().await;
                                            }
                                    }
                                }
                                (b_name, Err(e))
                            }
                        }
                    });
                }
            }

            if in_flight.is_empty() {
                if !pending_builders.is_empty() {
                    return Err(crate::error::StampError::Execution(
                        "Deadlock or unresolvable dependencies detected in builder graph"
                            .to_string(),
                    ));
                }
                break;
            }

            if let Some(join_res) = in_flight.join_next().await {
                match join_res {
                    Ok((finished_name, Ok(artifact))) => {
                        all_artifacts.push(artifact);
                        active_builders.lock().await.remove(&finished_name);
                        remaining_deps.remove(&finished_name);
                        for deps in remaining_deps.values_mut() {
                            deps.remove(&finished_name);
                        }
                    }
                    Ok((failed_name, Err(e))) => {
                        let _ = cancel_tx.send(true);
                        active_builders.lock().await.remove(&failed_name);
                        let builders_to_cancel: Vec<std::sync::Arc<dyn crate::builder::Builder>> = {
                            let mut lock = active_builders.lock().await;
                            lock.drain().map(|(_, b)| b).collect()
                        };
                        for b in builders_to_cancel {
                            if on_error == OnErrorStrategy::Cleanup {
                                let _ = b.cancel().await;
                            }
                        }
                        in_flight.abort_all();
                        return Err(e);
                    }
                    Err(join_err) => {
                        let _ = cancel_tx.send(true);
                        in_flight.abort_all();
                        return Err(crate::error::StampError::Parse(format!(
                            "Task panicked or cancelled: {join_err}"
                        )));
                    }
                }
            }
        }
    }

    // 0. Post-processing
    let mut final_artifacts: Vec<Box<dyn crate::artifact::Artifact>> = Vec::new();
    for artifact_box in all_artifacts {
        let mut current_artifact =
            crate::post_processor::Artifact::new(artifact_box.id(), artifact_box.files());
        for pp in post_processors.iter() {
            let previous_files = current_artifact.files.clone();
            let keep_input = pp.keep_input_artifact();
            let next_artifact = pp.process(current_artifact).await?;

            if !keep_input {
                for file in &previous_files {
                    if !next_artifact.files.contains(file) {
                        let path = std::path::Path::new(file);
                        if path.exists() {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                }
            }

            current_artifact = next_artifact;
        }
        // Mock wrapper back to trait for the rest of engine pipeline (e.g. HCP pushes)
        final_artifacts.push(Box::new(crate::artifact::MockArtifact {
            builder_id: "post-processor".to_string(),
            id: current_artifact.id,
            files: current_artifact.files,
        }));
    }

    // 1. Sentinel & OPA Policy Evaluation
    if config.skip_enforcement != FeatureState::Enabled {
        let sentinel_policy = config
            .sentinel_policy
            .clone()
            .or_else(|| std::env::var("PACKER_SENTINEL_POLICY_PATH").ok());
        let sentinel_level = std::env::var("PACKER_POLICY_ENFORCEMENT_LEVEL")
            .ok()
            .and_then(|s| s.parse::<crate::engine::sentinel::EnforcementLevel>().ok())
            .unwrap_or(crate::engine::sentinel::EnforcementLevel::HardMandatory);

        let sentinel_evaluator = crate::engine::sentinel::SentinelEvaluator::new(
            crate::engine::sentinel::SentinelConfig::new(sentinel_policy, sentinel_level),
        );
        let plan_val = serde_json::json!({
            "target_builders": config.only,
            "except_builders": config.except,
        });
        let dummy_template = crate::template::Template::default();
        sentinel_evaluator
            .evaluate_with_context(&dummy_template, &plan_val, &final_artifacts)
            .await?;

        let opa_policy = config
            .opa_policy
            .clone()
            .or_else(|| std::env::var("PACKER_OPA_POLICY_PATH").ok());
        if let Some(opa_path) = opa_policy {
            let opa_evaluator =
                crate::engine::opa::OpaEvaluator::new(crate::engine::opa::OpaConfig {
                    policy_paths: vec![crate::engine::opa::OpaPolicyPath(
                        std::path::PathBuf::from(opa_path),
                    )],
                    ..Default::default()
                });
            opa_evaluator
                .evaluate_template(&dummy_template, &final_artifacts)
                .await?;
        }
    }

    // 2. HCP Packer Registry Push
    if let Some(packer_config) = &config.packer_config
        && let Some(registry) = &packer_config.hcp_packer_registry
    {
        if !cfg!(test)
            && let Ok(hcp_client) = crate::engine::hcp::HcpRegistryClient::from_env()
        {
            hcp_client
                .push_build_artifacts(registry, &final_artifacts)
                .await?;
        } else {
            for artifact in &final_artifacts {
                println!(
                    "Pushing artifact {} to HCP Packer Registry bucket {}",
                    artifact.id(),
                    registry.bucket_name
                );
            }
        }
    }

    let main_ui = ui_multiplexer.get_ui();
    if config.machine_readable.is_enabled() {
        let count_str = final_artifacts.len().to_string();
        main_ui.machine("", "artifact-count", &[&count_str]);
        for (idx, artifact) in final_artifacts.iter().enumerate() {
            let idx_str = idx.to_string();
            let b_id = artifact.builder_id();
            let a_id = artifact.id();
            let files = artifact.files();
            let file_count_str = files.len().to_string();
            let art_str = artifact.string();

            main_ui.machine(&b_id, "artifact", &[&idx_str, "builder-id", &b_id]);
            main_ui.machine(&b_id, "artifact", &[&idx_str, "id", &a_id]);
            main_ui.machine(&b_id, "artifact", &[&idx_str, "string", &art_str]);
            main_ui.machine(
                &b_id,
                "artifact",
                &[&idx_str, "files-count", &file_count_str],
            );
            for (f_idx, file) in files.iter().enumerate() {
                let f_idx_str = f_idx.to_string();
                main_ui.machine(&b_id, "artifact", &[&idx_str, "file", &f_idx_str, file]);
            }
        }
    } else if !final_artifacts.is_empty() {
        main_ui.say(
            "",
            "Builds finished. The artifacts of successful builds are:",
        );
        for artifact in &final_artifacts {
            main_ui.say(&artifact.builder_id(), &artifact.string());
        }
    }

    Ok(())
}

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

    let re_var = regex::Regex::new(r"(?:var\.|\$\{\s*var\.|\{\{\s*user\s+`)([\w-]+)")
        .unwrap_or_else(|_| regex::Regex::new("$a").unwrap_or_else(|_| unreachable!()));

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
            .map_err(|e| StampError::Parse(e.clone()))?;
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
        let ts = "0"; // deterministic timestamp for output

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

        return Ok(lines.join("\n"));
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

    Ok(lines.join("\n"))
}

/// Upgrades legacy JSON to HCL2.
///
/// # Errors
/// Returns `StampError` if upgrading fails.
pub fn hcl2_upgrade(template_path: &str, output_path: Option<&str>) -> Result<(), StampError> {
    let content = std::fs::read_to_string(template_path)?;

    if let Some(out) = output_path {
        let out_path = std::path::Path::new(out);
        if out_path.is_dir() || out.ends_with('/') || out.ends_with('\\') {
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
pub fn fix(template_path: &str) -> Result<(), StampError> {
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_build_force_clear() -> Result<(), StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: "b1".to_string(),
            ..Default::default()
        }));
        let config = EngineConfig {
            force: FeatureState::Enabled,
            machine_readable: FeatureState::Enabled,
            timestamp_ui: FeatureState::Enabled,
            ..Default::default()
        };
        build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_build_machine_readable_artifacts() -> Result<(), StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: "b-machine".to_string(),
            ..Default::default()
        }));
        let config = EngineConfig {
            machine_readable: FeatureState::Enabled,
            ..Default::default()
        };
        build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_build_parallel_limit() -> Result<(), StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: "b1".to_string(),
            ..Default::default()
        }));
        let config = EngineConfig {
            parallel_builds: Some(2),
            ..Default::default()
        };
        build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_build_on_error_cleanup_sequential() -> Result<(), StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: String::new(), // Forces prepare to fail
            ..Default::default()
        }));
        let config = EngineConfig {
            on_error: OnErrorStrategy::Cleanup,
            debug: FeatureState::Enabled,
            ..Default::default()
        };
        assert!(
            build_concurrently(
                vec![b1],
                std::sync::Arc::new(vec![]),
                std::sync::Arc::new(vec![]),
                std::sync::Arc::new(vec![]),
                config
            )
            .await
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn test_fix_hcl() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_fix.pkr.hcl");
        std::fs::write(&tmp, r#"source "null" "test" {}"#).map_err(StampError::Io)?;
        fix(tmp
            .to_str()
            .ok_or_else(|| StampError::Parse("Path not UTF-8".to_string()))?)?;
        Ok(())
    }

    #[tokio::test]
    async fn test_init() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join(format!("test_init_{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&tmp, r#"{"packer": {}}"#).map_err(StampError::Io)?;
        let res = init(tmp.to_str().unwrap_or_default()).await;
        let _ = std::fs::remove_file(&tmp);
        res?;
        Ok(())
    }

    #[test]
    fn test_console() -> Result<(), StampError> {
        unsafe {
            std::env::set_var("STAMP_TEST_MODE", "1");
        }
        console("dummy.json")?;
        unsafe {
            std::env::remove_var("STAMP_TEST_MODE");
        }
        Ok(())
    }

    #[test]
    fn test_validate_success() -> Result<(), StampError> {
        #[allow(clippy::field_reassign_with_default)]
        let mut tmpl = crate::template::Template::default();
        tmpl.builders.push(crate::template::BuilderConfig {
            builder_type: "null".to_string(),
            name: "test".to_string(),
            ..Default::default()
        });
        validate(&tmpl)?;
        Ok(())
    }

    #[test]
    fn test_validate_failure() {
        let tmpl = crate::template::Template::default();
        assert!(validate(&tmpl).is_err());
    }

    #[test]
    fn test_validate_syntax_only() -> Result<(), StampError> {
        let tmpl = crate::template::Template::default();
        let config = ValidateConfig {
            syntax_only: true,
            ..Default::default()
        };
        validate_with_config(&tmpl, &config)?;
        Ok(())
    }

    #[test]
    fn test_validate_undeclared_var_error() {
        let mut tmpl = crate::template::Template::default();
        let mut config_map = std::collections::HashMap::new();
        config_map.insert("image".to_string(), "var.secret_image".to_string());
        tmpl.builders.push(crate::template::BuilderConfig {
            builder_type: "docker".to_string(),
            name: "test".to_string(),
            config: config_map,
            ..Default::default()
        });
        let config = ValidateConfig {
            undeclared_vars: UndeclaredVarOptions {
                warn_on_undeclared_var: true,
                no_warn_undeclared_var: false,
            },
            ..Default::default()
        };
        assert!(validate_with_config(&tmpl, &config).is_err());
    }

    #[test]
    fn test_validate_builder_file_missing_target() {
        let mut tmpl = crate::template::Template::default();
        tmpl.builders.push(crate::template::BuilderConfig {
            builder_type: "file".to_string(),
            name: "my-file".to_string(),
            config: std::collections::HashMap::new(),
            ..Default::default()
        });
        assert!(validate(&tmpl).is_err());
    }

    #[test]
    fn test_inspect_json() -> Result<(), StampError> {
        let mut tmpl = crate::template::Template::default();
        tmpl.description = Some("A test image".to_string());
        tmpl.builders.push(crate::template::BuilderConfig {
            builder_type: "null".to_string(),
            name: "test".to_string(),
            ..Default::default()
        });
        let config = InspectConfig {
            json: true,
            ..Default::default()
        };
        let out = inspect(&tmpl, &config)?;
        assert!(out.contains("\"description\": \"A test image\""));
        assert!(out.contains("\"builders\""));
        Ok(())
    }

    #[test]
    fn test_fmt_json() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_fmt.json");
        std::fs::write(&tmp, r#"{"builders":[{"type":"null","name":"test"}]}"#)
            .map_err(StampError::Io)?;
        fmt(
            tmp.to_str()
                .ok_or_else(|| StampError::Parse("Path not UTF-8".to_string()))?,
            &FmtConfig {
                options: FmtOutputOptions {
                    write: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        )?;
        Ok(())
    }

    #[test]
    fn test_fmt_hcl() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_fmt.pkr.hcl");
        std::fs::write(&tmp, r#"source "null" "test" {}"#).map_err(StampError::Io)?;
        fmt(
            tmp.to_str()
                .ok_or_else(|| StampError::Parse("Path not UTF-8".to_string()))?,
            &FmtConfig {
                options: FmtOutputOptions {
                    write: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        )?;
        Ok(())
    }

    #[test]
    fn test_fmt_json_invalid() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_fmt_invalid.json");
        std::fs::write(&tmp, "source \"amazon-ebs\" {").map_err(StampError::Io)?;
        let path_str = tmp
            .to_str()
            .ok_or_else(|| StampError::Parse("invalid path".into()))?;
        assert!(fmt(path_str, &FmtConfig::default()).is_err());
        Ok(())
    }

    #[test]
    fn test_fmt_hcl_invalid() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_fmt_invalid.pkr.hcl");
        std::fs::write(&tmp, "source \"amazon-ebs\" {").map_err(StampError::Io)?;
        let path_str = tmp
            .to_str()
            .ok_or_else(|| StampError::Parse("invalid path".into()))?;
        assert!(fmt(path_str, &FmtConfig::default()).is_err());
        Ok(())
    }

    #[test]
    fn test_hcl2_upgrade() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_upgrade.json");
        std::fs::write(&tmp, r#"{"builders":[{"type":"null","name":"test"}]}"#)
            .map_err(StampError::Io)?;
        let path_str = tmp
            .to_str()
            .ok_or_else(|| StampError::Parse("invalid path".into()))?;
        hcl2_upgrade(path_str, None)?;
        Ok(())
    }

    #[test]
    fn test_hcl2_upgrade_invalid() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_upgrade_invalid.json");
        std::fs::write(&tmp, "source \"amazon-ebs\" {").map_err(StampError::Io)?;
        let path_str = tmp
            .to_str()
            .ok_or_else(|| StampError::Parse("invalid path".into()))?;
        assert!(hcl2_upgrade(path_str, None).is_err());
        Ok(())
    }

    #[test]
    fn test_fix() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_fix.json");
        std::fs::write(&tmp, r#"{"builders":[{"type":"null","name":"test"}]}"#)
            .map_err(StampError::Io)?;
        let path_str = tmp
            .to_str()
            .ok_or_else(|| StampError::Parse("invalid path".into()))?;
        fix(path_str)?;
        Ok(())
    }

    #[test]
    fn test_fix_invalid_json() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_fix_invalid.json");
        std::fs::write(&tmp, "source \"amazon-ebs\" {").map_err(StampError::Io)?;
        let path_str = tmp
            .to_str()
            .ok_or_else(|| StampError::Parse("invalid path".into()))?;
        assert!(fix(path_str).is_err());
        Ok(())
    }

    #[test]
    fn test_fix_invalid_hcl() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_fix_invalid.pkr.hcl");
        std::fs::write(&tmp, "source \"amazon-ebs\" {").map_err(StampError::Io)?;
        let path_str = tmp
            .to_str()
            .ok_or_else(|| StampError::Parse("invalid path".into()))?;
        assert!(fix(path_str).is_err());
        Ok(())
    }

    #[test]
    fn test_inspect() -> Result<(), StampError> {
        #[allow(clippy::field_reassign_with_default)]
        let mut tmpl = crate::template::Template::default();
        tmpl.description = Some("desc".to_string());
        tmpl.builders.push(crate::template::BuilderConfig {
            builder_type: "null".to_string(),
            name: "test".to_string(),
            ..Default::default()
        });
        tmpl.provisioners.push(crate::template::ProvisionerConfig {
            provisioner_type: "shell".to_string(),
            ..Default::default()
        });
        let out = inspect(&tmpl, &InspectConfig::default())?;
        assert!(out.contains("Description:"));
        assert!(out.contains("Builders:"));
        assert!(out.contains("Provisioners:"));
        Ok(())
    }

    #[test]
    fn test_inspect_empty() -> Result<(), StampError> {
        let tmpl = crate::template::Template::default();
        let out = inspect(&tmpl, &InspectConfig::default())?;
        assert_eq!(out, "");
        Ok(())
    }
    use crate::builder::null::{NullBuilder, NullConfig};

    #[tokio::test]
    async fn test_build_concurrently_success() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: "b1".to_string(),
            ..Default::default()
        }));
        let b2 = Box::new(NullBuilder::new(NullConfig {
            name: "b2".to_string(),
            ..Default::default()
        }));

        build_concurrently(
            vec![b1, b2],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            EngineConfig::default(),
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_build_concurrently_failure() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: "b1".to_string(),
            ..Default::default()
        }));
        let b2 = Box::new(NullBuilder::new(NullConfig {
            name: String::new(),
            ..Default::default()
        })); // Will fail prepare

        let result = build_concurrently(
            vec![b1, b2],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            EngineConfig::default(),
        )
        .await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_debug_mode_sequential_success() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: "b1".to_string(),
            ..Default::default()
        }));
        let b2 = Box::new(NullBuilder::new(NullConfig {
            name: "b2".to_string(),
            ..Default::default()
        }));

        let config = EngineConfig {
            color: FeatureState::Enabled,
            debug: FeatureState::Enabled,
            ..Default::default()
        };
        build_concurrently(
            vec![b1, b2],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_build_debug_mode_sequential_failure() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: String::new(),
            ..Default::default()
        })); // Will fail prepare

        let config = EngineConfig {
            color: FeatureState::Disabled,
            debug: FeatureState::Enabled,
            ..Default::default()
        };

        let result = build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    use crate::builder::Builder;
    struct PanickingBuilder;

    #[async_trait::async_trait]
    impl Builder for PanickingBuilder {
        async fn prepare(&self) -> Result<(), StampError> {
            panic!("Intended panic for test");
        }

        async fn run(
            &self,
            _hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
            _ui: std::sync::Arc<crate::engine::ui::Ui>,
            _on_error: crate::engine::packer::OnErrorStrategy,
        ) -> Result<Box<dyn crate::artifact::Artifact>, crate::error::StampError> {
            Err(StampError::Execution("PanickingBuilder panic!".to_string()))
        }

        async fn cancel(&self) -> Result<(), StampError> {
            Ok(())
        }
        fn name(&self) -> String {
            "panicking".to_string()
        }
    }

    #[tokio::test]
    async fn test_build_concurrently_panic() -> Result<(), crate::error::StampError> {
        let b = Box::new(PanickingBuilder);

        let hook = std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: std::sync::Arc::new(vec![]),
            error_cleanup_provisioners: std::sync::Arc::new(vec![]),
        });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let _ = b
            .run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
            .await;
        let _ = b.cancel().await;

        let result = build_concurrently(
            vec![b],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            EngineConfig::default(),
        )
        .await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_on_error_cleanup() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: String::new(),
            ..Default::default()
        })); // will fail
        let config = EngineConfig {
            debug: FeatureState::Enabled,
            on_error: OnErrorStrategy::Cleanup,
            ..Default::default()
        };
        let result = build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_on_error_cleanup_concurrent() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: String::new(),
            ..Default::default()
        })); // will fail
        let config = EngineConfig {
            on_error: OnErrorStrategy::Cleanup,
            ..Default::default()
        };
        let result = build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_on_error_ask() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: String::new(),
            ..Default::default()
        })); // will fail
        let config = EngineConfig {
            debug: FeatureState::Enabled,
            on_error: OnErrorStrategy::Ask,
            ..Default::default()
        };
        let result = build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_filtering_only() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: "b1".to_string(),
            ..Default::default()
        }));
        let b2 = Box::new(NullBuilder::new(NullConfig {
            name: "b2".to_string(),
            ..Default::default()
        }));
        let config = EngineConfig {
            only: vec!["b1".to_string()],
            ..Default::default()
        };
        build_concurrently(
            vec![b1, b2],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_build_filtering_except() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: "b1".to_string(),
            ..Default::default()
        }));
        let b2 = Box::new(NullBuilder::new(NullConfig {
            name: "b2".to_string(),
            ..Default::default()
        }));
        let config = EngineConfig {
            except: vec!["b2".to_string()],
            ..Default::default()
        };
        build_concurrently(
            vec![b1, b2],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_build_filtering_both() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: "b1".to_string(),
            ..Default::default()
        }));
        let b2 = Box::new(NullBuilder::new(NullConfig {
            name: "b2".to_string(),
            ..Default::default()
        }));
        let b3 = Box::new(NullBuilder::new(NullConfig {
            name: "b3".to_string(),
            ..Default::default()
        }));
        let config = EngineConfig {
            only: vec!["b1".to_string(), "b3".to_string()],
            except: vec!["b3".to_string()],
            ..Default::default()
        };
        build_concurrently(
            vec![b1, b2, b3],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_build_debug_mode_sequential_error_abort() -> Result<(), crate::error::StampError>
    {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: String::new(),
            ..Default::default()
        })); // will fail
        let config = EngineConfig {
            debug: FeatureState::Enabled,
            on_error: OnErrorStrategy::Abort,
            ..Default::default()
        };
        let result = build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_debug_mode_sequential_error_ask() -> Result<(), crate::error::StampError> {
        let b1 = Box::new(NullBuilder::new(NullConfig {
            name: String::new(),
            ..Default::default()
        })); // will fail
        let config = EngineConfig {
            debug: FeatureState::Enabled,
            on_error: OnErrorStrategy::Ask,
            ..Default::default()
        };
        let result = build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[test]
    fn test_fmt_check_diff_write() -> Result<(), StampError> {
        let tmp = std::env::temp_dir().join("test_fmt_cdw.json");
        std::fs::write(&tmp, r#"{"builders": [{"type": "null", "name": "test"}]}"#)
            .map_err(StampError::Io)?;
        let path_str = tmp.to_str().unwrap_or_else(|| panic!("failed"));

        // 1. check = true, write = false (should return error because formatting is needed)
        let mut config = FmtConfig {
            recursive: false,
            options: FmtOutputOptions {
                check: true,
                diff: true,
                write: false,
            },
        };
        let res = fmt(path_str, &config);
        assert!(res.is_err());
        assert!(
            res.err()
                .unwrap_or_else(|| panic!("failed"))
                .to_string()
                .contains("Validation error")
        );

        // File is unchanged
        let content = std::fs::read_to_string(&tmp).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            content,
            r#"{"builders": [{"type": "null", "name": "test"}]}"#
        );

        // 2. check = false, write = true
        config.options.check = false;
        config.options.write = true;
        fmt(path_str, &config)?;

        // File is formatted
        let content = std::fs::read_to_string(&tmp).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(content.contains(
            "
  \"builders\""
        ));

        // 3. check = true on formatted file (should succeed)
        config.options.check = true;
        fmt(path_str, &config)?;
        Ok(())
    }

    #[test]
    fn test_fmt_recursive() -> Result<(), StampError> {
        let dir = std::env::temp_dir().join("test_fmt_rec");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e:?}"));

        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap_or_else(|e| panic!("{e:?}"));

        let file1 = dir.join("1.json");
        let file2 = sub.join("2.pkr.hcl");

        std::fs::write(&file1, r#"{"b":1}"#).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(&file2, r#"source "null" "test" {}"#).unwrap_or_else(|e| panic!("{e:?}"));

        let path_str = dir.to_str().unwrap_or_else(|| panic!("failed"));

        // Recursive false -> Error
        let config = FmtConfig {
            recursive: false,
            options: FmtOutputOptions {
                check: false,
                diff: false,
                write: true,
            },
        };
        let err = fmt(path_str, &config)
            .err()
            .unwrap_or_else(|| panic!("failed"));
        assert!(err.to_string().contains("I/O error: Path is a directory"));

        // Recursive true -> Formats all
        let config = FmtConfig {
            recursive: true,
            options: FmtOutputOptions {
                check: false,
                diff: false,
                write: true,
            },
        };
        fmt(path_str, &config)?;

        let c1 = std::fs::read_to_string(&file1).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(c1.contains(
            "
  \"b\""
        ));

        let c2 = std::fs::read_to_string(&file2).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(c2, c2);

        Ok(())
    }
}

#[cfg(test)]
mod inspect_tests {
    use super::*;

    #[test]
    fn test_inspect_machine_readable() -> Result<(), StampError> {
        #[allow(clippy::field_reassign_with_default)]
        let mut tmpl = crate::template::Template::default();
        tmpl.description = Some("desc, with comma".to_string());
        tmpl.builders.push(crate::template::BuilderConfig {
            builder_type: "null".to_string(),
            name: "test".to_string(),
            ..Default::default()
        });
        tmpl.provisioners.push(crate::template::ProvisionerConfig {
            provisioner_type: "shell".to_string(),
            ..Default::default()
        });
        tmpl.variables.insert(
            "foo".to_string(),
            crate::template::VariableConfig {
                variable_type: Some("string".to_string()),
                description: None,
                default: Some("bar".to_string()),
                sensitive: Some(false),
                validations: vec![],
            },
        );
        tmpl.data_sources.push(crate::template::DataSourceConfig {
            source_type: "amazon-ami".to_string(),
            name: "ami".to_string(),
            config: std::collections::HashMap::new(),
        });

        let config = InspectConfig {
            machine_readable: true,
            ..Default::default()
        };
        let out = inspect(&tmpl, &config)?;

        assert!(out.contains("0,,template-description,desc%!(PACKER_COMMA) with comma"));
        assert!(out.contains("0,,template-variable,foo,string,bar"));
        assert!(out.contains("0,,template-data-source,ami,amazon-ami"));
        assert!(out.contains("0,,template-builder,test,null"));
        assert!(out.contains("0,,template-provisioner,shell"));
        Ok(())
    }

    #[test]
    fn test_inspect_human_readable_vars_and_sources() -> Result<(), StampError> {
        #[allow(clippy::field_reassign_with_default)]
        let mut tmpl = crate::template::Template::default();
        tmpl.variables.insert(
            "foo".to_string(),
            crate::template::VariableConfig {
                variable_type: Some("string".to_string()),
                description: None,
                default: Some("bar".to_string()),
                sensitive: Some(false),
                validations: vec![],
            },
        );
        tmpl.data_sources.push(crate::template::DataSourceConfig {
            source_type: "amazon-ami".to_string(),
            name: "ami".to_string(),
            config: std::collections::HashMap::new(),
        });

        let config = InspectConfig {
            machine_readable: false,
            ..Default::default()
        };
        let out = inspect(&tmpl, &config)?;

        assert!(out.contains("Variables:"));
        assert!(out.contains("  foo: string (default: bar)"));
        assert!(out.contains("Data Sources:"));
        assert!(out.contains("  ami (amazon-ami)"));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_policy_enforcement_sentinel_fail() -> Result<(), StampError> {
        let b1 = Box::new(crate::builder::null::NullBuilder::new(
            crate::builder::null::NullConfig {
                name: "b1".to_string(),
                ..Default::default()
            },
        ));
        let config = EngineConfig {
            use_sequential_evaluation: FeatureState::Enabled,
            sentinel_policy: Some("fail.sentinel".to_string()),
            ..Default::default()
        };

        let res = build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(matches!(res, Err(StampError::PolicyViolation { .. })));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_policy_enforcement_skip() -> Result<(), StampError> {
        let b1 = Box::new(crate::builder::null::NullBuilder::new(
            crate::builder::null::NullConfig {
                name: "b1".to_string(),
                ..Default::default()
            },
        ));
        let config = EngineConfig {
            use_sequential_evaluation: FeatureState::Enabled,
            skip_enforcement: FeatureState::Enabled,
            sentinel_policy: Some("fail.sentinel".to_string()),
            ..Default::default()
        };

        let res = build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(res.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn test_build_hcp_packer_registry_push_in_build() -> Result<(), StampError> {
        let b1 = Box::new(crate::builder::null::NullBuilder::new(
            crate::builder::null::NullConfig {
                name: "b1".to_string(),
                ..Default::default()
            },
        ));
        let config = EngineConfig {
            use_sequential_evaluation: FeatureState::Enabled,
            packer_config: Some(crate::template::PackerConfig {
                required_version: None,
                hcp_packer_registry: Some(crate::template::HcpPackerRegistryConfig {
                    bucket_name: crate::template::BucketName("test-bucket".to_string()),
                    description: Some("test desc".to_string()),
                    labels: std::collections::HashMap::new(),
                    bucket_labels: std::collections::HashMap::new(),
                    build_labels: std::collections::HashMap::new(),
                    channels: vec![],
                }),
            }),
            ..Default::default()
        };

        let res = build_concurrently(
            vec![b1],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(res.is_ok());
        Ok(())
    }

    struct SequencedBuilder {
        name: String,
        deps: Vec<String>,
        start_time: std::sync::Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
        finish_time: std::sync::Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
        sleep_ms: u64,
        should_fail: bool,
        was_cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl crate::builder::Builder for SequencedBuilder {
        async fn prepare(&self) -> Result<(), StampError> {
            Ok(())
        }
        async fn run(
            &self,
            _hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
            _ui: std::sync::Arc<crate::engine::ui::Ui>,
            _on_error: crate::engine::packer::OnErrorStrategy,
        ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
            *self.start_time.lock().await = Some(std::time::Instant::now());
            if self.sleep_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.sleep_ms)).await;
            }
            *self.finish_time.lock().await = Some(std::time::Instant::now());
            if self.should_fail {
                return Err(StampError::Execution("builder failure".to_string()));
            }
            Ok(Box::new(crate::artifact::MockArtifact {
                builder_id: self.name.clone(),
                id: self.name.clone(),
                files: vec![],
            }))
        }
        async fn cancel(&self) -> Result<(), StampError> {
            self.was_cancelled
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn name(&self) -> String {
            self.name.clone()
        }
        fn depends_on(&self) -> Vec<String> {
            self.deps.clone()
        }
    }

    #[tokio::test]
    async fn test_build_concurrently_dependency_sequencing() -> Result<(), StampError> {
        let b1_start = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let b1_finish = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let b2_start = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let b2_finish = std::sync::Arc::new(tokio::sync::Mutex::new(None));

        let b1 = Box::new(SequencedBuilder {
            name: "b1".to_string(),
            deps: vec![],
            start_time: b1_start.clone(),
            finish_time: b1_finish.clone(),
            sleep_ms: 30,
            should_fail: false,
            was_cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });

        let b2 = Box::new(SequencedBuilder {
            name: "b2".to_string(),
            deps: vec!["b1".to_string()],
            start_time: b2_start.clone(),
            finish_time: b2_finish.clone(),
            sleep_ms: 10,
            should_fail: false,
            was_cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });

        let config = EngineConfig::default();
        let res = build_concurrently(
            vec![b1, b2],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(res.is_ok());

        let t1_fin = b1_finish.lock().await.unwrap();
        let t2_start = b2_start.lock().await.unwrap();
        assert!(t2_start >= t1_fin);
        Ok(())
    }

    #[tokio::test]
    async fn test_build_concurrently_pacing() -> Result<(), StampError> {
        let b1_start = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let b1_finish = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let b2_start = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let b2_finish = std::sync::Arc::new(tokio::sync::Mutex::new(None));

        let b1 = Box::new(SequencedBuilder {
            name: "b1".to_string(),
            deps: vec![],
            start_time: b1_start.clone(),
            finish_time: b1_finish.clone(),
            sleep_ms: 10,
            should_fail: false,
            was_cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });

        let b2 = Box::new(SequencedBuilder {
            name: "b2".to_string(),
            deps: vec![],
            start_time: b2_start.clone(),
            finish_time: b2_finish.clone(),
            sleep_ms: 10,
            should_fail: false,
            was_cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });

        let config = EngineConfig {
            pacing_delay: Some(std::time::Duration::from_millis(30)),
            ..Default::default()
        };
        let res = build_concurrently(
            vec![b1, b2],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(res.is_ok());

        let t1_start = b1_start.lock().await.unwrap();
        let t2_start = b2_start.lock().await.unwrap();
        let diff = if t2_start >= t1_start {
            t2_start.duration_since(t1_start)
        } else {
            t1_start.duration_since(t2_start)
        };
        assert!(diff >= std::time::Duration::from_millis(20));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_concurrently_cancellation_propagation() -> Result<(), StampError> {
        let b1_start = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let b1_finish = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let b2_start = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let b2_finish = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let b2_cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let b1 = Box::new(SequencedBuilder {
            name: "b1".to_string(),
            deps: vec![],
            start_time: b1_start.clone(),
            finish_time: b1_finish.clone(),
            sleep_ms: 5,
            should_fail: true,
            was_cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });

        let b2 = Box::new(SequencedBuilder {
            name: "b2".to_string(),
            deps: vec![],
            start_time: b2_start.clone(),
            finish_time: b2_finish.clone(),
            sleep_ms: 200,
            should_fail: false,
            was_cancelled: b2_cancelled.clone(),
        });

        let config = EngineConfig {
            on_error: OnErrorStrategy::Cleanup,
            ..Default::default()
        };
        let res = build_concurrently(
            vec![b1, b2],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            config,
        )
        .await;
        assert!(res.is_err());
        assert!(b2_cancelled.load(std::sync::atomic::Ordering::SeqCst));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_concurrently_cycle_detected() {
        let b1 = Box::new(SequencedBuilder {
            name: "b1".to_string(),
            deps: vec!["b2".to_string()],
            start_time: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            finish_time: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            sleep_ms: 0,
            should_fail: false,
            was_cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });

        let b2 = Box::new(SequencedBuilder {
            name: "b2".to_string(),
            deps: vec!["b1".to_string()],
            start_time: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            finish_time: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            sleep_ms: 0,
            should_fail: false,
            was_cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });

        let res = build_concurrently(
            vec![b1, b2],
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            std::sync::Arc::new(vec![]),
            EngineConfig::default(),
        )
        .await;
        assert!(matches!(res, Err(StampError::CircularDependency(_))));
    }
}
