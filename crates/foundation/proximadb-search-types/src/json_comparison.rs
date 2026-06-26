//! JSON Value Comparison Utilities
//!
//! Centralized JSON value comparison logic that handles numeric type coercion
//! correctly across integer and floating-point values.

use serde_json::{Number, Value};
use std::cmp::Ordering;

/// Compare two JSON numbers with type-aware comparison
///
/// This handles:
/// - Integer vs integer comparison (preserves precision)
/// - Float vs float comparison (with epsilon tolerance)
/// - Integer vs float comparison (converts to float)
/// - Special cases: NaN, Infinity
///
/// # Examples
/// ```rust,ignore
/// use serde_json::Number;
/// assert!(compare_json_numbers(&Number::from(2), &Number::from(2.0))); // true
/// assert!(compare_json_numbers(&Number::from(42), &Number::from(42))); // true
/// ```
pub fn compare_json_numbers(n1: &Number, n2: &Number) -> bool {
    // Try integer comparison first (preserves precision)
    if let (Some(i1), Some(i2)) = (n1.as_i64(), n2.as_i64()) {
        return i1 == i2;
    }

    // Try unsigned integer comparison for large positive numbers
    if let (Some(u1), Some(u2)) = (n1.as_u64(), n2.as_u64()) {
        return u1 == u2;
    }

    // Fall back to float comparison with epsilon for precision
    match (n1.as_f64(), n2.as_f64()) {
        (Some(f1), Some(f2)) => {
            // Handle special cases
            if f1.is_nan() && f2.is_nan() {
                return true; // NaN == NaN for metadata filtering
            }
            if f1.is_infinite() && f2.is_infinite() {
                return f1.signum() == f2.signum(); // +inf == +inf, -inf == -inf
            }
            // Use relative epsilon comparison for floats
            let epsilon = f64::EPSILON * f1.abs().max(f2.abs()).max(1.0);
            (f1 - f2).abs() < epsilon
        }
        _ => false,
    }
}

/// Compare JSON values for ordering (supports all JSON types)
///
/// Type precedence: Null < Bool < Number < String < Array < Object
pub fn compare_json_values(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Number(n1), Value::Number(n2)) => {
            // Try integer comparison first for precision
            if let (Some(i1), Some(i2)) = (n1.as_i64(), n2.as_i64()) {
                return i1.cmp(&i2);
            }

            // Try unsigned comparison for large numbers
            if let (Some(u1), Some(u2)) = (n1.as_u64(), n2.as_u64()) {
                return u1.cmp(&u2);
            }

            // Fall back to float comparison
            let f1 = n1.as_f64();
            let f2 = n2.as_f64();
            f1.partial_cmp(&f2).unwrap_or(Ordering::Equal)
        }
        (Value::String(s1), Value::String(s2)) => s1.cmp(s2),
        (Value::Bool(b1), Value::Bool(b2)) => b1.cmp(b2),
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Array(a1), Value::Array(a2)) => {
            // Lexicographic comparison of arrays
            for (v1, v2) in a1.iter().zip(a2.iter()) {
                match compare_json_values(v1, v2) {
                    Ordering::Equal => continue,
                    other => return other,
                }
            }
            a1.len().cmp(&a2.len())
        }
        // Type ordering: Null < Bool < Number < String < Array < Object
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        (Value::Bool(_), Value::Number(_)) => Ordering::Less,
        (Value::Bool(_), Value::String(_)) => Ordering::Less,
        (Value::Bool(_), Value::Array(_)) => Ordering::Less,
        (Value::Bool(_), Value::Object(_)) => Ordering::Less,
        (Value::Number(_), Value::Bool(_)) => Ordering::Greater,
        (Value::Number(_), Value::String(_)) => Ordering::Less,
        (Value::Number(_), Value::Array(_)) => Ordering::Less,
        (Value::Number(_), Value::Object(_)) => Ordering::Less,
        (Value::String(_), Value::Bool(_)) => Ordering::Greater,
        (Value::String(_), Value::Number(_)) => Ordering::Greater,
        (Value::String(_), Value::Array(_)) => Ordering::Less,
        (Value::String(_), Value::Object(_)) => Ordering::Less,
        (Value::Array(_), Value::Bool(_)) => Ordering::Greater,
        (Value::Array(_), Value::Number(_)) => Ordering::Greater,
        (Value::Array(_), Value::String(_)) => Ordering::Greater,
        (Value::Array(_), Value::Object(_)) => Ordering::Less,
        (Value::Object(_), _) => Ordering::Greater,
    }
}

/// Simple LIKE pattern matching for SQL-style patterns
/// Supports % (any chars) and _ (single char) wildcards
pub fn like_pattern_match(text: &str, pattern: &str) -> bool {
    let mut text_chars = text.chars().peekable();
    let mut pattern_chars = pattern.chars().peekable();

    while let Some(&pattern_char) = pattern_chars.peek() {
        match pattern_char {
            '%' => {
                pattern_chars.next(); // consume '%'

                // If pattern ends with '%', match the rest
                if pattern_chars.peek().is_none() {
                    return true;
                }

                // Try to match remaining pattern at each position in text
                let remaining_pattern: String = pattern_chars.collect();
                while text_chars.peek().is_some() {
                    let remaining_text: String = text_chars.clone().collect();
                    if like_pattern_match(&remaining_text, &remaining_pattern) {
                        return true;
                    }
                    text_chars.next();
                }
                return false;
            }
            '_' => {
                pattern_chars.next(); // consume '_'
                if text_chars.next().is_none() {
                    return false; // '_' must match exactly one character
                }
            }
            c => {
                pattern_chars.next(); // consume pattern char
                if text_chars.next() != Some(c) {
                    return false;
                }
            }
        }
    }

    // Pattern consumed, text should also be consumed
    text_chars.peek().is_none()
}

/// Evaluate a filter expression against metadata
///
/// This is the centralized filter evaluation logic used by all storage engines
pub fn evaluate_filter(
    expr: &proximadb_filter_expression::FilterExpression,
    metadata: &std::collections::HashMap<String, Value>,
) -> bool {
    // Thin adapter over the canonical operator-semantics seam
    // (`sql_value_filter::evaluate_filter_resolved` / `compare_json_op`): the
    // field resolver is a plain json-map lookup, and ALL operator logic —
    // including SQL null-on-absence, full ordering, rich array In/Contains,
    // and full LIKE — lives in the seam. This guarantees json-map callers
    // (search pipeline, ANN index, WAL) share identical semantics with the
    // canonical ProximaTree path. The primitives below
    // (`compare_json_numbers`/`compare_json_values`/`like_pattern_match`)
    // remain the shared comparison source that the seam calls into.
    crate::sql_value_filter::evaluate_filter_resolved(expr, &|field| metadata.get(field).cloned())
}
