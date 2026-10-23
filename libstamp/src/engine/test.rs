//! Template Testing Engine (`stamp test`).
//!
//! Provides a native testing framework for HCL2 templates.

use crate::error::StampError;
use crate::template::{Template, TestBlock, TestFailureDetails};

/// Configuration for running tests.
#[derive(Debug, Clone)]
pub struct TestConfig {
    /// Enable verbose output.
    pub verbose: bool,
    /// Path to output `JUnit` XML results (optional).
    pub junit_xml: Option<String>,
}

/// A runner that evaluates a series of test blocks.
pub struct Runner<'a> {
    /// The template containing the tests.
    template: &'a Template,
    /// The test configuration.
    config: &'a TestConfig,
}

impl<'a> Runner<'a> {
    /// Creates a new `Runner` for the given template and configuration.
    #[must_use]
    pub const fn new(template: &'a Template, config: &'a TestConfig) -> Self {
        Self { template, config }
    }

    /// Executes all tests in the template.
    ///
    /// # Errors
    ///
    /// Returns `StampError::TestFailure` if any assertion fails.
    pub fn run(&self) -> Result<(), StampError> {
        for test in &self.template.tests {
            if self.config.verbose {
                println!("Running test: {}", test.name);
            }
            Self::run_test_block(test)?;
        }
        Ok(())
    }

    /// Evaluates assertions within a single test block.
    fn run_test_block(test: &TestBlock) -> Result<(), StampError> {
        for assert in &test.assertions {
            let passed = assert.condition == r#""true""# || assert.condition == "true";

            if !passed {
                let failure = TestFailureDetails {
                    test_name: test.name.clone(),
                    failed_condition: assert.condition.clone(),
                    error_message: assert.error_message.clone(),
                };
                return Err(StampError::TestFailure(failure));
            }
        }
        Ok(())
    }
}

/// Runs the tests defined in the given template path.
///
/// # Errors
///
/// Returns `StampError::Io` if the file cannot be read.
/// Returns `StampError::Parse` if the file cannot be parsed.
/// Returns `StampError::TestFailure` if any tests fail.
pub fn run_tests(template_path: &str, config: &TestConfig) -> Result<(), StampError> {
    let content = std::fs::read_to_string(template_path)?;
    let vars = std::collections::HashMap::new();

    let tmpl = if std::path::Path::new(template_path)
        .extension()
        .unwrap_or_default()
        .eq_ignore_ascii_case("json")
    {
        crate::parser::json::parse_json(&content, &vars)?
    } else {
        crate::parser::hcl::parse_hcl(&content, &vars)?
    };

    let runner = Runner::new(&tmpl, config);
    runner.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_tests_missing_file() {
        let config = TestConfig {
            verbose: false,
            junit_xml: None,
        };
        let err = run_tests("missing.hcl", &config);
        assert!(matches!(err, Err(StampError::Io(_))));
    }

    #[tokio::test]
    async fn test_run_tests_success_json() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test.json");
        std::fs::write(
            &path,
            r#"{"tests": [{"name": "json_test", "assertions": [{"condition": "true"}]}]}"#,
        )?;

        let config = TestConfig {
            verbose: false,
            junit_xml: None,
        };
        run_tests(&path.to_string_lossy(), &config)?;
        Ok(())
    }

    #[tokio::test]
    async fn test_run_tests_success_no_ext() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test_no_ext");
        std::fs::write(
            &path,
            r#"test "hcl_test" {
  assert {
    condition = "true"
  }
}
"#,
        )?;

        let config = TestConfig {
            verbose: false,
            junit_xml: None,
        };
        run_tests(&path.to_string_lossy(), &config)?;
        Ok(())
    }

    #[tokio::test]
    async fn test_run_tests_success_hcl() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test.hcl");
        std::fs::write(
            &path,
            r#"test "hcl_test" {
  assert {
    condition = "true"
  }
}
"#,
        )?;

        let config = TestConfig {
            verbose: false,
            junit_xml: None,
        };
        run_tests(&path.to_string_lossy(), &config)?;
        Ok(())
    }

    #[tokio::test]
    async fn test_runner_pass_and_fail() -> Result<(), StampError> {
        let fail_tmpl = Template {
            tests: vec![TestBlock {
                name: "test1".into(),
                assertions: vec![crate::template::TestAssertBlock {
                    condition: "false".into(),
                    error_message: Some("Should be true".into()),
                }],
            }],
            ..Default::default()
        };
        let pass_tmpl = Template {
            tests: vec![TestBlock {
                name: "test1".into(),
                assertions: vec![crate::template::TestAssertBlock {
                    condition: "true".into(),
                    error_message: None,
                }],
            }],
            ..Default::default()
        };
        let config = TestConfig {
            verbose: true,
            junit_xml: None,
        };
        let r_fail = Runner::new(&fail_tmpl, &config);
        let r_pass = Runner::new(&pass_tmpl, &config);

        let fail_res = r_fail.run();
        assert!(matches!(
            fail_res,
            Err(StampError::TestFailure(ref f))
                if f.test_name == "test1" && f.error_message.as_deref() == Some("Should be true")
        ));
        r_pass.run()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_run_tests_invalid_json() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test_invalid.json");
        std::fs::write(&path, "invalid json")?;
        let config = TestConfig {
            verbose: false,
            junit_xml: None,
        };
        let err = run_tests(&path.to_string_lossy(), &config);
        assert!(matches!(err, Err(StampError::Json(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_run_tests_invalid_hcl() -> Result<(), StampError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test_invalid.pkr.hcl");
        std::fs::write(&path, "invalid hcl {{{")?;
        let config = TestConfig {
            verbose: false,
            junit_xml: None,
        };
        let err = run_tests(&path.to_string_lossy(), &config);
        assert!(matches!(err, Err(StampError::Parse(_))));
        Ok(())
    }
}
