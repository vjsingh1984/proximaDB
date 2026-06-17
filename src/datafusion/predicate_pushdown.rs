//! # Predicate Pushdown for ProximaDB
//!
//! Converts DataFusion expressions to ProximaDB FilterExpression for storage-level filtering.
//! Supports comparison, logical, and IN-list predicates.

use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::logical_expr::{BinaryExpr, Expr, Operator};
use datafusion::scalar::ScalarValue;
use tracing::debug;

use crate::storage::formats::{ComparisonOp, FilterExpression};
use crate::storage::schema::ProximaSchema;

/// Result of predicate pushdown analysis.
#[derive(Debug, Clone)]
pub struct FilterPushdownResult {
    /// Filter that can be pushed to storage.
    pub pushed_filter: Option<FilterExpression>,
    /// Expressions that must be evaluated post-scan.
    pub residual_exprs: Vec<Expr>,
}

/// Capability of pushing down a predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushdownCapability {
    /// Filter can be evaluated exactly at storage level.
    Exact,
    /// Filter can be partially evaluated (may have false positives).
    Inexact,
    /// Filter cannot be pushed down.
    Unsupported,
}

/// Convert a DataFusion expression to a ProximaDB FilterExpression.
///
/// Returns an error if the expression cannot be converted.
pub fn convert_expr_to_filter(expr: &Expr, schema: &ProximaSchema) -> DFResult<FilterExpression> {
    match expr {
        Expr::BinaryExpr(binary) => convert_binary_expr(binary, schema),
        Expr::Not(inner) => {
            let inner_filter = convert_expr_to_filter(inner, schema)?;
            Ok(FilterExpression::Not(Box::new(inner_filter)))
        }
        Expr::IsNull(inner) => {
            let column_name = extract_column_name(inner)?;
            validate_column_exists(&column_name, schema)?;
            Ok(FilterExpression::IsNull {
                column: column_name,
            })
        }
        Expr::IsNotNull(inner) => {
            let column_name = extract_column_name(inner)?;
            validate_column_exists(&column_name, schema)?;
            Ok(FilterExpression::IsNotNull {
                column: column_name,
            })
        }
        Expr::InList(in_list) => {
            let column_name = extract_column_name(&in_list.expr)?;
            validate_column_exists(&column_name, schema)?;

            let values: Result<Vec<_>, _> = in_list.list.iter().map(scalar_to_json_value).collect();

            let filter = FilterExpression::In {
                column: column_name,
                values: values?,
            };

            if in_list.negated {
                Ok(FilterExpression::Not(Box::new(filter)))
            } else {
                Ok(filter)
            }
        }
        Expr::Between(between) => {
            let column_name = extract_column_name(&between.expr)?;
            validate_column_exists(&column_name, schema)?;

            let low = scalar_to_json_value(&between.low)?;
            let high = scalar_to_json_value(&between.high)?;

            // BETWEEN is equivalent to: column >= low AND column <= high
            let filter = FilterExpression::And(vec![
                FilterExpression::Comparison {
                    column: column_name.clone(),
                    op: ComparisonOp::Ge,
                    value: low,
                },
                FilterExpression::Comparison {
                    column: column_name,
                    op: ComparisonOp::Le,
                    value: high,
                },
            ]);

            if between.negated {
                Ok(FilterExpression::Not(Box::new(filter)))
            } else {
                Ok(filter)
            }
        }
        Expr::Literal(scalar, _) => {
            // Literal true/false - convert to trivial filter
            match scalar {
                ScalarValue::Boolean(Some(true)) => {
                    // Always true - no filter needed, but we need to return something
                    // Return an IsNotNull on a system column or empty And
                    Ok(FilterExpression::And(vec![]))
                }
                ScalarValue::Boolean(Some(false)) => {
                    // Always false - will never match
                    // Return NOT of empty And (which is NOT true = false)
                    Ok(FilterExpression::Not(Box::new(FilterExpression::And(
                        vec![],
                    ))))
                }
                _ => Err(DataFusionError::Plan(format!(
                    "Cannot push down standalone literal: {:?}",
                    scalar
                ))),
            }
        }
        _ => Err(DataFusionError::Plan(format!(
            "Unsupported expression for pushdown: {:?}",
            expr
        ))),
    }
}

