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
    fn node_to_json(node: &ProximaTreeNode) -> serde_json::Value {
        match node {
            ProximaTreeNode::Value(pv) => proxima_value_to_json(pv),
            ProximaTreeNode::Object(subtree) => serde_json::Value::Object(
                subtree
                    .iter()
                    .map(|(k, n)| (k.clone(), node_to_json(n)))
                    .collect(),
            ),
        }
    }

    props
        .iter()
        .map(|(key, node)| (key.clone(), node_to_json(node)))
        .collect()
}

/// Flatten a `ProximaTree` into the canonical `OptimizedSearchRecord` metadata map.
///
/// Nested objects are preserved as `ProximaValue::Struct` so storage engines can return
/// canonical metadata without detouring through the deprecated v1 `SqlValue` envelope.
pub fn proxima_tree_to_value_map(props: &ProximaTree) -> HashMap<String, ProximaValue> {
    fn node_to_value(node: &ProximaTreeNode) -> ProximaValue {
        match node {
            ProximaTreeNode::Value(value) => value.clone(),
            ProximaTreeNode::Object(subtree) => ProximaValue::Struct(
                subtree
                    .iter()
                    .map(|(key, child)| (key.clone(), node_to_value(child)))
                    .collect(),
            ),
        }
    }

    props
        .iter()
        .map(|(key, node)| (key.clone(), node_to_value(node)))
        .collect()
}

/// Apply a single comparison operator to a field value already lowered to
/// `serde_json::Value` and the filter literal.
///
/// This is the **single source of operator semantics** for every
/// `FilterExpression` evaluator in the crate. Concrete evaluators differ only in
/// how they resolve a field name to a `serde_json::Value`; once resolved, they
/// must all funnel through here so a given `(operator, value, literal)` triple
/// behaves identically regardless of the storage representation it came from.
pub fn compare_json_op(
    operator: &ComparisonOperator,
    json_val: &serde_json::Value,
    value: &serde_json::Value,
) -> bool {
    match operator {
        ComparisonOperator::Equals => json_eq(json_val, value),
        ComparisonOperator::NotEquals => !json_eq(json_val, value),
        ComparisonOperator::LessThan => compare_json_lt(json_val, value),
        ComparisonOperator::LessThanOrEqual => compare_json_lte(json_val, value),
        ComparisonOperator::GreaterThan => compare_json_gt(json_val, value),
        ComparisonOperator::GreaterThanOrEqual => compare_json_gte(json_val, value),
        ComparisonOperator::In => match json_val {
            // Array-valued prop (e.g. `member_oids`): match when
            // the prop set intersects the query list.
            serde_json::Value::Array(items) => value.as_array().is_some_and(|values| {
                items
                    .iter()
                    .any(|item| values.iter().any(|v| json_eq(item, v)))
            }),
            // Scalar prop: membership in the query list.
            _ => value
                .as_array()
                .is_some_and(|values| values.iter().any(|v| json_eq(json_val, v))),
        },
        ComparisonOperator::NotIn => match json_val {
            // Array-valued prop: pass when the prop set is
            // disjoint from the query list.
            serde_json::Value::Array(items) => value.as_array().is_none_or(|values| {
                !items
                    .iter()
                    .any(|item| values.iter().any(|v| json_eq(item, v)))
            }),
            _ => value
                .as_array()
                .is_none_or(|values| values.iter().all(|v| !json_eq(json_val, v))),
        },
        ComparisonOperator::Contains => match json_val {
            // Array-valued prop: element membership
            // (e.g. `member_oids` contains `"u1"`).
            serde_json::Value::Array(items) => items.iter().any(|item| json_eq(item, value)),
            // Scalar string prop: substring match.
            _ => json_val
                .as_str()
                .zip(value.as_str())
                .is_some_and(|(haystack, needle)| haystack.contains(needle)),
        },
        ComparisonOperator::StartsWith => json_val
            .as_str()
            .zip(value.as_str())
            .is_some_and(|(haystack, prefix)| haystack.starts_with(prefix)),
        ComparisonOperator::EndsWith => json_val
            .as_str()
            .zip(value.as_str())
            .is_some_and(|(haystack, suffix)| haystack.ends_with(suffix)),
        ComparisonOperator::Between => value.as_array().is_some_and(|bounds| {
            bounds.len() == 2
                && compare_json_gte(json_val, &bounds[0])
                && compare_json_lte(json_val, &bounds[1])
        }),
        // Null tests on a value that the resolver already produced: present and
        // JSON-null ⇒ null. Field *absence* is handled in `evaluate_filter_resolved`
        // (absent ⇒ IS NULL), which is the only place that can observe absence.
        ComparisonOperator::IsNull => json_val.is_null(),
        ComparisonOperator::IsNotNull => !json_val.is_null(),
        // Full SQL LIKE: `%` = any run, `_` = exactly one char, anywhere in the
        // pattern. Shared with every evaluator via `json_comparison`.
        ComparisonOperator::Like => json_val
            .as_str()
            .zip(value.as_str())
            .is_some_and(|(haystack, pattern)| {
                crate::core::search::json_comparison::like_pattern_match(haystack, pattern)
            }),
    }
}

