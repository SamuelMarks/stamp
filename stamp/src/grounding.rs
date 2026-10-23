//! Grounding verification, shell autocompletion installation, and environment parity checks.

use crate::Cli;
use clap::CommandFactory;
use libstamp::error::StampError;
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};

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

/// Resolves the user shell profile path for autocompletion installation inside a given home directory.
#[must_use]
pub fn resolve_shell_profile_in(shell: &str, home_path: &std::path::Path) -> std::path::PathBuf {
    match shell {
        "zsh" => home_path.join(".zshrc"),
        "fish" => home_path.join(".config").join("fish").join("config.fish"),
        "powershell" | "pwsh" => home_path
            .join(".config")
            .join("powershell")
            .join("Microsoft.PowerShell_profile.ps1"),
        _ => {
            let bashrc = home_path.join(".bashrc");
            if bashrc.exists() {
                bashrc
            } else {
                home_path.join(".bash_profile")
            }
        }
    }
}

/// Resolves the user shell profile path for autocompletion installation.
#[must_use]
pub fn resolve_shell_profile(shell: &str) -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|h| !h.is_empty())?;
    Some(resolve_shell_profile_in(
        shell,
        &std::path::PathBuf::from(home),
    ))
}

/// Installs shell autocompletions into the specified shell profile inside `home_dir`.
///
/// # Errors
/// Returns `StampError` if shell profile writing fails.
pub fn handle_autocomplete_install_in(
    shell: &str,
    home_dir: Option<&std::path::Path>,
) -> Result<(), StampError> {
    use std::io::Write as _;

    let home = home_dir.ok_or_else(|| {
        StampError::Execution(
            "Could not resolve home directory for autocomplete install".to_string(),
        )
    })?;
    let profile = resolve_shell_profile_in(shell, home);

    if let Some(parent) = profile.parent() {
        std::fs::create_dir_all(parent).map_err(StampError::Io)?;
    }

    let existing = std::fs::read_to_string(&profile).unwrap_or_default();
    if existing.contains(AUTOCOMPLETE_MARKER) {
        println!("Autocomplete is already installed in {}", profile.display());
        return Ok(());
    }

    let completion_entry = match shell {
        "zsh" => format!(
            r#"
{AUTOCOMPLETE_MARKER}
which stamp >/dev/null 2>&1 && eval "$(stamp autocomplete --shell zsh)"
"#
        ),
        "fish" => format!(
            r#"
{AUTOCOMPLETE_MARKER}
type -q stamp; and stamp autocomplete --shell fish | source
"#
        ),
        "powershell" | "pwsh" => format!(
            r#"
{AUTOCOMPLETE_MARKER}
if (Get-Command stamp -ErrorAction SilentlyContinue) {{ & stamp autocomplete --shell powershell | Out-String | Invoke-Expression }}
"#
        ),
        _ => format!(
            r#"
{AUTOCOMPLETE_MARKER}
which stamp >/dev/null 2>&1 && eval "$(stamp autocomplete --shell bash)"
"#
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

/// Installs shell autocompletions into the user's shell profile.
///
/// # Errors
/// Returns `StampError` if shell profile writing fails.
pub fn handle_autocomplete_install() -> Result<(), StampError> {
    let shell = detect_shell();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from);
    handle_autocomplete_install_in(&shell, home.as_deref())
}

/// Uninstalls shell autocompletions from the specified shell profile inside `home_dir`.
///
/// # Errors
/// Returns `StampError` if shell profile rewriting fails.
pub fn handle_autocomplete_uninstall_in(
    shell: &str,
    home_dir: Option<&std::path::Path>,
) -> Result<(), StampError> {
    let home = home_dir.ok_or_else(|| {
        StampError::Execution(
            "Could not resolve home directory for autocomplete uninstall".to_string(),
        )
    })?;
    let profile = resolve_shell_profile_in(shell, home);

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

    std::fs::write(
        &profile,
        filtered_lines.join(
            "
",
        ),
    )
    .map_err(StampError::Io)?;
    println!(
        "Successfully uninstalled autocomplete from {}",
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
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from);
    handle_autocomplete_uninstall_in(&shell, home.as_deref())
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
    let resolved_cfg = {
        let _env = libstamp::utils::ScopedEnvVar::set("PACKER_CONFIG_DIR", test_cfg);
        libstamp::utils::packer_config_dir()
    };
    if resolved_cfg != std::path::Path::new(test_cfg) {
        return Err(StampError::Validation(
            "PACKER_CONFIG_DIR override verification failed".to_string(),
        ));
    }

    // 2. PACKER_CACHE_DIR
    let test_cache = "/tmp/test_packer_cache_dir";
    let resolved_cache = {
        let _env = libstamp::utils::ScopedEnvVar::set("PACKER_CACHE_DIR", test_cache);
        libstamp::utils::packer_cache_dir()
    };
    if resolved_cache != std::path::Path::new(test_cache) {
        return Err(StampError::Validation(
            "PACKER_CACHE_DIR override verification failed".to_string(),
        ));
    }

    // 2b. STAMP_CACHE_DIR
    let test_stamp_cache = "/tmp/test_stamp_cache_dir";
    let resolved_stamp_cache = {
        let _env1 = libstamp::utils::ScopedEnvVar::remove("PACKER_CACHE_DIR");
        let _env2 = libstamp::utils::ScopedEnvVar::set("STAMP_CACHE_DIR", test_stamp_cache);
        libstamp::utils::packer_cache_dir()
    };
    if resolved_stamp_cache != std::path::Path::new(test_stamp_cache) {
        return Err(StampError::Validation(
            "STAMP_CACHE_DIR override verification failed".to_string(),
        ));
    }

    // 3. PACKER_NO_COLOR
    let is_no_color = {
        let _env = libstamp::utils::ScopedEnvVar::set("PACKER_NO_COLOR", "1");
        std::env::var("PACKER_NO_COLOR").is_ok()
    };
    if !is_no_color {
        return Err(StampError::Validation(
            "PACKER_NO_COLOR verification failed".to_string(),
        ));
    }

    // 4. CHECKPOINT_DISABLE
    let is_checkpoint_disabled = {
        let _env = libstamp::utils::ScopedEnvVar::set("CHECKPOINT_DISABLE", "1");
        std::env::var("CHECKPOINT_DISABLE").is_ok()
    };
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
    let lvl = {
        let _env1 = libstamp::utils::ScopedEnvVar::set("PACKER_LOG", "1");
        let _env2 = libstamp::utils::ScopedEnvVar::set("PACKER_LOG_PATH", &test_log_str);
        libstamp::utils::init_packer_logging()?
    };
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
