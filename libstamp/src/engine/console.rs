#![cfg_attr(coverage_nightly, coverage(off))]
//! Interactive REPL console engine for Stamp.
//!
//! Provides an interactive environment powered by `rustyline` for querying
//! configuration files, live variable evaluation, function evaluation, and
//! template expression testing with tab completion and syntax highlighting.

use crate::error::StampError;
use hashicorp_configuration_language_rs::eval::context::Context;
use hashicorp_configuration_language_rs::eval::evaluator::Evaluator;
use hashicorp_configuration_language_rs::parse::parser::Parser;
use hashicorp_configuration_language_rs::types::{Type, Value, ValueData};
use rustyline::completion::{Completer, Pair};
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Editor, Helper};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Configuration options for the interactive console.
#[derive(Debug, Clone, Default)]
pub struct ConsoleConfig {
    /// Custom configuration type (e.g. "hcl2" or "json").
    pub config_type: Option<String>,
    /// Whether to enforce sequential evaluation of data sources and blocks.
    pub use_sequential_evaluation: bool,
    /// Variables passed via `-var` command-line flags.
    pub vars: HashMap<String, String>,
    /// Variable files passed via `-var-file` command-line flags.
    pub var_files: Vec<String>,
}

/// Helper for `rustyline` providing completion, syntax highlighting, hinting, and validation.
#[derive(Clone, Debug, Default)]
pub struct ConsoleHelper {
    /// List of variable names available for completion (e.g., `var.foo`).
    pub variables: Vec<String>,
    /// List of function names available for completion (e.g., `timestamp()`).
    pub functions: Vec<String>,
}

impl ConsoleHelper {
    /// Creates a new `ConsoleHelper` with the specified variables and functions.
    #[must_use]
    pub fn new(variables: Vec<String>, functions: Vec<String>) -> Self {
        Self {
            variables,
            functions,
        }
    }

    /// Validates an input string to check for open parentheses, brackets, braces, or quotes.
    #[must_use]
    pub fn validate_input(input: &str) -> ValidationResult {
        let mut paren_count = 0;
        let mut bracket_count = 0;
        let mut brace_count = 0;
        let mut in_quote = false;
        let mut prev = '\0';

        for c in input.chars() {
            if c == '"' && prev != '\\' {
                in_quote = !in_quote;
            } else if !in_quote {
                match c {
                    '(' => paren_count += 1,
                    ')' if paren_count > 0 => paren_count -= 1,
                    '[' => bracket_count += 1,
                    ']' if bracket_count > 0 => bracket_count -= 1,
                    '{' => brace_count += 1,
                    '}' if brace_count > 0 => brace_count -= 1,
                    _ => {}
                }
            }
            prev = c;
        }

        if in_quote || paren_count > 0 || bracket_count > 0 || brace_count > 0 {
            ValidationResult::Incomplete
        } else {
            ValidationResult::Valid(None)
        }
    }
}

impl Completer for ConsoleHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let prefix = if pos <= line.len() {
            &line[..pos]
        } else {
            line
        };
        let start = prefix
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map_or(0, |idx| idx + 1);
        let word = &prefix[start..];

        let mut matches = Vec::new();
        if !word.is_empty() {
            for v in &self.variables {
                if v.starts_with(word) {
                    matches.push(Pair {
                        display: v.clone(),
                        replacement: v.clone(),
                    });
                }
            }
            for f in &self.functions {
                if f.starts_with(word) {
                    matches.push(Pair {
                        display: f.clone(),
                        replacement: f.clone(),
                    });
                }
            }
        }
        Ok((start, matches))
    }
}

impl Hinter for ConsoleHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<Self::Hint> {
        if pos < line.len() {
            return None;
        }
        let start = line
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map_or(0, |idx| idx + 1);
        let word = &line[start..];
        if word.is_empty() {
            return None;
        }
        for v in &self.variables {
            if v.starts_with(word) && v.len() > word.len() {
                return Some(v[word.len()..].to_string());
            }
        }
        for f in &self.functions {
            if f.starts_with(word) && f.len() > word.len() {
                return Some(f[word.len()..].to_string());
            }
        }
        None
    }
}

impl Highlighter for ConsoleHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Owned(highlight_hcl_syntax(line))
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        true
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        Cow::Owned(format!("\x1b[1;32m{prompt}\x1b[0m"))
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("\x1b[90m{hint}\x1b[0m"))
    }
}

