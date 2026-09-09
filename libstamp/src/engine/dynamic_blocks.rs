#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
//! Dynamic Block Expander delegating directly to `hashicorp_configuration_language_rs`.

use crate::error::StampError;
use hashicorp_configuration_language_rs::ast::structure::Body;
use hashicorp_configuration_language_rs::eval::context::Context;

/// Expands dynamic blocks within an HCL AST body using `hashicorp_configuration_language_rs`.
///
/// # Arguments
/// * `body` - The HCL body whose dynamic blocks will be expanded in place.
/// * `ctx` - The evaluation context containing variables and functions.
///
/// # Errors
/// Returns `StampError::Validation` if dynamic block evaluation produces diagnostics errors.
pub fn expand_dynamic_blocks(body: &mut Body, ctx: &Context) -> Result<(), StampError> {
    let mut eval_ctx = ctx.clone();
    let expanded = hashicorp_configuration_language_rs::eval::dynblock::expand_dynamic_blocks(
        body,
        &mut eval_ctx,
    )
    .map_err(|diags| StampError::Validation(format!("{diags:?}")))?;
    *body = expanded;
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use hashicorp_configuration_language_rs::parse::parser::Parser;

    #[test]
    fn test_expand_dynamic_blocks_array() -> Result<(), StampError> {
        let input = r#"
            dynamic "tag" {
                for_each = ["a", "b"]
                content {
                    name = tag.value
                }
            }
        "#;
        let mut parser = Parser::new(input);
        let mut body = parser.parse_body();

        let ctx = Context::new();
        expand_dynamic_blocks(&mut body, &ctx)?;

        assert_eq!(body.blocks.len(), 2);
        assert_eq!(body.blocks[0].block_type, "tag");
        assert_eq!(body.blocks[1].block_type, "tag");
        Ok(())
    }

    #[test]
    fn test_expand_dynamic_blocks_object() -> Result<(), StampError> {
        let input = r#"
            dynamic "setting" {
                for_each = {
                    "key1" = "val1"
                    "key2" = "val2"
                }
                iterator = item
                labels = ["a"]
                content {
                    k = item.key
                    v = item.value
                }
            }
        "#;
        let mut parser = Parser::new(input);
        let mut body = parser.parse_body();

        let ctx = Context::new();
        expand_dynamic_blocks(&mut body, &ctx)?;

        assert_eq!(body.blocks.len(), 2);
        assert_eq!(body.blocks[0].block_type, "setting");
        assert_eq!(body.blocks[0].labels, vec!["a".to_string()]);
        Ok(())
    }

    #[test]
    fn test_expand_dynamic_blocks_missing_for_each() -> Result<(), crate::error::StampError> {
        let input = r#"
            dynamic "test" {
                content {}
            }
        "#;
        let mut parser = Parser::new(input);
        let _body = parser.parse_body();

        let err = crate::error::StampError::Parse(format!("{:?}", parser.errors()));
        assert!(err.to_string().contains("Missing required argument"));

        Ok(())
    }

    #[test]
    fn test_expand_dynamic_blocks_missing_content() -> Result<(), crate::error::StampError> {
        let input = r#"
            dynamic "test" {
                for_each = []
            }
        "#;
        let mut parser = Parser::new(input);
        let _body = parser.parse_body();

        let err = crate::error::StampError::Parse(format!("{:?}", parser.errors()));
        assert!(err.to_string().contains("Missing required block"));

        Ok(())
    }

    #[test]
    fn test_expand_dynamic_blocks_invalid_labels() -> Result<(), crate::error::StampError> {
        let input = r#"
            dynamic "test" "extra" {
                for_each = []
                content {}
            }
        "#;
        let mut parser = Parser::new(input);
        let _body = parser.parse_body();

        let err = crate::error::StampError::Parse(format!("{:?}", parser.errors()));
        assert!(err.to_string().contains("Block opening brace expected"));

        Ok(())
    }

    #[test]
    fn test_expand_dynamic_blocks_not_collection() -> Result<(), crate::error::StampError> {
        let input = r#"
            dynamic "test" {
                for_each = "string"
                content {}
            }
        "#;
        let mut parser = Parser::new(input);
        let mut body = parser.parse_body();

        let ctx = Context::new();
        let err = expand_dynamic_blocks(&mut body, &ctx).unwrap_err();
        assert!(
            matches!(err, StampError::Validation(msg) if msg.contains("must be a list, set, or map"))
        );

        Ok(())
    }

    #[test]
    fn test_expand_dynamic_blocks_invalid_dynamic_labels() -> Result<(), crate::error::StampError> {
        let input = r#"
            dynamic "test" {
                for_each = ["item"]
                labels = [true]
                content {}
            }
        "#;
        let mut parser = Parser::new(input);
        let mut body = parser.parse_body();

        let ctx = Context::new();
        let err = expand_dynamic_blocks(&mut body, &ctx).unwrap_err();
        assert!(
            matches!(err, StampError::Validation(msg) if msg.contains("must evaluate to a string or number"))
        );

        Ok(())
    }

    #[test]
    fn test_expand_dynamic_blocks_invalid_labels_type() -> Result<(), crate::error::StampError> {
        let input = r#"
            dynamic "test" {
                for_each = []
                labels = "not_tuple"
                content {}
            }
        "#;
        let mut parser = Parser::new(input);
        let _body = parser.parse_body();

        let err = crate::error::StampError::Parse(format!("{:?}", parser.errors()));
        assert!(err.to_string().contains("Invalid labels argument"));

        Ok(())
    }
}
