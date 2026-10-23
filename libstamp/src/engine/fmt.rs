//! Canonical HCL2 formatter delegating to `hashicorp_configuration_language_rs`.

use crate::error::StampError;
use std::fmt::Write as _;

/// Formats HCL2 source code canonically using `hashicorp_configuration_language_rs`.
///
/// # Errors
/// Returns `StampError::Parse` if formatting fails due to syntax errors.
pub fn try_format_hcl(input: &str) -> Result<String, StampError> {
    hashicorp_configuration_language_rs::cst::format::format_str(input)
        .map_err(|e| StampError::Parse(format!("HCL format error: {e}")))
}

/// Formats HCL2 source code canonically using `hashicorp_configuration_language_rs`.
///
/// If formatting fails due to syntax errors, the original unformatted input is returned.
#[must_use]
pub fn format_hcl_canonical(input: &str) -> String {
    try_format_hcl(input).unwrap_or_else(|_| {
        let mut fallback = input.to_string();
        if !fallback.is_empty() && !fallback.ends_with('\n') {
            fallback.push('\n');
        }
        fallback
    })
}

/// Generates a diff string between two text inputs.
#[must_use]
pub fn generate_diff(old: &str, new: &str) -> String {
    let mut out = String::new();
    if old == new {
        return out;
    }
    out.push_str("--- old\n+++ new\n");
    for line in old.lines() {
        let _ = writeln!(out, "-{line}");
    }
    for line in new.lines() {
        let _ = writeln!(out, "+{line}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_hcl_canonical_basic() -> Result<(), StampError> {
        let input = r#"
source "amazon-ebs" "basic" {
ami_name = "my-ami"
instance_type = "t3.micro"
}

build {
sources = [
"source.amazon-ebs.basic",
]
}
"#;
        let formatted = format_hcl_canonical(input);
        assert!(formatted.contains(r#"source "amazon-ebs" "basic""#));
        assert!(formatted.contains("ami_name"));
        assert!(formatted.contains("instance_type"));

        let checked = try_format_hcl(input)?;
        assert_eq!(checked, formatted);
        assert!(try_format_hcl("invalid hcl {{{").is_err());
        Ok(())
    }

    #[test]
    fn test_format_hcl_invalid_syntax_fallback() {
        let input = "invalid hcl {{{";
        let formatted = format_hcl_canonical(input);
        assert!(formatted.contains("invalid hcl"));
        assert!(formatted.ends_with('\n'));

        let input_with_newline = "invalid hcl {{{\n";
        let formatted_newline = format_hcl_canonical(input_with_newline);
        assert_eq!(formatted_newline, "invalid hcl {{{\n");

        let empty = format_hcl_canonical("");
        assert_eq!(empty, "");
    }

    #[test]
    fn test_generate_diff() {
        let diff = generate_diff(
            "a
", "b
",
        );
        assert!(diff.contains("-a"));
        assert!(diff.contains("+b"));

        let no_diff = generate_diff("same", "same");
        assert_eq!(no_diff, "");
    }
}
