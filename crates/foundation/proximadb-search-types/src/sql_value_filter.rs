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

use proximadb_data_model::ProximaValue;
use proximadb_filter_expression::{ComparisonOperator, FilterExpression};
use proximadb_proto::proximadb_v1::SqlValue;
use proximadb_proto::proximadb_v1::sql_value::Value as SqlVal;
use proximadb_records::{ProximaTree, ProximaTreeNode};

/// Fail-loud error from the strict v2 metadata-filter evaluators (TD-FILT-1). The legacy
/// [`evaluate_filter_resolved`] silently folds an unresolved field into `false` (dropping the row);
/// the strict variants surface it so a caller can reject the result instead of accepting a silently
/// wrong set. v2-canonical: this lives on the `ProximaValue`/`ProximaTree` seam (ADR-024), not the
/// deprecated v1 `MetadataValue` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterEvalError {
    /// A value comparison referenced a field the resolver could not lower to a comparable value —
    /// either absent from the record, or a non-scalar leaf. (`IsNull`/`IsNotNull` are NOT errors:
    /// field absence is their domain, per SQL semantics.)
    MissingField { field: String },
}

impl std::fmt::Display for FilterEvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterEvalError::MissingField { field } => write!(
                f,
                "filter field {field:?} could not be resolved to a comparable value (absent or non-scalar leaf)"
            ),
        }
    }
}

impl std::error::Error for FilterEvalError {}

/// Evaluate a filter expression against proto `SqlValue` (wire-format) metadata.
///
/// Thin adapter over the canonical [`evaluate_filter_resolved`] seam: each field's
/// `SqlValue` is lowered to `serde_json::Value` via [`sql_val_to_json`] (numbers
/// stay numeric so integer precision is preserved by `compare_json_numbers`), so
/// this path supports the full operator set with semantics identical to
/// [`evaluate_filter_proxima`]. Previously this evaluator handled only
/// equality + numeric ordering; it now matches the canonical behavior.
///
/// * `expr` - The filter expression to evaluate
/// * `metadata` - The record's metadata as a proto `SqlValue` map
pub fn evaluate_filter(expr: &FilterExpression, metadata: &HashMap<String, SqlValue>) -> bool {
    evaluate_filter_resolved(expr, &|field| {
        metadata
            .get(field)
            .and_then(|sql_value| sql_value.value.as_ref())
            .map(sql_val_to_json)
    })
}

/// Lower a proto `SqlValue` payload to `serde_json::Value` so the wire/`SqlValue`
/// metadata path shares the canonical operator semantics (the seam compares on
/// `serde_json::Value`). Numbers stay numeric (so `compare_json_numbers` keeps
/// integer precision); bytes become a JSON array of byte values.
fn sql_val_to_json(value: &SqlVal) -> serde_json::Value {
    match value {
        SqlVal::StringValue(s) => serde_json::Value::String(s.clone()),
        SqlVal::NumberValue(n) => serde_json::Number::from_f64(*n)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        SqlVal::BoolValue(b) => serde_json::Value::Bool(*b),
        SqlVal::Int64Value(i) => serde_json::Value::Number((*i).into()),
        SqlVal::BytesValue(bytes) => serde_json::Value::Array(
            bytes
                .iter()
                .map(|byte| serde_json::Value::Number((*byte).into()))
                .collect(),
        ),
        SqlVal::NullValue(_) => serde_json::Value::Null,
        SqlVal::ArrayValue(array) => serde_json::Value::Array(
            array
                .values
                .iter()
                .map(|v| {
                    v.value
                        .as_ref()
                        .map_or(serde_json::Value::Null, sql_val_to_json)
                })
                .collect(),
        ),
        SqlVal::ObjectValue(object) => serde_json::Value::Object(
            object
                .fields
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.value
                            .as_ref()
                            .map_or(serde_json::Value::Null, sql_val_to_json),
                    )
                })
                .collect(),
        ),
    }
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
        ComparisonOperator::Like => {
            json_val
                .as_str()
                .zip(value.as_str())
                .is_some_and(|(haystack, pattern)| {
                    crate::json_comparison::like_pattern_match(haystack, pattern)
                })
        }
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
        FilterExpression::And(exprs) => exprs.iter().all(|e| evaluate_filter_resolved(e, resolve)),
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

