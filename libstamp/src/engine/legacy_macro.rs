#![cfg_attr(coverage_nightly, coverage(off))]
//! Legacy macro interpolation engine for Packer JSON templates.
//!
//! Evaluates legacy `{{ ... }}` macro expressions such as `{{ user `var` }}`,
//! `{{ env `ENV` }}`, `{{ timestamp }}`, `{{ isotime }}`, `{{ uuid }}`,
//! `{{ pwd }}`, `{{ template_dir }}`, `{{ clean_resource_name `name` }}`,
//! `{{ build `ID` }}`, `{{ build_name }}`, and `{{ build_type }}`.

use crate::error::StampError;
use chrono::Utc;
use std::collections::HashMap;

/// Execution context for interpolating legacy Packer JSON template macros.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyMacroContext {
    /// User variables passed via `-var`, `-var-file`, or declared defaults.
    pub user_variables: HashMap<String, String>,
    /// Environment variables (falls back to system env if empty).
    pub env_variables: HashMap<String, String>,
    /// Current build name (`{{ build_name }}` or `{{ .BuildName }}`).
    pub build_name: String,
    /// Current builder type (`{{ build_type }}` or `{{ .BuildType }}`).
    pub build_type: String,
    /// Current build ID (`{{ build `ID` }}`).
    pub build_id: String,
    /// Unique build session UUID (`{{ build `PackerRunUUID` }}`).
    pub packer_run_uuid: String,
    /// Directory containing the template file (`{{ template_dir }}` or `{{ .TemplateDir }}`).
    pub template_dir: String,
    /// Current working directory (`{{ pwd }}`).
    pub pwd: Option<String>,
    /// Target host name or IP (`{{ build `Host` }}`).
    pub host: Option<String>,
    /// Connection user (`{{ build `User` }}`).
    pub user: Option<String>,
    /// Connection port (`{{ build `Port` }}`).
    pub port: Option<u16>,
    /// Connection password (`{{ build `Password` }}`).
    pub password: Option<String>,
    /// Source AMI ID (`{{ build `SourceAMI` }}`).
    pub source_ami: Option<String>,
    /// Source AMI Name (`{{ build `SourceAMIName` }}`).
    pub source_ami_name: Option<String>,
    /// Optional fixed Unix timestamp for deterministic testing.
    pub timestamp: Option<i64>,
    /// Optional fixed UUID for deterministic testing.
    pub uuid: Option<String>,
    /// Whether to allow missing variables or deferred build macros without returning an error.
    pub permissive: bool,
    /// Whether to preserve unresolvable runtime macros (like build `ID`) as literal `{{ ... }}`.
    pub preserve_unresolved: bool,
}

impl LegacyMacroContext {
    /// Creates a new default `LegacyMacroContext`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a user variable in the context.
    pub fn set_user_var(&mut self, key: impl Into<String>, val: impl Into<String>) {
        self.user_variables.insert(key.into(), val.into());
    }

    /// Sets an environment variable override in the context.
    pub fn set_env_var(&mut self, key: impl Into<String>, val: impl Into<String>) {
        self.env_variables.insert(key.into(), val.into());
    }
}

/// A lexical token inside a `{{ ... }}` macro expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroToken {
    /// An identifier or function name.
    Ident(String),
    /// A string literal enclosed in backticks, double quotes, or single quotes.
    StringLiteral(String),
    /// A pipe operator `|` for chaining function calls.
    Pipe,
    /// An opening parenthesis `(`.
    OpenParen,
    /// A closing parenthesis `)`.
    CloseParen,
}

