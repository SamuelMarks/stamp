//! Integration tests for Stamp CLI binary and Packer drop-in compatibility.

use clap::Parser;
use clap::error::ErrorKind;
use libstamp::error::StampError;
use stamp::{Cli, Commands, normalize_go_flags, parse_cli_from};

/// Verifies that the CLI definition is valid and handles the `--version` flag.
/// Ensures output format matches `Packer v1.11.2 (Stamp drop-in replacement)`
/// so that Bento's `cmd.stdout.split(' ')[1]` extracts `v1.11.2`.
#[test]
fn test_cli_definition_and_version_flag() -> Result<(), StampError> {
    let args = ["stamp", "--version"];
    let Err(err) = Cli::try_parse_from(args) else {
        return Err(StampError::Execution(
            "Expected DisplayVersion error".to_string(),
        ));
    };
    assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    let output = err.to_string();
    assert!(
        output.contains("Packer v1.11.2 (Stamp drop-in replacement)"),
        "Unexpected version string: {output}"
    );

    // Validate Bento split parsing
    let token = output.split(' ').nth(1);
    assert_eq!(token, Some("v1.11.2"));
    Ok(())
}

/// Verifies single-dash `-version` parsing via `parse_cli_from`.
#[test]
fn test_single_dash_version_flag() -> Result<(), StampError> {
    let args = ["packer", "-version"];
    let Err(err) = parse_cli_from(args) else {
        return Err(StampError::Execution(
            "Expected DisplayVersion error".to_string(),
        ));
    };
    assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    let output = err.to_string();
    assert!(output.contains("Packer v1.11.2"));
    Ok(())
}

/// Verifies parsing the standard `version` subcommand and verbose flag.
#[test]
fn test_cli_parse_subcommand_version() {
    let args = ["stamp", "version"];
    let cli = parse_cli_from(args);
    assert!(cli.is_ok());
    if let Ok(c) = cli {
        assert!(matches!(
            c.command,
            Some(Commands::Version {
                v: false,
                machine_readable: false,
                check_updates: false,
            })
        ));
    }

    let args_verbose = ["packer", "version", "-v"];
    let cli_verbose = parse_cli_from(args_verbose);
    assert!(cli_verbose.is_ok());
    if let Ok(c) = cli_verbose {
        assert!(matches!(c.command, Some(Commands::Version { v: true, .. })));
    }
}

/// Verifies parsing Go-style single-dash flags on the `build` subcommand.
#[test]
fn test_go_style_flags_build() -> Result<(), StampError> {
    let args = [
        "packer",
        "build",
        "-timestamp-ui",
        "-force",
        "-debug",
        "-on-error=abort",
        "-parallel=false",
        "-only=qemu.vm,virtualbox-iso.vm",
        "-except=parallels-iso.vm",
        "-var",
        "cpus=4",
        "-var",
        r#"sources_enabled=["source.qemu.vm"]"#,
        "-var-file=os.pkrvars.hcl",
        "packer_templates",
    ];

    let cli = parse_cli_from(args).map_err(|e| StampError::Parse(e.to_string()))?;
    let Some(Commands::Build {
        template,
        timestamp_ui,
        force,
        debug,
        on_error,
        parallel,
        only,
        except,
        var,
        var_file,
        ..
    }) = cli.command
    else {
        return Err(StampError::Parse("Expected Commands::Build".to_string()));
    };
    assert_eq!(template, vec!["packer_templates"]);
    assert!(timestamp_ui);
    assert!(force);
    assert!(debug);
    assert_eq!(on_error, Some("abort".to_string()));
    assert_eq!(parallel, Some(false));
    assert_eq!(only, Some("qemu.vm,virtualbox-iso.vm".to_string()));
    assert_eq!(except, Some("parallels-iso.vm".to_string()));
    assert_eq!(
        var,
        Some(vec![
            "cpus=4".to_string(),
            r#"sources_enabled=["source.qemu.vm"]"#.to_string()
        ])
    );
    assert_eq!(var_file, Some(vec!["os.pkrvars.hcl".to_string()]));
    Ok(())
}

/// Verifies parsing Go-style flags on `validate`, `init`, and `fix`.
#[test]
fn test_go_style_flags_subcommands() -> Result<(), StampError> {
    // Validate
    let val_args = [
        "packer",
        "validate",
        "-syntax-only",
        "-var-file=vars.pkrvars.hcl",
        "template.pkr.hcl",
    ];
    let val_cli = parse_cli_from(val_args).map_err(|e| StampError::Parse(e.to_string()))?;
    let Some(Commands::Validate {
        syntax_only,
        var_file,
        template,
        ..
    }) = val_cli.command
    else {
        return Err(StampError::Parse("Expected Commands::Validate".to_string()));
    };
    assert!(syntax_only);
    assert_eq!(var_file, Some(vec!["vars.pkrvars.hcl".to_string()]));
    assert_eq!(template, "template.pkr.hcl");

    // Init
    let init_args = ["packer", "init", "-upgrade", "-force", "packer_templates"];
    let init_cli = parse_cli_from(init_args).map_err(|e| StampError::Parse(e.to_string()))?;
    let Some(Commands::Init {
        upgrade,
        force,
        template,
    }) = init_cli.command
    else {
        return Err(StampError::Parse("Expected Commands::Init".to_string()));
    };
    assert!(upgrade);
    assert!(force);
    assert_eq!(template, "packer_templates");

    // Fix
    let fix_args = ["packer", "fix", "-validate", "template.json"];
    let fix_cli = parse_cli_from(fix_args).map_err(|e| StampError::Parse(e.to_string()))?;
    let Some(Commands::Fix { validate, template }) = fix_cli.command else {
        return Err(StampError::Parse("Expected Commands::Fix".to_string()));
    };
    assert!(validate);
    assert_eq!(template, "template.json");
    Ok(())
}

/// Verifies normalization helper rules for numeric and short flags.
#[test]
fn test_normalize_go_flags_cases() {
    let input = vec![
        "packer".to_string(),
        "-v".to_string(),
        "-h".to_string(),
        "-force".to_string(),
        "-timestamp-ui".to_string(),
        "--already-long".to_string(),
        "-1".to_string(),
        "-42".to_string(),
        "positional".to_string(),
    ];
    let normalized = normalize_go_flags(input);
    assert_eq!(
        normalized,
        vec![
            "packer",
            "-v",
            "-h",
            "--force",
            "--timestamp-ui",
            "--already-long",
            "-1",
            "-42",
            "positional"
        ]
    );
}
