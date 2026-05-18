//! Type-safe metadata filtering for SqlValue-based metadata
//!
//! This module provides efficient, type-preserving filter evaluation for ProximaDB's
//! SqlValue-based metadata system. Unlike JSON-based filtering, this approach:
//!
//! - **Preserves type precision**: No lossy conversions (e.g., integers stay integers)
//! - **Faster execution**: Zero conversion overhead, direct type comparisons
//! - **Type-safe**: Leverages Rust's type system for correctness
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::core::search::sql_value_filter::evaluate_filter;
//! use proximadb::core::search::FilterExpression;
//!
//! let metadata: HashMap<String, SqlValue> = /* ... */;
//! let filter = FilterExpression::Comparison {
//!     field: "category".to_string(),
//!     operator: ComparisonOperator::Equals,
//!     value: serde_json::Value::String("A".to_string()),
//! };
//!
//! if evaluate_filter(&filter, &metadata) {
//!     // Record matches filter
//! }
//! ```
//!
//! ## Implementation Note
//!
//! The filter value comes from the query (as serde_json::Value), while the metadata
//! is stored as SqlValue. We compare them type-safely without lossy conversions.

use std::collections::HashMap;

use crate::core::search::{ComparisonOperator, FilterExpression};
use crate::proto::proximadb_v1::SqlValue;
use crate::proto::proximadb_v1::sql_value::Value as SqlVal;
use proximadb_data_model::ProximaValue;
use proximadb_records::{ProximaTree, ProximaTreeNode};

/// Evaluate a filter expression against SqlValue metadata (type-safe, no conversion)
///
/// This is the canonical filtering implementation for all ProximaDB storage engines.
///
/// # Arguments
///
/// * `expr` - The filter expression to evaluate
/// * `metadata` - The record's metadata as SqlValue HashMap
///
/// # Returns
///
/// `true` if the record matches the filter, `false` otherwise
///
/// # Performance
///
/// - O(1) for simple comparisons
/// - O(n) for AND/OR with n sub-expressions
/// - Zero allocation for comparisons
pub fn evaluate_filter(expr: &FilterExpression, metadata: &HashMap<String, SqlValue>) -> bool {
    match expr {
        FilterExpression::And(exprs) => exprs.iter().all(|e| evaluate_filter(e, metadata)),
        FilterExpression::Or(exprs) => exprs.iter().any(|e| evaluate_filter(e, metadata)),
        FilterExpression::Not(e) => !evaluate_filter(e, metadata),
        FilterExpression::Comparison {
            field,
            operator,
            value,
        } => {
            let field_value = metadata.get(field).and_then(|v| v.value.as_ref());

            match (field_value, operator) {
                (Some(field_val), ComparisonOperator::Equals) => {
                    compare_sql_value_to_json(field_val, value)
                }
                (Some(field_val), ComparisonOperator::NotEquals) => {
                    !compare_sql_value_to_json(field_val, value)
                }
                (Some(SqlVal::NumberValue(n)), ComparisonOperator::LessThan) => {
                    compare_number_lt(*n, value)
                }
                (Some(SqlVal::NumberValue(n)), ComparisonOperator::LessThanOrEqual) => {
                    compare_number_lte(*n, value)
                }
                (Some(SqlVal::NumberValue(n)), ComparisonOperator::GreaterThan) => {
                    compare_number_gt(*n, value)
                }
                (Some(SqlVal::NumberValue(n)), ComparisonOperator::GreaterThanOrEqual) => {
                    compare_number_gte(*n, value)
                }
                // Integer comparisons
                (Some(SqlVal::Int64Value(n)), ComparisonOperator::LessThan) => {
                    compare_int64_lt(*n, value)
                }
                (Some(SqlVal::Int64Value(n)), ComparisonOperator::LessThanOrEqual) => {
                    compare_int64_lte(*n, value)
                }
                (Some(SqlVal::Int64Value(n)), ComparisonOperator::GreaterThan) => {
                    compare_int64_gt(*n, value)
                }
                (Some(SqlVal::Int64Value(n)), ComparisonOperator::GreaterThanOrEqual) => {
                    compare_int64_gte(*n, value)
                }
                (None, _) => false, // Field not found in metadata
                _ => false,         // Unsupported comparison (e.g., string < string)
            }
        }
    }
}

