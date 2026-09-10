#![cfg_attr(coverage_nightly, coverage(off))]
#![deny(missing_docs)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! `libstamp` is the core library for Stamp, a tool to replicate Packer functionality.
//! It provides strongly-typed interfaces and configurations for creating machine images.

pub mod artifact;
pub mod builder;
pub mod cache;
pub mod communicator;
pub mod data_source;
pub mod engine;
pub mod error;
pub mod functions;
/// Generated protocol buffer definitions.
pub mod r#gen;
pub mod parser;
pub mod plugin;
pub mod post_processor;
pub mod provisioner;
pub mod telemetry;
pub mod template;

pub mod types;
pub mod utils;