/// Tokenizes the body of a `{{ ... }}` macro into lexical tokens.
///
/// # Errors
/// Returns `StampError::Parse` if an unclosed quote is encountered.
pub fn tokenize_macro(body: &str) -> Result<Vec<MacroToken>, StampError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = body.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }

        if c == '|' {
            tokens.push(MacroToken::Pipe);
            i += 1;
            continue;
        }

        if c == '(' {
            tokens.push(MacroToken::OpenParen);
            i += 1;
            continue;
        }

        if c == ')' {
            tokens.push(MacroToken::CloseParen);
            i += 1;
            continue;
        }

        if c == '`' || c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            let mut s = String::new();
            let mut closed = false;
            while i < len {
                if chars[i] == '\\' && i + 1 < len && chars[i + 1] == quote {
                    s.push(quote);
                    i += 2;
                } else if chars[i] == quote {
                    closed = true;
                    i += 1;
                    break;
                } else {
                    s.push(chars[i]);
                    i += 1;
                }
            }
            if !closed {
                return Err(StampError::Parse(format!(
                    "Unterminated string literal in macro: {body}"
                )));
            }
            tokens.push(MacroToken::StringLiteral(s));
            continue;
        }

        // Identifier or number
        let mut ident = String::new();
        while i < len
            && !chars[i].is_whitespace()
            && chars[i] != '|'
            && chars[i] != '('
            && chars[i] != ')'
            && chars[i] != '`'
            && chars[i] != '"'
            && chars[i] != '\''
        {
            ident.push(chars[i]);
            i += 1;
        }
        if !ident.is_empty() {
            tokens.push(MacroToken::Ident(ident));
        }
    }

    Ok(tokens)
}

/// Converts a Go-style reference time format string to a Chrono strftime format string.
#[must_use]
pub fn convert_go_time_format_to_chrono(go_fmt: &str) -> String {
    if go_fmt.contains('%') {
        return go_fmt.to_string();
    }

    let mut result = go_fmt.to_string();
    let replacements = [
        ("January", "%B"),
        ("Monday", "%A"),
        ("Jan", "%b"),
        ("Mon", "%a"),
        ("2006", "%Y"),
        ("06", "%y"),
        ("Z07:00", "%:z"),
        ("Z0700", "%z"),
        ("Z07", "%z"),
        ("MST", "%Z"),
        ("15", "%H"),
        ("PM", "%p"),
        ("pm", "%P"),
        ("01", "%m"),
        ("02", "%d"),
        ("03", "%I"),
        ("04", "%M"),
        ("05", "%S"),
    ];

    for (go_token, chrono_token) in replacements {
        result = result.replace(go_token, chrono_token);
    }

    result
}

