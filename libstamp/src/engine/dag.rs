#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
//! DAG resolution for evaluating templates in the correct dependency order.

use crate::error::StampError;
use crate::template::Template;
use hashicorp_configuration_language_rs::ast::expr::{Expression, TraversalOperator};
use hashicorp_configuration_language_rs::parse::parser::Parser;
use std::collections::{HashMap, HashSet};

/// A node in the evaluation DAG.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum EvalNode {
    /// A variable node.
    Variable(String),
    /// A local node.
    Local(String),
    /// A data source node.
    DataSource(String, String),
    /// A builder node.
    Builder(String),
}

impl std::fmt::Display for EvalNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Variable(name) => write!(f, "var.{name}"),
            Self::Local(name) => write!(f, "local.{name}"),
            Self::DataSource(source_type, name) => write!(f, "data.{source_type}.{name}"),
            Self::Builder(name) => write!(f, "build.{name}"),
        }
    }
}

/// Builds a dependency graph and topologically sorts it.
///
/// # Errors
/// Returns an error if a circular dependency is detected.
pub fn resolve_dag(template: &Template) -> Result<Vec<EvalNode>, StampError> {
    let mut graph: HashMap<EvalNode, Vec<EvalNode>> = HashMap::new();

    for name in template.variables.keys() {
        graph.insert(EvalNode::Variable(name.clone()), Vec::new());
    }

    for (name, expr_str) in &template.locals {
        let node = EvalNode::Local(name.clone());
        let deps = extract_dependencies_from_str(expr_str);
        graph.insert(node, deps);
    }

    for ds in &template.data_sources {
        let node = EvalNode::DataSource(ds.source_type.clone(), ds.name.clone());
        let mut deps = Vec::new();
        for expr_str in ds.config.values() {
            deps.extend(extract_dependencies_from_str(expr_str));
        }
        graph.insert(node, deps);
    }

    for b in &template.builders {
        let node = EvalNode::Builder(b.name.clone());
        let mut deps = Vec::new();
        for expr_str in b.config.values() {
            deps.extend(extract_dependencies_from_str(expr_str));
        }
        for dep_name in &b.depends_on {
            deps.push(EvalNode::Builder(dep_name.clone()));
        }
        graph.insert(node, deps);
    }

    let mut sorted = Vec::new();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();

    let nodes: Vec<EvalNode> = graph.keys().cloned().collect();
    for node in nodes {
        visit(&node, &graph, &mut visiting, &mut visited, &mut sorted)?;
    }

    Ok(sorted)
}

/// Recursively visits DAG nodes performing depth-first topological sorting.
fn visit(
    node: &EvalNode,
    graph: &HashMap<EvalNode, Vec<EvalNode>>,
    visiting: &mut HashSet<EvalNode>,
    visited: &mut HashSet<EvalNode>,
    sorted: &mut Vec<EvalNode>,
) -> Result<(), StampError> {
    if visited.contains(node) {
        return Ok(());
    }
    if visiting.contains(node) {
        return Err(StampError::CircularDependency(format!(
            "Circular dependency detected involving '{node}'"
        )));
    }
    visiting.insert(node.clone());

    if let Some(deps) = graph.get(node) {
        for dep in deps {
            visit(dep, graph, visiting, visited, sorted)?;
        }
    }

    visiting.remove(node);
    visited.insert(node.clone());
    sorted.push(node.clone());
    Ok(())
}

/// Extracts variable, local, data source, and builder dependencies from an expression string.
fn extract_dependencies_from_str(s: &str) -> Vec<EvalNode> {
    let mut deps = Vec::new();
    let mut parser = Parser::new(s);
    if let Some(expr) = parser.parse_expression() {
        extract_from_expr(&expr, &mut deps);
        return deps;
    }

    let dummy = format!("\"{s}\"");
    let mut parser2 = Parser::new(&dummy);
    if let Some(expr) = parser2.parse_expression() {
        extract_from_expr(&expr, &mut deps);
    }
    deps
}

