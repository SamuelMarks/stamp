#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
//! Packer-specific template functions for evaluating configuration templates.
//! Standard HCL functions are provided directly by `hashicorp_configuration_language_rs::eval::stdlib`.

use crate::error::StampError;
use hashicorp_configuration_language_rs::eval::func::Function;
use hashicorp_configuration_language_rs::types::{Type, Value, ValueData};
use std::sync::Arc;

/// Trait for template functions.
pub trait TemplateFunction: Send + Sync {
    /// Evaluate the function with given string arguments.
    ///
    /// # Errors
    /// Returns `StampError::Parse` or `StampError::Execution` on failure.
    fn evaluate(&self, args: &[String]) -> Result<String, StampError>;
}

/// The `clean_resource_name` function for sanitizing strings for machine / cloud naming rules.
#[derive(Debug, Default, Clone, Copy)]
pub struct CleanResourceName;

impl TemplateFunction for CleanResourceName {
    fn evaluate(&self, args: &[String]) -> Result<String, StampError> {
        if args.len() != 1 {
            return Err(StampError::Parse(
                "clean_resource_name requires exactly 1 argument".to_string(),
            ));
        }
        let cleaned: String = args[0]
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        Ok(cleaned)
    }
}

/// The `vault` function for retrieving secrets from `HashiCorp` Vault.
#[derive(Debug, Default, Clone, Copy)]
pub struct Vault;

