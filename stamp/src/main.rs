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

/// Core CLI execution point.
#[tokio::main]
///
/// # Panics
/// Panics if the async runtime fails to start.
///
/// # Errors
/// Returns `StampError` if the CLI command execution fails.
pub async fn main() -> Result<(), StampError> {
    let _ = libstamp::utils::init_packer_logging();
    let cli = Cli::parse();
    let is_tty = std::io::stdout().is_terminal();

    if cli.autocomplete_install {
        return stamp::handle_autocomplete_install();
    }
    if cli.autocomplete_uninstall {
        return stamp::handle_autocomplete_uninstall();
    }

    tokio::spawn(async {
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
    });

    execute_command(&cli.command, cli.machine_readable, is_tty).await
}
