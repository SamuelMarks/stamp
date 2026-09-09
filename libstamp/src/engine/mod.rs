#![cfg_attr(coverage_nightly, coverage(off))]
//! Core concurrent workflow engine for Stamp.

pub mod console;
pub mod dag;
pub mod dynamic_blocks;
pub mod evaluator;
pub mod fix;
pub mod fmt;
pub mod hcp;
pub mod hook;
pub mod legacy_macro;
pub mod multistep;
pub mod opa;
pub mod packer;
pub mod packer_init;
pub mod plugins;
pub mod remote_ui;
pub mod schema;
pub mod sentinel;
pub mod supervisor;
pub mod test;
pub mod ui;
pub mod upgrade;