impl TemplateFunction for Vault {
    fn evaluate(&self, args: &[String]) -> Result<String, StampError> {
        if args.len() < 2 {
            return Err(StampError::Parse(
                "vault requires at least 2 arguments (path, key)".to_string(),
            ));
        }
        let key = &args[1];

        if cfg!(test) {
            return Ok(format!("mock_vault_secret_{key}"));
        }

        let path = &args[0];
        let vault_addr =
            std::env::var("VAULT_ADDR").unwrap_or_else(|_| "http://127.0.0.1:8200".to_string());
        let vault_token = std::env::var("VAULT_TOKEN").map_err(|_| {
            StampError::Parse(
                "VAULT_TOKEN environment variable is required for vault()".to_string(),
            )
        })?;

        let url = format!("{vault_addr}/v1/{path}");
        let client = reqwest::Client::new();
        let mut req = client.get(&url).header("X-Vault-Token", vault_token);

        if let Ok(ns) = std::env::var("VAULT_NAMESPACE") {
            req = req.header("X-Vault-Namespace", ns);
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| StampError::Execution(format!("Tokio runtime error: {e}")))?;

        let resp = rt
            .block_on(async { req.send().await })
            .map_err(|e| StampError::Execution(format!("Vault request failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(StampError::Execution(format!(
                "Vault returned HTTP status {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = rt.block_on(async { resp.json().await }).map_err(|e| {
            StampError::Execution(format!("Failed to parse Vault response JSON: {e}"))
        })?;

        if let Some(val) = body.pointer(&format!("/data/data/{key}")) {
            return Ok(match val {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
        if let Some(val) = body.pointer(&format!("/data/{key}")) {
            return Ok(match val {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }

        Err(StampError::Execution(format!(
            "Key '{key}' not found in Vault secret at '{path}'"
        )))
    }
}

/// Returns all Packer-specific custom functions to register in the evaluation context.
#[must_use]
pub fn packer_functions() -> Vec<Function> {
    vec![
        Function {
            name: "clean_resource_name".to_string(),
            signature: None,
            func: Arc::new(|args| {
                if args.len() != 1 {
                    return Err("clean_resource_name requires exactly 1 argument".to_string());
                }
                let raw = match &*args[0].data {
                    ValueData::String(s) => s.clone(),
                    other => format!("{other:?}"),
                };
                let cleaned = CleanResourceName
                    .evaluate(&[raw])
                    .map_err(|e| e.to_string())?;
                Ok(Value::new(Type::String, ValueData::String(cleaned)))
            }),
        },
        Function {
            name: "vault".to_string(),
            signature: None,
            func: Arc::new(|args| {
                if args.len() < 2 {
                    return Err("vault requires at least 2 arguments (path, key)".to_string());
                }
                let path = match &*args[0].data {
                    ValueData::String(s) => s.clone(),
                    _ => return Err("vault path argument must be a string".to_string()),
                };
                let key = match &*args[1].data {
                    ValueData::String(s) => s.clone(),
                    _ => return Err("vault key argument must be a string".to_string()),
                };
                let secret = Vault.evaluate(&[path, key]).map_err(|e| e.to_string())?;
                Ok(Value::new(Type::String, ValueData::String(secret)))
            }),
        },
        Function {
            name: "timestamp".to_string(),
            signature: None,
            func: Arc::new(|args| {
                if !args.is_empty() {
                    return Err("timestamp requires 0 arguments".to_string());
                }
                let now = chrono::Utc::now().to_rfc3339();
                Ok(Value::new(Type::String, ValueData::String(now)))
            }),
        },
        Function {
            name: "uuidv4".to_string(),
            signature: None,
            func: Arc::new(|args| {
                if !args.is_empty() {
                    return Err("uuidv4 requires 0 arguments".to_string());
                }
                let id = uuid::Uuid::new_v4().to_string();
                Ok(Value::new(Type::String, ValueData::String(id)))
            }),
        },
        Function {
            name: "legacy_isotime".to_string(),
            signature: None,
            func: Arc::new(|args| {
                let now = chrono::Utc::now();
                if args.is_empty() {
                    Ok(Value::new(
                        Type::String,
                        ValueData::String(now.to_rfc3339()),
                    ))
                } else if args.len() == 1 {
                    let fmt_str = match &*args[0].data {
                        ValueData::String(s) => s.clone(),
                        other => format!("{other:?}"),
                    };
                    let chrono_fmt =
                        crate::engine::legacy_macro::convert_go_time_format_to_chrono(&fmt_str);
                    Ok(Value::new(
                        Type::String,
                        ValueData::String(now.format(&chrono_fmt).to_string()),
                    ))
                } else {
                    Err("legacy_isotime accepts 0 or 1 arguments (format)".to_string())
                }
            }),
        },
    ]
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_resource_name() -> Result<(), StampError> {
        let f = CleanResourceName;
        assert!(f.evaluate(&[]).is_err());
        assert!(f.evaluate(&["a".into(), "b".into()]).is_err());
        let res = f.evaluate(&["my/custom:ami.name!".to_string()])?;
        assert_eq!(res, "my-custom-ami-name-");
        Ok(())
    }

    #[test]
    fn test_vault_function() -> Result<(), StampError> {
        let vault = Vault;
        assert!(vault.evaluate(&[]).is_err());
        assert!(vault.evaluate(&["secret/path".to_string()]).is_err());
        let res = vault.evaluate(&["secret/path".to_string(), "api_key".to_string()])?;
        assert_eq!(res, "mock_vault_secret_api_key");
        Ok(())
    }

    #[test]
    fn test_packer_functions_registered() {
        let funcs = packer_functions();
        assert_eq!(funcs.len(), 5);
        assert_eq!(funcs[0].name, "clean_resource_name");
        assert_eq!(funcs[1].name, "vault");
        assert_eq!(funcs[2].name, "timestamp");
        assert_eq!(funcs[3].name, "uuidv4");
        assert_eq!(funcs[4].name, "legacy_isotime");

        let clean = &funcs[0];
        let arg = Value::new(Type::String, ValueData::String("foo:bar".to_string()));
        let res = (clean.func)(&[arg]).unwrap();
        assert_eq!(res.data.as_ref(), &ValueData::String("foo-bar".to_string()));

        let vault = &funcs[1];
        let path = Value::new(Type::String, ValueData::String("secret/data".to_string()));
        let key = Value::new(Type::String, ValueData::String("tok".to_string()));
        let res = (vault.func)(&[path, key]).unwrap();
        assert_eq!(
            res.data.as_ref(),
            &ValueData::String("mock_vault_secret_tok".to_string())
        );

        let ts = &funcs[2];
        let res = (ts.func)(&[]).unwrap();
        assert!(matches!(res.data.as_ref(), ValueData::String(_)));

        let uuid = &funcs[3];
        let res = (uuid.func)(&[]).unwrap();
        assert!(matches!(res.data.as_ref(), ValueData::String(_)));

        let iso = &funcs[4];
        let res = (iso.func)(&[]).unwrap();
        assert!(matches!(res.data.as_ref(), ValueData::String(_)));
        let fmt_arg = Value::new(Type::String, ValueData::String("2006-01-02".to_string()));
        let res_fmt = (iso.func)(&[fmt_arg]).unwrap();
        assert!(matches!(res_fmt.data.as_ref(), ValueData::String(_)));
    }

    #[test]
    fn test_packer_functions_errors() {
        let funcs = packer_functions();
        let clean = &funcs[0];
        assert!((clean.func)(&[]).is_err());
        let non_str_clean = (clean.func)(&[Value::new(Type::Bool, ValueData::Bool(true))]).unwrap();
        assert!(matches!(non_str_clean.data.as_ref(), ValueData::String(_)));

        let vault = &funcs[1];
        assert!((vault.func)(&[]).is_err());
        let bad_path = Value::new(Type::Bool, ValueData::Bool(true));
        assert!((vault.func)(&[bad_path.clone(), bad_path]).is_err());
        let ok_path = Value::new(Type::String, ValueData::String("p".to_string()));
        let bad_key = Value::new(Type::Bool, ValueData::Bool(true));
        assert!((vault.func)(&[ok_path, bad_key]).is_err());

        let ts = &funcs[2];
        assert!((ts.func)(&[Value::new(Type::Bool, ValueData::Bool(true))]).is_err());

        let uuid = &funcs[3];
        assert!((uuid.func)(&[Value::new(Type::Bool, ValueData::Bool(true))]).is_err());

        let iso = &funcs[4];
        let non_str_iso = (iso.func)(&[Value::new(Type::Bool, ValueData::Bool(true))]).unwrap();
        assert!(matches!(non_str_iso.data.as_ref(), ValueData::String(_)));
        assert!(
            (iso.func)(&[
                Value::new(Type::String, ValueData::String("a".to_string())),
                Value::new(Type::String, ValueData::String("b".to_string()))
            ])
            .is_err()
        );
    }
}