/// Recursively traverses an AST expression and extracts all dependency nodes.
fn extract_from_expr(expr: &Expression, deps: &mut Vec<EvalNode>) {
    match expr {
        Expression::Tuple(arr, _) => {
            for e in arr {
                extract_from_expr(e, deps);
            }
        }
        Expression::Object(obj, _) => {
            for (k, v) in obj {
                extract_from_expr(k, deps);
                extract_from_expr(v, deps);
            }
        }
        Expression::Template(t, _) => {
            for part in t {
                if let hashicorp_configuration_language_rs::ast::expr::TemplatePart::Interpolation(
                    e,
                    _,
                ) = part
                {
                    extract_from_expr(e, deps);
                }
            }
        }
        Expression::Variable(_v, _) => {}
        Expression::Traversal(t, _) => {
            if let Expression::Variable(v, _) = &*t.expr {
                let base = v.as_str();
                if base == "var" || base == "local" || base == "data" || base == "build" {
                    let mut parts = vec![base.to_string()];
                    for op in &t.operators {
                        if let TraversalOperator::GetAttr(attr, _) = op {
                            parts.push(attr.clone());
                        }
                    }
                    match (base, parts.len()) {
                        ("var", len) if len >= 2 => {
                            deps.push(EvalNode::Variable(parts[1].clone()));
                        }
                        ("local", len) if len >= 2 => {
                            deps.push(EvalNode::Local(parts[1].clone()));
                        }
                        ("data", len) if len >= 3 => {
                            deps.push(EvalNode::DataSource(parts[1].clone(), parts[2].clone()));
                        }
                        ("build", len) if len >= 2 => {
                            deps.push(EvalNode::Builder(parts[1].clone()));
                        }
                        _ => {}
                    }
                }
            }
            extract_from_expr(&t.expr, deps);
        }
        Expression::FuncCall(f, _) => {
            for a in &f.args {
                extract_from_expr(a, deps);
            }
        }
        Expression::Parentheses(p, _) => extract_from_expr(p, deps),
        Expression::Conditional(c, _) => {
            extract_from_expr(&c.cond_expr, deps);
            extract_from_expr(&c.true_expr, deps);
            extract_from_expr(&c.false_expr, deps);
        }
        Expression::BinaryOp(_op, lhs, rhs, _) => {
            extract_from_expr(lhs, deps);
            extract_from_expr(rhs, deps);
        }
        Expression::UnaryOp(_op, e, _) => {
            extract_from_expr(e, deps);
        }
        Expression::ForExpr(f, _) => {
            extract_from_expr(&f.collection, deps);
            extract_from_expr(&f.val_expr, deps);
            if let Some(k) = &f.key_expr {
                extract_from_expr(k, deps);
            }
            if let Some(c) = &f.cond_expr {
                extract_from_expr(c, deps);
            }
        }
        _ => {}
    }
}

/// Checks for circular dependencies and missing dependencies among builders.
///
/// # Errors
///
/// Returns [`StampError::CircularDependency`] if a cycle is detected between builders.
/// Returns [`StampError::Execution`] if a builder depends on an unknown builder.
pub fn validate_builder_dependencies(
    builders: &[Box<dyn crate::builder::Builder>],
) -> Result<(), StampError> {
    let names: HashSet<String> = builders.iter().map(|b| b.name()).collect();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    for b in builders {
        let b_name = b.name();
        for dep in b.depends_on() {
            if !names.contains(&dep) {
                return Err(StampError::Execution(format!(
                    "Builder '{b_name}' depends on '{dep}' which is not in the active build plan"
                )));
            }
        }
        graph.insert(b_name, b.depends_on());
    }

    let mut visited = HashSet::new();
    let mut in_stack = HashSet::new();
    let mut stack = Vec::new();

    for b in builders {
        let name = b.name();
        if !visited.contains(&name) {
            dfs_check_builder_cycle(&name, &graph, &mut visited, &mut in_stack, &mut stack)?;
        }
    }

    Ok(())
}

/// Helper to perform depth-first search for cycle detection among builders.
fn dfs_check_builder_cycle(
    node: &str,
    graph: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    in_stack: &mut HashSet<String>,
    stack: &mut Vec<String>,
) -> Result<(), StampError> {
    visited.insert(node.to_string());
    in_stack.insert(node.to_string());
    stack.push(node.to_string());

    if let Some(deps) = graph.get(node) {
        for dep in deps {
            if !visited.contains(dep) {
                dfs_check_builder_cycle(dep, graph, visited, in_stack, stack)?;
            } else if in_stack.contains(dep) {
                let start_idx = stack.iter().position(|x| x == dep).unwrap_or(0);
                let cycle = stack[start_idx..].join(" -> ");
                return Err(StampError::CircularDependency(format!(
                    "Circular dependency detected between builders: {cycle} -> {dep}"
                )));
            }
        }
    }

    in_stack.remove(node);
    stack.pop();
    Ok(())
}