/// Sanitizes a string according to cloud resource naming conventions (CleanResourceName).
#[must_use]
pub fn clean_resource_name(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Evaluates a sequence of tokens representing a single function call.
///
/// # Errors
/// Returns `StampError` if required arguments are missing or evaluation fails.
pub fn evaluate_function_tokens(
    func_name: &str,
    args: &[String],
    ctx: &LegacyMacroContext,
) -> Result<String, StampError> {
    match func_name {
        "user" => {
            if args.is_empty() {
                return Err(StampError::Parse(
                    "Macro 'user' requires 1 argument (variable name)".to_string(),
                ));
            }
            let var_name = &args[0];
            if let Some(val) = ctx.user_variables.get(var_name) {
                Ok(val.clone())
            } else if ctx.permissive {
                Ok(String::new())
            } else {
                Err(StampError::Validation(format!(
                    "Unknown user variable: '{var_name}'"
                )))
            }
        }
        "env" => {
            if args.is_empty() {
                return Err(StampError::Parse(
                    "Macro 'env' requires 1 argument (environment variable name)".to_string(),
                ));
            }
            let env_name = &args[0];
            if let Some(val) = ctx.env_variables.get(env_name) {
                Ok(val.clone())
            } else {
                Ok(std::env::var(env_name).unwrap_or_default())
            }
        }
        "timestamp" => {
            let ts = ctx.timestamp.unwrap_or_else(|| Utc::now().timestamp());
            Ok(ts.to_string())
        }
        "isotime" => {
            let now = ctx
                .timestamp
                .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
                .unwrap_or_else(Utc::now);

            if args.is_empty() {
                Ok(now.to_rfc3339())
            } else {
                let fmt = convert_go_time_format_to_chrono(&args[0]);
                Ok(now.format(&fmt).to_string())
            }
        }
        "uuid" => {
            if let Some(ref fixed_uuid) = ctx.uuid {
                Ok(fixed_uuid.clone())
            } else {
                Ok(uuid::Uuid::new_v4().to_string())
            }
        }
        "pwd" => {
            if let Some(ref pwd) = ctx.pwd {
                Ok(pwd.clone())
            } else {
                Ok(std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string()))
            }
        }
        "template_dir" | ".TemplateDir" | ".Path" => Ok(ctx.template_dir.clone()),
        "clean_resource_name" => {
            if args.is_empty() {
                return Err(StampError::Parse(
                    "Macro 'clean_resource_name' requires 1 argument".to_string(),
                ));
            }
            Ok(clean_resource_name(&args[0]))
        }
        "build" => {
            if args.is_empty() {
                return Err(StampError::Parse(
                    "Macro 'build' requires 1 argument (property name)".to_string(),
                ));
            }
            let prop = &args[0];
            match prop.as_str() {
                "ID" | "id" => {
                    if ctx.build_id.is_empty() && ctx.preserve_unresolved {
                        return Err(StampError::Parse("__UNRESOLVED__".to_string()));
                    }
                    Ok(ctx.build_id.clone())
                }
                "PackerRunUUID" | "packer_run_uuid" => {
                    if ctx.packer_run_uuid.is_empty() && ctx.preserve_unresolved {
                        return Err(StampError::Parse("__UNRESOLVED__".to_string()));
                    }
                    Ok(ctx.packer_run_uuid.clone())
                }
                "Host" | "host" => {
                    if ctx.host.is_none() && ctx.preserve_unresolved {
                        return Err(StampError::Parse("__UNRESOLVED__".to_string()));
                    }
                    Ok(ctx.host.clone().unwrap_or_default())
                }
                "User" | "user" => {
                    if ctx.user.is_none() && ctx.preserve_unresolved {
                        return Err(StampError::Parse("__UNRESOLVED__".to_string()));
                    }
                    Ok(ctx.user.clone().unwrap_or_default())
                }
                "Port" | "port" => {
                    if ctx.port.is_none() && ctx.preserve_unresolved {
                        return Err(StampError::Parse("__UNRESOLVED__".to_string()));
                    }
                    Ok(ctx.port.map_or_else(String::new, |p| p.to_string()))
                }
                "Password" | "password" => {
                    if ctx.password.is_none() && ctx.preserve_unresolved {
                        return Err(StampError::Parse("__UNRESOLVED__".to_string()));
                    }
                    Ok(ctx.password.clone().unwrap_or_default())
                }
                "SourceAMI" | "source_ami" => {
                    if ctx.source_ami.is_none() && ctx.preserve_unresolved {
                        return Err(StampError::Parse("__UNRESOLVED__".to_string()));
                    }
                    Ok(ctx.source_ami.clone().unwrap_or_default())
                }
                "SourceAMIName" | "source_ami_name" => {
                    if ctx.source_ami_name.is_none() && ctx.preserve_unresolved {
                        return Err(StampError::Parse("__UNRESOLVED__".to_string()));
                    }
                    Ok(ctx.source_ami_name.clone().unwrap_or_default())
                }
                "name" | "Name" => {
                    if ctx.build_name.is_empty() && ctx.preserve_unresolved {
                        return Err(StampError::Parse("__UNRESOLVED__".to_string()));
                    }
                    Ok(ctx.build_name.clone())
                }
                "type" | "Type" => {
                    if ctx.build_type.is_empty() && ctx.preserve_unresolved {
                        return Err(StampError::Parse("__UNRESOLVED__".to_string()));
                    }
                    Ok(ctx.build_type.clone())
                }
                other => {
                    if ctx.permissive {
                        Ok(String::new())
                    } else {
                        Err(StampError::Validation(format!(
                            "Unknown build property: '{other}'"
                        )))
                    }
                }
            }
        }
        "build_name" | ".BuildName" => {
            if ctx.build_name.is_empty() && ctx.preserve_unresolved {
                return Err(StampError::Parse("__UNRESOLVED__".to_string()));
            }
            Ok(ctx.build_name.clone())
        }
        "build_type" | ".BuildType" => {
            if ctx.build_type.is_empty() && ctx.preserve_unresolved {
                return Err(StampError::Parse("__UNRESOLVED__".to_string()));
            }
            Ok(ctx.build_type.clone())
        }
        ".Vars" => Ok(String::new()),
        "lower" => {
            if args.is_empty() {
                return Err(StampError::Parse(
                    "Macro 'lower' requires 1 argument".to_string(),
                ));
            }
            Ok(args[0].to_lowercase())
        }
        "upper" => {
            if args.is_empty() {
                return Err(StampError::Parse(
                    "Macro 'upper' requires 1 argument".to_string(),
                ));
            }
            Ok(args[0].to_uppercase())
        }
        "split" => {
            if args.len() < 2 {
                return Err(StampError::Parse(
                    "Macro 'split' requires 2 arguments (string, delimiter)".to_string(),
                ));
            }
            let parts: Vec<&str> = args[0].split(&args[1]).collect();
            Ok(parts.first().copied().unwrap_or_default().to_string())
        }
        "replace" => {
            if args.len() < 3 {
                return Err(StampError::Parse(
                    "Macro 'replace' requires 3 arguments (string, old, new)".to_string(),
                ));
            }
            Ok(args[0].replace(&args[1], &args[2]))
        }
        other => {
            if ctx.permissive {
                Ok(String::new())
            } else {
                Err(StampError::Parse(format!(
                    "Unknown macro function: '{other}'"
                )))
            }
        }
    }
}

