#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![deny(missing_docs)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! The main entry point for the Packer CLI drop-in replacement binary.

use libstamp::error::StampError;

/// Core CLI execution point forwarding to Stamp.
#[tokio::main]
///
/// # Panics
/// Panics if the async runtime fails to start.
///
/// # Errors
/// Returns `StampError` if the CLI command execution fails.
pub async fn main() -> Result<(), StampError> {
    stamp::run().await
}