/// Convert a binary expression to FilterExpression.
fn convert_binary_expr(binary: &BinaryExpr, schema: &ProximaSchema) -> DFResult<FilterExpression> {
    match binary.op {
        // Logical operators
        Operator::And => {
            let left = convert_expr_to_filter(&binary.left, schema)?;
            let right = convert_expr_to_filter(&binary.right, schema)?;
            Ok(FilterExpression::And(vec![left, right]))
        }
        Operator::Or => {
            let left = convert_expr_to_filter(&binary.left, schema)?;
            let right = convert_expr_to_filter(&binary.right, schema)?;
            Ok(FilterExpression::Or(vec![left, right]))
        }

        // Comparison operators
        Operator::Eq => convert_comparison(&binary.left, &binary.right, schema, ComparisonOp::Eq),
        Operator::NotEq => {
            convert_comparison(&binary.left, &binary.right, schema, ComparisonOp::Ne)
        }
        Operator::Lt => convert_comparison(&binary.left, &binary.right, schema, ComparisonOp::Lt),
        Operator::LtEq => convert_comparison(&binary.left, &binary.right, schema, ComparisonOp::Le),
        Operator::Gt => convert_comparison(&binary.left, &binary.right, schema, ComparisonOp::Gt),
        Operator::GtEq => convert_comparison(&binary.left, &binary.right, schema, ComparisonOp::Ge),

        // String operators
        Operator::LikeMatch => {
            let column_name = extract_column_name(&binary.left)?;
            validate_column_exists(&column_name, schema)?;
            let pattern = scalar_to_json_value(&binary.right)?;
            Ok(FilterExpression::Comparison {
                column: column_name,
                op: ComparisonOp::Like,
                value: pattern,
            })
        }
        Operator::ILikeMatch => {
            // Case-insensitive LIKE - treat as LIKE for now
            let column_name = extract_column_name(&binary.left)?;
            validate_column_exists(&column_name, schema)?;
            let pattern = scalar_to_json_value(&binary.right)?;
            Ok(FilterExpression::Comparison {
                column: column_name,
                op: ComparisonOp::Like,
                value: pattern,
            })
        }

        _ => Err(DataFusionError::Plan(format!(
            "Unsupported operator for pushdown: {:?}",
            binary.op
        ))),
    }
}

/// Convert a comparison expression.
fn convert_comparison(
    left: &Expr,
    right: &Expr,
    schema: &ProximaSchema,
    op: ComparisonOp,
) -> DFResult<FilterExpression> {
    // Try column op literal
    if let (Ok(column), Ok(value)) = (extract_column_name(left), scalar_to_json_value(right)) {
        validate_column_exists(&column, schema)?;
        return Ok(FilterExpression::Comparison { column, op, value });
    }

    // Try literal op column (reverse)
    if let (Ok(value), Ok(column)) = (scalar_to_json_value(left), extract_column_name(right)) {
        validate_column_exists(&column, schema)?;
        // Reverse the comparison for literal op column
        let reversed_op = match op {
            ComparisonOp::Lt => ComparisonOp::Gt,
            ComparisonOp::Le => ComparisonOp::Ge,
            ComparisonOp::Gt => ComparisonOp::Lt,
            ComparisonOp::Ge => ComparisonOp::Le,
            other => other,
        };
        return Ok(FilterExpression::Comparison {
            column,
            op: reversed_op,
            value,
        });
    }

    Err(DataFusionError::Plan(
        "Comparison must be between column and literal".to_string(),
    ))
}

/// Extract column name from expression.
fn extract_column_name(expr: &Expr) -> DFResult<String> {
    match expr {
        Expr::Column(col) => Ok(col.name.clone()),
        _ => Err(DataFusionError::Plan(format!(
            "Expected column reference, got: {:?}",
            expr
        ))),
    }
}

/// Validate that a column exists in the schema.
fn validate_column_exists(column_name: &str, schema: &ProximaSchema) -> DFResult<()> {
    if schema.column_by_name(column_name).is_some() {
        Ok(())
    } else {
        Err(DataFusionError::Plan(format!(
            "Column '{}' not found in schema",
            column_name
        )))
    }
}