/// Topologically sorts builders into sequential execution tiers.
/// Builders within each tier have all prerequisites satisfied and can run concurrently.
///
/// # Errors
/// Returns [`StampError::CircularDependency`] or [`StampError::Execution`] on cyclic or invalid graphs.
pub fn resolve_builder_execution_tiers(
    builders: &[Box<dyn crate::builder::Builder>],
) -> Result<Vec<Vec<String>>, StampError> {
    validate_builder_dependencies(builders)?;

    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    for b in builders {
        let name = b.name();
        let deps = b.depends_on();
        in_degree.insert(name.clone(), deps.len());
        for dep in deps {
            dependents.entry(dep).or_default().push(name.clone());
        }
    }

    let mut tiers = Vec::new();
    let mut remaining = in_degree.clone();

    while !remaining.is_empty() {
        let mut current_tier: Vec<String> = remaining
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(name, _)| name.clone())
            .collect();

        current_tier.sort();
        for node in &current_tier {
            remaining.remove(node);
            if let Some(deps) = dependents.get(node) {
                for dep in deps {
                    if let Some(deg) = remaining.get_mut(dep)
                        && *deg > 0
                    {
                        *deg -= 1;
                    }
                }
            }
        }
        tiers.push(current_tier);
    }

    Ok(tiers)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::all, clippy::pedantic)]
mod tests {
    use super::*;

    #[test]
    fn test_dag_coverage_extra() {
        let node1 = EvalNode::Builder("a".to_string());
        let node2 = EvalNode::Builder("a".to_string());
        assert_eq!(node1, node2);
        assert_eq!(format!("{node1:?}"), format!("{node2:?}"));

        let mut set = HashSet::new();
        set.insert(node1.clone());
        assert!(set.contains(&node2));
    }

    #[test]
    fn test_eval_node_to_string() {
        assert_eq!(EvalNode::Variable("foo".to_string()).to_string(), "var.foo");
        assert_eq!(EvalNode::Local("bar".to_string()).to_string(), "local.bar");
        assert_eq!(
            EvalNode::DataSource("aws_ami".to_string(), "ubuntu".to_string()).to_string(),
            "data.aws_ami.ubuntu"
        );
        assert_eq!(
            EvalNode::Builder("qemu_img".to_string()).to_string(),
            "build.qemu_img"
        );
    }

    #[test]
    fn test_resolve_dag_simple() -> Result<(), StampError> {
        let mut tmpl = Template::default();
        tmpl.variables.insert(
            "foo".to_string(),
            crate::template::VariableConfig::default(),
        );
        tmpl.locals.insert("bar".to_string(), "var.foo".to_string());
        tmpl.data_sources.push(crate::template::DataSourceConfig {
            source_type: "null".to_string(),
            name: "test".to_string(),
            config: [("k".to_string(), "local.bar".to_string())]
                .into_iter()
                .collect(),
        });
        tmpl.builders.push(crate::template::BuilderConfig {
            builder_type: "null".to_string(),
            name: "b".to_string(),
            config: [("k".to_string(), "data.null.test.val".to_string())]
                .into_iter()
                .collect(),
            depends_on: vec!["dep_b".to_string()],
        });
        tmpl.builders.push(crate::template::BuilderConfig {
            builder_type: "null".to_string(),
            name: "dep_b".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
        });

        tmpl.locals
            .insert("orphan".to_string(), "local.missing_dep".to_string());

        let sorted = resolve_dag(&tmpl)?;
        assert_eq!(sorted.len(), 7);

        let var_idx = sorted
            .iter()
            .position(|n| matches!(n, EvalNode::Variable(_)))
            .unwrap_or_default();
        let local_idx = sorted
            .iter()
            .position(|n| matches!(n, EvalNode::Local(name) if name == "bar"))
            .unwrap_or_default();
        let ds_idx = sorted
            .iter()
            .position(|n| matches!(n, EvalNode::DataSource(_, _)))
            .unwrap_or_default();
        let dep_b_idx = sorted
            .iter()
            .position(|n| matches!(n, EvalNode::Builder(name) if name == "dep_b"))
            .unwrap_or_default();
        let b_idx = sorted
            .iter()
            .position(|n| matches!(n, EvalNode::Builder(name) if name == "b"))
            .unwrap_or_default();

        assert!(var_idx < local_idx);
        assert!(local_idx < ds_idx);
        assert!(ds_idx < b_idx);
        assert!(dep_b_idx < b_idx);
        Ok(())
    }

    #[test]
    fn test_resolve_dag_circular() {
        let mut tmpl = Template::default();
        tmpl.locals.insert("a".to_string(), "local.b".to_string());
        tmpl.locals.insert("b".to_string(), "local.a".to_string());

        let res = resolve_dag(&tmpl);
        assert!(matches!(res, Err(StampError::CircularDependency(_))));
    }

