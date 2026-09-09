#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![deny(missing_docs)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! The main entry point for the Stamp CLI executable.

use clap::Parser;
use libstamp::error::StampError;
use stamp::{Cli, execute_command};
use std::io::IsTerminal;

/// Dispatches CLI commands and handles special flags.
///
/// # Arguments
/// * `cli` - The parsed command-line interface arguments.
/// * `is_tty` - Flag indicating if stdout is attached to an interactive terminal.
/// * `spawn_signals` - Flag indicating if OS signal traps should be spawned.
///
/// # Errors
/// Returns `StampError` if command execution fails.
pub async fn run_cli(cli: Cli, is_tty: bool, spawn_signals: bool) -> Result<(), StampError> {
    let _ = libstamp::utils::init_packer_logging();

    if cli.autocomplete_install {
        return stamp::handle_autocomplete_install();
    }
    if cli.autocomplete_uninstall {
        return stamp::handle_autocomplete_uninstall();
    }

    if spawn_signals {
        tokio::spawn(handle_signals());
    }

    execute_command(&cli.command, cli.machine_readable, is_tty).await
}

/// Listens for termination and interruption signals asynchronously.
pub async fn handle_signals() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                eprintln!(
                    "\n==> Signal received (SIGINT). Waiting for cleanup to complete... Press Ctrl+C again for immediate abort."
                );
            }
            () = async {
                if let Some(ref mut term) = sigterm {
                    term.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                eprintln!(
                    "\n==> Termination signal received (SIGTERM). Waiting for cleanup to complete..."
                );
            }
        }
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\n==> Immediate abort requested. Exiting.");
            std::process::exit(1);
        }
    }
    #[cfg(not(unix))]
    {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!(
                "\n==> Signal received. Waiting for cleanup to complete... Press Ctrl+C again for immediate abort."
            );
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\n==> Immediate abort requested. Exiting.");
                std::process::exit(1);
            }
        }
    }
}

/// Core CLI execution point.
#[tokio::main]
///
/// # Panics
/// Panics if the async runtime fails to start.
///
/// # Errors
/// Returns `StampError` if the CLI command execution fails.
pub async fn main() -> Result<(), StampError> {
    let cli = Cli::parse();
    let is_tty = std::io::stdout().is_terminal();
    run_cli(cli, is_tty, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_cli_version() -> Result<(), StampError> {
        let cli = Cli {
            command: Some(stamp::Commands::Version {
                check_updates: false,
                v: false,
                machine_readable: false,
            }),
            machine_readable: false,
            autocomplete_install: false,
            autocomplete_uninstall: false,
        };
        run_cli(cli, false, false).await
    }

    #[tokio::test]
    async fn test_run_cli_autocomplete_flags() -> Result<(), StampError> {
        let cli_install = Cli {
            command: None,
            machine_readable: false,
            autocomplete_install: true,
            autocomplete_uninstall: false,
        };
        let _ = run_cli(cli_install, false, false).await;

        let cli_uninstall = Cli {
            command: None,
            machine_readable: false,
            autocomplete_install: false,
            autocomplete_uninstall: true,
        };
        let _ = run_cli(cli_uninstall, false, false).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_run_cli_empty_command() -> Result<(), StampError> {
        let cli = Cli {
            command: None,
            machine_readable: false,
            autocomplete_install: false,
            autocomplete_uninstall: false,
        };
        run_cli(cli, false, true).await
    }

    #[tokio::test]
    async fn test_handle_signals_timeout() {
        tokio::select! {
            () = handle_signals() => {}
            () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }
    }
}