/// Convert a scalar expression to serde_json::Value.
fn scalar_to_json_value(expr: &Expr) -> DFResult<serde_json::Value> {
    match expr {
        Expr::Literal(scalar, _) => scalar_value_to_json(scalar),
        _ => Err(DataFusionError::Plan(format!(
            "Expected literal, got: {:?}",
            expr
        ))),
    }
}

/// Convert ScalarValue to serde_json::Value.
fn scalar_value_to_json(scalar: &ScalarValue) -> DFResult<serde_json::Value> {
    match scalar {
        ScalarValue::Null => Ok(serde_json::Value::Null),
        ScalarValue::Boolean(Some(b)) => Ok(serde_json::Value::Bool(*b)),
        ScalarValue::Int8(Some(i)) => Ok(serde_json::json!(*i)),
        ScalarValue::Int16(Some(i)) => Ok(serde_json::json!(*i)),
        ScalarValue::Int32(Some(i)) => Ok(serde_json::json!(*i)),
        ScalarValue::Int64(Some(i)) => Ok(serde_json::json!(*i)),
        ScalarValue::UInt8(Some(i)) => Ok(serde_json::json!(*i)),
        ScalarValue::UInt16(Some(i)) => Ok(serde_json::json!(*i)),
        ScalarValue::UInt32(Some(i)) => Ok(serde_json::json!(*i)),
        ScalarValue::UInt64(Some(i)) => Ok(serde_json::json!(*i)),
        ScalarValue::Float32(Some(f)) => Ok(serde_json::json!(*f)),
        ScalarValue::Float64(Some(f)) => Ok(serde_json::json!(*f)),
        ScalarValue::Utf8(Some(s)) | ScalarValue::LargeUtf8(Some(s)) => {
            Ok(serde_json::Value::String(s.clone()))
        }
        ScalarValue::Binary(Some(b)) | ScalarValue::LargeBinary(Some(b)) => {
            // Encode binary as base64
            Ok(serde_json::Value::String(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                b,
            )))
        }
        _ => Err(DataFusionError::Plan(format!(
            "Unsupported scalar type for pushdown: {:?}",
            scalar
        ))),
    }
}

/// Analyze predicates and determine pushdown strategy.
pub fn analyze_predicates(predicates: &[Expr], schema: &ProximaSchema) -> FilterPushdownResult {
    let mut pushed_filters = Vec::new();
    let mut residual_exprs = Vec::new();

    for expr in predicates {
        match convert_expr_to_filter(expr, schema) {
            Ok(filter) => {
                debug!("Pushing down filter: {:?}", filter);
                pushed_filters.push(filter);
            }
            Err(e) => {
                debug!("Cannot push down expression: {:?}, reason: {}", expr, e);
                residual_exprs.push(expr.clone());
            }
        }
    }

    // Combine pushed filters with AND
    let pushed_filter = if pushed_filters.is_empty() {
        None
    } else if pushed_filters.len() == 1 {
        pushed_filters.into_iter().next()
    } else {
        Some(FilterExpression::And(pushed_filters))
    };

    FilterPushdownResult {
        pushed_filter,
        residual_exprs,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::logical_expr::col;

    fn test_schema() -> ProximaSchema {
        ProximaSchema::vector_record_schema(128)
    }

    #[test]
    fn test_simple_equality() {
        let schema = test_schema();
        let expr = col("id").eq(Expr::Literal(
            ScalarValue::Utf8(Some("test".to_string())),
            None,
        ));

        let result = convert_expr_to_filter(&expr, &schema);
        assert!(result.is_ok());
    }

    #[test]
    fn test_and_expression() {
        let schema = test_schema();
        let expr = col("id")
            .eq(Expr::Literal(
                ScalarValue::Utf8(Some("test".to_string())),
                None,
            ))
            .and(col("id").is_not_null());

        let result = convert_expr_to_filter(&expr, &schema);
        assert!(result.is_ok());
    }

    #[test]
    fn test_invalid_column() {
        let schema = test_schema();
        let expr = col("nonexistent").eq(Expr::Literal(ScalarValue::Int64(Some(42)), None));

        let result = convert_expr_to_filter(&expr, &schema);
        assert!(result.is_err());
    }
}