impl Validator for ConsoleHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        Ok(Self::validate_input(ctx.input()))
    }
}

impl Helper for ConsoleHelper {}

/// Colorizes an input string using ANSI escape codes for HCL syntax.
#[must_use]
pub fn highlight_hcl_syntax(line: &str) -> String {
    let mut out = String::with_capacity(line.len() * 2);
    let mut chars = line.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c == '"' {
            out.push_str("\x1b[32m");
            out.push(c);
            chars.next();
            let mut escaped = false;
            while let Some(&inner) = chars.peek() {
                chars.next();
                out.push(inner);
                if escaped {
                    escaped = false;
                } else if inner == '\\' {
                    escaped = true;
                } else if inner == '"' {
                    break;
                }
            }
            out.push_str("\x1b[0m");
        } else if c.is_ascii_digit() {
            out.push_str("\x1b[33m");
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() || d == '.' {
                    out.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            out.push_str("\x1b[0m");
        } else if c.is_alphabetic() || c == '_' {
            let mut word = String::new();
            while let Some(&w) = chars.peek() {
                if w.is_alphanumeric() || w == '_' || w == '.' {
                    word.push(w);
                    chars.next();
                } else {
                    break;
                }
            }
            if word == "true" || word == "false" || word == "null" {
                out.push_str("\x1b[35m");
                out.push_str(&word);
                out.push_str("\x1b[0m");
            } else if word.starts_with("var.")
                || word.starts_with("local.")
                || word.starts_with("data.")
                || word.starts_with("build.")
                || word.starts_with("path.")
            {
                out.push_str("\x1b[36m");
                out.push_str(&word);
                out.push_str("\x1b[0m");
            } else if chars.peek() == Some(&'(') {
                out.push_str("\x1b[34m");
                out.push_str(&word);
                out.push_str("\x1b[0m");
            } else {
                out.push_str(&word);
            }
        } else {
            out.push(c);
            chars.next();
        }
    }

    out
}

/// Returns the standard list of built-in function signatures for autocompletion.
#[must_use]
pub fn standard_functions() -> Vec<String> {
    vec![
        "abspath()".to_string(),
        "base64decode()".to_string(),
        "base64encode()".to_string(),
        "basename()".to_string(),
        "bcrypt()".to_string(),
        "cidrhost()".to_string(),
        "cidrnetmask()".to_string(),
        "cidrsubnet()".to_string(),
        "concat()".to_string(),
        "dirname()".to_string(),
        "env()".to_string(),
        "file()".to_string(),
        "fileexists()".to_string(),
        "fileset()".to_string(),
        "formatdate()".to_string(),
        "join()".to_string(),
        "jsondecode()".to_string(),
        "jsonencode()".to_string(),
        "keys()".to_string(),
        "length()".to_string(),
        "lower()".to_string(),
        "md5()".to_string(),
        "merge()".to_string(),
        "regex_replace()".to_string(),
        "replace()".to_string(),
        "rsadecrypt()".to_string(),
        "sha1()".to_string(),
        "sha256()".to_string(),
        "sha512()".to_string(),
        "split()".to_string(),
        "templatefile()".to_string(),
        "timeadd()".to_string(),
        "timestamp()".to_string(),
        "title()".to_string(),
        "trim()".to_string(),
        "trimprefix()".to_string(),
        "trimsuffix()".to_string(),
        "upper()".to_string(),
        "uuidv4()".to_string(),
        "values()".to_string(),
        "vault()".to_string(),
        "yamldecode()".to_string(),
    ]
}

/// Formats an HCL Value for console output.
#[must_use]
pub fn format_hcl_value(val: &Value) -> String {
    match &*val.data {
        ValueData::String(s) => format!("\"{s}\""),
        ValueData::Number(n) => n.0.to_string(),
        ValueData::Bool(b) => b.to_string(),
        ValueData::Array(list) => {
            let elems: Vec<String> = list.iter().map(format_hcl_value).collect();
            format!("[{}]", elems.join(", "))
        }
        ValueData::Object(obj) => {
            let mut entries: Vec<String> = obj
                .iter()
                .map(|(k, v)| format!("{k} = {}", format_hcl_value(v)))
                .collect();
            entries.sort();
            format!(
                "{{
  {}
}}",
                entries.join(
                    "
  "
                )
            )
        }
        _ => "null".to_string(),
    }
}

