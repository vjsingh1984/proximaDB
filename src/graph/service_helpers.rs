//! Graph service helpers (comparison and value extraction utilities)
//!
//! This module centralizes small helper functions used across node/edge
//! query, filtering and index utilities to keep service modules lean.

use crate::proto::proximadb_v1::property_value::Value;

// Numeric/string extractors from proto PropertyValue
pub(super) fn extract_number_from_value(
    value: &crate::proto::proximadb_v1::PropertyValue,
) -> Option<f64> {
    match &value.value {
        Some(Value::IntValue(i)) => Some(*i as f64),
        Some(Value::DoubleValue(d)) => Some(*d),
        Some(Value::StringValue(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

pub(super) fn extract_string_from_value(
    value: &crate::proto::proximadb_v1::PropertyValue,
) -> Option<&str> {
    match &value.value {
        Some(Value::StringValue(s)) => Some(s.as_str()),
        _ => None,
    }
}

// Convert PropertyValue to a normalized string key for index maps
pub(super) fn index_key_for_value(value: &crate::graph::PropertyValue) -> String {
    match &value.value {
        Some(Value::StringValue(s)) => s.clone(),
        Some(Value::IntValue(i)) => i.to_string(),
        Some(Value::DoubleValue(d)) => d.to_string(),
        Some(Value::BoolValue(b)) => b.to_string(),
        Some(Value::BytesValue(b)) => format!("bytes:{}", b.len()),
        Some(Value::ArrayValue(_)) => "array".to_string(),
        Some(Value::ObjectValue(_)) => "object".to_string(),
        Some(Value::VectorValue(_)) => "vector".to_string(),
        None => "null".to_string(),
    }
}

// Simple f64 parser for ordered-key comparisons
pub(super) fn parse_f64_key(s: &str) -> Option<f64> {
    s.parse::<f64>().ok()
}

// Internal helpers to compare keys (string-ordered or numeric)
fn cmp_key_gt(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target {
        key.parse::<f64>().map(|v| v > *t).unwrap_or(false)
    } else if let Some(s) = str_target {
        key > s
    } else {
        false
    }
}
fn cmp_key_ge(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target {
        key.parse::<f64>().map(|v| v >= *t).unwrap_or(false)
    } else if let Some(s) = str_target {
        key >= s
    } else {
        false
    }
}
fn cmp_key_lt(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target {
        key.parse::<f64>().map(|v| v < *t).unwrap_or(false)
    } else if let Some(s) = str_target {
        key < s
    } else {
        false
    }
}
fn cmp_key_le(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target {
        key.parse::<f64>().map(|v| v <= *t).unwrap_or(false)
    } else if let Some(s) = str_target {
        key <= s
    } else {
        false
    }
}

// Property-value comparisons used by filters
pub(super) fn cmp_prop_gt(
    lhs: Option<&crate::graph::PropertyValue>,
    rhs: &crate::proto::proximadb_v1::PropertyValue,
) -> bool {
    match lhs {
        Some(v) => extract_number_from_value(v)
            .zip(extract_number_from_value(rhs))
            .map(|(a, b)| a > b)
            .unwrap_or(false),
        None => false,
    }
}
pub(super) fn cmp_prop_ge(
    lhs: Option<&crate::graph::PropertyValue>,
    rhs: &crate::proto::proximadb_v1::PropertyValue,
) -> bool {
    match lhs {
        Some(v) => extract_number_from_value(v)
            .zip(extract_number_from_value(rhs))
            .map(|(a, b)| a >= b)
            .unwrap_or(false),
        None => false,
    }
}
pub(super) fn cmp_prop_lt(
    lhs: Option<&crate::graph::PropertyValue>,
    rhs: &crate::proto::proximadb_v1::PropertyValue,
) -> bool {
    match lhs {
        Some(v) => extract_number_from_value(v)
            .zip(extract_number_from_value(rhs))
            .map(|(a, b)| a < b)
            .unwrap_or(false),
        None => false,
    }
}
pub(super) fn cmp_prop_le(
    lhs: Option<&crate::graph::PropertyValue>,
    rhs: &crate::proto::proximadb_v1::PropertyValue,
) -> bool {
    match lhs {
        Some(v) => extract_number_from_value(v)
            .zip(extract_number_from_value(rhs))
            .map(|(a, b)| a <= b)
            .unwrap_or(false),
        None => false,
    }
}

pub(super) fn prop_starts_with(
    lhs: Option<&crate::graph::PropertyValue>,
    rhs: &crate::proto::proximadb_v1::PropertyValue,
) -> bool {
    match (lhs, extract_string_from_value(rhs)) {
        (Some(v), Some(prefix)) => match &v.value {
            Some(Value::StringValue(s)) => s.starts_with(prefix),
            _ => false,
        },
        _ => false,
    }
}

pub(super) fn prop_contains(
    lhs: Option<&crate::graph::PropertyValue>,
    rhs: &crate::proto::proximadb_v1::PropertyValue,
) -> bool {
    match (lhs, extract_string_from_value(rhs)) {
        (Some(v), Some(substr)) => match &v.value {
            Some(Value::StringValue(s)) => s.contains(substr),
            _ => false,
        },
        _ => false,
    }
}
