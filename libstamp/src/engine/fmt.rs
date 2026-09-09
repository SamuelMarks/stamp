#![cfg_attr(coverage_nightly, coverage(off))]
//! Canonical HCL2 formatter delegating to `hashicorp_configuration_language_rs`.

use std::fmt::Write as _;

/// Formats HCL2 source code canonically using `hashicorp_configuration_language_rs`.
///
/// If formatting fails due to syntax errors, the original unformatted input is returned.
#[must_use]
pub fn format_hcl_canonical(input: &str) -> String {
    hashicorp_configuration_language_rs::cst::format::format_str(input).unwrap_or_else(|_| {
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
    out.push_str(
        "--- old
+++ new
",
    );
    for line in old.lines() {
        let _ = writeln!(out, "-{line}");
    }
    for line in new.lines() {
        let _ = writeln!(out, "+{line}");
    }
    out
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_format_hcl_canonical_basic() {
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
        assert!(formatted.contains("source \"amazon-ebs\" \"basic\""));
        assert!(formatted.contains("ami_name"));
        assert!(formatted.contains("instance_type"));
    }

    #[test]
    fn test_format_hcl_invalid_syntax_fallback() {
        let input = "invalid hcl {{{";
        let formatted = format_hcl_canonical(input);
        assert!(formatted.contains("invalid hcl"));
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
        assert!(no_diff.is_empty());
    }
}