/// Evaluates a list of parsed tokens with support for parentheses and pipes.
///
/// # Errors
/// Returns `StampError` if token parsing or evaluation fails.
pub fn evaluate_tokens(
    tokens: &[MacroToken],
    ctx: &LegacyMacroContext,
) -> Result<String, StampError> {
    if tokens.is_empty() {
        return Ok(String::new());
    }

    // Split pipeline segments by Pipe `|`
    let mut stages: Vec<Vec<MacroToken>> = Vec::new();
    let mut current_stage = Vec::new();

    for token in tokens {
        if *token == MacroToken::Pipe {
            if current_stage.is_empty() {
                return Err(StampError::Parse("Empty pipe stage in macro".to_string()));
            }
            stages.push(current_stage);
            current_stage = Vec::new();
        } else {
            current_stage.push(token.clone());
        }
    }
    if !current_stage.is_empty() {
        stages.push(current_stage);
    }

    let mut current_value: Option<String> = None;

    for stage in stages {
        // Evaluate inner parentheses if any
        let mut flat_args: Vec<String> = Vec::new();
        let mut func_name: Option<String> = None;

        let mut i = 0;
        while i < stage.len() {
            match &stage[i] {
                MacroToken::OpenParen => {
                    let mut depth = 1;
                    let mut inner_tokens = Vec::new();
                    i += 1;
                    while i < stage.len() && depth > 0 {
                        match &stage[i] {
                            MacroToken::OpenParen => {
                                depth += 1;
                                inner_tokens.push(stage[i].clone());
                            }
                            MacroToken::CloseParen => {
                                depth -= 1;
                                if depth > 0 {
                                    inner_tokens.push(stage[i].clone());
                                }
                            }
                            other => {
                                inner_tokens.push(other.clone());
                            }
                        }
                        i += 1;
                    }
                    if depth != 0 {
                        return Err(StampError::Parse("Unmatched '(' in macro".to_string()));
                    }
                    let inner_val = evaluate_tokens(&inner_tokens, ctx)?;
                    flat_args.push(inner_val);
                    continue;
                }
                MacroToken::CloseParen => {
                    return Err(StampError::Parse("Unexpected ')' in macro".to_string()));
                }
                MacroToken::Ident(ident) => {
                    if func_name.is_none() {
                        func_name = Some(ident.clone());
                    } else {
                        flat_args.push(ident.clone());
                    }
                }
                MacroToken::StringLiteral(s) => {
                    flat_args.push(s.clone());
                }
                MacroToken::Pipe => unreachable!(),
            }
            i += 1;
        }

        let Some(name) = func_name else {
            return Err(StampError::Parse(
                "Macro stage missing function name".to_string(),
            ));
        };

        // If a piped value exists, append it as the input argument
        if let Some(prev) = current_value {
            flat_args.push(prev);
        }

        let stage_result = evaluate_function_tokens(&name, &flat_args, ctx)?;
        current_value = Some(stage_result);
    }

    Ok(current_value.unwrap_or_default())
}

/// Evaluates a raw macro body string (without `{{` and `}}`).
///
/// # Errors
/// Returns `StampError` if tokenization or macro evaluation fails.
pub fn parse_and_interpolate_macro(
    macro_body: &str,
    ctx: &LegacyMacroContext,
) -> Result<String, StampError> {
    let tokens = tokenize_macro(macro_body.trim())?;
    evaluate_tokens(&tokens, ctx)
}

