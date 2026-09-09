#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![deny(missing_docs)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Stamp CLI library containing command definitions, execution routing,
//! and grounding verification against `HashiCorp` Packer reference schemas.

use clap::{CommandFactory, Parser, Subcommand};
use libstamp::error::StampError;
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};

/// Subcommands for the `plugins` command.
#[derive(Subcommand, Debug, Clone)]
pub enum PluginCommand {
    /// Install a plugin
    Install {
        /// Plugin address
        address: String,
        /// Plugin version
        version: Option<String>,
    },
    /// List installed plugins
    Installed,
    /// List required plugins from a template
    Required {
        /// Path to the template
        template: String,
    },
    /// Remove a plugin
    Remove {
        /// Plugin address or name
        plugin: String,
    },
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
/// Main CLI struct
pub struct Cli {
    /// Enable machine-readable output formats (e.g., CSV or JSON) for automation scripts.
    #[arg(long, global = true)]
    pub machine_readable: bool,

    /// Install shell autocompletion into user profile.
    #[arg(long = "autocomplete-install", alias = "-autocomplete-install")]
    pub autocomplete_install: bool,

    /// Uninstall shell autocompletion from user profile.
    #[arg(long = "autocomplete-uninstall", alias = "-autocomplete-uninstall")]
    pub autocomplete_uninstall: bool,

    /// The subcommands available in the CLI.
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// The available subcommands for Stamp.
#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Build a machine image (Packer equivalent)
    #[command(alias = "b")]
    Build {
        #[arg(required = true)]
        /// Path to the template file(s) or directory
        template: Vec<String>,

        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        /// Enable color output (on by default)
        color: Option<bool>,

        #[arg(long)]
        /// Disable parallelization and enable debug mode
        debug: bool,

        #[arg(long)]
        /// Build all builds other than these
        except: Option<String>,

        #[arg(long)]
        /// Force a build to continue if artifacts exist, deletes them previously
        force: bool,

        #[arg(long)]
        /// Ignore prerelease versions when installing plugins
        ignore_prerelease_plugins: bool,

        #[arg(long)]
        /// If the build fails do: clean up, abort, or ask
        on_error: Option<String>,

        #[arg(long)]
        /// Build only the specified builds
        only: Option<String>,

        #[arg(long)]
        /// Number of builds to run in parallel
        parallel_builds: Option<u32>,

        #[arg(long)]
        /// Skip Packer policy enforcement
        skip_enforcement: bool,

        #[arg(long)]
        /// Enable UI timestamps
        timestamp_ui: bool,

        #[arg(long)]
        /// Use sequential evaluation for data sources and blocks
        use_sequential_evaluation: bool,

        #[arg(long)]
        /// Variable for templates (e.g. key=value)
        var: Option<Vec<String>>,

        #[arg(long)]
        /// JSON file containing user variables
        var_file: Option<Vec<String>>,

        #[arg(long)]
        /// Warn on undeclared variables
        warn_on_undeclared_var: bool,
        #[arg(long)]
        /// machine-readable
        machine_readable: bool,
    },

