//! Integration tests for Stamp CLI binary.

use clap::Parser;
use clap::error::ErrorKind;
use stamp::Cli;

/// Verifies that the CLI definition is valid and handles the `--version` flag.
#[test]
fn test_cli_definition_and_version_flag() {
    let args = ["stamp", "--version"];
    assert!(matches!(
        Cli::try_parse_from(args),
        Err(e) if e.kind() == ErrorKind::DisplayVersion
    ));
}

/// Verifies parsing a standard subcommand.
#[test]
fn test_cli_parse_subcommand() {
    let args = ["stamp", "version"];
    assert!(Cli::try_parse_from(args).is_ok());
}