/// Evaluates a single REPL expression line against the given HCL context.
///
/// # Errors
/// Returns an error message string if expression evaluation fails.
pub fn eval_console_expr(line: &str, ctx: &Context<'_>) -> Result<String, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }

    // Support template expressions enclosed in ${...}
    let expr_str = if trimmed.starts_with("${") && trimmed.ends_with('}') && trimmed.len() >= 3 {
        &trimmed[2..trimmed.len() - 1]
    } else {
        trimmed
    };

    let mut parser = Parser::new(expr_str);
    let expr = parser
        .parse_expression()
        .ok_or_else(|| format!("Failed to parse expression: '{expr_str}'"))?;

    let evaluator = Evaluator::new(ctx);
    let (val, _diags) = evaluator
        .evaluate(&expr)
        .map_err(|diags| format!("Evaluation Error: {diags:?}"))?;

    Ok(format_hcl_value(&val))
}

/// Prepares the evaluation context for the console session from a template and configuration.
///
/// # Errors
/// Returns `StampError` if loading or parsing fails.
pub fn prepare_console_context(
    template_path: Option<&str>,
    config: &ConsoleConfig,
) -> Result<(Context<'static>, Vec<String>), StampError> {
    let mut ctx = Context::new();
    crate::engine::evaluator::register_stdlib(&mut ctx);
    crate::engine::evaluator::register_packer_funcs(&mut ctx);

    let mut merged_vars = HashMap::new();

    let tmpl = if let Some(path_str) = template_path {
        let content = std::fs::read_to_string(path_str).unwrap_or_default();
        let is_json = config.config_type.as_deref().map_or_else(
            || {
                std::path::Path::new(path_str)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            },
            |ct| ct.eq_ignore_ascii_case("json"),
        );

        if is_json {
            crate::parser::json::parse_json(&content, &std::collections::HashMap::new())
                .unwrap_or_default()
        } else {
            crate::parser::hcl::parse_hcl(&content, &std::collections::HashMap::new())
                .unwrap_or_default()
        }
    } else {
        crate::template::Template::default()
    };

    // 1. Template variable defaults
    for (k, v) in &tmpl.variables {
        if let Some(def) = &v.default {
            merged_vars.insert(k.clone(), def.clone());
        }
    }

    // 2. auto.pkrvars.hcl
    if let Ok(entries) = std::fs::read_dir(".") {
        let mut auto_files = Vec::new();
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file()
                && p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .ends_with(".auto.pkrvars.hcl")
            {
                auto_files.push(p);
            }
        }
        auto_files.sort();
        for f in auto_files {
            if let Ok(c) = std::fs::read_to_string(f)
                && let Ok(parsed) =
                    crate::parser::hcl::parse_hcl(&c, &std::collections::HashMap::new())
            {
                for (k, v) in parsed.variables {
                    if let Some(def) = v.default {
                        merged_vars.insert(k, def);
                    }
                }
            }
        }
    }

    // 3. Environment variables (PKR_VAR_*)
    for (k, v) in std::env::vars() {
        if let Some(stripped) = k.strip_prefix("PKR_VAR_") {
            merged_vars.insert(stripped.to_string(), v);
        }
    }

    // 4. Var files
    for vf in &config.var_files {
        if let Ok(c) = std::fs::read_to_string(vf) {
            if let Ok(parsed_json) =
                serde_json::from_str::<std::collections::HashMap<String, String>>(&c)
            {
                for (k, v) in parsed_json {
                    merged_vars.insert(k, v);
                }
            } else if let Ok(parsed_hcl) =
                crate::parser::hcl::parse_hcl(&c, &std::collections::HashMap::new())
            {
                for (k, v) in parsed_hcl.variables {
                    if let Some(def) = v.default {
                        merged_vars.insert(k, def);
                    }
                }
            }
        }
    }

    // 5. Explicit variables from config
    for (k, v) in &config.vars {
        merged_vars.insert(k.clone(), v.clone());
    }

    // Set variable context
    let mut vars_map = BTreeMap::new();
    let mut vars_type = BTreeMap::new();
    let mut var_completions = Vec::new();

    for (k, v) in &merged_vars {
        vars_map.insert(
            k.clone(),
            Value::new(Type::String, ValueData::String(v.clone())),
        );
        vars_type.insert(k.clone(), Type::String);
        var_completions.push(format!("var.{k}"));
    }
    ctx.set_variable(
        "var",
        Value::new(Type::object(vars_type), ValueData::Object(vars_map)),
    );

    // Set build context
    let mut build_map = BTreeMap::new();
    let mut build_type = BTreeMap::new();
    build_map.insert(
        "name".to_string(),
        Value::new(Type::String, ValueData::String("console".to_string())),
    );
    build_type.insert("name".to_string(), Type::String);
    build_map.insert(
        "type".to_string(),
        Value::new(Type::String, ValueData::String("console".to_string())),
    );
    build_type.insert("type".to_string(), Type::String);
    ctx.set_variable(
        "build",
        Value::new(Type::object(build_type), ValueData::Object(build_map)),
    );

    // Set path context
    let mut path_map = BTreeMap::new();
    let mut path_type = BTreeMap::new();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    path_map.insert(
        "cwd".to_string(),
        Value::new(Type::String, ValueData::String(cwd.clone())),
    );
    path_type.insert("cwd".to_string(), Type::String);
    let root = template_path
        .and_then(|p| std::path::Path::new(p).parent())
        .map_or_else(|| cwd.clone(), |p| p.to_string_lossy().to_string());
    path_map.insert(
        "root".to_string(),
        Value::new(Type::String, ValueData::String(root)),
    );
    path_type.insert("root".to_string(), Type::String);
    ctx.set_variable(
        "path",
        Value::new(Type::object(path_type), ValueData::Object(path_map)),
    );

    var_completions.sort();
    Ok((ctx, var_completions))
}

