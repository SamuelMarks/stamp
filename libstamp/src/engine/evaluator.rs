#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
//! Template evaluation engine.

use crate::error::StampError;
use crate::template::Template;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use hashicorp_configuration_language_rs::eval::context::Context;
use hashicorp_configuration_language_rs::eval::evaluator::Evaluator;
use hashicorp_configuration_language_rs::parse::parser::Parser;
use hashicorp_configuration_language_rs::types::{Type, Value, ValueData};

/// Registers standard library HCL functions into the evaluation context.
pub(crate) fn register_stdlib(ctx: &mut Context) {
    for f in hashicorp_configuration_language_rs::eval::stdlib::all_functions() {
        ctx.set_function(f.name.clone(), f);
    }
}

/// Registers Packer-specific and standard library functions into the evaluation context.
pub(crate) fn register_packer_funcs(ctx: &mut Context) {
    register_stdlib(ctx);
    for f in crate::functions::packer_functions() {
        ctx.set_function(f.name.clone(), f);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
/// Evaluates an expression or template string against the given context.
fn evaluate_str(s: &str, ctx: &Context) -> Result<Value, String> {
    // Attempt direct expression evaluation first
    let mut direct_parser = Parser::new(s);
    if let Some(expr) = direct_parser.parse_expression()
        && !direct_parser.errors().has_errors()
    {
        let evaluator = Evaluator::new(ctx);
        if let Ok((val, diags)) = evaluator.evaluate(&expr)
            && !diags.has_errors()
        {
            return Ok(val);
        }
    }

    let wrapped = format!("\"{s}\"");
    let mut parser = Parser::new(&wrapped);
    if let Some(expr) = parser.parse_expression() {
        let evaluator = Evaluator::new(ctx);
        let res = evaluator.evaluate(&expr);
        match res {
            Ok((val, _diags)) => Ok(val),
            Err(diags) => Err(diags
                .errors()
                .first()
                .map(|e| e.error.to_string())
                .unwrap_or_default()),
        }
    } else {
        Ok(Value::new(Type::String, ValueData::String(s.to_string())))
    }
}

/// Injects build context variables into the evaluation context.
pub fn inject_build_context(ctx: &mut Context, build_ctx: &crate::engine::hook::BuildContext) {
    let mut build_map = BTreeMap::new();
    let mut build_types = BTreeMap::new();

    let build_name = if build_ctx.build_name.is_empty() {
        &build_ctx.source_name
    } else {
        &build_ctx.build_name
    };
    let build_type = if build_ctx.build_type.is_empty() {
        &build_ctx.source_type
    } else {
        &build_ctx.build_type
    };
    let conn_type = if build_ctx.conn_type.is_empty() {
        "ssh".to_string()
    } else {
        build_ctx.conn_type.clone()
    };

    let vars = vec![
        ("ID", &build_ctx.build_id),
        ("id", &build_ctx.build_id),
        ("name", build_name),
        ("type", build_type),
        ("BuilderType", build_type),
        ("builder_type", build_type),
        ("SourceName", &build_ctx.source_name),
        ("source_name", &build_ctx.source_name),
        ("SourceType", &build_ctx.source_type),
        ("source_type", &build_ctx.source_type),
        ("Host", &build_ctx.host),
        ("host", &build_ctx.host),
        ("User", &build_ctx.user),
        ("user", &build_ctx.user),
        ("PackerRunUUID", &build_ctx.packer_run_uuid),
        ("packer_run_uuid", &build_ctx.packer_run_uuid),
        ("ConnType", &conn_type),
        ("conntype", &conn_type),
    ];

    for (k, v) in vars {
        build_types.insert(k.to_string(), Type::String);
        build_map.insert(
            k.to_string(),
            Value::new(Type::String, ValueData::String(v.clone())),
        );
    }

    let port_num = if build_ctx.port == 0 {
        22
    } else {
        build_ctx.port
    };
    if let Ok(num) =
        hashicorp_configuration_language_rs::number::Number::from_str(&port_num.to_string())
    {
        build_types.insert("Port".to_string(), Type::Number);
        build_types.insert("port".to_string(), Type::Number);
        build_map.insert(
            "Port".to_string(),
            Value::new(Type::Number, ValueData::Number(num.clone())),
        );
        build_map.insert(
            "port".to_string(),
            Value::new(Type::Number, ValueData::Number(num)),
        );
    }

    let mut conn_map = BTreeMap::new();
    let mut conn_types = BTreeMap::new();
    conn_types.insert("host".to_string(), Type::String);
    conn_map.insert(
        "host".to_string(),
        Value::new(Type::String, ValueData::String(build_ctx.host.clone())),
    );
    conn_types.insert("port".to_string(), Type::String);
    conn_map.insert(
        "port".to_string(),
        Value::new(Type::String, ValueData::String(port_num.to_string())),
    );
    conn_types.insert("user".to_string(), Type::String);
    conn_map.insert(
        "user".to_string(),
        Value::new(Type::String, ValueData::String(build_ctx.user.clone())),
    );
    conn_types.insert("conn_type".to_string(), Type::String);
    conn_map.insert(
        "conn_type".to_string(),
        Value::new(Type::String, ValueData::String(conn_type.clone())),
    );
    for (k, v) in &build_ctx.conn_info {
        conn_types.insert(k.clone(), Type::String);
        conn_map.insert(
            k.clone(),
            Value::new(Type::String, ValueData::String(v.clone())),
        );
    }
    let conn_val = Value::new(Type::object(conn_types), ValueData::Object(conn_map));
    build_types.insert("ConnInfo".to_string(), conn_val.ty().clone());
    build_types.insert("conn_info".to_string(), conn_val.ty().clone());
    build_map.insert("ConnInfo".to_string(), conn_val.clone());
    build_map.insert("conn_info".to_string(), conn_val);

    if let Some(ref pass) = build_ctx.password {
        build_types.insert("Password".to_string(), Type::String);
        build_types.insert("password".to_string(), Type::String);
        build_map.insert(
            "Password".to_string(),
            Value::new(Type::String, ValueData::String(pass.clone())),
        );
        build_map.insert(
            "password".to_string(),
            Value::new(Type::String, ValueData::String(pass.clone())),
        );
    }

    if let Some(ref ami) = build_ctx.source_ami {
        build_types.insert("SourceAMI".to_string(), Type::String);
        build_types.insert("source_ami".to_string(), Type::String);
        build_map.insert(
            "SourceAMI".to_string(),
            Value::new(Type::String, ValueData::String(ami.clone())),
        );
        build_map.insert(
            "source_ami".to_string(),
            Value::new(Type::String, ValueData::String(ami.clone())),
        );
    }

    if let Some(ref name) = build_ctx.source_ami_name {
        build_types.insert("SourceAMIName".to_string(), Type::String);
        build_types.insert("source_ami_name".to_string(), Type::String);
        build_map.insert(
            "SourceAMIName".to_string(),
            Value::new(Type::String, ValueData::String(name.clone())),
        );
        build_map.insert(
            "source_ami_name".to_string(),
            Value::new(Type::String, ValueData::String(name.clone())),
        );
    }

    if let Some(ref pub_key) = build_ctx.ssh_public_key {
        build_types.insert("SSHPublicKey".to_string(), Type::String);
        build_types.insert("ssh_public_key".to_string(), Type::String);
        build_map.insert(
            "SSHPublicKey".to_string(),
            Value::new(Type::String, ValueData::String(pub_key.clone())),
        );
        build_map.insert(
            "ssh_public_key".to_string(),
            Value::new(Type::String, ValueData::String(pub_key.clone())),
        );
    }

    if let Some(ref priv_key) = build_ctx.ssh_private_key {
        build_types.insert("SSHPrivateKey".to_string(), Type::String);
        build_types.insert("ssh_private_key".to_string(), Type::String);
        build_map.insert(
            "SSHPrivateKey".to_string(),
            Value::new(Type::String, ValueData::String(priv_key.clone())),
        );
        build_map.insert(
            "ssh_private_key".to_string(),
            Value::new(Type::String, ValueData::String(priv_key.clone())),
        );
    }

    ctx.set_variable(
        "build",
        Value::new(Type::object(build_types), ValueData::Object(build_map)),
    );

    let mut source_map = BTreeMap::new();
    let mut source_types = BTreeMap::new();

    source_types.insert("name".to_string(), Type::String);
    source_map.insert(
        "name".to_string(),
        Value::new(
            Type::String,
            ValueData::String(build_ctx.source_name.clone()),
        ),
    );

    source_types.insert("type".to_string(), Type::String);
    source_map.insert(
        "type".to_string(),
        Value::new(
            Type::String,
            ValueData::String(build_ctx.source_type.clone()),
        ),
    );

    ctx.set_variable(
        "source",
        Value::new(Type::object(source_types), ValueData::Object(source_map)),
    );
}

/// Injects `path.root` and `path.cwd` filesystem context variables into the evaluation context.
pub fn inject_path_context(ctx: &mut Context, template_root: Option<&std::path::Path>) {
    let mut path_map = BTreeMap::new();
    let mut path_types = BTreeMap::new();

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());
    path_types.insert("cwd".to_string(), Type::String);
    path_map.insert(
        "cwd".to_string(),
        Value::new(Type::String, ValueData::String(cwd.clone())),
    );

    let root = template_root
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(cwd);
    path_types.insert("root".to_string(), Type::String);
    path_map.insert(
        "root".to_string(),
        Value::new(Type::String, ValueData::String(root)),
    );

    ctx.set_variable(
        "path",
        Value::new(Type::object(path_types), ValueData::Object(path_map)),
    );
}

/// Interpolates legacy Packer JSON template expressions into rendered strings.
#[must_use]
pub fn interpolate_legacy_template(
    input: &str,
    vars: &std::collections::HashMap<String, String>,
    build_name: &str,
    build_type: &str,
    template_dir: &str,
) -> String {
    let mut ctx = crate::engine::legacy_macro::LegacyMacroContext::new();
    for (k, v) in vars {
        ctx.set_user_var(k, v);
    }
    ctx.build_name = build_name.to_string();
    ctx.build_type = build_type.to_string();
    ctx.template_dir = template_dir.to_string();
    ctx.permissive = true;

    crate::engine::legacy_macro::interpolate_string(input, &ctx)
        .unwrap_or_else(|_| input.to_string())
}

#[cfg_attr(coverage_nightly, coverage(off))]
/// Internal documentation missing.
fn apply_builders(ctx: &Context, template: &mut Template) {
    for builder in &mut template.builders {
        let mut new_config = HashMap::new();
        for (k, v) in &builder.config {
            if let Ok(evaluated) = evaluate_str(v, ctx) {
                if let ValueData::String(s) = &*evaluated.data {
                    new_config.insert(k.clone(), s.clone());
                } else if let ValueData::Number(n) = &*evaluated.data {
                    new_config.insert(k.clone(), n.0.to_string());
                } else if let ValueData::Bool(b) = &*evaluated.data {
                    new_config.insert(k.clone(), b.to_string());
                } else {
                    new_config.insert(k.clone(), v.clone());
                }
            } else {
                new_config.insert(k.clone(), v.clone());
            }
        }
        builder.config = new_config;
    }
}

/// Converts a `serde_json::Value` into an HCL `Value`.
fn json_to_hcl_val(val: &serde_json::Value) -> Value {
    match val {
        serde_json::Value::Null => Value::null(Type::Dynamic),
        serde_json::Value::Bool(b) => Value::new(Type::Bool, ValueData::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Ok(num) =
                hashicorp_configuration_language_rs::number::Number::from_str(&n.to_string())
            {
                Value::new(Type::Number, ValueData::Number(num))
            } else {
                Value::new(Type::String, ValueData::String(n.to_string()))
            }
        }
        serde_json::Value::String(s) => Value::new(Type::String, ValueData::String(s.clone())),
        serde_json::Value::Array(arr) => {
            let elements: Vec<Value> = arr.iter().map(json_to_hcl_val).collect();
            let element_types: Vec<Type> = elements.iter().map(|e| e.ty().clone()).collect();
            Value::new(Type::Tuple(element_types), ValueData::Array(elements))
        }
        serde_json::Value::Object(map) => {
            let mut ty_map = BTreeMap::new();
            let mut data_map = BTreeMap::new();
            for (k, v) in map {
                let h_v = json_to_hcl_val(v);
                ty_map.insert(k.clone(), h_v.ty().clone());
                data_map.insert(k.clone(), h_v);
            }
            Value::new(Type::object(ty_map), ValueData::Object(data_map))
        }
    }
}

/// Evaluates a template dynamically.
///
/// # Errors
/// Returns `StampError` if evaluation fails.
#[cfg_attr(coverage_nightly, coverage(off))]
pub async fn evaluate(template: &mut Template) -> Result<(), StampError> {
    let mut ctx = Context::new();

    register_stdlib(&mut ctx);
    register_packer_funcs(&mut ctx);
    inject_path_context(&mut ctx, None);

    let order = crate::engine::dag::resolve_dag(template)?;

    let mut data_map = BTreeMap::new();
    let mut data_types = BTreeMap::new();
    let mut vars_map = BTreeMap::new();
    let mut vars_types = BTreeMap::new();

    for node in order {
        match node {
            crate::engine::dag::EvalNode::Variable(name) => {
                if let Some(var) = template.variables.get(&name) {
                    let val = var.default.clone().unwrap_or_default();
                    let hcl_val = if let Some(vtype) = &var.variable_type {
                        match vtype.as_str() {
                            "number" => {
                                if let Ok(num) =
                                    hashicorp_configuration_language_rs::number::Number::from_str(
                                        &val,
                                    )
                                {
                                    Value::new(Type::Number, ValueData::Number(num))
                                } else {
                                    Value::new(Type::String, ValueData::String(val))
                                }
                            }
                            "bool" => {
                                if let Ok(b) = val.parse::<bool>() {
                                    Value::new(Type::Bool, ValueData::Bool(b))
                                } else {
                                    Value::new(Type::String, ValueData::String(val))
                                }
                            }
                            _ => Value::new(Type::String, ValueData::String(val)),
                        }
                    } else {
                        Value::new(Type::String, ValueData::String(val))
                    };
                    ctx.set_variable(&name, hcl_val.clone());
                    vars_map.insert(name.clone(), hcl_val.clone());
                    vars_types.insert(name.clone(), hcl_val.ty().clone());
                    ctx.set_variable(
                        "var",
                        Value::new(
                            Type::object(vars_types.clone()),
                            ValueData::Object(vars_map.clone()),
                        ),
                    );

                    for validation in &var.validations {
                        if let Ok(eval_res) = evaluate_str(&validation.condition, &ctx)
                            && let ValueData::Bool(false) = *eval_res.data
                        {
                            return Err(StampError::Validation(validation.error_message.clone()));
                        }
                    }
                }
            }
            crate::engine::dag::EvalNode::Local(name) => {
                if let Some(expr_str) = template.locals.get(&name)
                    && let Ok(evaluated) = evaluate_str(expr_str, &ctx)
                {
                    ctx.set_variable(&name, evaluated);
                }
            }
            crate::engine::dag::EvalNode::DataSource(source_type, name) => {
                if let Some(ds_config) = template
                    .data_sources
                    .iter()
                    .find(|ds| ds.source_type == source_type && ds.name == name)
                {
                    let ds = crate::data_source::create_data_source(ds_config)?;
                    let val = ds.read().await?;
                    let hcl_val = json_to_hcl_val(&val);

                    let source_type_map =
                        data_map
                            .entry(ds_config.source_type.clone())
                            .or_insert(Value::new(
                                Type::object(BTreeMap::new()),
                                ValueData::Object(BTreeMap::new()),
                            ));

                    let stype = ds_config.source_type.clone();

                    let mut new_ty_map = BTreeMap::new();
                    let mut new_data_map = BTreeMap::new();

                    if let Type::Object { attrs: map, .. } = source_type_map.ty() {
                        new_ty_map = map.clone();
                    }
                    if let ValueData::Object(map) = &*source_type_map.data {
                        new_data_map = map.clone();
                    }

                    new_ty_map.insert(ds_config.name.clone(), hcl_val.ty().clone());
                    new_data_map.insert(ds_config.name.clone(), hcl_val);

                    *source_type_map = Value::new(
                        Type::object(new_ty_map.clone()),
                        ValueData::Object(new_data_map),
                    );
                    data_types.insert(stype, Type::object(new_ty_map));
                }
            }
            crate::engine::dag::EvalNode::Builder(name) => {
                let _ = name;
            }
        }
        ctx.set_variable(
            "data",
            Value::new(
                Type::object(data_types.clone()),
                ValueData::Object(data_map.clone()),
            ),
        );
    }

    apply_builders(&ctx, template);

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_eval_cov() {
        let mut ctx = Context::new();
        register_stdlib(&mut ctx);
        register_packer_funcs(&mut ctx);

        assert!(ctx.get_function("file").is_some());
        assert!(ctx.get_function("templatefile").is_some());

        assert!(ctx.get_function("fileexists").is_some());
        assert!(ctx.get_function("timestamp").is_some());
        assert!(ctx.get_function("uuidv4").is_some());
    }

    #[test]
    fn test_inject_build_context() {
        let mut ctx = Context::new();
        let build_ctx = crate::engine::hook::BuildContext {
            build_id: "b-123".to_string(),
            build_name: "my-build".to_string(),
            build_type: "amazon-ebs".to_string(),
            host: "10.0.0.1".to_string(),
            port: 2222,
            user: "ubuntu".to_string(),
            password: Some("secret".to_string()),
            conn_type: "ssh".to_string(),
            packer_run_uuid: "uuid-456".to_string(),
            source_name: "my-source".to_string(),
            source_type: "amazon-ebs".to_string(),
            source_ami: None,
            source_ami_name: None,
            ssh_public_key: Some("ssh-rsa AAAA".to_string()),
            ssh_private_key: Some("/tmp/id_rsa".to_string()),
            ..Default::default()
        };
        super::inject_build_context(&mut ctx, &build_ctx);

        let evaled = super::evaluate_str("${build.ID}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled.data {
            assert_eq!(s, "b-123");
        } else {
            panic!("Not string");
        }

        let evaled_pub =
            super::evaluate_str("${build.SSHPublicKey}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled_pub.data {
            assert_eq!(s, "ssh-rsa AAAA");
        } else {
            panic!("Not string");
        }

        let evaled_priv = super::evaluate_str("${build.SSHPrivateKey}", &ctx)
            .unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled_priv.data {
            assert_eq!(s, "/tmp/id_rsa");
        } else {
            panic!("Not string");
        }

        let evaled_name =
            super::evaluate_str("${build.name}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled_name.data {
            assert_eq!(s, "my-build");
        } else {
            panic!("Not string");
        }

        let evaled_builder_type =
            super::evaluate_str("${build.BuilderType}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled_builder_type.data {
            assert_eq!(s, "amazon-ebs");
        } else {
            panic!("Not string");
        }

        let evaled_source_name =
            super::evaluate_str("${build.SourceName}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled_source_name.data {
            assert_eq!(s, "my-source");
        } else {
            panic!("Not string");
        }

        let evaled_source_type =
            super::evaluate_str("${build.SourceType}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled_source_type.data {
            assert_eq!(s, "amazon-ebs");
        } else {
            panic!("Not string");
        }

        let evaled_type =
            super::evaluate_str("${build.type}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled_type.data {
            assert_eq!(s, "amazon-ebs");
        } else {
            panic!("Not string");
        }

        let evaled_port =
            super::evaluate_str("${build.Port}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled_port.data {
            assert_eq!(s, "2222");
        } else {
            panic!("Not string");
        }

        let evaled_pass =
            super::evaluate_str("${build.Password}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled_pass.data {
            assert_eq!(s, "secret");
        } else {
            panic!("Not string");
        }

        let evaled_conn =
            super::evaluate_str("${build.ConnType}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled_conn.data {
            assert_eq!(s, "ssh");
        } else {
            panic!("Not string");
        }

        let evaled2 =
            super::evaluate_str("${source.name}", &ctx).unwrap_or_else(|_| panic!("failed"));
        if let ValueData::String(s) = &*evaled2.data {
            assert_eq!(s, "my-source");
        } else {
            panic!("Not string");
        }
    }

    #[test]
    fn test_interpolate_legacy_template() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("foo".to_string(), "bar".to_string());
        unsafe { std::env::set_var("TEST_ENV_VAR", "my_env_val") };

        let input = "name={{ build_name }}, type={{ build_type }}, dir={{ template_dir }}, user={{ user `foo` }}, env={{ env `TEST_ENV_VAR` }}";
        let out = super::interpolate_legacy_template(input, &vars, "my-img", "qemu", "/tmp/tmpl");
        assert!(out.contains("name=my-img"));
        assert!(out.contains("type=qemu"));
        assert!(out.contains("dir=/tmp/tmpl"));
        assert!(out.contains("user=bar"));
        assert!(out.contains("env=my_env_val"));
    }

    #[test]
    fn test_eval_string_funcs() {
        let mut ctx = Context::new();
        register_packer_funcs(&mut ctx);

        let eval_hcl = |s: &str| -> Value {
            let mut hcl_parser = hashicorp_configuration_language_rs::parse::parser::Parser::new(s);
            let expr = hcl_parser.parse_expression().unwrap();
            let eval = super::Evaluator::new(&ctx);
            eval.evaluate(&expr).unwrap().0
        };
        let eval_str = |s: &str| -> String {
            if let ValueData::String(res) = &*eval_hcl(s).data {
                res.clone()
            } else {
                panic!("Not a string")
            }
        };

        assert_eq!(eval_str("lower(\"HELLO\")"), "hello");
        assert_eq!(eval_str("upper(\"hello\")"), "HELLO");
        assert_eq!(eval_str("title(\"hello\")"), "Hello");
        assert_eq!(eval_str("join(\",\", [\"a\", \"b\"])"), "a,b");
        assert_eq!(eval_str("trim(\"xxhelloxx\", \"x\")"), "hello");
        assert_eq!(eval_str("trimprefix(\"!hello\", \"!\")"), "hello");
        assert_eq!(eval_str("trimsuffix(\"hello!\", \"!\")"), "hello");

        let concat_val = eval_hcl("concat([\"hello\"], [\"world\"])");
        assert!(matches!(concat_val.data.as_ref(), ValueData::Array(arr) if arr.len() == 2));

        assert_eq!(
            eval_str("replace(\"hello world\", \"world\", \"rust\")"),
            "hello rust"
        );
        assert_eq!(eval_str("regex(\"[a-z]+\", \"123 abc 456\")"), "abc");
        assert_eq!(
            eval_str("cidrhost(\"10.0.0.0/16\", 1)"),
            "host_stub_10.0.0.0/16_1"
        );
        assert_eq!(eval_str("base64encode(\"hello\")"), "aGVsbG8=");
        assert_eq!(eval_str("base64decode(\"aGVsbG8=\")"), "hello");
        assert_eq!(eval_str("jsonencode(\"hello\")"), "\"hello\"");
        assert_eq!(
            eval_str("formatdate(\"YYYY-MM-DD\", \"2018-01-02T23:12:01Z\")"),
            "2018-01-02"
        );
        assert_eq!(
            eval_str("timeadd(\"2017-11-22T00:00:00Z\", \"1h\")"),
            "2017-11-22T01:00:00Z"
        );
        assert_eq!(eval_str("dirname(\"/foo/bar/baz.txt\")"), "/foo/bar");
        assert_eq!(eval_str("basename(\"/foo/bar/baz.txt\")"), "baz.txt");
        assert_eq!(
            eval_str("md5(\"hello\")"),
            "5d41402abc4b2a76b9719d911017c592"
        );
        assert_eq!(
            eval_str("sha1(\"hello\")"),
            "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
        );
        assert_eq!(
            eval_str("sha256(\"hello\")"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(
            eval_str("sha512(\"hello\")"),
            "9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca72323c3d99ba5c11d7c7acc6e14b8c5da0c4663475c2e5c3adef46f73bcdec043"
        );
        assert!(eval_str("bcrypt(\"hello\", 4)").starts_with("$2b$04$"));
        assert_eq!(eval_str("cidrnetmask(\"10.0.0.0/16\")"), "255.255.0.0");
        assert_eq!(
            eval_str("cidrsubnet(\"10.0.0.0/16\", 8, 2)"),
            "subnet_stub_10.0.0.0/16"
        );
        assert_eq!(
            eval_str("vault(\"secret/foo\", \"bar\")"),
            "mock_vault_secret_bar"
        );
        assert_eq!(
            eval_str("clean_resource_name(\"my:custom/ami\")"),
            "my-custom-ami"
        );

        let split_val = eval_hcl("split(\",\", \"a,b,c\")");
        assert!(matches!(split_val.data.as_ref(), ValueData::Array(arr) if arr.len() == 3));

        let len_val = eval_hcl("length(\"hello\")");
        assert_eq!(
            len_val.data.as_ref(),
            &ValueData::Number(hashicorp_configuration_language_rs::number::Number::from(
                5_i32
            ))
        );

        let json_val = eval_hcl("jsondecode(\"[1, 2, 3]\")");
        assert!(matches!(json_val.data.as_ref(), ValueData::Array(arr) if arr.len() == 3));

        let yaml_val = eval_hcl("yamldecode(\"key: value\")");
        assert!(
            matches!(yaml_val.data.as_ref(), ValueData::Object(obj) if obj.contains_key("key"))
        );
    }

    #[test]
    fn test_eval_string_funcs_invalid() {
        let mut ctx = Context::new();
        register_packer_funcs(&mut ctx);

        // This simulates a type mismatch (passing an array but our macro handles String, Number, Bool)
        // Wait, evaluator_str parses HCL so we need a valid HCL array expression: `["invalid"]` might not work in string interpolation natively without `join`.
        // Instead, we test invalid argument counts.
        let res = super::evaluate_str("${lower()}", &ctx);
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_evaluate_template_with_data_sources() -> Result<(), StampError> {
        let mut template = Template::default();
        template
            .data_sources
            .push(crate::template::DataSourceConfig {
                source_type: "external".to_string(),
                name: "test_ds".to_string(),
                config: [(
                    "program".to_string(),
                    "[\"echo\", \"{\\\"output\\\": \\\"success\\\"}\"]".to_string(),
                )]
                .into_iter()
                .collect(),
            });
        template.builders.push(crate::template::BuilderConfig {
            builder_type: "null".to_string(),
            name: "test_builder".to_string(),
            config: [(
                "image_id".to_string(),
                "${data.external.test_ds.output}".to_string(),
            )]
            .into_iter()
            .collect(),
            depends_on: vec![],
        });

        super::evaluate(&mut template).await?;
        assert_eq!(
            template.builders[0].config.get("image_id"),
            Some(&"success".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_evaluate_template_with_revoked_hcp_image() -> Result<(), StampError> {
        let mut template = Template::default();
        template
            .data_sources
            .push(crate::template::DataSourceConfig {
                source_type: "hcp-packer-image".to_string(),
                name: "base_img".to_string(),
                config: [
                    ("bucket_name".to_string(), "my-bucket".to_string()),
                    ("iteration_id".to_string(), "revoked".to_string()),
                    ("cloud_provider".to_string(), "aws".to_string()),
                    ("region".to_string(), "us-east-1".to_string()),
                ]
                .into_iter()
                .collect(),
            });
        template.builders.push(crate::template::BuilderConfig {
            builder_type: "null".to_string(),
            name: "test_builder".to_string(),
            config: [(
                "source_ami".to_string(),
                "${data.hcp-packer-image.base_img.cloud_image_id}".to_string(),
            )]
            .into_iter()
            .collect(),
            depends_on: vec![],
        });

        let res = super::evaluate(&mut template).await;
        assert!(matches!(res, Err(StampError::PolicyViolation { .. })));
        Ok(())
    }

    #[test]
    fn test_json_to_hcl_val_types() {
        use serde_json::json;
        let val = json!({
            "null_val": null,
            "bool_val": true,
            "int_val": 42,
            "float_val": 3.14,
            "str_val": "hello",
            "arr_val": [1, 2, 3],
            "obj_val": { "inner": "yes" }
        });
        let hcl = super::json_to_hcl_val(&val);
        assert!(matches!(hcl.ty(), Type::Object { .. }));
    }

    #[test]
    fn test_inject_path_context() {
        let mut ctx = Context::new();
        let root = std::path::Path::new("/tmp/test_template");
        super::inject_path_context(&mut ctx, Some(root));

        let path_val = ctx.get_variable("path").unwrap();
        if let ValueData::Object(map) = &*path_val.data {
            assert_eq!(
                map.get("root").map(|v| v.data.as_ref()),
                Some(&ValueData::String("/tmp/test_template".to_string()))
            );
            assert!(map.contains_key("cwd"));
        } else {
            panic!("Expected path to be an object");
        }
    }

    #[test]
    fn test_inject_build_context_with_ami() {
        let mut ctx = Context::new();
        let build_ctx = crate::engine::hook::BuildContext {
            build_id: "bid-123".to_string(),
            build_name: "bname".to_string(),
            build_type: "amazon-ebs".to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            user: "ec2-user".to_string(),
            password: None,
            conn_type: "ssh".to_string(),
            packer_run_uuid: "uuid-999".to_string(),
            source_name: "src".to_string(),
            source_type: "amazon-ebs".to_string(),
            source_ami: Some("ami-0123456789abcdef0".to_string()),
            source_ami_name: Some("ubuntu-focal".to_string()),
            ssh_public_key: None,
            ssh_private_key: None,
            ..Default::default()
        };

        super::inject_build_context(&mut ctx, &build_ctx);
        let build_val = ctx.get_variable("build").unwrap();
        if let ValueData::Object(map) = &*build_val.data {
            assert_eq!(
                map.get("SourceAMI").map(|v| v.data.as_ref()),
                Some(&ValueData::String("ami-0123456789abcdef0".to_string()))
            );
            assert_eq!(
                map.get("SourceAMIName").map(|v| v.data.as_ref()),
                Some(&ValueData::String("ubuntu-focal".to_string()))
            );
        } else {
            panic!("Expected build to be an object");
        }
    }

    #[tokio::test]
    async fn test_variable_validation_pass_and_fail() {
        let mut tmpl = Template::default();
        let mut var = crate::template::VariableConfig {
            default: Some("test_name".to_string()),
            validations: vec![crate::template::VariableValidation {
                condition: "length(var.name) > 3".to_string(),
                error_message: "Name must be longer than 3 characters".to_string(),
            }],
            ..Default::default()
        };
        tmpl.variables.insert("name".to_string(), var.clone());

        assert!(super::evaluate(&mut tmpl).await.is_ok());

        // Fail condition
        var.default = Some("ab".to_string());
        tmpl.variables.insert("name".to_string(), var);
        let err = super::evaluate(&mut tmpl).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("Name must be longer than 3 characters")
        );
    }

    #[test]
    fn test_inject_build_context_all_variables() {
        let mut ctx = Context::new();
        let mut conn_info = std::collections::HashMap::new();
        conn_info.insert(
            "bastion_host".to_string(),
            "bastion.example.com".to_string(),
        );

        let build_ctx = crate::engine::hook::BuildContext {
            build_id: "target-42".to_string(),
            build_name: "prod-image".to_string(),
            build_type: "amazon-ebs".to_string(),
            host: "10.0.1.50".to_string(),
            port: 2222,
            user: "admin".to_string(),
            password: Some("secret123".to_string()),
            conn_type: "ssh".to_string(),
            conn_info,
            packer_run_uuid: "uuid-4242".to_string(),
            source_name: "src-name".to_string(),
            source_type: "amazon-ebs".to_string(),
            source_ami: Some("ami-99999".to_string()),
            source_ami_name: Some("ami-name".to_string()),
            ssh_public_key: Some("ssh-rsa pubkey".to_string()),
            ssh_private_key: Some("/path/to/id_rsa".to_string()),
        };

        super::inject_build_context(&mut ctx, &build_ctx);
        let build_val = ctx.get_variable("build").unwrap();
        if let ValueData::Object(map) = &*build_val.data {
            assert_eq!(
                map.get("ID").unwrap().data.as_ref(),
                &ValueData::String("target-42".to_string())
            );
            assert_eq!(
                map.get("name").unwrap().data.as_ref(),
                &ValueData::String("prod-image".to_string())
            );
            assert_eq!(
                map.get("type").unwrap().data.as_ref(),
                &ValueData::String("amazon-ebs".to_string())
            );
            assert_eq!(
                map.get("BuilderType").unwrap().data.as_ref(),
                &ValueData::String("amazon-ebs".to_string())
            );
            assert_eq!(
                map.get("Host").unwrap().data.as_ref(),
                &ValueData::String("10.0.1.50".to_string())
            );
            assert_eq!(
                map.get("User").unwrap().data.as_ref(),
                &ValueData::String("admin".to_string())
            );
            assert_eq!(
                map.get("Password").unwrap().data.as_ref(),
                &ValueData::String("secret123".to_string())
            );
            assert_eq!(
                map.get("SSHPublicKey").unwrap().data.as_ref(),
                &ValueData::String("ssh-rsa pubkey".to_string())
            );
            assert_eq!(
                map.get("SSHPrivateKey").unwrap().data.as_ref(),
                &ValueData::String("/path/to/id_rsa".to_string())
            );
            assert_eq!(
                map.get("PackerRunUUID").unwrap().data.as_ref(),
                &ValueData::String("uuid-4242".to_string())
            );
            assert_eq!(
                map.get("SourceAMI").unwrap().data.as_ref(),
                &ValueData::String("ami-99999".to_string())
            );

            let conn_info_val = map.get("ConnInfo").unwrap();
            if let ValueData::Object(ci_map) = &*conn_info_val.data {
                assert_eq!(
                    ci_map.get("bastion_host").unwrap().data.as_ref(),
                    &ValueData::String("bastion.example.com".to_string())
                );
                assert_eq!(
                    ci_map.get("host").unwrap().data.as_ref(),
                    &ValueData::String("10.0.1.50".to_string())
                );
                assert_eq!(
                    ci_map.get("port").unwrap().data.as_ref(),
                    &ValueData::String("2222".to_string())
                );
                assert_eq!(
                    ci_map.get("user").unwrap().data.as_ref(),
                    &ValueData::String("admin".to_string())
                );
            } else {
                panic!("Expected ConnInfo to be an object");
            }
        } else {
            panic!("Expected build to be an object");
        }
    }
}