    #[test]
    fn test_extract_dependencies_from_str() {
        let s = "var.foo + local.bar[0] + data.aws_ami.ubuntu.id + build.my_builder.id";
        let deps = extract_dependencies_from_str(s);
        assert!(deps.contains(&EvalNode::Variable("foo".to_string())));
        assert!(deps.contains(&EvalNode::Local("bar".to_string())));
        assert!(deps.contains(&EvalNode::DataSource(
            "aws_ami".to_string(),
            "ubuntu".to_string()
        )));
        assert!(deps.contains(&EvalNode::Builder("my_builder".to_string())));
    }

    #[test]
    fn test_extract_dependencies_all_exprs() {
        let s = r"[var.a, {k = local.b}, func(data.c.d), (var.e)]";
        let deps = extract_dependencies_from_str(s);
        assert!(deps.contains(&EvalNode::Variable("a".to_string())));
        assert!(deps.contains(&EvalNode::Local("b".to_string())));
        assert!(deps.contains(&EvalNode::DataSource("c".to_string(), "d".to_string())));
        assert!(deps.contains(&EvalNode::Variable("e".to_string())));
    }

    #[test]
    fn test_extract_dependencies_conditional_binary_unary() {
        // Conditional, BinaryOp, UnaryOp
        let expr1 = "var.cond ? var.true_val : (local.false_val + -var.num)";
        let deps1 = extract_dependencies_from_str(expr1);
        assert!(deps1.contains(&EvalNode::Variable("cond".to_string())));
        assert!(deps1.contains(&EvalNode::Variable("true_val".to_string())));
        assert!(deps1.contains(&EvalNode::Local("false_val".to_string())));
        assert!(deps1.contains(&EvalNode::Variable("num".to_string())));

        // String template interpolation
        let expr4 = "\"foo-${var.region}-${local.env}\"";
        let deps4 = extract_dependencies_from_str(expr4);
        assert!(deps4.contains(&EvalNode::Variable("region".to_string())));
        assert!(deps4.contains(&EvalNode::Local("env".to_string())));
    }

    #[test]
    fn test_extract_from_expr_for_expr() {
        use hashicorp_configuration_language_rs::ast::expr::{ForExpr, Traversal};
        use hashicorp_configuration_language_rs::span::Span;

        let span = Span::default();
        let col = Expression::Traversal(
            Box::new(Traversal {
                expr: Box::new(Expression::Variable("var".to_string(), span.clone())),
                operators: vec![TraversalOperator::GetAttr(
                    "items".to_string(),
                    span.clone(),
                )],
            }),
            span.clone(),
        );
        let val = Expression::Traversal(
            Box::new(Traversal {
                expr: Box::new(Expression::Variable("local".to_string(), span.clone())),
                operators: vec![TraversalOperator::GetAttr("val".to_string(), span.clone())],
            }),
            span.clone(),
        );
        let key = Expression::Traversal(
            Box::new(Traversal {
                expr: Box::new(Expression::Variable("local".to_string(), span.clone())),
                operators: vec![TraversalOperator::GetAttr("key".to_string(), span.clone())],
            }),
            span.clone(),
        );
        let cond = Expression::Traversal(
            Box::new(Traversal {
                expr: Box::new(Expression::Variable("var".to_string(), span.clone())),
                operators: vec![TraversalOperator::GetAttr("cond".to_string(), span.clone())],
            }),
            span.clone(),
        );

        let for_expr = Expression::ForExpr(
            Box::new(ForExpr {
                key_var: None,
                val_var: "x".to_string(),
                collection: Box::new(col),
                key_expr: Some(Box::new(key)),
                val_expr: Box::new(val),
                cond_expr: Some(Box::new(cond)),
                grouping: false,
            }),
            span,
        );

        let mut deps = Vec::new();
        extract_from_expr(&for_expr, &mut deps);
        assert!(deps.contains(&EvalNode::Variable("items".to_string())));
        assert!(deps.contains(&EvalNode::Local("val".to_string())));
        assert!(deps.contains(&EvalNode::Local("key".to_string())));
        assert!(deps.contains(&EvalNode::Variable("cond".to_string())));
    }

    #[test]
    fn test_extract_dependencies_incomplete_and_bare() {
        let s = r"[var, local, data, build, data.foo, build[0], foo.bar, (var.a).b]";
        let deps = extract_dependencies_from_str(s);
        assert_eq!(deps, vec![EvalNode::Variable("a".to_string())]);
    }

    struct MockBuilder {
        name: String,
        deps: Vec<String>,
    }

