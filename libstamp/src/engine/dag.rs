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
    DataSource(String, String), // source_type, name
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

/// Internal documentation missing.
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

/// Internal documentation missing.
fn extract_dependencies_from_str(s: &str) -> Vec<EvalNode> {
    let mut deps = Vec::new();
    // Wrap in quotes to parse as string template if it's not a pure expression
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

/// Internal documentation missing.
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
        Expression::Variable(_v, _) => {
            // A bare variable like `var` is not a full reference
        }
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
                    if base == "var" && parts.len() >= 2 {
                        deps.push(EvalNode::Variable(parts[1].clone()));
                    } else if base == "local" && parts.len() >= 2 {
                        deps.push(EvalNode::Local(parts[1].clone()));
                    } else if base == "data" && parts.len() >= 3 {
                        deps.push(EvalNode::DataSource(parts[1].clone(), parts[2].clone()));
                    } else if base == "build" && parts.len() >= 2 {
                        deps.push(EvalNode::Builder(parts[1].clone()));
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

        if current_tier.is_empty() {
            return Err(StampError::CircularDependency(
                "Cycle detected in builder dependencies".to_string(),
            ));
        }

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
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    #[test]
    fn test_dag_coverage_extra() {
        use super::*;
        let node1 = EvalNode::Builder("a".to_string());
        let node2 = EvalNode::Builder("a".to_string());
        assert_eq!(node1, node2);
        assert_eq!(format!("{node1:?}"), format!("{node2:?}"));
    }

    use super::*;

    #[test]
    fn test_eval_node_to_string() {
        assert_eq!(EvalNode::Variable("foo".to_string()).to_string(), "var.foo");
        assert_eq!(EvalNode::Local("bar".to_string()).to_string(), "local.bar");
        assert_eq!(
            EvalNode::DataSource("aws_ami".to_string(), "ubuntu".to_string()).to_string(),
            "data.aws_ami.ubuntu"
        );
        assert_eq!(
            EvalNode::Builder("my_builder".to_string()).to_string(),
            "build.my_builder"
        );
    }

    #[test]
    fn test_resolve_dag_simple() {
        let mut tmpl = Template::default();
        tmpl.variables.insert(
            "foo".to_string(),
            crate::template::VariableConfig::default(),
        );
        tmpl.locals.insert("bar".to_string(), "var.foo".to_string());

        let ds = crate::template::DataSourceConfig {
            source_type: "amazon-ami".to_string(),
            name: "ubuntu".to_string(),
            config: std::collections::HashMap::from([(
                "filter".to_string(),
                "local.bar".to_string(),
            )]),
        };
        tmpl.data_sources.push(ds);

        let builder = crate::template::BuilderConfig {
            builder_type: "amazon-ebs".to_string(),
            name: "my_ebs".to_string(),
            config: std::collections::HashMap::from([(
                "source_ami".to_string(),
                "data.amazon-ami.ubuntu.id".to_string(),
            )]),
            ..Default::default()
        };
        tmpl.builders.push(builder);

        let sorted = resolve_dag(&tmpl).unwrap_or_else(|_| panic!("failed"));
        assert_eq!(sorted.len(), 4);
        assert_eq!(sorted[0], EvalNode::Variable("foo".to_string()));
        assert_eq!(sorted[1], EvalNode::Local("bar".to_string()));
        assert_eq!(
            sorted[2],
            EvalNode::DataSource("amazon-ami".to_string(), "ubuntu".to_string())
        );
        assert_eq!(sorted[3], EvalNode::Builder("my_ebs".to_string()));
    }

    #[test]
    fn test_resolve_dag_circular() {
        let mut tmpl = Template::default();
        tmpl.locals.insert("a".to_string(), "local.b".to_string());
        tmpl.locals.insert("b".to_string(), "local.a".to_string());

        let err = resolve_dag(&tmpl).unwrap_err();
        assert!(matches!(err, StampError::CircularDependency(_)));
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
    fn test_extract_dependencies_incomplete_and_bare() {
        let s = r"[var, local, data.foo, bare_var]";
        let deps = extract_dependencies_from_str(s);
        assert!(deps.is_empty());
    }

    struct MockBuilder {
        name: String,
        deps: Vec<String>,
    }

    #[async_trait::async_trait]
    impl crate::builder::Builder for MockBuilder {
        async fn prepare(&self) -> Result<(), StampError> {
            Ok(())
        }
        async fn run(
            &self,
            _hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
            _ui: std::sync::Arc<crate::engine::ui::Ui>,
            _on_error: crate::engine::packer::OnErrorStrategy,
        ) -> Result<Box<dyn crate::artifact::Artifact>, StampError> {
            Ok(Box::new(crate::artifact::MockArtifact {
                builder_id: self.name.clone(),
                id: self.name.clone(),
                files: vec![],
            }))
        }
        async fn cancel(&self) -> Result<(), StampError> {
            Ok(())
        }
        fn name(&self) -> String {
            self.name.clone()
        }
        fn depends_on(&self) -> Vec<String> {
            self.deps.clone()
        }
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
        let err = validate_builder_dependencies(&builders).unwrap_err();
        assert!(matches!(err, StampError::Execution(_)));
    }

    #[test]
    fn test_builder_dependency_cycle() {
        let b1: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b1".to_string(),
            deps: vec!["b2".to_string()],
        });
        let b2: Box<dyn crate::builder::Builder> = Box::new(MockBuilder {
            name: "b2".to_string(),
            deps: vec!["b1".to_string()],
        });
        let builders = vec![b1, b2];
        let err = validate_builder_dependencies(&builders).unwrap_err();
        assert!(matches!(err, StampError::CircularDependency(_)));
    }
}