/// Walk a `FilterExpression`, resolving each comparison's field to a
/// `serde_json::Value` via `resolve`, and apply [`compare_json_op`].
///
/// `resolve` returns `None` when the field is absent or is not a scalar leaf the
/// evaluator handles. For every operator except the null tests, an unresolved
/// field makes the comparison `false`. The null tests follow SQL semantics on
/// absence: a *missing* field IS NULL (`IsNull` ⇒ `true`, `IsNotNull` ⇒ `false`),
/// distinct from a field that is present-and-null (also null).
///
/// This is the shared spine; concrete evaluators are thin adapters that supply a
/// representation-specific `resolve` (see [`evaluate_filter_proxima`]).
pub fn evaluate_filter_resolved<F>(expr: &FilterExpression, resolve: &F) -> bool
where
    F: Fn(&str) -> Option<serde_json::Value>,
{
    match expr {
        FilterExpression::And(exprs) => {
            exprs.iter().all(|e| evaluate_filter_resolved(e, resolve))
        }
        FilterExpression::Or(exprs) => exprs.iter().any(|e| evaluate_filter_resolved(e, resolve)),
        FilterExpression::Not(e) => !evaluate_filter_resolved(e, resolve),
        FilterExpression::Comparison {
            field,
            operator,
            value,
        } => match operator {
            // Null tests are the only operators whose result depends on field
            // *absence*, so they are decided here (where the resolver's `Option`
            // distinguishes absent from present). SQL semantics: an absent field
            // IS NULL. `compare_json_op` cannot see absence, so it must never be
            // reached for these.
            ComparisonOperator::IsNull => resolve(field).is_none_or(|json_val| json_val.is_null()),
            ComparisonOperator::IsNotNull => {
                resolve(field).is_some_and(|json_val| !json_val.is_null())
            }
            _ => match resolve(field) {
                Some(json_val) => compare_json_op(operator, &json_val, value),
                None => false,
            },
        },
    }
}

/// Evaluate a filter expression against a `ProximaTree` (canonical v2 path).
///
/// Thin adapter over [`evaluate_filter_resolved`]: each field resolves to its
/// scalar leaf lowered via [`proxima_value_to_json`]; nested `Object` nodes and
/// absent fields resolve to `None`.
pub fn evaluate_filter_proxima(expr: &FilterExpression, props: &ProximaTree) -> bool {
    evaluate_filter_resolved(expr, &|field| match props.get(field) {
        Some(ProximaTreeNode::Value(pv)) => Some(proxima_value_to_json(pv)),
        _ => None,
    })
}

/// Numeric-aware equality for JSON values — the single equality primitive for
/// every `FilterExpression` evaluator.
///
/// `serde_json::Value`'s derived `PartialEq` compares numbers by their internal
/// representation, so an integer `2` and a float `2.0` are NOT equal, and a large
/// `i64` beyond `f64` exact range can mis-compare. Numbers are compared via the
/// shared, integer-first [`compare_json_numbers`] (precise for `i64`/`u64`, epsilon
/// for floats, `NaN == NaN`); all other JSON values fall back to structural
/// equality (correct for strings, bools, arrays, objects, null).
fn json_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use crate::core::search::json_comparison::compare_json_numbers;
    match (a, b) {
        (serde_json::Value::Number(n1), serde_json::Value::Number(n2)) => {
            compare_json_numbers(n1, n2)
        }
        _ => a == b,
    }
}