/// Fail-loud variant of [`evaluate_filter_resolved`] (TD-FILT-1, v2-canonical). Returns
/// `Err(MissingField)` for a value comparison whose field the resolver cannot lower (absent or a
/// non-scalar leaf), instead of silently returning `false`. `IsNull`/`IsNotNull` keep SQL absence
/// semantics (`Ok`, never an error). The comparison layer is unchanged (it total-orders by JSON
/// type precedence, so there is no per-comparison type mismatch to surface on this path).
///
/// Callers wanting ADR-043 fail-loud semantics migrate here; the legacy `evaluate_filter*` stay the
/// silent default until call-sites are migrated slice-by-slice (default-off / behavior-preserving).
pub fn evaluate_filter_resolved_strict<F>(
    expr: &FilterExpression,
    resolve: &F,
) -> Result<bool, FilterEvalError>
where
    F: Fn(&str) -> Option<serde_json::Value>,
{
    match expr {
        FilterExpression::And(exprs) => {
            // Non-short-circuit: evaluate every child so a MissingField in a later arm surfaces
            // (fail-loud) rather than being hidden by an earlier Ok(false).
            let mut acc = true;
            for e in exprs {
                acc &= evaluate_filter_resolved_strict(e, resolve)?;
            }
            Ok(acc)
        }
        FilterExpression::Or(exprs) => {
            let mut pending_err: Option<FilterEvalError> = None;
            for e in exprs {
                match evaluate_filter_resolved_strict(e, resolve) {
                    Ok(true) => return Ok(true),
                    Ok(false) => {}
                    Err(e) => pending_err = pending_err.or(Some(e)),
                }
            }
            // No arm was true; surface a deferred error if any arm failed (fail-loud), else false.
            match pending_err {
                Some(e) => Err(e),
                None => Ok(false),
            }
        }
        FilterExpression::Not(e) => Ok(!evaluate_filter_resolved_strict(e, resolve)?),
        FilterExpression::Comparison {
            field,
            operator,
            value,
        } => match operator {
            // Null tests are decided by presence (SQL: an absent field IS NULL) — never an error.
            ComparisonOperator::IsNull => {
                Ok(resolve(field).is_none_or(|json_val| json_val.is_null()))
            }
            ComparisonOperator::IsNotNull => {
                Ok(resolve(field).is_some_and(|json_val| !json_val.is_null()))
            }
            _ => match resolve(field) {
                Some(json_val) => Ok(compare_json_op(operator, &json_val, value)),
                None => Err(FilterEvalError::MissingField {
                    field: field.clone(),
                }),
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

/// Evaluate an **authorization** filter against a `ProximaTree` (canonical v2 path).
///
/// The type-strict twin of [`evaluate_filter_proxima`], and the adapter a
/// security predicate should be evaluated with: it resolves on the
/// `ProximaValue`/`ProximaTree` seam (ADR-024) — never the deprecated v1
/// `SqlValue` envelope — and compares via [`compare_json_op_type_strict`], so a
/// numeric policy threshold cannot be satisfied by a string-valued field
/// (TD-FOUNDATION-3 slice FA-a2 / TF-2 S3).
///
/// Note the two independent `strict` axes on this path, which are easy to
/// conflate:
///
/// * [`evaluate_filter_proxima_strict`] is **fail-loud on an unresolved field**
///   (TD-FILT-1) — it surfaces a missing field as an error instead of dropping
///   the row.
/// * this function is **type-strict** — it refuses cross-class comparisons.
///   Absence is handled exactly as the default walker handles it.
pub fn evaluate_filter_proxima_type_strict(expr: &FilterExpression, props: &ProximaTree) -> bool {
    evaluate_filter_resolved_type_strict(expr, &|field| match props.get(field) {
        Some(ProximaTreeNode::Value(pv)) => Some(proxima_value_to_json(pv)),
        _ => None,
    })
}

/// Native, allocation-free ordering for the filter's common scalar types, matching
/// [`crate::json_comparison::compare_json_values`] over the JSON-lowered value. Returns `None`
/// (→ JSON-path fallback) for numbers, `Decimal`, and exotic types: `compare_json_numbers` uses an
/// epsilon for float/huge-int *equality* that a single ordering can't express, so a native numeric
/// ordering would risk diverging from the JSON path (breaking the strict≡default parity this slice
/// guarantees). The real, parity-safe win here is `String`/`Symbol` (avoids the per-field clone).
/// (TD-FILT-1 slice 3.)
fn native_scalar_order(
    pv: &ProximaValue,
    literal: &serde_json::Value,
) -> Option<std::cmp::Ordering> {
    use serde_json::Value as J;
    match (pv, literal) {
        (ProximaValue::Boolean(b), J::Bool(jb)) => Some(b.cmp(jb)),
        (ProximaValue::String(s) | ProximaValue::Symbol(s), J::String(js)) => {
            Some(s.as_str().cmp(js.as_str()))
        }
        _ => None,
    }
}

/// Map an ordering result onto a comparison operator's boolean outcome.
fn apply_op(op: &ComparisonOperator, ord: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    match op {
        ComparisonOperator::Equals => ord == Equal,
        ComparisonOperator::NotEquals => ord != Equal,
        ComparisonOperator::LessThan => ord == Less,
        ComparisonOperator::LessThanOrEqual => matches!(ord, Less | Equal),
        ComparisonOperator::GreaterThan => ord == Greater,
        ComparisonOperator::GreaterThanOrEqual => matches!(ord, Greater | Equal),
        // Unreachable: compare_proxima_op only routes the six ordering/equality ops here.
        _ => false,
    }
}

/// Compare a stored [`ProximaValue`] against a query literal (`serde_json::Value`). Native
/// (allocation-free) for `Boolean` and `String`/`Symbol` — matching the JSON-path semantics
/// exactly — and a JSON-path fallback for everything else. Guarantees strict ≡ default on which
/// rows match (native cases match by construction; fallback cases ARE the JSON path).
/// (TD-FILT-1 slice 3.)
fn compare_proxima_op(
    pv: &ProximaValue,
    op: &ComparisonOperator,
    literal: &serde_json::Value,
) -> bool {
    use ComparisonOperator::*;
    match op {
        Equals | NotEquals | LessThan | LessThanOrEqual | GreaterThan | GreaterThanOrEqual => {
            match native_scalar_order(pv, literal) {
                Some(ord) => apply_op(op, ord),
                None => compare_json_op(op, &proxima_value_to_json(pv), literal),
            }
        }
        // Operators that don't reduce to a single ordering always use the JSON path.
        _ => compare_json_op(op, &proxima_value_to_json(pv), literal),
    }
}

/// Fail-loud variant of [`evaluate_filter_proxima`] (TD-FILT-1) on the canonical `ProximaTree`
/// path. Resolves each field to its raw [`ProximaValue`] (no JSON lowering at resolve time) and
/// compares via [`compare_proxima_op`]. Surfaces an unresolved field (absent or a non-scalar leaf)
/// as [`FilterEvalError::MissingField`] instead of silently dropping the row. Match results are
/// identical to the (JSON-lowering) [`evaluate_filter_proxima`]; only unresolved fields differ
/// (strict errors, default silently drops).
pub fn evaluate_filter_proxima_strict(
    expr: &FilterExpression,
    props: &ProximaTree,
) -> Result<bool, FilterEvalError> {
    fn resolve<'a>(props: &'a ProximaTree, field: &'a str) -> Option<&'a ProximaValue> {
        match props.get(field) {
            Some(ProximaTreeNode::Value(pv)) => Some(pv),
            _ => None,
        }
    }
    match expr {
        FilterExpression::And(exprs) => {
            // Non-short-circuit: evaluate every child so a MissingField in a later arm surfaces.
            let mut acc = true;
            for e in exprs {
                acc &= evaluate_filter_proxima_strict(e, props)?;
            }
            Ok(acc)
        }
        FilterExpression::Or(exprs) => {
            let mut pending: Option<FilterEvalError> = None;
            for e in exprs {
                match evaluate_filter_proxima_strict(e, props) {
                    Ok(true) => return Ok(true),
                    Ok(false) => {}
                    Err(e) => pending = pending.or(Some(e)),
                }
            }
            match pending {
                Some(e) => Err(e),
                None => Ok(false),
            }
        }
        FilterExpression::Not(e) => Ok(!evaluate_filter_proxima_strict(e, props)?),
        FilterExpression::Comparison {
            field,
            operator,
            value,
        } => match operator {
            // Null tests are decided by presence (SQL: an absent field IS NULL) — never an error.
            ComparisonOperator::IsNull => {
                Ok(resolve(props, field).is_none_or(|pv| matches!(pv, ProximaValue::Null)))
            }
            ComparisonOperator::IsNotNull => {
                Ok(resolve(props, field).is_some_and(|pv| !matches!(pv, ProximaValue::Null)))
            }
            _ => match resolve(props, field) {
                Some(pv) => Ok(compare_proxima_op(pv, operator, value)),
                None => Err(FilterEvalError::MissingField {
                    field: field.clone(),
                }),
            },
        },
    }
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
    use crate::json_comparison::compare_json_numbers;
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
    crate::json_comparison::compare_json_values(a, b) == Ordering::Less
}
fn compare_json_lte(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use std::cmp::Ordering;
    matches!(
        crate::json_comparison::compare_json_values(a, b),
        Ordering::Less | Ordering::Equal
    )
}
fn compare_json_gt(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use std::cmp::Ordering;
    crate::json_comparison::compare_json_values(a, b) == Ordering::Greater
}
fn compare_json_gte(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use std::cmp::Ordering;
    matches!(
        crate::json_comparison::compare_json_values(a, b),
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
            (
                "account_id",
                ComparisonOperator::Equals,
                json!("acctA"),
                true,
            ),
            (
                "account_id",
                ComparisonOperator::Equals,
                json!("acctB"),
                false,
            ),
            // int prop vs int literal AND float-typed literal (numeric-aware).
            ("tier", ComparisonOperator::Equals, json!(2), true),
            ("tier", ComparisonOperator::Equals, json!(2.0), true),
            ("tier", ComparisonOperator::GreaterThan, json!(1), true),
            ("tier", ComparisonOperator::LessThan, json!(2), false),
            (
                "score",
                ComparisonOperator::LessThanOrEqual,
                json!(0.5),
                true,
            ),
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
            assert_eq!(
                evaluate_filter_proxima(&filter, &props),
                expected,
                "{label}"
            );
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

    // ---- TD-FILT-1: evaluate_filter_proxima_strict (fail-loud, v2 ProximaValue path) ----

    fn proxima_props_one(field: &str, pv: ProximaValue) -> ProximaTree {
        let mut props = ProximaTree::new();
        props.insert(field.to_string(), ProximaTreeNode::Value(pv));
        props
    }

    #[test]
    fn strict_clean_match_agrees_with_legacy() {
        let props = proxima_props_one("tier", ProximaValue::Int64(2));
        let filter = FilterExpression::Comparison {
            field: "tier".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!(2),
        };
        assert!(evaluate_filter_proxima(&filter, &props));
        assert!(evaluate_filter_proxima_strict(&filter, &props).unwrap());
    }

    #[test]
    fn strict_missing_field_surfaces_error_legacy_silently_drops() {
        // No "tier" field in the (empty) props.
        let props = ProximaTree::new();
        let filter = FilterExpression::Comparison {
            field: "tier".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!(2),
        };
        // The bug (TD-FILT-1): the legacy path silently drops the row (`None => false`).
        assert!(
            !evaluate_filter_proxima(&filter, &props),
            "legacy evaluate_filter_proxima silently drops the unresolved-field row"
        );
        // The fix: strict surfaces it as a typed error.
        assert_eq!(
            evaluate_filter_proxima_strict(&filter, &props),
            Err(FilterEvalError::MissingField {
                field: "tier".to_string()
            })
        );
    }

    #[test]
    fn strict_isnull_on_missing_is_ok_not_error() {
        // Absent field IS NULL (SQL semantics) — not a MissingField error.
        let props = ProximaTree::new();
        let filter = FilterExpression::Comparison {
            field: "tier".to_string(),
            operator: ComparisonOperator::IsNull,
            value: json!(null),
        };
        assert!(evaluate_filter_proxima_strict(&filter, &props).unwrap());
    }

    #[test]
    fn strict_and_surfaces_missing_field_even_when_an_earlier_arm_is_false() {
        // arm1 (a==2) is false (a is 1); arm2 references a missing field. Non-short-circuit strict
        // must surface the MissingField rather than hide it behind arm1's Ok(false).
        let mut props = ProximaTree::new();
        props.insert(
            "a".to_string(),
            ProximaTreeNode::Value(ProximaValue::Int64(1)),
        );
        let filter = FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "a".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!(2),
            },
            FilterExpression::Comparison {
                field: "missing".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!(1),
            },
        ]);
        assert!(matches!(
            evaluate_filter_proxima_strict(&filter, &props),
            Err(FilterEvalError::MissingField { .. })
        ));
    }

    // ---- TD-FILT-1 slice 3: native comparator parity (strict ≡ default on matches) ----

    #[test]
    fn native_compare_matches_json_path_for_supported_scalars() {
        // For every (pv, literal, op) the native path handles (Boolean↔Bool, String/Symbol↔String),
        // compare_proxima_op MUST equal the JSON path — so strict and default agree on matches.
        let pvs = [
            ProximaValue::Boolean(true),
            ProximaValue::Boolean(false),
            ProximaValue::String("apple".to_string()),
            ProximaValue::String("banana".to_string()),
            ProximaValue::Symbol("sym".to_string()),
            ProximaValue::String("".to_string()),
        ];
        let lits = [
            json!(true),
            json!(false),
            json!("apple"),
            json!("mango"),
            json!("banana"),
            json!("sym"),
            json!(""),
        ];
        let ops = [
            ComparisonOperator::Equals,
            ComparisonOperator::NotEquals,
            ComparisonOperator::LessThan,
            ComparisonOperator::LessThanOrEqual,
            ComparisonOperator::GreaterThan,
            ComparisonOperator::GreaterThanOrEqual,
        ];
        for pv in &pvs {
            for lit in &lits {
                if native_scalar_order(pv, lit).is_some() {
                    for op in &ops {
                        let native = compare_proxima_op(pv, op, lit);
                        let json_path = compare_json_op(op, &proxima_value_to_json(pv), lit);
                        assert_eq!(
                            native, json_path,
                            "parity break: pv={pv:?} lit={lit} op={op:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn numbers_and_exotic_types_fall_back_to_json_path() {
        // Numbers / Decimal / exotic / type-mismatched pairs are not handled natively (the
        // epsilon-equality semantics make a native numeric ordering unsafe for parity), so
        // compare_proxima_op IS the JSON path for them.
        let cases = [
            (ProximaValue::Int64(5), json!(5)),
            (ProximaValue::Int64(2), json!(2.0)),
            (ProximaValue::Float64(2.5), json!(2.5)),
            (ProximaValue::Boolean(true), json!(5)),
        ];
        for (pv, lit) in cases {
            assert!(
                native_scalar_order(&pv, &lit).is_none(),
                "expected JSON-path fallback: {pv:?} vs {lit}"
            );
            let op = ComparisonOperator::Equals;
            assert_eq!(
                compare_proxima_op(&pv, &op, &lit),
                compare_json_op(&op, &proxima_value_to_json(&pv), &lit),
                "fallback parity: {pv:?} vs {lit}"
            );
        }
    }
}

// ===========================================================================
// Type-strict operator evaluation (TD-FOUNDATION-3 slice FA-a2 / TF-2 S3)
// ===========================================================================

/// [`compare_json_op`], but **deny-biased on a type mismatch**.
///
/// The permissive dispatch answers every comparison, falling back to JSON type
/// precedence when the operands are of different classes. For a *user* filter
/// that is a defensible total order. For an *authorization* predicate it is a
/// fail-open channel, in two directions:
///
/// * **Ordered operators widen.** `String` outranks `Number` by precedence, so a
///   `clearance >= 3` policy returns `true` for every record whose `clearance`
///   holds a string — regardless of the string's content. A clearance gate
///   admits the corpus it exists to withhold.
/// * **Negative operators widen.** `dept != 5` is `true` for a string-valued
///   `dept`, because "not equal" is trivially satisfied by "not comparable".
///
/// The rule here is one line: **if the operands are not comparable, the
/// predicate does not admit.** Operators that are already type-exact
/// (`Equals`/`In` via `json_eq`, the string operators via `as_str`, the null
/// tests via presence) delegate unchanged — this only tightens the cases that
/// could widen.
///
/// Additive: [`compare_json_op`] is untouched, so the live metadata-search,
/// document-filter and pushdown paths keep their current semantics. The security
/// path opts in.
pub fn compare_json_op_type_strict(
    operator: &ComparisonOperator,
    json_val: &serde_json::Value,
    value: &serde_json::Value,
) -> bool {
    use crate::json_comparison::comparable_class;

    let same_class =
        |other: &serde_json::Value| comparable_class(json_val) == comparable_class(other);

    match operator {
        // Ordered comparisons are meaningless across classes — deny rather than
        // fall through to precedence ordering.
        ComparisonOperator::LessThan
        | ComparisonOperator::LessThanOrEqual
        | ComparisonOperator::GreaterThan
        | ComparisonOperator::GreaterThanOrEqual => {
            same_class(value) && compare_json_op(operator, json_val, value)
        }

        // Both bounds must be comparable with the field, or the range is
        // meaningless.
        ComparisonOperator::Between => value.as_array().is_some_and(|bounds| {
            bounds.len() == 2
                && same_class(&bounds[0])
                && same_class(&bounds[1])
                && compare_json_op(operator, json_val, value)
        }),

        // "Not equal" must not be satisfied merely by being incomparable.
        ComparisonOperator::NotEquals => {
            same_class(value) && compare_json_op(operator, json_val, value)
        }

        // Likewise "not in": require at least one list element the field could
        // actually have equalled, else the exclusion is vacuous.
        ComparisonOperator::NotIn => value.as_array().is_some_and(|values| {
            values.iter().any(same_class) && compare_json_op(operator, json_val, value)
        }),

        // Already type-exact: Equals/In compare via `json_eq`, the string
        // operators require `as_str` on both sides, and the null tests are
        // decided by presence.
        _ => compare_json_op(operator, json_val, value),
    }
}

#[cfg(test)]
mod type_strict_tests {
    use super::*;
    use serde_json::json;

    fn permissive(
        op: ComparisonOperator,
        field: serde_json::Value,
        lit: serde_json::Value,
    ) -> bool {
        compare_json_op(&op, &field, &lit)
    }
    fn strict(op: ComparisonOperator, field: serde_json::Value, lit: serde_json::Value) -> bool {
        compare_json_op_type_strict(&op, &field, &lit)
    }

    #[test]
    fn a_numeric_threshold_no_longer_admits_a_string_clearance() {
        // TF-2 §1.4's case, end to end: `clearance >= 3` against a record whose
        // clearance is a string.
        assert!(
            permissive(
                ComparisonOperator::GreaterThanOrEqual,
                json!("TOP_SECRET"),
                json!(3)
            ),
            "characterizing the defect: the permissive dispatch admits"
        );
        assert!(!strict(
            ComparisonOperator::GreaterThanOrEqual,
            json!("TOP_SECRET"),
            json!(3)
        ));
        // …including the string that merely looks like a smaller number.
        assert!(!strict(
            ComparisonOperator::GreaterThanOrEqual,
            json!("2"),
            json!(3)
        ));
    }

    #[test]
    fn ordered_operators_still_work_within_a_class() {
        assert!(strict(ComparisonOperator::GreaterThan, json!(5), json!(3)));
        assert!(!strict(ComparisonOperator::GreaterThan, json!(2), json!(3)));
        assert!(strict(ComparisonOperator::LessThan, json!("a"), json!("b")));
        // Integer field against a float literal must keep working.
        assert!(strict(
            ComparisonOperator::GreaterThan,
            json!(1),
            json!(0.8)
        ));
    }

    #[test]
    fn not_equals_is_not_satisfied_by_being_incomparable() {
        assert!(
            permissive(ComparisonOperator::NotEquals, json!("eng"), json!(5)),
            "characterizing the defect: incomparable satisfies !="
        );
        assert!(!strict(
            ComparisonOperator::NotEquals,
            json!("eng"),
            json!(5)
        ));
        // Same-class inequality is unaffected.
        assert!(strict(
            ComparisonOperator::NotEquals,
            json!("eng"),
            json!("hr")
        ));
        assert!(!strict(
            ComparisonOperator::NotEquals,
            json!("eng"),
            json!("eng")
        ));
    }

    #[test]
    fn not_in_requires_a_comparable_list_element() {
        assert!(
            permissive(ComparisonOperator::NotIn, json!("eng"), json!([1, 2])),
            "characterizing the defect: a wholly incomparable list excludes nothing"
        );
        assert!(!strict(
            ComparisonOperator::NotIn,
            json!("eng"),
            json!([1, 2])
        ));
        // A same-class list behaves normally.
        assert!(strict(
            ComparisonOperator::NotIn,
            json!("eng"),
            json!(["hr", "legal"])
        ));
        assert!(!strict(
            ComparisonOperator::NotIn,
            json!("eng"),
            json!(["eng", "hr"])
        ));
    }

    #[test]
    fn between_requires_both_bounds_to_be_comparable() {
        assert!(strict(
            ComparisonOperator::Between,
            json!(5),
            json!([1, 10])
        ));
        assert!(!strict(
            ComparisonOperator::Between,
            json!(5),
            json!([1, "10"])
        ));
        assert!(!strict(
            ComparisonOperator::Between,
            json!("5"),
            json!([1, 10])
        ));
    }

    #[test]
    fn already_exact_operators_are_unchanged() {
        // Equals, In, the string operators and the null tests are type-exact
        // already; strict mode must not alter them.
        for (op, field, lit) in [
            (ComparisonOperator::Equals, json!("eng"), json!("eng")),
            (ComparisonOperator::Equals, json!("eng"), json!(5)),
            (ComparisonOperator::In, json!("eng"), json!(["eng", "hr"])),
            (ComparisonOperator::In, json!("eng"), json!([1, 2])),
            (
                ComparisonOperator::StartsWith,
                json!("engineering"),
                json!("eng"),
            ),
            (ComparisonOperator::StartsWith, json!(5), json!("eng")),
            (
                ComparisonOperator::Contains,
                json!("engineering"),
                json!("gin"),
            ),
            (
                ComparisonOperator::Like,
                json!("engineering"),
                json!("eng%"),
            ),
            (ComparisonOperator::IsNull, json!(null), json!(null)),
            (ComparisonOperator::IsNotNull, json!("eng"), json!(null)),
        ] {
            assert_eq!(
                compare_json_op_type_strict(&op, &field, &lit),
                compare_json_op(&op, &field, &lit),
                "strict mode changed an already-exact operator: {op:?} {field} {lit}"
            );
        }
    }

    #[test]
    fn strict_never_admits_more_than_permissive() {
        // The deny-biased property, over the operator × value grid: strict mode
        // may only ever tighten.
        let values = [
            json!(3),
            json!(3.5),
            json!("3"),
            json!("eng"),
            json!(true),
            json!(null),
            json!([1, 2]),
        ];
        let ops = [
            ComparisonOperator::Equals,
            ComparisonOperator::NotEquals,
            ComparisonOperator::LessThan,
            ComparisonOperator::LessThanOrEqual,
            ComparisonOperator::GreaterThan,
            ComparisonOperator::GreaterThanOrEqual,
            ComparisonOperator::In,
            ComparisonOperator::NotIn,
            ComparisonOperator::Between,
            ComparisonOperator::Contains,
            ComparisonOperator::StartsWith,
            ComparisonOperator::EndsWith,
            ComparisonOperator::Like,
        ];
        for op in &ops {
            for field in &values {
                for lit in &values {
                    if compare_json_op_type_strict(op, field, lit) {
                        assert!(
                            compare_json_op(op, field, lit),
                            "strict admitted what permissive denies: {op:?} {field} {lit}"
                        );
                    }
                }
            }
        }
    }
}

/// [`evaluate_filter_resolved`], but evaluating every comparison through
/// [`compare_json_op_type_strict`] — the walker an **authorization** expression
/// is evaluated with (TD-FOUNDATION-3 slice FA-a2 / TF-2 S3).
///
/// # Why a separate walker rather than a strict mode on the shared one
///
/// A security predicate and a user's own filter end up ANDed together, but they
/// do not want the same semantics: the user's `clearance >= 3` may legitimately
/// use the permissive total order, while the policy's must not, or a
/// string-valued `clearance` walks through the gate. A single tree evaluated by a
/// single walker cannot give two subtrees two semantics without a marker node in
/// `FilterExpression` — a foundation type used everywhere.
///
/// `AND` associativity makes that unnecessary. Evaluating the two expressions
/// **separately** and requiring both is exactly equivalent to evaluating their
/// conjunction, so the security expression can be walked strictly here while the
/// user's is walked by [`evaluate_filter_resolved`] as before. No shared type
/// changes, and no live query changes meaning.
///
/// Absence handling is identical to the permissive walker (an absent field IS
/// NULL for the null tests, and fails every other comparison), because that axis
/// is already deny-biased. This adds only the type axis.
pub fn evaluate_filter_resolved_type_strict<F>(expr: &FilterExpression, resolve: &F) -> bool
where
    F: Fn(&str) -> Option<serde_json::Value>,
{
    match expr {
        FilterExpression::And(exprs) => exprs
            .iter()
            .all(|e| evaluate_filter_resolved_type_strict(e, resolve)),
        FilterExpression::Or(exprs) => exprs
            .iter()
            .any(|e| evaluate_filter_resolved_type_strict(e, resolve)),
        FilterExpression::Not(e) => !evaluate_filter_resolved_type_strict(e, resolve),
        FilterExpression::Comparison {
            field,
            operator,
            value,
        } => match operator {
            // Decided by presence, exactly as in the permissive walker.
            ComparisonOperator::IsNull => resolve(field).is_none_or(|json_val| json_val.is_null()),
            ComparisonOperator::IsNotNull => {
                resolve(field).is_some_and(|json_val| !json_val.is_null())
            }
            _ => match resolve(field) {
                Some(json_val) => compare_json_op_type_strict(operator, &json_val, value),
                None => false,
            },
        },
    }
}

#[cfg(test)]
mod type_strict_walker_tests {
    use super::*;
    use serde_json::json;

    fn cmp(
        field: &str,
        operator: ComparisonOperator,
        value: serde_json::Value,
    ) -> FilterExpression {
        FilterExpression::Comparison {
            field: field.to_string(),
            operator,
            value,
        }
    }

    fn admits(expr: &FilterExpression, row: &serde_json::Value) -> bool {
        evaluate_filter_resolved_type_strict(expr, &|f| row.get(f).cloned())
    }
    fn admits_permissive(expr: &FilterExpression, row: &serde_json::Value) -> bool {
        evaluate_filter_resolved(expr, &|f| row.get(f).cloned())
    }

    #[test]
    fn a_clearance_gate_no_longer_admits_a_string_valued_record() {
        // The end-to-end shape of TF-2 §1.4's ClassificationBased case.
        let policy = cmp(
            "clearance",
            ComparisonOperator::GreaterThanOrEqual,
            json!(3),
        );
        let row = json!({ "clearance": "TOP_SECRET" });

        assert!(
            admits_permissive(&policy, &row),
            "characterizing the defect: the permissive walker admits"
        );
        assert!(!admits(&policy, &row));
        // A genuinely-cleared record still passes.
        assert!(admits(&policy, &json!({ "clearance": 5 })));
        assert!(!admits(&policy, &json!({ "clearance": 1 })));
    }

    #[test]
    fn absence_is_handled_exactly_as_in_the_permissive_walker() {
        let row = json!({ "other": 1 });
        for expr in [
            cmp("dept", ComparisonOperator::Equals, json!("eng")),
            cmp("dept", ComparisonOperator::GreaterThan, json!(3)),
            cmp("dept", ComparisonOperator::IsNull, json!(null)),
            cmp("dept", ComparisonOperator::IsNotNull, json!(null)),
        ] {
            assert_eq!(
                admits(&expr, &row),
                admits_permissive(&expr, &row),
                "absence handling diverged for {expr:?}"
            );
        }
    }

    #[test]
    fn the_null_guard_from_fa_a_composes_with_type_strictness() {
        // FA-a lowers `Not(P)` as `And([Not(P), IsNotNull(f)])`. Together with
        // strict comparison that closes both the absence axis and the type axis.
        let guarded = FilterExpression::And(vec![
            FilterExpression::Not(Box::new(cmp(
                "clearance",
                ComparisonOperator::Equals,
                json!(3),
            ))),
            cmp("clearance", ComparisonOperator::IsNotNull, json!(null)),
        ]);

        assert!(!admits(&guarded, &json!({})), "absent field excluded");
        assert!(
            !admits(&guarded, &json!({ "clearance": null })),
            "null field excluded"
        );
        assert!(admits(&guarded, &json!({ "clearance": 5 })));
        assert!(!admits(&guarded, &json!({ "clearance": 3 })));
    }

    #[test]
    fn and_associativity_makes_separate_evaluation_equivalent_to_conjunction() {
        // The property the two-walker design rests on: evaluating a conjunction
        // is the same as evaluating each conjunct and requiring both. Verified
        // over a grid so the design note is not just prose.
        let security = cmp("dept", ComparisonOperator::Equals, json!("eng"));
        let user = cmp("score", ComparisonOperator::GreaterThan, json!(0.5));
        let conjunction = FilterExpression::And(vec![security.clone(), user.clone()]);

        for row in [
            json!({ "dept": "eng", "score": 0.9 }),
            json!({ "dept": "eng", "score": 0.1 }),
            json!({ "dept": "hr", "score": 0.9 }),
            json!({}),
            json!({ "dept": "eng" }),
            json!({ "score": 0.9 }),
        ] {
            assert_eq!(
                admits(&conjunction, &row),
                admits(&security, &row) && admits(&user, &row),
                "separate evaluation diverged from the conjunction for {row}"
            );
        }
    }

    #[test]
    fn the_strict_walker_never_admits_more_than_the_permissive_one() {
        // Deny-biased over a grid of expression shapes × rows.
        let exprs = vec![
            cmp("a", ComparisonOperator::GreaterThanOrEqual, json!(3)),
            cmp("a", ComparisonOperator::NotEquals, json!(3)),
            cmp("a", ComparisonOperator::NotIn, json!([1, 2])),
            cmp("a", ComparisonOperator::Between, json!([1, 10])),
            FilterExpression::Not(Box::new(cmp("a", ComparisonOperator::Equals, json!(3)))),
            FilterExpression::And(vec![
                cmp("a", ComparisonOperator::GreaterThan, json!(1)),
                cmp("b", ComparisonOperator::Equals, json!("x")),
            ]),
            FilterExpression::Or(vec![
                cmp("a", ComparisonOperator::LessThan, json!(1)),
                cmp("b", ComparisonOperator::NotEquals, json!("x")),
            ]),
        ];
        let rows = vec![
            json!({}),
            json!({ "a": 5 }),
            json!({ "a": "5" }),
            json!({ "a": null }),
            json!({ "a": true }),
            json!({ "a": 5, "b": "x" }),
            json!({ "a": "5", "b": 7 }),
        ];
        for e in &exprs {
            for row in &rows {
                if admits(e, row) {
                    assert!(
                        admits_permissive(e, row),
                        "strict walker admitted what permissive denies: {e:?} {row}"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod type_strict_proxima_tests {
    use super::*;
    use proximadb_data_model::ProximaValue;
    use serde_json::json;

    fn tree(pairs: Vec<(&str, ProximaValue)>) -> ProximaTree {
        let mut t = ProximaTree::new();
        for (k, v) in pairs {
            t.insert(k.to_string(), ProximaTreeNode::Value(v));
        }
        t
    }

    #[test]
    fn the_canonical_seam_denies_a_string_clearance_against_a_numeric_gate() {
        let policy = FilterExpression::Comparison {
            field: "clearance".to_string(),
            operator: ComparisonOperator::GreaterThanOrEqual,
            value: json!(3),
        };

        // ProximaValue path — no SqlValue envelope anywhere.
        let string_clearance = tree(vec![(
            "clearance",
            ProximaValue::String("TOP_SECRET".to_string()),
        )]);
        assert!(
            evaluate_filter_proxima(&policy, &string_clearance),
            "characterizing the defect on the canonical seam"
        );
        assert!(!evaluate_filter_proxima_type_strict(
            &policy,
            &string_clearance
        ));

        // A genuinely-cleared record still passes.
        assert!(evaluate_filter_proxima_type_strict(
            &policy,
            &tree(vec![("clearance", ProximaValue::Int64(5))])
        ));
        assert!(!evaluate_filter_proxima_type_strict(
            &policy,
            &tree(vec![("clearance", ProximaValue::Int64(1))])
        ));
    }

    #[test]
    fn the_canonical_seam_matches_the_default_walker_within_a_class() {
        let policy = FilterExpression::Comparison {
            field: "dept".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("eng"),
        };
        for props in [
            tree(vec![("dept", ProximaValue::String("eng".to_string()))]),
            tree(vec![("dept", ProximaValue::String("hr".to_string()))]),
            tree(vec![]),
        ] {
            assert_eq!(
                evaluate_filter_proxima_type_strict(&policy, &props),
                evaluate_filter_proxima(&policy, &props),
                "type strictness changed a same-class comparison"
            );
        }
    }
}