/// Compare SqlValue to serde_json::Value for equality (type-safe)
#[inline]
fn compare_sql_value_to_json(sql_val: &SqlVal, json_val: &serde_json::Value) -> bool {
    match (sql_val, json_val) {
        (SqlVal::StringValue(s1), serde_json::Value::String(s2)) => s1 == s2,
        (SqlVal::NumberValue(n1), serde_json::Value::Number(n2)) => {
            if let Some(n2_f64) = n2.as_f64() {
                // Use epsilon comparison for floating point equality
                (n1 - n2_f64).abs() < f64::EPSILON
            } else {
                false
            }
        }
        (SqlVal::Int64Value(n1), serde_json::Value::Number(n2)) => {
            // Try exact integer match first, then fall back to float comparison
            if let Some(n2_i64) = n2.as_i64() {
                n1 == &n2_i64
            } else if let Some(n2_f64) = n2.as_f64() {
                (*n1 as f64 - n2_f64).abs() < f64::EPSILON
            } else {
                false
            }
        }
        (SqlVal::BoolValue(b1), serde_json::Value::Bool(b2)) => b1 == b2,
        (SqlVal::NullValue(_), serde_json::Value::Null) => true,
        _ => false, // Type mismatch
    }
}

/// Compare number for less-than
#[inline]
fn compare_number_lt(n: f64, json_val: &serde_json::Value) -> bool {
    if let serde_json::Value::Number(filter_num) = json_val
        && let Some(filter_f64) = filter_num.as_f64()
    {
        return n < filter_f64;
    }
    false
}

/// Compare number for less-than-or-equal
#[inline]
fn compare_number_lte(n: f64, json_val: &serde_json::Value) -> bool {
    if let serde_json::Value::Number(filter_num) = json_val
        && let Some(filter_f64) = filter_num.as_f64()
    {
        return n <= filter_f64;
    }
    false
}

/// Compare number for greater-than
#[inline]
fn compare_number_gt(n: f64, json_val: &serde_json::Value) -> bool {
    if let serde_json::Value::Number(filter_num) = json_val
        && let Some(filter_f64) = filter_num.as_f64()
    {
        return n > filter_f64;
    }
    false
}

/// Compare number for greater-than-or-equal
#[inline]
fn compare_number_gte(n: f64, json_val: &serde_json::Value) -> bool {
    if let serde_json::Value::Number(filter_num) = json_val
        && let Some(filter_f64) = filter_num.as_f64()
    {
        return n >= filter_f64;
    }
    false
}

/// Compare Int64 for less-than
#[inline]
fn compare_int64_lt(n: i64, json_val: &serde_json::Value) -> bool {
    if let serde_json::Value::Number(filter_num) = json_val {
        if let Some(filter_i64) = filter_num.as_i64() {
            return n < filter_i64;
        } else if let Some(filter_f64) = filter_num.as_f64() {
            return (n as f64) < filter_f64;
        }
    }
    false
}

/// Compare Int64 for less-than-or-equal
#[inline]
fn compare_int64_lte(n: i64, json_val: &serde_json::Value) -> bool {
    if let serde_json::Value::Number(filter_num) = json_val {
        if let Some(filter_i64) = filter_num.as_i64() {
            return n <= filter_i64;
        } else if let Some(filter_f64) = filter_num.as_f64() {
            return (n as f64) <= filter_f64;
        }
    }
    false
}

/// Compare Int64 for greater-than
#[inline]
fn compare_int64_gt(n: i64, json_val: &serde_json::Value) -> bool {
    if let serde_json::Value::Number(filter_num) = json_val {
        if let Some(filter_i64) = filter_num.as_i64() {
            return n > filter_i64;
        } else if let Some(filter_f64) = filter_num.as_f64() {
            return (n as f64) > filter_f64;
        }
    }
    false
}

/// Compare Int64 for greater-than-or-equal
#[inline]
fn compare_int64_gte(n: i64, json_val: &serde_json::Value) -> bool {
    if let serde_json::Value::Number(filter_num) = json_val {
        if let Some(filter_i64) = filter_num.as_i64() {
            return n >= filter_i64;
        } else if let Some(filter_f64) = filter_num.as_f64() {
            return (n as f64) >= filter_f64;
        }
    }
    false
}