/// Interpolates all `{{ ... }}` macro expressions within an input string.
///
/// # Errors
/// Returns `StampError` if any macro fails to parse or evaluate (unless `ctx.permissive` is set).
pub fn interpolate_string(input: &str, ctx: &LegacyMacroContext) -> Result<String, StampError> {
    let mut result = String::with_capacity(input.len());
    let mut remainder = input;

    while let Some(start) = remainder.find("{{") {
        result.push_str(&remainder[..start]);
        let after_start = &remainder[start + 2..];

        if let Some(end) = after_start.find("}}") {
            let macro_body = &after_start[..end];
            match parse_and_interpolate_macro(macro_body, ctx) {
                Ok(evaluated) => {
                    result.push_str(&evaluated);
                }
                Err(StampError::Parse(msg)) if msg == "__UNRESOLVED__" => {
                    result.push_str("{{");
                    result.push_str(macro_body);
                    result.push_str("}}");
                }
                Err(e) => return Err(e),
            }
            remainder = &after_start[end + 2..];
        } else {
            // Unclosed {{: keep verbatim
            result.push_str(&remainder[start..start + 2]);
            remainder = after_start;
        }
    }

    result.push_str(remainder);
    Ok(result)
}

/// Recursively traverses and interpolates all string values in a `serde_json::Value`.
///
/// # Errors
/// Returns `StampError` if macro interpolation fails within any string.
pub fn interpolate_json_value(
    val: &mut serde_json::Value,
    ctx: &LegacyMacroContext,
) -> Result<(), StampError> {
    match val {
        serde_json::Value::String(s) => {
            *s = interpolate_string(s, ctx)?;
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                interpolate_json_value(item, ctx)?;
            }
        }
        serde_json::Value::Object(map) => {
            for (_key, item) in map {
                interpolate_json_value(item, ctx)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_macro_valid_and_unclosed() {
        let tokens = tokenize_macro("user `my_var` | clean_resource_name").unwrap();
        assert_eq!(
            tokens,
            vec![
                MacroToken::Ident("user".to_string()),
                MacroToken::StringLiteral("my_var".to_string()),
                MacroToken::Pipe,
                MacroToken::Ident("clean_resource_name".to_string()),
            ]
        );

        let err = tokenize_macro("user `unclosed").unwrap_err();
        assert!(matches!(err, StampError::Parse(_)));
    }

    #[test]
    fn test_quotes_variations() {
        let ctx = LegacyMacroContext {
            user_variables: [("key".to_string(), "val".to_string())].into(),
            ..Default::default()
        };

        assert_eq!(interpolate_string("{{ user `key` }}", &ctx).unwrap(), "val");
        assert_eq!(interpolate_string("{{ user 'key' }}", &ctx).unwrap(), "val");
        assert_eq!(
            interpolate_string("{{ user \"key\" }}", &ctx).unwrap(),
            "val"
        );
        assert_eq!(interpolate_string("{{user `key`}}", &ctx).unwrap(), "val");
    }

    #[test]
    fn test_user_and_env_variables() {
        let mut ctx = LegacyMacroContext::new();
        ctx.set_user_var("app", "web-server");
        ctx.set_env_var("MY_ENV", "prod");

        let input = "name={{ user `app` }}-{{ env `MY_ENV` }}";
        assert_eq!(
            interpolate_string(input, &ctx).unwrap(),
            "name=web-server-prod"
        );

        // Missing user var without permissive mode
        let err = interpolate_string("{{ user `missing` }}", &ctx).unwrap_err();
        assert!(matches!(err, StampError::Validation(_)));

        // Permissive mode
        ctx.permissive = true;
        assert_eq!(
            interpolate_string("{{ user `missing` }}", &ctx).unwrap(),
            ""
        );
    }

    #[test]
    fn test_timestamp_and_isotime() {
        let mut ctx = LegacyMacroContext::new();
        ctx.timestamp = Some(1700000000);

        assert_eq!(
            interpolate_string("{{ timestamp }}", &ctx).unwrap(),
            "1700000000"
        );

        let iso_default = interpolate_string("{{ isotime }}", &ctx).unwrap();
        assert_eq!(iso_default, "2023-11-14T22:13:20+00:00");

        let iso_formatted =
            interpolate_string("{{ isotime `2006-01-02-15-04-05` }}", &ctx).unwrap();
        assert_eq!(iso_formatted, "2023-11-14-22-13-20");

        let iso_date_only = interpolate_string("{{ isotime `2006-01-02` }}", &ctx).unwrap();
        assert_eq!(iso_date_only, "2023-11-14");
    }

    #[test]
    fn test_uuid_pwd_template_dir() {
        let mut ctx = LegacyMacroContext::new();
        ctx.uuid = Some("12345678-1234-1234-1234-123456789abc".to_string());
        ctx.pwd = Some("/current/dir".to_string());
        ctx.template_dir = "/template/path".to_string();

        assert_eq!(
            interpolate_string("{{ uuid }}", &ctx).unwrap(),
            "12345678-1234-1234-1234-123456789abc"
        );
        assert_eq!(
            interpolate_string("{{ pwd }}", &ctx).unwrap(),
            "/current/dir"
        );
        assert_eq!(
            interpolate_string("{{ template_dir }}", &ctx).unwrap(),
            "/template/path"
        );
        assert_eq!(
            interpolate_string("{{ .TemplateDir }}", &ctx).unwrap(),
            "/template/path"
        );
        assert_eq!(
            interpolate_string("{{ .Path }}", &ctx).unwrap(),
            "/template/path"
        );
    }

    #[test]
    fn test_clean_resource_name() {
        let ctx = LegacyMacroContext::default();
        let res = interpolate_string("{{ clean_resource_name `my/custom:name!` }}", &ctx).unwrap();
        assert_eq!(res, "my-custom-name-");
    }

    #[test]
    fn test_build_context_variables() {
        let mut ctx = LegacyMacroContext::new();
        ctx.build_name = "ubuntu-ami".to_string();
        ctx.build_type = "amazon-ebs".to_string();
        ctx.build_id = "ami-0123456789abcdef0".to_string();
        ctx.packer_run_uuid = "run-uuid-123".to_string();
        ctx.host = Some("10.0.0.1".to_string());
        ctx.user = Some("ec2-user".to_string());
        ctx.port = Some(22);
        ctx.password = Some("secret".to_string());
        ctx.source_ami = Some("ami-source-111".to_string());
        ctx.source_ami_name = Some("ubuntu-base".to_string());

        assert_eq!(
            interpolate_string("{{ build_name }}", &ctx).unwrap(),
            "ubuntu-ami"
        );
        assert_eq!(
            interpolate_string("{{ .BuildName }}", &ctx).unwrap(),
            "ubuntu-ami"
        );
        assert_eq!(
            interpolate_string("{{ build_type }}", &ctx).unwrap(),
            "amazon-ebs"
        );
        assert_eq!(
            interpolate_string("{{ .BuildType }}", &ctx).unwrap(),
            "amazon-ebs"
        );
        assert_eq!(
            interpolate_string("{{ build `ID` }}", &ctx).unwrap(),
            "ami-0123456789abcdef0"
        );
        assert_eq!(
            interpolate_string("{{ build `PackerRunUUID` }}", &ctx).unwrap(),
            "run-uuid-123"
        );
        assert_eq!(
            interpolate_string("{{ build `Host` }}", &ctx).unwrap(),
            "10.0.0.1"
        );
        assert_eq!(
            interpolate_string("{{ build `User` }}", &ctx).unwrap(),
            "ec2-user"
        );
        assert_eq!(
            interpolate_string("{{ build `Port` }}", &ctx).unwrap(),
            "22"
        );
        assert_eq!(
            interpolate_string("{{ build `Password` }}", &ctx).unwrap(),
            "secret"
        );
        assert_eq!(
            interpolate_string("{{ build `SourceAMI` }}", &ctx).unwrap(),
            "ami-source-111"
        );
        assert_eq!(
            interpolate_string("{{ build `SourceAMIName` }}", &ctx).unwrap(),
            "ubuntu-base"
        );
    }

    #[test]
    fn test_pipes_and_subexpressions() {
        let mut ctx = LegacyMacroContext::new();
        ctx.set_user_var("raw_name", "My App: v1.0");

        let piped = "{{ user `raw_name` | clean_resource_name }}";
        assert_eq!(interpolate_string(piped, &ctx).unwrap(), "My-App--v1-0");

        let nested = "{{ clean_resource_name (user `raw_name`) }}";
        assert_eq!(interpolate_string(nested, &ctx).unwrap(), "My-App--v1-0");

        let lower = "{{ user `raw_name` | lower }}";
        assert_eq!(interpolate_string(lower, &ctx).unwrap(), "my app: v1.0");

        let upper = "{{ user `raw_name` | upper }}";
        assert_eq!(interpolate_string(upper, &ctx).unwrap(), "MY APP: V1.0");
    }

    #[test]
    fn test_split_and_replace() {
        let ctx = LegacyMacroContext::default();
        let split = "{{ split `a/b/c` `/` }}";
        assert_eq!(interpolate_string(split, &ctx).unwrap(), "a");

        let replaced = "{{ replace `hello world` `world` `packers` }}";
        assert_eq!(interpolate_string(replaced, &ctx).unwrap(), "hello packers");
    }

    #[test]
    fn test_interpolate_json_value() {
        let mut ctx = LegacyMacroContext::new();
        ctx.set_user_var("env", "staging");
        ctx.build_name = "my-box".to_string();

        let mut val: serde_json::Value = serde_json::json!({
            "target": "server-{{ user `env` }}",
            "metadata": {
                "builder": "{{ build_name }}"
            },
            "tags": [
                "tag-{{ user `env` }}"
            ]
        });

        interpolate_json_value(&mut val, &ctx).unwrap();

        assert_eq!(val["target"], "server-staging");
        assert_eq!(val["metadata"]["builder"], "my-box");
        assert_eq!(val["tags"][0], "tag-staging");
    }

    #[test]
    fn test_unclosed_and_verbatim_macros() {
        let ctx = LegacyMacroContext::default();
        let unclosed = "hello {{ world without closing";
        assert_eq!(interpolate_string(unclosed, &ctx).unwrap(), unclosed);
    }

    #[test]
    fn test_macro_error_paths_and_edge_cases() {
        let mut ctx = LegacyMacroContext::default();

        // 1. Missing arguments for macros
        assert!(interpolate_string("{{ user }}", &ctx).is_err());
        assert!(interpolate_string("{{ env }}", &ctx).is_err());
        assert!(interpolate_string("{{ clean_resource_name }}", &ctx).is_err());
        assert!(interpolate_string("{{ build }}", &ctx).is_err());
        assert!(interpolate_string("{{ lower }}", &ctx).is_err());
        assert!(interpolate_string("{{ upper }}", &ctx).is_err());
        assert!(interpolate_string("{{ split `a` }}", &ctx).is_err());
        assert!(interpolate_string("{{ replace `a` `b` }}", &ctx).is_err());

        // 2. Unknown build property (non-permissive vs permissive)
        assert!(interpolate_string("{{ build `nonexistent` }}", &ctx).is_err());
        ctx.permissive = true;
        assert_eq!(
            interpolate_string("{{ build `nonexistent` }}", &ctx).unwrap(),
            ""
        );

        // 3. Unknown function (non-permissive vs permissive)
        ctx.permissive = false;
        assert!(interpolate_string("{{ unknown_func }}", &ctx).is_err());
        ctx.permissive = true;
        assert_eq!(interpolate_string("{{ unknown_func }}", &ctx).unwrap(), "");

        // 4. Syntax errors in tokens (empty pipe, mismatched parentheses)
        ctx.permissive = false;
        assert!(interpolate_string("{{ | user `x` }}", &ctx).is_err());
        assert!(interpolate_string("{{ user `x` ( }}", &ctx).is_err());
        assert!(interpolate_string("{{ user `x` ) }}", &ctx).is_err());
        assert!(interpolate_string("{{ }}", &ctx).unwrap().is_empty());

        // 5. Escaped quote in string literal
        let escaped = r#"{{ user "my\"var" }}"#;
        ctx.set_user_var("my\"var", "escaped_val");
        assert_eq!(interpolate_string(escaped, &ctx).unwrap(), "escaped_val");

        // 6. Chrono strftime format passthrough
        let strftime_res = convert_go_time_format_to_chrono("%Y-%m-%d");
        assert_eq!(strftime_res, "%Y-%m-%d");
    }
}