/// Total-order comparison shared by every evaluator's ordering operators
/// (`<`, `<=`, `>`, `>=`, `Between`). Delegates to [`compare_json_values`], which
/// orders numbers precisely, strings/bools/arrays by value, and otherwise by JSON
/// type precedence (`Null < Bool < Number < String < Array < Object`).
fn compare_json_lt(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use std::cmp::Ordering;
    crate::core::search::json_comparison::compare_json_values(a, b) == Ordering::Less
}
fn compare_json_lte(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use std::cmp::Ordering;
    matches!(
        crate::core::search::json_comparison::compare_json_values(a, b),
        Ordering::Less | Ordering::Equal
    )
}
fn compare_json_gt(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use std::cmp::Ordering;
    crate::core::search::json_comparison::compare_json_values(a, b) == Ordering::Greater
}
fn compare_json_gte(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use std::cmp::Ordering;
    matches!(
        crate::core::search::json_comparison::compare_json_values(a, b),
        Ordering::Greater | Ordering::Equal
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_sql_value(value: SqlVal) -> SqlValue {
        SqlValue { value: Some(value) }
    }

    fn proxima_array_props(field: &str, values: &[&str]) -> ProximaTree {
        let mut props = ProximaTree::new();
        props.insert(
            field.to_string(),
            ProximaTreeNode::Value(ProximaValue::Array(
                values
                    .iter()
                    .map(|v| ProximaValue::String(v.to_string()))
                    .collect(),
            )),
        );
        props
    }

    #[test]
    fn proxima_scalar_typed_props_compare_by_value() {
        // Covers the string/int/float/bool prop envelope through the canonical
        // ProximaTree evaluator: each typed ProximaValue must round-trip to a
        // JSON value the comparators handle by value (not by representation).
        let mut props = ProximaTree::new();
        props.insert(
            "account_id".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("acctA".to_string())),
        );
        props.insert(
            "tier".to_string(),
            ProximaTreeNode::Value(ProximaValue::Int64(2)),
        );
        props.insert(
            "score".to_string(),
            ProximaTreeNode::Value(ProximaValue::Float64(0.5)),
        );
        props.insert(
            "active".to_string(),
            ProximaTreeNode::Value(ProximaValue::Boolean(true)),
        );

        let cases: Vec<(&str, ComparisonOperator, serde_json::Value, bool)> = vec![
            ("account_id", ComparisonOperator::Equals, json!("acctA"), true),
            ("account_id", ComparisonOperator::Equals, json!("acctB"), false),
            // int prop vs int literal AND float-typed literal (numeric-aware).
            ("tier", ComparisonOperator::Equals, json!(2), true),
            ("tier", ComparisonOperator::Equals, json!(2.0), true),
            ("tier", ComparisonOperator::GreaterThan, json!(1), true),
            ("tier", ComparisonOperator::LessThan, json!(2), false),
            ("score", ComparisonOperator::LessThanOrEqual, json!(0.5), true),
            ("score", ComparisonOperator::GreaterThan, json!(0.9), false),
            ("active", ComparisonOperator::Equals, json!(true), true),
            ("active", ComparisonOperator::NotEquals, json!(false), true),
        ];
        for (field, operator, value, expected) in cases {
            let label = format!("{field} {operator:?} {value} should be {expected}");
            let filter = FilterExpression::Comparison {
                field: field.to_string(),
                operator,
                value,
            };
            assert_eq!(evaluate_filter_proxima(&filter, &props), expected, "{label}");
        }
    }

    #[test]
    fn proxima_array_prop_contains_membership() {
        let props = proxima_array_props("member_oids", &["u1", "u2"]);

        let hit = FilterExpression::Comparison {
            field: "member_oids".to_string(),
            operator: ComparisonOperator::Contains,
            value: json!("u1"),
        };
        let miss = FilterExpression::Comparison {
            field: "member_oids".to_string(),
            operator: ComparisonOperator::Contains,
            value: json!("u9"),
        };
        assert!(evaluate_filter_proxima(&hit, &props));
        assert!(!evaluate_filter_proxima(&miss, &props));
    }

    #[test]
    fn proxima_array_prop_in_intersection() {
        let props = proxima_array_props("member_oids", &["u1", "u2"]);

        let intersects = FilterExpression::Comparison {
            field: "member_oids".to_string(),
            operator: ComparisonOperator::In,
            value: json!(["u2", "u3"]),
        };
        let disjoint = FilterExpression::Comparison {
            field: "member_oids".to_string(),
            operator: ComparisonOperator::In,
            value: json!(["u7", "u8"]),
        };
        assert!(evaluate_filter_proxima(&intersects, &props));
        assert!(!evaluate_filter_proxima(&disjoint, &props));

        // NotIn is the negation: disjoint passes, intersecting fails.
        let not_in_disjoint = FilterExpression::Comparison {
            field: "member_oids".to_string(),
            operator: ComparisonOperator::NotIn,
            value: json!(["u7", "u8"]),
        };
        assert!(evaluate_filter_proxima(&not_in_disjoint, &props));
    }

    /// Golden conformance + cross-representation parity. Pins the canonical
    /// operator semantics (the approved "richest union") AND asserts the
    /// `ProximaTree` adapter and a json-map resolver agree on every case — the
    /// safety net for converging the other evaluators onto this seam.
    #[test]
    fn golden_operator_semantics_parity() {
        use std::collections::HashMap;

        // The same record in both representations.
        let mut props = ProximaTree::new();
        props.insert(
            "account_id".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("acctA".to_string())),
        );
        props.insert(
            "tier".to_string(),
            ProximaTreeNode::Value(ProximaValue::Int64(2)),
        );
        props.insert(
            "score".to_string(),
            ProximaTreeNode::Value(ProximaValue::Float64(0.5)),
        );
        props.insert(
            "active".to_string(),
            ProximaTreeNode::Value(ProximaValue::Boolean(true)),
        );
        props.insert(
            "member_oids".to_string(),
            ProximaTreeNode::Value(ProximaValue::Array(vec![
                ProximaValue::String("u1".to_string()),
                ProximaValue::String("u2".to_string()),
            ])),
        );
        props.insert(
            "maybe".to_string(),
            ProximaTreeNode::Value(ProximaValue::Null),
        );

        let jmap: HashMap<String, serde_json::Value> = HashMap::from([
            ("account_id".to_string(), json!("acctA")),
            ("tier".to_string(), json!(2)),
            ("score".to_string(), json!(0.5)),
            ("active".to_string(), json!(true)),
            ("member_oids".to_string(), json!(["u1", "u2"])),
            ("maybe".to_string(), serde_json::Value::Null),
        ]);

        use ComparisonOperator::*;
        // (field, operator, literal, expected)
        let cases: Vec<(&str, ComparisonOperator, serde_json::Value, bool)> = vec![
            // equality — numeric-aware (int literal AND float-typed literal)
            ("account_id", Equals, json!("acctA"), true),
            ("account_id", NotEquals, json!("acctB"), true),
            ("tier", Equals, json!(2), true),
            ("tier", Equals, json!(2.0), true),
            ("score", Equals, json!(0.5), true),
            // ordering — numeric
            ("tier", GreaterThan, json!(1), true),
            ("tier", LessThan, json!(2), false),
            ("tier", GreaterThanOrEqual, json!(2), true),
            ("score", LessThanOrEqual, json!(0.5), true),
            // ordering — STRING (union: lexicographic; was false on old proxima)
            ("account_id", LessThan, json!("acctB"), true),
            ("account_id", GreaterThan, json!("acctB"), false),
            // In / NotIn — scalar membership
            ("tier", In, json!([1, 2, 3]), true),
            ("tier", NotIn, json!([1, 3]), true),
            // In / Contains / NotIn — array-valued prop (intersection / membership)
            ("member_oids", In, json!(["u2", "u9"]), true),
            ("member_oids", In, json!(["u7", "u8"]), false),
            ("member_oids", Contains, json!("u1"), true),
            ("member_oids", Contains, json!("u9"), false),
            ("member_oids", NotIn, json!(["u7", "u8"]), true),
            // affix
            ("account_id", StartsWith, json!("acc"), true),
            ("account_id", EndsWith, json!("tA"), true),
            ("account_id", StartsWith, json!("xyz"), false),
            // Between (numeric, inclusive)
            ("tier", Between, json!([1, 3]), true),
            ("tier", Between, json!([3, 4]), false),
            // LIKE — full engine: leading/trailing %, internal %, and `_`
            ("account_id", Like, json!("acctA"), true),
            ("account_id", Like, json!("ac%tA"), true),
            ("account_id", Like, json!("%cct%"), true),
            ("account_id", Like, json!("_cctA"), true),
            ("account_id", Like, json!("ac_A"), false),
            // IsNull / IsNotNull — present-and-null
            ("maybe", IsNull, serde_json::Value::Null, true),
            ("maybe", IsNotNull, serde_json::Value::Null, false),
            // present-and-non-null
            ("account_id", IsNull, serde_json::Value::Null, false),
            ("account_id", IsNotNull, serde_json::Value::Null, true),
            // ABSENT field — SQL semantics: absent IS NULL
            ("ghost", IsNull, serde_json::Value::Null, true),
            ("ghost", IsNotNull, serde_json::Value::Null, false),
            ("ghost", Equals, json!("x"), false),
        ];

        for (field, operator, value, expected) in cases {
            let filter = FilterExpression::Comparison {
                field: field.to_string(),
                operator: operator.clone(),
                value,
            };
            let via_proxima = evaluate_filter_proxima(&filter, &props);
            let via_jmap = evaluate_filter_resolved(&filter, &|f| jmap.get(f).cloned());
            assert_eq!(
                via_proxima, expected,
                "proxima: {field} {operator:?} expected {expected}"
            );
            assert_eq!(
                via_jmap, via_proxima,
                "representation parity broke for {field} {operator:?}"
            );
        }
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
