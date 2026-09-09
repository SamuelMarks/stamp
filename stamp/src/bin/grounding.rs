#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![deny(missing_docs)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

//! Grounding verification binary for validating Stamp CLI parity against `HashiCorp` Packer reference schemas.

fn main() -> Result<(), libstamp::error::StampError> {
    let reference = include_str!("../../cli_reference.json");
    stamp::verify_cli_grounding(reference)?;
    stamp::verify_environment_variables()?;
    stamp::verify_exit_codes()?;
    println!(
        "All CLI subcommands, flags, exit codes, and environment variables match HashiCorp Packer reference schemas!"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grounding_main() -> Result<(), libstamp::error::StampError> {
        main()
    }

    #[test]
    fn test_corrupted_json_grounding() {
        let bad_json = "{\"commands\": {\"nonexistent_cmd\": {}}}";
        assert!(stamp::verify_cli_grounding(bad_json).is_err());
    }
}