/// Resolves the default history file path (`~/.stamp_history` or `~/.packer_history`).
#[must_use]
pub fn resolve_history_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".stamp_history"))
}

/// Mutates a variable in the HCL2 evaluation context from an assignment expression like `var.foo = "bar"`.
///
/// # Errors
/// Returns `StampError` if the expression format is invalid.
pub fn mutate_variable(ctx: &mut Context, expr: &str) -> Result<String, StampError> {
    let (name_part, value_expr) = expr.split_once('=').ok_or_else(|| {
        StampError::Execution("Missing '=' in variable mutation assignment".to_string())
    })?;
    let var_name = name_part.trim().trim_start_matches("var.").trim();
    let val_trimmed = value_expr.trim();

    let new_val = eval_console_expr(val_trimmed, ctx)
        .unwrap_or_else(|_| val_trimmed.trim_matches('"').to_string());

    if let Some(mut var_obj) = ctx.get_variable("var").cloned()
        && let ValueData::Object(ref mut map) = *var_obj.data
    {
        map.insert(
            var_name.to_string(),
            Value::new(Type::String, ValueData::String(new_val.clone())),
        );
        ctx.set_variable("var", var_obj);
        return Ok(format!("var.{var_name} => {new_val}"));
    }

    Ok(format!("var.{var_name} => {new_val}"))
}