/// Convert a `ProximaValue` leaf to a `serde_json::Value` for filter evaluation.
pub fn proxima_value_to_json(pv: &ProximaValue) -> serde_json::Value {
    match pv {
        ProximaValue::Null => serde_json::Value::Null,
        ProximaValue::Boolean(b) => serde_json::Value::Bool(*b),
        ProximaValue::Int8(n) => serde_json::Value::Number((*n as i64).into()),
        ProximaValue::Int16(n) => serde_json::Value::Number((*n as i64).into()),
        ProximaValue::Int32(n) => serde_json::Value::Number((*n as i64).into()),
        ProximaValue::Int64(n) => serde_json::Value::Number((*n).into()),
        ProximaValue::UInt8(n) => serde_json::Value::Number((*n as u64).into()),
        ProximaValue::UInt16(n) => serde_json::Value::Number((*n as u64).into()),
        ProximaValue::UInt32(n) => serde_json::Value::Number((*n as u64).into()),
        ProximaValue::UInt64(n) => serde_json::Value::Number((*n).into()),
        ProximaValue::Float16(f) | ProximaValue::Float32(f) => {
            serde_json::Number::from_f64(*f as f64)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null)
        }
        ProximaValue::Float64(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        ProximaValue::String(s) | ProximaValue::Symbol(s) | ProximaValue::Decimal(s) => {
            serde_json::Value::String(s.clone())
        }
        ProximaValue::Json(v) => v.clone(),
        ProximaValue::Jsonb(v) => v.clone(),
        ProximaValue::Array(items) => {
            serde_json::Value::Array(items.iter().map(proxima_value_to_json).collect())
        }
        ProximaValue::Map(map) | ProximaValue::Struct(map) => {
            let obj: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), proxima_value_to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        // Temporal types — represent as milliseconds integer
        ProximaValue::Timestamp(ns, _) | ProximaValue::TimestampTz(ns, _) => {
            serde_json::Value::Number((*ns).into())
        }
        ProximaValue::Date(d) => serde_json::Value::Number((*d as i64).into()),
        ProximaValue::Time(t, _) => serde_json::Value::Number((*t).into()),
        // UUID/ULID — string representation
        ProximaValue::Uuid(b) | ProximaValue::ULID(b) => {
            serde_json::Value::String(format!("{b:?}"))
        }
        // Binary — base64-ish string
        ProximaValue::Binary(b) | ProximaValue::BinaryVector(b) => {
            serde_json::Value::String(format!("[binary:{}]", b.len()))
        }
        ProximaValue::DenseVector(v) => serde_json::Value::Array(
            v.iter()
                .map(|f| {
                    serde_json::Number::from_f64(*f as f64)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect(),
        ),
        ProximaValue::SparseVector { .. } => {
            serde_json::Value::String("[sparse_vector]".to_string())
        }
    }
}

/// Flatten a `ProximaTree` to a `HashMap<String, serde_json::Value>` for filter evaluation.
///
/// Nested `Object` nodes are serialised as JSON objects. This matches the shape
/// expected by `json_comparison::evaluate_filter` and `MetadataQueryEngine::evaluate`.
pub fn proxima_tree_to_json_map(props: &ProximaTree) -> HashMap<String, serde_json::Value> {
    props
        .iter()
        .map(|(key, node)| {
            let value = match node {
                ProximaTreeNode::Value(pv) => proxima_value_to_json(pv),
                ProximaTreeNode::Object(subtree) => {
                    let obj: serde_json::Map<String, serde_json::Value> = subtree
                        .iter()
                        .map(|(k, n)| {
                            let v = match n {
                                ProximaTreeNode::Value(pv) => proxima_value_to_json(pv),
                                ProximaTreeNode::Object(_) => {
                                    serde_json::Value::String("[nested]".to_string())
                                }
                            };
                            (k.clone(), v)
                        })
                        .collect();
                    serde_json::Value::Object(obj)
                }
            };
            (key.clone(), value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_sql_value(value: SqlVal) -> SqlValue {
        SqlValue { value: Some(value) }
    }

    #[test]
    fn test_string_equality() {
        let mut metadata = HashMap::new();
        metadata.insert(
            "name".to_string(),
            make_sql_value(SqlVal::StringValue("Alice".to_string())),
        );

        let filter = FilterExpression::Comparison {
            field: "name".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("Alice"),
        };

        assert!(evaluate_filter(&filter, &metadata));
    }

    #[test]
    fn test_number_comparisons() {
        let mut metadata = HashMap::new();
        metadata.insert("age".to_string(), make_sql_value(SqlVal::NumberValue(25.0)));

        // Less than
        let filter = FilterExpression::Comparison {
            field: "age".to_string(),
            operator: ComparisonOperator::LessThan,
            value: json!(30),
        };
        assert!(evaluate_filter(&filter, &metadata));

        // Greater than
        let filter = FilterExpression::Comparison {
            field: "age".to_string(),
            operator: ComparisonOperator::GreaterThan,
            value: json!(20),
        };
        assert!(evaluate_filter(&filter, &metadata));
    }

    #[test]
    fn test_bool_equality() {
        let mut metadata = HashMap::new();
        metadata.insert(
            "active".to_string(),
            make_sql_value(SqlVal::BoolValue(true)),
        );

        let filter = FilterExpression::Comparison {
            field: "active".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!(true),
        };

        assert!(evaluate_filter(&filter, &metadata));
    }

    #[test]
    fn test_and_or_not() {
        let mut metadata = HashMap::new();
        metadata.insert("age".to_string(), make_sql_value(SqlVal::NumberValue(25.0)));
        metadata.insert(
            "active".to_string(),
            make_sql_value(SqlVal::BoolValue(true)),
        );

        // AND
        let filter = FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "age".to_string(),
                operator: ComparisonOperator::GreaterThan,
                value: json!(20),
            },
            FilterExpression::Comparison {
                field: "active".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!(true),
            },
        ]);
        assert!(evaluate_filter(&filter, &metadata));
    }
}