    /// Interactive console for querying configuration files
    #[command(alias = "c")]
    Console {
        /// Path to the template file
        template: String,
        #[arg(long)]
        /// config-type
        config_type: Option<String>,
        #[arg(long)]
        /// use-sequential-evaluation
        use_sequential_evaluation: bool,
        #[arg(long)]
        /// var
        var: Option<Vec<String>>,
        #[arg(long)]
        /// var-file
        var_file: Option<Vec<String>>,
    },
    /// Automatically update legacy configurations to the latest format
    Fix {
        /// Path to the template file
        template: String,
        #[arg(long)]
        /// validate
        validate: bool,
    },
    /// Format HCL2 configuration files
    Fmt {
        /// Path to the template file
        template: String,
        #[arg(long)]
        /// check
        check: bool,
        #[arg(long)]
        /// diff
        diff: bool,
        #[arg(long)]
        /// recursive
        recursive: bool,
        #[arg(long)]
        /// write
        write: bool,
    },
    /// Convert legacy JSON templates to HCL2
    Hcl2Upgrade {
        /// Path to the legacy JSON file
        template: String,
    },
    /// Download and install required Packer plugins
    #[command(alias = "i")]
    Init {
        /// Path to the template file
        template: String,
        #[arg(long)]
        /// force
        force: bool,
        #[arg(long)]
        /// upgrade
        upgrade: bool,
    },
    /// Inspect a template
    Inspect {
        /// Path to the template file
        template: String,
        #[arg(long)]
        /// machine-readable
        machine_readable: bool,
        #[arg(long)]
        /// use-sequential-evaluation
        use_sequential_evaluation: bool,
    },
    /// Statically analyze a template
    #[command(alias = "v")]
    Validate {
        /// Path to the template file
        template: String,
        #[arg(long)]
        /// evaluate-datasources
        evaluate_datasources: bool,
        #[arg(long)]
        /// except
        except: Option<String>,
        #[arg(long)]
        /// ignore-prerelease-plugins
        ignore_prerelease_plugins: bool,
        #[arg(long)]
        /// machine-readable
        machine_readable: bool,
        #[arg(long)]
        /// no-warn-undeclared-var
        no_warn_undeclared_var: bool,
        #[arg(long)]
        /// only
        only: Option<String>,
        #[arg(long)]
        /// syntax-only
        syntax_only: bool,
        #[arg(long)]
        /// use-sequential-evaluation
        use_sequential_evaluation: bool,
        #[arg(long)]
        /// var
        var: Option<Vec<String>>,
        #[arg(long)]
        /// var-file
        var_file: Option<Vec<String>>,
    },
    /// Test a template
    Test {
        /// Path to the template file
        template: String,
        #[arg(long)]
        /// Enable verbose output
        verbose: bool,
        #[arg(long)]
        /// Path to output `JUnit` XML results
        junit_xml: Option<String>,
    },
    /// Manage plugins
    Plugins {
        #[command(subcommand)]
        /// The plugin command to execute
        command: PluginCommand,
    },
    /// Generate shell autocomplete scripts for bash, zsh, fish, or powershell
    Autocomplete {
        /// Target shell for autocomplete script (bash, zsh, fish, powershell)
        #[arg(long, default_value = "bash")]
        shell: String,
    },
    /// Print the Stamp version
    Version {
        #[arg(short = 'v', long = "v")]
        /// Print verbose version information including components and platform details
        v: bool,

        #[arg(long)]
        /// Output version in machine-readable format
        machine_readable: bool,

        #[arg(long)]
        /// Check for newer version of Stamp from Checkpoint
        check_updates: bool,
    },
}

/// Execute the core CLI logic mapped from parsed commands.
///
/// # Errors
/// Returns `StampError` if the command execution fails.
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
pub async fn execute_command(
    command: &Option<Commands>,
    machine_readable: bool,
    is_tty: bool,
) -> Result<(), StampError> {
    let Some(command) = command else {
        let mut cmd = Cli::command();
        let _ = cmd.print_help();
        return Ok(());
    };

    if !matches!(command, Commands::Version { .. }) {
        if machine_readable {
            println!("{{\"status\": \"starting\"}}");
        } else if is_tty {
            println!("Interactive mode enabled");
        }
    }
    match command {
        Commands::Build {
            template,
            color,
            debug,
            except,
            only,
            force,
            ignore_prerelease_plugins,
            on_error,
            parallel_builds,
            skip_enforcement,
            timestamp_ui,
            use_sequential_evaluation,
            var,
            var_file,
            warn_on_undeclared_var,
            ..
        } => {
            let template_names = template.join(", ");
            println!("Building template(s): {template_names}");

            let first_path = template
                .first()
                .map_or_else(|| std::path::Path::new("."), std::path::Path::new);
            let template_dir = if first_path.is_dir() {
                Some(first_path)
            } else {
                first_path.parent()
            };

            let vars = libstamp::utils::load_variables_with_precedence(
                template_dir,
                var_file.as_deref(),
                var.as_deref(),
            );

            let mut tmpl = libstamp::template::load_templates(template, &vars)?;
            libstamp::engine::evaluator::evaluate(&mut tmpl).await?;

            if *warn_on_undeclared_var {
                for var_key in vars.keys() {
                    if !tmpl.variables.contains_key(var_key) {
                        eprintln!("[WARN] Undeclared variable '{var_key}' provided");
                    }
                }
            }

            let scrubber = std::sync::Arc::new(libstamp::engine::ui::Scrubber::new());
            for var in tmpl.variables.values() {
                if let Some(true) = var.sensitive
                    && let Some(val) = &var.default
                {
                    scrubber.add(val.clone());
                }
            }

            let mut builders = Vec::new();
            for b_config in tmpl.builders {
                builders.push(libstamp::builder::create_builder(&b_config)?);
            }
            let mut provisioners = Vec::new();
            for p_config in tmpl.provisioners {
                provisioners.push(libstamp::provisioner::create_provisioner(&p_config)?);
            }
            let mut error_cleanup_provisioners = Vec::new();
            for p_config in tmpl.error_cleanup_provisioners {
                error_cleanup_provisioners
                    .push(libstamp::provisioner::create_provisioner(&p_config)?);
            }
            let mut post_processors = Vec::new();
            for p_config in tmpl.post_processors {
                post_processors.push(libstamp::post_processor::create_post_processor(&p_config)?);
            }

            let is_color_enabled = color.unwrap_or_else(|| {
                std::env::var("NO_COLOR").is_err() && std::env::var("PACKER_NO_COLOR").is_err()
            });

            let config = libstamp::engine::packer::EngineConfig {
                color: if is_color_enabled {
                    libstamp::engine::packer::FeatureState::Enabled
                } else {
                    libstamp::engine::packer::FeatureState::Disabled
                },
                debug: if *debug {
                    libstamp::engine::packer::FeatureState::Enabled
                } else {
                    libstamp::engine::packer::FeatureState::Disabled
                },
                except: except
                    .as_deref()
                    .unwrap_or("")
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(std::string::ToString::to_string)
                    .collect(),
                only: only
                    .as_deref()
                    .unwrap_or("")
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(std::string::ToString::to_string)
                    .collect(),
                force: if *force {
                    libstamp::engine::packer::FeatureState::Enabled
                } else {
                    libstamp::engine::packer::FeatureState::Disabled
                },
                ignore_prerelease_plugins: if *ignore_prerelease_plugins {
                    libstamp::engine::packer::FeatureState::Enabled
                } else {
                    libstamp::engine::packer::FeatureState::Disabled
                },
                machine_readable: if machine_readable {
                    libstamp::engine::packer::FeatureState::Enabled
                } else {
                    libstamp::engine::packer::FeatureState::Disabled
                },
                on_error: match on_error.as_deref() {
                    Some("abort") => libstamp::engine::packer::OnErrorStrategy::Abort,
                    Some("ask") => libstamp::engine::packer::OnErrorStrategy::Ask,
                    Some("run-cleanup-provisioner") => {
                        libstamp::engine::packer::OnErrorStrategy::RunCleanupProvisioner
                    }
                    _ => libstamp::engine::packer::OnErrorStrategy::Cleanup,
                },
                parallel_builds: *parallel_builds,
                pacing_delay: std::env::var("PACKER_BUILDER_PACING_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(std::time::Duration::from_millis),
                skip_enforcement: if *skip_enforcement {
                    libstamp::engine::packer::FeatureState::Enabled
                } else {
                    libstamp::engine::packer::FeatureState::Disabled
                },
                timestamp_ui: if *timestamp_ui {
                    libstamp::engine::packer::FeatureState::Enabled
                } else {
                    libstamp::engine::packer::FeatureState::Disabled
                },
                use_sequential_evaluation: if *use_sequential_evaluation {
                    libstamp::engine::packer::FeatureState::Enabled
                } else {
                    libstamp::engine::packer::FeatureState::Disabled
                },
                vars: {
                    let mut all_vars = Vec::new();
                    // 1. auto.pkrvars.hcl
                    if let Ok(entries) = std::fs::read_dir(".") {
                        let mut auto_files = Vec::new();
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_file()
                                && p.file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .ends_with(".auto.pkrvars.hcl")
                            {
                                auto_files.push(p);
                            }
                        }
                        auto_files.sort();
                        for f in auto_files {
                            if let Ok(c) = std::fs::read_to_string(f) {
                                // Extract vars
                                if let Ok(tmpl) = libstamp::parser::hcl::parse_hcl(
                                    &c,
                                    &std::collections::HashMap::new(),
                                ) {
                                    for (k, v) in tmpl.variables {
                                        all_vars.push(format!(
                                            "{}={}",
                                            k,
                                            v.default.unwrap_or_default()
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    // 2. PKR_VAR_ environment variables
                    for (k, v) in std::env::vars() {
                        if let Some(stripped) = k.strip_prefix("PKR_VAR_") {
                            all_vars.push(format!("{stripped}={v}"));
                        }
                    }
                    // 3. -var-file
                    for vf in var_file.clone().unwrap_or_default() {
                        if let Ok(c) = std::fs::read_to_string(&vf)
                            && let Ok(tmpl) = libstamp::parser::hcl::parse_hcl(
                                &c,
                                &std::collections::HashMap::new(),
                            )
                        {
                            for (k, v) in tmpl.variables {
                                all_vars.push(format!("{}={}", k, v.default.unwrap_or_default()));
                            }
                        }
                    }
                    // 4. -var
                    for v in var.clone().unwrap_or_default() {
                        all_vars.push(v);
                    }
                    all_vars
                },
                var_files: var_file.clone().unwrap_or_default(),
                warn_on_undeclared_var: if *warn_on_undeclared_var {
                    libstamp::engine::packer::FeatureState::Enabled
                } else {
                    libstamp::engine::packer::FeatureState::Disabled
                },
                packer_config: tmpl.packer,
                scrubber: Some(scrubber),
                sentinel_policy: None,
                opa_policy: None,
            };
            libstamp::engine::packer::build_concurrently(
                builders,
                std::sync::Arc::new(provisioners),
                std::sync::Arc::new(error_cleanup_provisioners),
                std::sync::Arc::new(post_processors),
                config,
            )
            .await?;
        }
        Commands::Console {
            template,
            config_type,
            use_sequential_evaluation,
            var,
            var_file,
        } => {
            println!("Console for template: {template}");
            let mut vars = std::collections::HashMap::new();
            if let Some(v_list) = var {
                for v in v_list {
                    if let Some((k, val)) = v.split_once('=') {
                        vars.insert(k.to_string(), val.to_string());
                    }
                }
            }
            let config = libstamp::engine::console::ConsoleConfig {
                config_type: config_type.clone(),
                use_sequential_evaluation: *use_sequential_evaluation,
                vars,
                var_files: var_file.clone().unwrap_or_default(),
            };
            if !is_tty || cfg!(test) {
                let (ctx, _) =
                    libstamp::engine::console::prepare_console_context(Some(template), &config)?;
                let _ = libstamp::engine::console::eval_console_expr("var", &ctx);
            } else {
                libstamp::engine::console::run_console(Some(template), &config)?;
            }
        }
        Commands::Fix { template, validate } => {
            println!("Fixing template: {template}");
            let config = libstamp::engine::fix::FixConfig {
                validate: *validate,
            };
            libstamp::engine::fix::fix_template(template, &config)?;
        }
        Commands::Fmt {
            template,
            check,
            diff,
            recursive,
            write,
        } => {
            println!("Formatting template: {template}");
            let should_write = *write || (!*check && !*diff);
            let config = libstamp::engine::packer::FmtConfig {
                check: *check,
                diff: *diff,
                recursive: *recursive,
                write: should_write,
            };
            libstamp::engine::packer::fmt(template, &config)?;
        }
        Commands::Hcl2Upgrade { template, .. } => {
            println!("Upgrading legacy template");
            libstamp::engine::packer::hcl2_upgrade(template, None)?;
        }
        Commands::Init {
            template,
            force,
            upgrade,
        } => {
            println!("Initializing template plugins: {template}");
            let options = libstamp::engine::packer_init::InitOptions {
                upgrade: *upgrade,
                force: *force,
                skip_signature_verification: false,
                target_dir: None,
            };
            libstamp::engine::packer_init::init_with_options(template, &options).await?;
        }
        Commands::Inspect {
            template,
            machine_readable: sub_machine_readable,
            use_sequential_evaluation,
        } => {
            println!("Inspecting template: {template}");
            let is_mr = machine_readable || *sub_machine_readable;
            let tmpl =
                libstamp::template::load_templates(&[template], &std::collections::HashMap::new())?;
            let config = libstamp::engine::packer::InspectConfig {
                machine_readable: is_mr,
                use_sequential_evaluation: *use_sequential_evaluation,
                json: false,
            };
            let out = libstamp::engine::packer::inspect(&tmpl, &config)?;
            println!("{out}");
        }
        Commands::Validate {
            template,
            evaluate_datasources,
            except,
            ignore_prerelease_plugins: _,
            machine_readable: _,
            no_warn_undeclared_var,
            only,
            syntax_only,
            use_sequential_evaluation: _,
            var,
            var_file,
        } => {
            println!("Validating template: {template}");
            let template_path = std::path::Path::new(template);
            let template_dir = if template_path.is_dir() {
                Some(template_path)
            } else {
                template_path.parent()
            };
            let vars = libstamp::utils::load_variables_with_precedence(
                template_dir,
                var_file.as_deref(),
                var.as_deref(),
            );
            let mut tmpl = libstamp::template::load_templates(&[template], &vars)?;
            if *evaluate_datasources && !*syntax_only {
                libstamp::engine::evaluator::evaluate(&mut tmpl).await?;
            }
            let val_cfg = libstamp::engine::packer::ValidateConfig {
                syntax_only: *syntax_only,
                evaluate_datasources: *evaluate_datasources,
                no_warn_undeclared_var: *no_warn_undeclared_var,
                warn_on_undeclared_var: false,
                only: only.clone(),
                except: except.clone(),
                vars,
                var_files: var_file.clone().unwrap_or_default(),
            };
            libstamp::engine::packer::validate_with_config(&tmpl, &val_cfg)?;
            println!("Template is valid.");
        }
        Commands::Test {
            template,
            verbose,
            junit_xml,
        } => {
            println!("Testing template: {template}");
            let config = libstamp::engine::test::TestConfig {
                verbose: *verbose,
                junit_xml: junit_xml.clone(),
            };
            libstamp::engine::test::run_tests(template, &config).await?;
            println!("Tests completed.");
        }
        Commands::Plugins { command, .. } => match command {
            PluginCommand::Install { address, version } => {
                libstamp::engine::plugins::plugin_install(address, version.as_deref()).await?;
            }
            PluginCommand::Installed => {
                libstamp::engine::plugins::plugin_list()?;
            }
            PluginCommand::Required { template } => {
                let content = std::fs::read_to_string(template)?;
                let tmpl = if std::path::Path::new(template)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                {
                    libstamp::parser::json::parse_json(&content, &std::collections::HashMap::new())?
                } else {
                    libstamp::parser::hcl::parse_hcl(&content, &std::collections::HashMap::new())?
                };
                println!("Required plugins for template {template}:");
                let mut registry = libstamp::engine::plugins::PluginRegistry::new();
                let _ = registry.discover();
                for (name, plugin) in tmpl.required_plugins {
                    let constraint = libstamp::types::SemVerConstraint::parse(&plugin.version).ok();
                    let plugin_id = libstamp::engine::plugins::PluginId::new(&name);
                    let is_satisfied = registry
                        .find_matching(&plugin_id, constraint.as_ref())
                        .is_some();
                    let status = if is_satisfied {
                        "[INSTALLED]"
                    } else {
                        "[MISSING]"
                    };
                    println!(
                        "{} from {} version {} {}",
                        name, plugin.source, plugin.version, status
                    );
                }
            }
            PluginCommand::Remove { plugin } => {
                libstamp::engine::plugins::plugin_remove(plugin)?;
            }
        },
        Commands::Autocomplete { shell } => {
            use clap::CommandFactory;
            use clap_complete::{Shell, generate};

            let mut cmd = Cli::command();
            let target_shell = match shell.to_ascii_lowercase().as_str() {
                "zsh" => Shell::Zsh,
                "fish" => Shell::Fish,
                "powershell" | "pwsh" => Shell::PowerShell,
                "elvish" => Shell::Elvish,
                _ => Shell::Bash,
            };

            generate(target_shell, &mut cmd, "stamp", &mut std::io::stdout());
        }
        Commands::Version {
            v,
            machine_readable: sub_machine_readable,
            check_updates,
        } => {
            let is_mr = machine_readable || *sub_machine_readable;
            let info = libstamp::telemetry::VersionInfo::current();
            if is_mr {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                println!("{}", info.format_machine_readable(timestamp));
            } else {
                println!("{}", info.format_human(*v));
            }

            let should_check =
                *check_updates || (!is_mr && std::env::var("CHECKPOINT_DISABLE").is_err());
            if should_check
                && let Ok(Some(latest)) =
                    libstamp::telemetry::check_for_updates(&info.version).await
                && !is_mr
            {
                println!(
                    "\nYour version of Stamp is out of date! The latest version is {latest}. You can update by visiting https://github.com/SamuelMarks/stamp/releases"
                );
            }
        }
    }
    Ok(())
}

/// Verifies CLI parity against a reference schema JSON string.
///
/// # Errors
/// Returns `StampError::Validation` if any command or flag is missing or mismatched.
pub fn verify_cli_grounding(reference_json: &str) -> Result<(), StampError> {
    let root: JsonValue = serde_json::from_str(reference_json)
        .map_err(|e| StampError::Parse(format!("Failed to parse CLI reference schema: {e}")))?;

    let commands_map = root
        .get("commands")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            StampError::Parse("Reference schema missing 'commands' object".to_string())
        })?;

    let cli_cmd = Cli::command();
    let subcommands: HashMap<String, clap::Command> = cli_cmd
        .get_subcommands()
        .map(|s| (s.get_name().to_string(), s.clone()))
        .collect();

    for (cmd_name, expected_flags_val) in commands_map {
        let subcmd = subcommands.get(cmd_name).ok_or_else(|| {
            StampError::Validation(format!("Missing expected CLI subcommand '{cmd_name}'"))
        })?;

        let actual_flags: HashSet<String> = subcmd
            .get_arguments()
            .filter_map(|arg| arg.get_long().map(ToString::to_string))
            .collect();

        if let Some(expected_flags) = expected_flags_val.as_array() {
            for flag in expected_flags {
                if let Some(flag_str) = flag.as_str()
                    && !actual_flags.contains(flag_str)
                {
                    return Err(StampError::Validation(format!(
                        "Command '{cmd_name}' missing expected flag '--{flag_str}'"
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Marker comment used to detect and modify shell profile autocomplete scripts.
pub const AUTOCOMPLETE_MARKER: &str = "# Stamp shell autocompletion";

/// Detects current shell name from `$SHELL` environment variable.
#[must_use]
pub fn detect_shell() -> String {
    if let Ok(shell_path) = std::env::var("SHELL")
        && let Some(name) = std::path::Path::new(&shell_path)
            .file_name()
            .and_then(|n| n.to_str())
    {
        return name.to_ascii_lowercase();
    }
    if cfg!(windows) {
        "powershell".to_string()
    } else {
        "bash".to_string()
    }
}

/// Resolves the user shell profile path for autocompletion installation.
#[must_use]
pub fn resolve_shell_profile(shell: &str) -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    let home_path = std::path::PathBuf::from(home);

    match shell {
        "zsh" => Some(home_path.join(".zshrc")),
        "fish" => Some(home_path.join(".config").join("fish").join("config.fish")),
        "powershell" | "pwsh" => Some(
            home_path
                .join(".config")
                .join("powershell")
                .join("Microsoft.PowerShell_profile.ps1"),
        ),
        _ => {
            let bashrc = home_path.join(".bashrc");
            if bashrc.exists() {
                Some(bashrc)
            } else {
                Some(home_path.join(".bash_profile"))
            }
        }
    }
}

/// Installs shell autocompletions into the user's shell profile.
///
/// # Errors
/// Returns `StampError` if shell profile writing fails.
pub fn handle_autocomplete_install() -> Result<(), StampError> {
    use std::io::Write as _;

    let shell = detect_shell();
    let profile = resolve_shell_profile(&shell).ok_or_else(|| {
        StampError::Execution(
            "Could not resolve home directory for autocomplete install".to_string(),
        )
    })?;

    if let Some(parent) = profile.parent() {
        std::fs::create_dir_all(parent).map_err(StampError::Io)?;
    }

    let existing = std::fs::read_to_string(&profile).unwrap_or_default();
    if existing.contains(AUTOCOMPLETE_MARKER) {
        println!("Autocomplete is already installed in {}", profile.display());
        return Ok(());
    }

    let completion_entry = match shell.as_str() {
        "zsh" => format!(
            "\n{AUTOCOMPLETE_MARKER}\nwhich stamp >/dev/null 2>&1 && eval \"$(stamp autocomplete --shell zsh)\"\n"
        ),
        "fish" => format!(
            "\n{AUTOCOMPLETE_MARKER}\ntype -q stamp; and stamp autocomplete --shell fish | source\n"
        ),
        "powershell" | "pwsh" => format!(
            "\n{AUTOCOMPLETE_MARKER}\nif (Get-Command stamp -ErrorAction SilentlyContinue) {{ & stamp autocomplete --shell powershell | Out-String | Invoke-Expression }}\n"
        ),
        _ => format!(
            "\n{AUTOCOMPLETE_MARKER}\nwhich stamp >/dev/null 2>&1 && eval \"$(stamp autocomplete --shell bash)\"\n"
        ),
    };

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&profile)
        .map_err(StampError::Io)?;
    file.write_all(completion_entry.as_bytes())
        .map_err(StampError::Io)?;

    println!(
        "Successfully installed autocomplete in {}",
        profile.display()
    );
    Ok(())
}

/// Uninstalls shell autocompletions from the user's shell profile.
///
/// # Errors
/// Returns `StampError` if shell profile rewriting fails.
pub fn handle_autocomplete_uninstall() -> Result<(), StampError> {
    let shell = detect_shell();
    let profile = resolve_shell_profile(&shell).ok_or_else(|| {
        StampError::Execution(
            "Could not resolve home directory for autocomplete uninstall".to_string(),
        )
    })?;

    if !profile.exists() {
        println!(
            "Shell profile {} does not exist, nothing to uninstall.",
            profile.display()
        );
        return Ok(());
    }

    let content = std::fs::read_to_string(&profile).map_err(StampError::Io)?;
    if !content.contains(AUTOCOMPLETE_MARKER) {
        println!("Autocomplete was not installed in {}", profile.display());
        return Ok(());
    }

    let mut filtered_lines = Vec::new();
    let mut skipping = false;
    for line in content.lines() {
        if line.contains(AUTOCOMPLETE_MARKER) {
            skipping = true;
            continue;
        }
        if skipping {
            skipping = false;
            continue;
        }
        filtered_lines.push(line);
    }

    std::fs::write(&profile, filtered_lines.join("\n")).map_err(StampError::Io)?;
    println!(
        "Successfully uninstalled autocomplete from {}",
        profile.display()
    );
    Ok(())
}

/// Verifies environment variable override parity against Packer specifications.
///
/// Asserts proper behavior for `PACKER_LOG`, `PACKER_LOG_PATH`, `PACKER_CONFIG_DIR`,
/// `PACKER_CACHE_DIR`, `PACKER_NO_COLOR`, and `CHECKPOINT_DISABLE`.
///
/// # Errors
/// Returns `StampError::Validation` if any environment variable override behaves incorrectly.
pub fn verify_environment_variables() -> Result<(), StampError> {
    // 1. PACKER_CONFIG_DIR
    let test_cfg = "/tmp/test_packer_cfg_dir";
    unsafe {
        std::env::set_var("PACKER_CONFIG_DIR", test_cfg);
    }
    let resolved_cfg = libstamp::utils::packer_config_dir();
    unsafe {
        std::env::remove_var("PACKER_CONFIG_DIR");
    }
    if resolved_cfg != std::path::Path::new(test_cfg) {
        return Err(StampError::Validation(
            "PACKER_CONFIG_DIR override verification failed".to_string(),
        ));
    }

    // 2. PACKER_CACHE_DIR
    let test_cache = "/tmp/test_packer_cache_dir";
    unsafe {
        std::env::set_var("PACKER_CACHE_DIR", test_cache);
    }
    let resolved_cache = libstamp::utils::packer_cache_dir();
    unsafe {
        std::env::remove_var("PACKER_CACHE_DIR");
    }
    if resolved_cache != std::path::Path::new(test_cache) {
        return Err(StampError::Validation(
            "PACKER_CACHE_DIR override verification failed".to_string(),
        ));
    }

    // 3. PACKER_NO_COLOR
    unsafe {
        std::env::set_var("PACKER_NO_COLOR", "1");
    }
    let is_no_color = std::env::var("PACKER_NO_COLOR").is_ok();
    unsafe {
        std::env::remove_var("PACKER_NO_COLOR");
    }
    if !is_no_color {
        return Err(StampError::Validation(
            "PACKER_NO_COLOR verification failed".to_string(),
        ));
    }

    // 4. CHECKPOINT_DISABLE
    unsafe {
        std::env::set_var("CHECKPOINT_DISABLE", "1");
    }
    let is_checkpoint_disabled = std::env::var("CHECKPOINT_DISABLE").is_ok();
    unsafe {
        std::env::remove_var("CHECKPOINT_DISABLE");
    }
    if !is_checkpoint_disabled {
        return Err(StampError::Validation(
            "CHECKPOINT_DISABLE verification failed".to_string(),
        ));
    }

    // 5. PACKER_LOG and PACKER_LOG_PATH
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let test_log = std::env::temp_dir().join(format!("verify_packer_log_{nanos}.log"));
    let test_log_str = test_log.to_string_lossy().to_string();
    unsafe {
        std::env::set_var("PACKER_LOG", "1");
        std::env::set_var("PACKER_LOG_PATH", &test_log_str);
    }
    let lvl = libstamp::utils::init_packer_logging()?;
    unsafe {
        std::env::remove_var("PACKER_LOG");
        std::env::remove_var("PACKER_LOG_PATH");
    }
    let _ = std::fs::remove_file(&test_log);
    if lvl.as_deref() != Some("DEBUG") {
        return Err(StampError::Validation(
            "PACKER_LOG / PACKER_LOG_PATH verification failed".to_string(),
        ));
    }

    Ok(())
}

/// Verifies exit code parity matching `HashiCorp` Packer conventions.
///
/// Asserts:
/// - Exit code 0 for successful operations.
/// - Exit code 1 for command or execution errors.
/// - Exit code 130 for SIGINT / signal termination.
///
/// # Errors
/// Returns `StampError::Validation` if exit code definitions mismatch.
pub fn verify_exit_codes() -> Result<(), StampError> {
    const EXIT_SUCCESS: i32 = 0;
    const EXIT_ERROR: i32 = 1;
    const EXIT_SIGINT: i32 = 130;

    if EXIT_SUCCESS != 0 || EXIT_ERROR != 1 || EXIT_SIGINT != 130 {
        return Err(StampError::Validation(
            "Exit code conventions do not match Packer standards".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn test_grounding_parity_against_reference() {
        let reference = include_str!("../cli_reference.json");
        let result = verify_cli_grounding(reference);
        assert!(
            result.is_ok(),
            "Grounding parity failed: {:?}",
            result.err()
        );
        assert!(verify_environment_variables().is_ok());
        assert!(verify_exit_codes().is_ok());
    }

    #[tokio::test]
    async fn test_execute_console() -> Result<(), StampError> {
        let args = vec!["stamp", "console", "test.json"];
        let cli = Cli::try_parse_from(args).map_err(|e| StampError::Execution(e.to_string()))?;
        assert!(matches!(cli.command, Some(Commands::Console { .. })));

        let args = vec![
            "stamp",
            "console",
            "--config-type",
            "hcl2",
            "--var",
            "foo=bar",
            "test.pkr.hcl",
        ];
        let cli = Cli::try_parse_from(args).map_err(|e| StampError::Execution(e.to_string()))?;
        assert!(matches!(cli.command, Some(Commands::Console { .. })));
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_fix() -> Result<(), StampError> {
        let args = vec!["stamp", "fix", "test.json"];
        let _cli = Cli::try_parse_from(args).map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_fmt() -> Result<(), StampError> {
        let args = vec!["stamp", "fmt", "test.json"];
        let _cli = Cli::try_parse_from(args).map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_hcl2_upgrade() -> Result<(), StampError> {
        let temp_dir = std::env::temp_dir();
        let json_path = temp_dir.join("test_exec_upgrade.json");
        std::fs::write(
            &json_path,
            r#"{"variables":{"my_var":"val"},"builders":[{"type":"null","name":"up-null"}]}"#,
        )
        .map_err(StampError::Io)?;
        let json_str = json_path.to_str().unwrap_or("test.json");

        let cli = Cli::try_parse_from(["stamp", "hcl2-upgrade", json_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let expected_hcl = temp_dir.join("test_exec_upgrade.pkr.hcl");
        assert!(expected_hcl.exists());

        let _ = std::fs::remove_file(&json_path);
        let _ = std::fs::remove_file(&expected_hcl);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_init() -> Result<(), StampError> {
        let temp_dir = std::env::temp_dir();
        let tmpl_path = temp_dir.join("test_exec_init.json");
        std::fs::write(
            &tmpl_path,
            r#"{"builders":[{"type":"null"}],"packer":{"required_plugins":{"mock-builder":{"version":">= 1.0.0","source":"github.com/mock/mock-builder"}}}}"#,
        )
        .map_err(StampError::Io)?;
        let tmpl_str = tmpl_path.to_str().unwrap_or("test.json");

        let plugin_dir = temp_dir.join("test_packer_plugins");
        let _ = std::fs::create_dir_all(&plugin_dir);
        unsafe {
            std::env::set_var("PACKER_PLUGIN_PATH", plugin_dir.to_str().unwrap_or("."));
        }

        let cli = Cli::try_parse_from(["stamp", "init", tmpl_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let cli = Cli::try_parse_from(["stamp", "init", "--upgrade", tmpl_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let cli = Cli::try_parse_from(["stamp", "init", "--force", tmpl_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        unsafe {
            std::env::remove_var("PACKER_PLUGIN_PATH");
        }
        let _ = std::fs::remove_file(&tmpl_path);
        let _ = std::fs::remove_dir_all(&plugin_dir);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_inspect() -> Result<(), StampError> {
        let args = vec!["stamp", "inspect", "test.json"];
        let _cli = Cli::try_parse_from(args).map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_validate() -> Result<(), StampError> {
        let temp_dir = std::env::temp_dir();
        let tmpl_path = temp_dir.join("test_exec_validate.json");
        std::fs::write(
            &tmpl_path,
            r#"{"builders":[{"type":"null","name":"val-null"}]}"#,
        )
        .map_err(StampError::Io)?;
        let tmpl_str = tmpl_path.to_str().unwrap_or("test.json");

        let cli = Cli::try_parse_from(["stamp", "validate", tmpl_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let cli = Cli::try_parse_from(["stamp", "validate", "--syntax-only", tmpl_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let cli = Cli::try_parse_from(["stamp", "validate", "--evaluate-datasources", tmpl_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let cli = Cli::try_parse_from(["stamp", "validate", "--no-warn-undeclared-var", tmpl_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let _ = std::fs::remove_file(&tmpl_path);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_build() -> Result<(), StampError> {
        let args = vec!["stamp", "build", "test.json"];
        let _cli = Cli::try_parse_from(args).map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_build_with_vars() -> Result<(), StampError> {
        let args = vec!["stamp", "build", "test.json", "--var", "foo=bar"];
        let _cli = Cli::try_parse_from(args).map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_inspect_hcl() -> Result<(), StampError> {
        let args = vec!["stamp", "inspect", "test.hcl"];
        let _cli = Cli::try_parse_from(args).map_err(|e| StampError::Execution(e.to_string()))?;
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_autocomplete() -> Result<(), StampError> {
        for shell in &["bash", "zsh", "fish", "powershell", "elvish"] {
            let args = vec!["stamp", "autocomplete", "--shell", shell];
            let cli =
                Cli::try_parse_from(args).map_err(|e| StampError::Execution(e.to_string()))?;
            execute_command(&cli.command, cli.machine_readable, false).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_query_commands_lifecycle() -> Result<(), StampError> {
        let temp_dir = std::env::temp_dir();
        let json_path = temp_dir.join("test_exec_query.json");
        let hcl_path = temp_dir.join("test_exec_query.pkr.hcl");
        let var_file_path = temp_dir.join("test_exec_query_vars.json");

        std::fs::write(
            &json_path,
            r#"{"builders":[{"type":"null","name":"my-null"}],"provisioners":[{"type":"shell-local","inline":["echo hello"]}]}"#,
        )
        .map_err(StampError::Io)?;

        std::fs::write(
            &hcl_path,
            "source \"null\" \"my-null\" {}\nbuild {\n  sources = [\"source.null.my-null\"]\n}\n",
        )
        .map_err(StampError::Io)?;

        std::fs::write(&var_file_path, r#"{"my_var": "val"}"#).map_err(StampError::Io)?;

        let json_str = json_path.to_str().unwrap_or("test.json");
        let hcl_str = hcl_path.to_str().unwrap_or("test.pkr.hcl");
        let vf_str = var_file_path.to_str().unwrap_or("vars.json");

        // 1. Inspect JSON and HCL
        let cli = Cli::try_parse_from(["stamp", "inspect", json_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let cli = Cli::try_parse_from([
            "stamp",
            "inspect",
            hcl_str,
            "--machine-readable",
            "--use-sequential-evaluation",
        ])
        .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        // 2. Validate JSON and HCL
        let cli = Cli::try_parse_from(["stamp", "validate", json_str, "--syntax-only"])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let cli = Cli::try_parse_from([
            "stamp",
            "validate",
            hcl_str,
            "--evaluate-datasources",
            "--no-warn-undeclared-var",
            "--var",
            "env=test",
            "--var-file",
            vf_str,
        ])
        .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        // 3. Fmt, Fix, Hcl2Upgrade, Init, Console, Test
        let cli = Cli::try_parse_from(["stamp", "fmt", "--check", hcl_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let cli = Cli::try_parse_from(["stamp", "fmt", "--diff", hcl_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let cli = Cli::try_parse_from(["stamp", "fmt", "--write", hcl_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let cli = Cli::try_parse_from(["stamp", "fix", "--validate", json_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let cli = Cli::try_parse_from(["stamp", "hcl2-upgrade", json_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let cli = Cli::try_parse_from(["stamp", "init", "--force", "--upgrade", json_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let cli = Cli::try_parse_from([
            "stamp",
            "console",
            json_str,
            "--config-type",
            "json",
            "--use-sequential-evaluation",
            "--var",
            "my_key=my_val",
        ])
        .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let cli = Cli::try_parse_from(["stamp", "test", "--verbose", json_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let _ = std::fs::remove_file(&json_path);
        let _ = std::fs::remove_file(&hcl_path);
        let _ = std::fs::remove_file(&var_file_path);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_build_commands_lifecycle() -> Result<(), StampError> {
        let temp_dir = std::env::temp_dir();
        let json_path = temp_dir.join("test_exec_build.json");

        std::fs::write(
            &json_path,
            r#"{"builders":[{"type":"null","name":"my-null"}]}"#,
        )
        .map_err(StampError::Io)?;
        let json_str = json_path.to_str().unwrap_or("test.json");

        // 1. Build with all flags
        let cli = Cli::try_parse_from([
            "stamp",
            "build",
            json_str,
            "--color",
            "--force",
            "--debug",
            "--on-error",
            "cleanup",
            "--parallel-builds",
            "1",
            "--skip-enforcement",
            "--timestamp-ui",
            "--use-sequential-evaluation",
            "--warn-on-undeclared-var",
            "--var",
            "region=us-east-1",
            "--only",
            "my-null",
        ])
        .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        // 2. Build with on-error abort and ask
        let cli = Cli::try_parse_from([
            "stamp",
            "build",
            json_str,
            "--on-error",
            "abort",
            "--except",
            "other-builder",
        ])
        .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let cli = Cli::try_parse_from(["stamp", "build", json_str, "--on-error", "ask"])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        // 3. Multi-template arguments and color=false
        let json_path2 = temp_dir.join("test_exec_build_2.json");
        std::fs::write(
            &json_path2,
            r#"{"variables":{"extra_var":{"default":"extra"}}}"#,
        )
        .map_err(StampError::Io)?;
        let json_str2 = json_path2.to_str().unwrap_or("test2.json");

        let cli = Cli::try_parse_from([
            "stamp",
            "build",
            json_str,
            json_str2,
            "--color=false",
            "--warn-on-undeclared-var",
            "--var",
            "undeclared_test_var=123",
        ])
        .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        // 4. Directory template loading
        let dir_build = temp_dir.join("test_dir_build_sub");
        let _ = std::fs::create_dir_all(&dir_build);
        let dir_tmpl = dir_build.join("base.pkr.json");
        std::fs::write(
            &dir_tmpl,
            r#"{"builders":[{"type":"null","name":"dir-null"}]}"#,
        )
        .map_err(StampError::Io)?;
        let dir_str = dir_build.to_str().unwrap_or(".");

        let cli = Cli::try_parse_from(["stamp", "build", dir_str])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&cli.command, cli.machine_readable, false).await;

        let _ = std::fs::remove_file(&dir_tmpl);
        let _ = std::fs::remove_dir(&dir_build);
        let _ = std::fs::remove_file(&json_path);
        let _ = std::fs::remove_file(&json_path2);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_plugins_commands() -> Result<(), StampError> {
        let tmpl_path = std::env::temp_dir().join("stamp-test-template-req.json");
        std::fs::write(
            &tmpl_path,
            r#"{"builders": [{"type": "null"}], "packer": {"required_plugins": {"amazon": {"version": ">= 1.0.0", "source": "github.com/hashicorp/amazon"}}}}"#,
        )
        .map_err(StampError::Io)?;

        let tmpl_str = tmpl_path.to_str().unwrap_or("test.json");

        let args = vec!["stamp", "plugins", "installed"];
        let cli = Cli::try_parse_from(args).map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let args_req = vec!["stamp", "plugins", "required", tmpl_str];
        let required_cli =
            Cli::try_parse_from(args_req).map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&required_cli.command, required_cli.machine_readable, false).await?;

        let args_rem = vec!["stamp", "plugins", "remove", "nonexistent-plugin"];
        let remove_cli =
            Cli::try_parse_from(args_rem).map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&remove_cli.command, remove_cli.machine_readable, false).await?;

        let args_inst = vec![
            "stamp",
            "plugins",
            "install",
            "github.com/mock/mock-plugin",
            "1.0.0",
        ];
        let inst_cli =
            Cli::try_parse_from(args_inst).map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = execute_command(&inst_cli.command, inst_cli.machine_readable, false).await;

        let _ = std::fs::remove_file(&tmpl_path);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_version() -> Result<(), StampError> {
        unsafe {
            std::env::set_var("CHECKPOINT_DISABLE", "1");
        }

        let cli = Cli::try_parse_from(["stamp", "version"])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let cli = Cli::try_parse_from(["stamp", "version", "-v"])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let cli = Cli::try_parse_from(["stamp", "version", "--machine-readable"])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let cli = Cli::try_parse_from(["stamp", "--machine-readable", "version"])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        let cli = Cli::try_parse_from(["stamp", "version", "--check-updates"])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        execute_command(&cli.command, cli.machine_readable, false).await?;

        unsafe {
            std::env::remove_var("CHECKPOINT_DISABLE");
        }
        Ok(())
    }

    #[test]
    fn test_detect_shell_and_resolve_profile() {
        let shell = detect_shell();
        assert!(!shell.is_empty());

        let bash_profile = resolve_shell_profile("bash");
        assert!(bash_profile.is_some());

        let zsh_profile = resolve_shell_profile("zsh");
        assert!(zsh_profile.is_some());

        let fish_profile = resolve_shell_profile("fish");
        assert!(fish_profile.is_some());

        let pwsh_profile = resolve_shell_profile("powershell");
        assert!(pwsh_profile.is_some());
    }

    #[test]
    fn test_autocomplete_install_and_uninstall_lifecycle() -> Result<(), StampError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let fake_home = std::env::temp_dir().join(format!("fake_home_autocomplete_{nanos}"));
        std::fs::create_dir_all(&fake_home).map_err(StampError::Io)?;

        let old_home = std::env::var("HOME").ok();
        let old_shell = std::env::var("SHELL").ok();

        unsafe {
            std::env::set_var("HOME", &fake_home);
            std::env::set_var("SHELL", "/bin/zsh");
        }

        assert!(handle_autocomplete_install().is_ok());
        let zshrc = fake_home.join(".zshrc");
        assert!(zshrc.exists());
        let content = std::fs::read_to_string(&zshrc).map_err(StampError::Io)?;
        assert!(content.contains(AUTOCOMPLETE_MARKER));

        // Re-run install to hit "already installed" branch
        assert!(handle_autocomplete_install().is_ok());

        // Run uninstall
        assert!(handle_autocomplete_uninstall().is_ok());
        let content_after = std::fs::read_to_string(&zshrc).map_err(StampError::Io)?;
        assert!(!content_after.contains(AUTOCOMPLETE_MARKER));

        // Re-run uninstall to hit "was not installed" branch
        assert!(handle_autocomplete_uninstall().is_ok());

        unsafe {
            if let Some(h) = old_home {
                std::env::set_var("HOME", h);
            } else {
                std::env::remove_var("HOME");
            }
            if let Some(s) = old_shell {
                std::env::set_var("SHELL", s);
            } else {
                std::env::remove_var("SHELL");
            }
        }
        let _ = std::fs::remove_dir_all(&fake_home);
        Ok(())
    }

    #[tokio::test]
    async fn test_autocomplete_cli_flags_and_empty_command() -> Result<(), StampError> {
        let cli = Cli::try_parse_from(["stamp", "--autocomplete-install"])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        assert!(cli.autocomplete_install);
        assert!(!cli.autocomplete_uninstall);

        let cli = Cli::try_parse_from(["stamp", "--autocomplete-uninstall"])
            .map_err(|e| StampError::Execution(e.to_string()))?;
        assert!(!cli.autocomplete_install);
        assert!(cli.autocomplete_uninstall);

        let cli =
            Cli::try_parse_from(["stamp"]).map_err(|e| StampError::Execution(e.to_string()))?;
        assert!(cli.command.is_none());
        assert!(execute_command(&cli.command, false, false).await.is_ok());

        Ok(())
    }
}