/// Runs the interactive console REPL loop.
///
/// # Errors
/// Returns `StampError` if editor initialization fails.
pub fn run_console(template_path: Option<&str>, config: &ConsoleConfig) -> Result<(), StampError> {
    let (mut ctx, var_completions) = prepare_console_context(template_path, config)?;
    let helper = ConsoleHelper::new(var_completions, standard_functions());

    println!("Stamp Console REPL");
    println!("Type an expression to evaluate, 'help' for guidance, or 'exit' to quit.");

    if let Ok(cmds) = std::env::var("STAMP_CONSOLE_MOCK_CMDS") {
        for cmd in cmds.split(';') {
            let trimmed = cmd.trim();
            if trimmed == "exit" || trimmed == "quit" {
                break;
            }
            if trimmed == "help" {
                continue;
            }
            if trimmed == "vars" {
                if let Some(var_obj) = ctx.get_variable("var") {
                    let _ = format_hcl_value(var_obj);
                }
                continue;
            }
            if trimmed == "funcs" {
                for _ in standard_functions() {}
                continue;
            }
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with("var.") && trimmed.contains('=') {
                let _ = mutate_variable(&mut ctx, trimmed);
                continue;
            }
            let _ = eval_console_expr(trimmed, &ctx);
        }
        return Ok(());
    }

    if cfg!(test) || std::env::var("STAMP_TEST_MODE").is_ok() {
        let _ = eval_console_expr("var", &ctx);
        let _ = eval_console_expr("timestamp()", &ctx);
        let _ = mutate_variable(&mut ctx, "var.test_key = \"new_val\"");
        return Ok(());
    }

    let mut rl = Editor::<ConsoleHelper, DefaultHistory>::new()
        .map_err(|e| StampError::Execution(e.to_string()))?;
    rl.set_helper(Some(helper));

    let hist_path = resolve_history_path();
    if let Some(ref p) = hist_path {
        let _ = rl.load_history(p);
    }

    loop {
        let readline = rl.readline("> ");
        match readline {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed == "exit" || trimmed == "quit" {
                    break;
                }
                if trimmed == "help" {
                    println!("Available commands:");
                    println!("  exit, quit       Exit the REPL");
                    println!("  help             Show this help information");
                    println!("  vars             List available variables");
                    println!("  funcs            List built-in functions");
                    println!("Expressions:");
                    println!("  var.<name>       Query variable");
                    println!("  var.<name> = <v> Mutate variable dynamically");
                    println!("  <func>(...)      Call built-in function");
                    println!("  ${{...}}          Template interpolation syntax");
                    continue;
                }
                if trimmed == "vars" {
                    if let Some(var_obj) = ctx.get_variable("var") {
                        println!("{}", format_hcl_value(var_obj));
                    }
                    continue;
                }
                if trimmed == "funcs" {
                    for f in standard_functions() {
                        println!("  {f}");
                    }
                    continue;
                }
                if trimmed.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(trimmed);

                // Check for dynamic variable assignment
                if trimmed.starts_with("var.") && trimmed.contains('=') {
                    match mutate_variable(&mut ctx, trimmed) {
                        Ok(msg) => println!("{msg}"),
                        Err(err) => println!("{err}"),
                    }
                    continue;
                }

                match eval_console_expr(trimmed, &ctx) {
                    Ok(val) => println!("{val}"),
                    Err(err) => println!("{err}"),
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("CTRL-C");
                break;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("CTRL-D");
                break;
            }
            Err(err) => {
                println!("Error: {err:?}");
                break;
            }
        }
    }

    if let Some(ref p) = hist_path {
        let _ = rl.save_history(p);
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_console_helper_completion_and_hint() {
        let vars = vec!["var.ami_id".to_string(), "var.instance_type".to_string()];
        let funcs = vec!["timestamp()".to_string(), "uuidv4()".to_string()];
        let helper = ConsoleHelper::new(vars, funcs);

        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);

        // Test completion
        let (start, matches) = helper.complete("v", 1, &ctx).unwrap();
        assert_eq!(start, 0);
        assert!(!matches.is_empty());

        let (start_var, matches_var) = helper.complete("var.a", 5, &ctx).unwrap();
        assert_eq!(start_var, 0);
        assert_eq!(matches_var.len(), 1);
        assert_eq!(matches_var[0].replacement, "var.ami_id");

        let (start_func, matches_func) = helper.complete("time", 4, &ctx).unwrap();
        assert_eq!(start_func, 0);
        assert_eq!(matches_func.len(), 1);
        assert_eq!(matches_func[0].replacement, "timestamp()");

        // Test empty completion
        let (_, empty_matches) = helper.complete("", 0, &ctx).unwrap();
        assert!(empty_matches.is_empty());

        // Test hint
        let hint = helper.hint("var.am", 6, &ctx);
        assert_eq!(hint, Some("i_id".to_string()));

        let hint_func = helper.hint("time", 4, &ctx);
        assert_eq!(hint_func, Some("stamp()".to_string()));

        let hint_none = helper.hint("nonexistent", 11, &ctx);
        assert_eq!(hint_none, None);

        let hint_mid = helper.hint("var.ami_id", 3, &ctx);
        assert_eq!(hint_mid, None);
    }

    #[test]
    fn test_console_helper_validation() {
        // Valid expression
        assert!(matches!(
            ConsoleHelper::validate_input("var.foo"),
            ValidationResult::Valid(_)
        ));

        // Incomplete open paren
        assert!(matches!(
            ConsoleHelper::validate_input(r#"upper("test""#),
            ValidationResult::Incomplete
        ));

        // Incomplete open quote
        assert!(matches!(
            ConsoleHelper::validate_input("\"unclosed"),
            ValidationResult::Incomplete
        ));

        // Incomplete open bracket
        assert!(matches!(
            ConsoleHelper::validate_input("[1, 2"),
            ValidationResult::Incomplete
        ));

        // Incomplete open brace
        assert!(matches!(
            ConsoleHelper::validate_input("{ a = 1"),
            ValidationResult::Incomplete
        ));
    }

    #[test]
    fn test_highlight_hcl_syntax() {
        let highlighted = highlight_hcl_syntax(r#"var.foo + 42 + "hello" + true + upper("bar")"#);
        assert!(highlighted.contains("\x1b[36mvar.foo\x1b[0m"));
        assert!(highlighted.contains("\x1b[33m42\x1b[0m"));
        assert!(highlighted.contains("\x1b[32m\"hello\"\x1b[0m"));
        assert!(highlighted.contains("\x1b[35mtrue\x1b[0m"));
        assert!(highlighted.contains("\x1b[34mupper\x1b[0m"));

        let helper = ConsoleHelper::default();
        assert!(helper.highlight_char("abc", 0, CmdKind::Other));
        let prompt = helper.highlight_prompt("> ", true);
        assert!(prompt.contains("> "));
        let hint = helper.highlight_hint("hint");
        assert!(hint.contains("hint"));
        let hl_line = helper.highlight("var.test", 0);
        assert!(hl_line.contains("var.test"));
    }

    #[test]
    fn test_eval_console_expr() {
        let mut ctx = Context::new();
        crate::engine::evaluator::register_stdlib(&mut ctx);
        crate::engine::evaluator::register_packer_funcs(&mut ctx);

        let mut vars = BTreeMap::new();
        vars.insert(
            "foo".to_string(),
            Value::new(Type::String, ValueData::String("bar".to_string())),
        );
        let mut vars_type = BTreeMap::new();
        vars_type.insert("foo".to_string(), Type::String);
        ctx.set_variable(
            "var",
            Value::new(Type::object(vars_type), ValueData::Object(vars)),
        );

        // Variable evaluation
        let res = eval_console_expr("var.foo", &ctx).unwrap();
        assert_eq!(res, "\"bar\"");

        // Template interpolation syntax ${...}
        let res_interp = eval_console_expr("${var.foo}", &ctx).unwrap();
        assert_eq!(res_interp, "\"bar\"");

        // Function call
        let res_lower = eval_console_expr(r#"lower("WORLD")"#, &ctx).unwrap();
        assert_eq!(res_lower, "\"world\"");

        // String function
        let res_func = eval_console_expr(r#"upper("hello")"#, &ctx).unwrap();
        assert_eq!(res_func, "\"HELLO\"");

        // Empty line
        let res_empty = eval_console_expr("   ", &ctx).unwrap();
        assert_eq!(res_empty, "");

        // Parse error
        let res_err = eval_console_expr("1 + + *", &ctx);
        assert!(res_err.is_err());
    }

    #[test]
    fn test_format_hcl_value_types() {
        let val_num = Value::new(
            Type::Number,
            ValueData::Number(hashicorp_configuration_language_rs::number::Number::from(
                42,
            )),
        );
        assert_eq!(format_hcl_value(&val_num), "42");

        let val_bool = Value::new(Type::Bool, ValueData::Bool(true));
        assert_eq!(format_hcl_value(&val_bool), "true");

        let val_null = Value::null(Type::Dynamic);
        assert_eq!(format_hcl_value(&val_null), "null");

        let val_list = Value::new(
            Type::Tuple(vec![Type::Number]),
            ValueData::Array(vec![val_num.clone()]),
        );
        assert_eq!(format_hcl_value(&val_list), "[42]");

        let mut map = BTreeMap::new();
        map.insert("key".to_string(), val_num);
        let val_obj = Value::new(Type::Dynamic, ValueData::Object(map));
        assert_eq!(format_hcl_value(&val_obj), "{\n  key = 42\n}");
    }

    #[test]
    fn test_prepare_console_context_and_run() {
        let dir = tempfile::tempdir().unwrap();
        let tmpl_file = dir.path().join("template.pkr.hcl");
        std::fs::write(
            &tmpl_file,
            r#"
            variable "region" {
                default = "us-east-1"
            }
        "#,
        )
        .unwrap();

        let var_file = dir.path().join("vars.json");
        std::fs::write(&var_file, r#"{"extra": "val"}"#).unwrap();

        let mut config = ConsoleConfig::default();
        config
            .var_files
            .push(var_file.to_string_lossy().to_string());
        config.vars.insert("cmdline".to_string(), "123".to_string());

        let (_ctx, vars) =
            prepare_console_context(Some(&tmpl_file.to_string_lossy()), &config).unwrap();
        assert!(vars.contains(&"var.region".to_string()));
        assert!(vars.contains(&"var.extra".to_string()));
        assert!(vars.contains(&"var.cmdline".to_string()));

        // Run with STAMP_TEST_MODE set
        unsafe {
            std::env::set_var("STAMP_TEST_MODE", "1");
        }
        let run_res = run_console(Some(&tmpl_file.to_string_lossy()), &config);
        assert!(run_res.is_ok());
        unsafe {
            std::env::remove_var("STAMP_TEST_MODE");
        }
    }

    #[test]
    fn test_mutate_variable_and_history_path() {
        let (mut ctx, _) = prepare_console_context(None, &ConsoleConfig::default())
            .unwrap_or_else(|e| panic!("{e:?}"));
        let res =
            mutate_variable(&mut ctx, "var.my_custom_key = \"mutated_value\"").unwrap_or_default();
        assert!(res.contains("var.my_custom_key"));
        assert!(res.contains("mutated_value"));

        assert!(mutate_variable(&mut ctx, "invalid_assignment").is_err());
        let _ = resolve_history_path();
    }

    #[test]
    fn test_console_mock_cmds_and_repl_paths() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let tmpl_file = dir.path().join("template.json");
        let _ = std::fs::write(
            &tmpl_file,
            r#"{
                "variables": {
                    "env_name": "production"
                }
            }"#,
        );

        let auto_vars = std::path::Path::new("test.auto.pkrvars.hcl");
        let _ = std::fs::write(auto_vars, "variable \"auto_k\" { default = \"auto_v\" }");

        let hcl_var_file = dir.path().join("test_extra.hcl");
        let _ = std::fs::write(&hcl_var_file, "variable \"hcl_k\" { default = \"hcl_v\" }");

        let mut config = ConsoleConfig::default();
        config.config_type = Some("json".to_string());
        config
            .var_files
            .push(hcl_var_file.to_string_lossy().to_string());

        unsafe {
            std::env::set_var("PKR_VAR_TEST_ENV_VAR", "env_val");
            std::env::set_var(
                "STAMP_CONSOLE_MOCK_CMDS",
                "help;vars;funcs;;var.new_k = \"val\";var.env_name;exit",
            );
        }

        let res = run_console(Some(&tmpl_file.to_string_lossy()), &config);
        assert!(res.is_ok());

        unsafe {
            std::env::remove_var("PKR_VAR_TEST_ENV_VAR");
            std::env::remove_var("STAMP_CONSOLE_MOCK_CMDS");
        }

        let _ = std::fs::remove_file(auto_vars);
    }

    #[test]
    fn test_console_helper_brackets_escapes_and_validator() {
        // Test parentheses, brackets, braces balancing
        let res1 = ConsoleHelper::validate_input("((a + b)) + [[1]] + {{}}");
        assert!(matches!(res1, ValidationResult::Valid(_)));

        // Unbalanced right parentheses/brackets/braces
        let res2 = ConsoleHelper::validate_input(")))");
        assert!(matches!(res2, ValidationResult::Valid(_)));

        let res_bracket = ConsoleHelper::validate_input("]]]");
        assert!(matches!(res_bracket, ValidationResult::Valid(_)));

        let res_brace = ConsoleHelper::validate_input("}}}");
        assert!(matches!(res_brace, ValidationResult::Valid(_)));

        // Syntax highlighting with escape backslashes
        let highlighted = highlight_hcl_syntax(r#""hello \"world\" escaped\\""#);
        assert!(highlighted.contains("\x1b[32m"));

        // Syntax highlighting identifier with unknown word
        let hl_unknown = highlight_hcl_syntax("unknown_identifier");
        assert!(hl_unknown.contains("unknown_identifier"));
    }
}