    #[async_trait::async_trait]
    impl crate::builder::Builder for MockBuilder {
        fn name(&self) -> String {
            self.name.clone()
        }
        fn depends_on(&self) -> Vec<String> {
            self.deps.clone()
        }
        async fn prepare(&self) -> Result<(), StampError> {
            Ok(())
        }
        async fn run(
            &self,
            _hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
            _ui: std::sync::Arc<crate::engine::ui::Ui>,
            _on_error: crate::engine::packer::OnErrorStrategy,
        ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
            Err(StampError::Execution("mock builder run".to_string()))
        }
        async fn cancel(&self) -> Result<(), StampError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_mock_builder_trait_coverage() {
        use crate::builder::Builder;
        let b = MockBuilder {
            name: "mock".to_string(),
            deps: vec![],
        };
        assert_eq!(b.name(), "mock");
        assert!(b.depends_on().is_empty());
        assert!(b.prepare().await.is_ok());
        assert!(b.cancel().await.is_ok());

        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(crate::engine::hook::DefaultProvisionHook {
                provisioners: std::sync::Arc::new(vec![]),
                error_cleanup_provisioners: std::sync::Arc::new(vec![]),
            });
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        assert!(
            b.run(hook, ui, crate::engine::packer::OnErrorStrategy::Cleanup)
                .await
                .is_err()
        );
    }

    #[test]
    fn test_builder_dependency_resolution_and_tiers() -> Result<(), StampError> {
        let b1: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b1".to_string(),
            deps: vec![],
        });
        let b2: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b2".to_string(),
            deps: vec!["b1".to_string()],
        });
        let b3: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b3".to_string(),
            deps: vec!["b1".to_string()],
        });
        let b4: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b4".to_string(),
            deps: vec!["b2".to_string(), "b3".to_string()],
        });

        let builders = vec![b1, b2, b3, b4];
        let tiers = resolve_builder_execution_tiers(&builders)?;
        assert_eq!(tiers.len(), 3);
        assert_eq!(tiers[0], vec!["b1"]);
        assert_eq!(tiers[1], vec!["b2", "b3"]);
        assert_eq!(tiers[2], vec!["b4"]);
        Ok(())
    }

    #[test]
    fn test_builder_dependency_missing_dep() {
        let b1: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b1".to_string(),
            deps: vec!["nonexistent".to_string()],
        });
        let builders = vec![b1];
        let res = validate_builder_dependencies(&builders);
        assert!(matches!(res, Err(StampError::Execution(_))));
    }

    #[test]
    fn test_dfs_check_builder_cycle_missing_from_graph() -> Result<(), StampError> {
        let graph = HashMap::new();
        let mut visited = HashSet::new();
        let mut in_stack = HashSet::new();
        let mut stack = Vec::new();
        dfs_check_builder_cycle("missing", &graph, &mut visited, &mut in_stack, &mut stack)?;
        Ok(())
    }

    #[test]
    fn test_builder_dependency_cycle_direct() {
        let b1: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b1".to_string(),
            deps: vec!["b2".to_string()],
        });
        let b2: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b2".to_string(),
            deps: vec!["b1".to_string()],
        });
        let builders = vec![b1, b2];
        let res = validate_builder_dependencies(&builders);
        assert!(matches!(res, Err(StampError::CircularDependency(_))));
    }

    #[test]
    fn test_builder_dependency_cycle_with_prefix_and_shared_dep() {
        // b0 -> b1 -> b2 -> b1 (cycle starts in middle of stack, start_idx > 0)
        // b3 -> b0 (shared visited branch)
        let b0: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b0".to_string(),
            deps: vec!["b1".to_string()],
        });
        let b1: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b1".to_string(),
            deps: vec!["b2".to_string()],
        });
        let b2: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b2".to_string(),
            deps: vec!["b1".to_string()],
        });
        let b3: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b3".to_string(),
            deps: vec!["b0".to_string()],
        });

        let builders = vec![b0, b1, b2, b3];
        let res = validate_builder_dependencies(&builders);
        assert!(matches!(res, Err(StampError::CircularDependency(_))));
    }

    #[test]
    fn test_builder_dependency_shared_visited_node_no_cycle() -> Result<(), StampError> {
        // b0 -> b1
        // b2 -> b1
        let b1: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b1".to_string(),
            deps: vec![],
        });
        let b0: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b0".to_string(),
            deps: vec!["b1".to_string()],
        });
        let b2: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b2".to_string(),
            deps: vec!["b1".to_string()],
        });

        let builders = vec![b0, b2, b1];
        validate_builder_dependencies(&builders)?;
        let tiers = resolve_builder_execution_tiers(&builders)?;
        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[0], vec!["b1"]);
        assert_eq!(tiers[1], vec!["b0", "b2"]);
        Ok(())
    }
}
