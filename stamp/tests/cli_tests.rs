//! Integration tests for Stamp CLI binary and Packer drop-in compatibility.

use clap::Parser;
use clap::error::ErrorKind;
use stamp::{Cli, Commands, normalize_go_flags, parse_cli_from};

/// Verifies that the CLI definition is valid and handles the `--version` flag.
/// Ensures output format matches `Packer v1.11.2 (Stamp drop-in replacement)`
/// so that Bento's `cmd.stdout.split(' ')[1]` extracts `v1.11.2`.
#[test]
fn test_cli_definition_and_version_flag() {
    let args = ["stamp", "--version"];
    let err = Cli::try_parse_from(args).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    let output = err.to_string();
    assert!(
        output.contains("Packer v1.11.2 (Stamp drop-in replacement)"),
        "Unexpected version string: {output}"
    );

    // Validate Bento split parsing
    let token = output.split(' ').nth(1);
    assert_eq!(token, Some("v1.11.2"));
}

/// Verifies single-dash `-version` parsing via `parse_cli_from`.
#[test]
fn test_single_dash_version_flag() {
    let args = ["packer", "-version"];
    let err = parse_cli_from(args).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    let output = err.to_string();
    assert!(output.contains("Packer v1.11.2"));
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
fn test_go_style_flags_build() {
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

    let cli = parse_cli_from(args).expect("Failed to parse Go-style build flags");
    match cli.command {
        Some(Commands::Build {
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
        }) => {
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
        }
        other => panic!("Expected Commands::Build, got {other:?}"),
    }
}

/// Verifies parsing Go-style flags on `validate`, `init`, and `fix`.
#[test]
fn test_go_style_flags_subcommands() {
    // Validate
    let val_args = [
        "packer",
        "validate",
        "-syntax-only",
        "-var-file=vars.pkrvars.hcl",
        "template.pkr.hcl",
    ];
    let val_cli = parse_cli_from(val_args).expect("Failed to parse validate command");
    match val_cli.command {
        Some(Commands::Validate {
            syntax_only,
            var_file,
            template,
            ..
        }) => {
            assert!(syntax_only);
            assert_eq!(var_file, Some(vec!["vars.pkrvars.hcl".to_string()]));
            assert_eq!(template, "template.pkr.hcl");
        }
        other => panic!("Expected Commands::Validate, got {other:?}"),
    }

    // Init
    let init_args = ["packer", "init", "-upgrade", "-force", "packer_templates"];
    let init_cli = parse_cli_from(init_args).expect("Failed to parse init command");
    match init_cli.command {
        Some(Commands::Init {
            upgrade,
            force,
            template,
        }) => {
            assert!(upgrade);
            assert!(force);
            assert_eq!(template, "packer_templates");
        }
        other => panic!("Expected Commands::Init, got {other:?}"),
    }

    // Fix
    let fix_args = ["packer", "fix", "-validate", "template.json"];
    let fix_cli = parse_cli_from(fix_args).expect("Failed to parse fix command");
    match fix_cli.command {
        Some(Commands::Fix { validate, template }) => {
            assert!(validate);
            assert_eq!(template, "template.json");
        }
        other => panic!("Expected Commands::Fix, got {other:?}"),
    }
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
