//! Query operators implementing the PhysicalOperator trait
//!
//! This module provides core query execution operators following the Volcano iterator model:
//! - `NodeScanOperator`: Scans nodes by label and/or property filters
//! - `ExpandOperator`: Expands edges from source nodes (traversal)
//! - `FilterOperator`: Filters tuples based on predicates
//! - `ProjectOperator`: Projects specific columns from tuples
//! - `LimitOperator`: Limits result set size
//!
//! # Design Principles
//!
//! - **Reuse**: Operators reuse GraphEngine trait, no duplication
//! - **Composability**: Operators can be chained (Scan → Expand → Filter → Project)
//! - **Streaming**: Results produced incrementally, not materialized upfront
//! - **Testability**: Each operator tested independently with mock engines

use crate::core::error::ProximaDBError;
use crate::graph::engines::GraphEngine;
use crate::graph::query::execution_traits::{
    ColumnSpec, PhysicalOperator, QueryValue, ResultTuple, ValueType,
};
use crate::proto::proximadb_v1::{Edge, Node, PropertyFilter, PropertyFilterOperator};
use anyhow::Result;
use std::sync::Arc;

pub mod scan;
pub mod expand;
pub mod filter;
pub mod project;
pub mod limit;

pub use scan::NodeScanOperator;
pub use expand::ExpandOperator;
pub use filter::FilterOperator;
pub use project::ProjectOperator;
pub use limit::LimitOperator;

/// Edge direction for expansion
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDirection {
    /// Outgoing edges (source → target)
    Outgoing,
    /// Incoming edges (source ← target)
    Incoming,
    /// Bidirectional (both directions)
    Bidirectional,
}

/// Helper function to evaluate property filter against a value
///
/// Reuses existing PropertyFilter proto type, no duplication of filter logic.
pub fn evaluate_property_filter(
    filter: &PropertyFilter,
    actual_value: &crate::proto::proximadb_v1::PropertyValue,
) -> bool {
    let expected_value = match &filter.value {
        Some(v) => v,
        None => return false,
    };

    let operator = PropertyFilterOperator::try_from(filter.operator).unwrap_or(PropertyFilterOperator::Equals);

    match operator {
        PropertyFilterOperator::Equals => compare_property_values(actual_value, expected_value) == std::cmp::Ordering::Equal,
        PropertyFilterOperator::NotEquals => compare_property_values(actual_value, expected_value) != std::cmp::Ordering::Equal,
        PropertyFilterOperator::GreaterThan => compare_property_values(actual_value, expected_value) == std::cmp::Ordering::Greater,
        PropertyFilterOperator::LessThan => compare_property_values(actual_value, expected_value) == std::cmp::Ordering::Less,
        PropertyFilterOperator::GreaterEqual => {
            let cmp = compare_property_values(actual_value, expected_value);
            cmp == std::cmp::Ordering::Greater || cmp == std::cmp::Ordering::Equal
        }
        PropertyFilterOperator::LessEqual => {
            let cmp = compare_property_values(actual_value, expected_value);
            cmp == std::cmp::Ordering::Less || cmp == std::cmp::Ordering::Equal
        }
        PropertyFilterOperator::Contains => {
            if let (Some(actual_str), Some(expected_str)) = (get_string_value(actual_value), get_string_value(expected_value)) {
                actual_str.contains(expected_str)
            } else {
                false
            }
        }
        PropertyFilterOperator::StartsWith => {
            if let (Some(actual_str), Some(expected_str)) = (get_string_value(actual_value), get_string_value(expected_value)) {
                actual_str.starts_with(expected_str)
            } else {
                false
            }
        }
        PropertyFilterOperator::EndsWith => {
            if let (Some(actual_str), Some(expected_str)) = (get_string_value(actual_value), get_string_value(expected_value)) {
                actual_str.ends_with(expected_str)
            } else {
                false
            }
        }
        // Note: IN operator not yet in PropertyFilterOperator enum
        _ => false,
    }
}

/// Compare two property values
fn compare_property_values(
    a: &crate::proto::proximadb_v1::PropertyValue,
    b: &crate::proto::proximadb_v1::PropertyValue,
) -> std::cmp::Ordering {
    use crate::proto::proximadb_v1::property_value::Value;

    match (&a.value, &b.value) {
        (Some(Value::StringValue(a)), Some(Value::StringValue(b))) => a.cmp(b),
        (Some(Value::IntValue(a)), Some(Value::IntValue(b))) => a.cmp(b),
        (Some(Value::DoubleValue(a)), Some(Value::DoubleValue(b))) => {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        }
        (Some(Value::BoolValue(a)), Some(Value::BoolValue(b))) => a.cmp(b),
        _ => std::cmp::Ordering::Equal,
    }
}

/// Extract string value from PropertyValue
fn get_string_value(value: &crate::proto::proximadb_v1::PropertyValue) -> Option<&str> {
    use crate::proto::proximadb_v1::property_value::Value;

    match &value.value {
        Some(Value::StringValue(s)) => Some(s.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::property_value::Value;
    use crate::proto::proximadb_v1::PropertyValue;

    #[test]
    fn test_evaluate_property_filter_equals() {
        let filter = PropertyFilter {
            key: "name".to_string(),
            operator: PropertyFilterOperator::Equals as i32,
            value: Some(PropertyValue {
                value: Some(Value::StringValue("Alice".to_string())),
            }),
        };

        let actual = PropertyValue {
            value: Some(Value::StringValue("Alice".to_string())),
        };

        assert!(evaluate_property_filter(&filter, &actual));

        let different = PropertyValue {
            value: Some(Value::StringValue("Bob".to_string())),
        };

        assert!(!evaluate_property_filter(&filter, &different));
    }

    #[test]
    fn test_evaluate_property_filter_greater_than() {
        let filter = PropertyFilter {
            key: "age".to_string(),
            operator: PropertyFilterOperator::GreaterThan as i32,
            value: Some(PropertyValue {
                value: Some(Value::IntValue(25)),
            }),
        };

        let older = PropertyValue {
            value: Some(Value::IntValue(30)),
        };

        assert!(evaluate_property_filter(&filter, &older));

        let younger = PropertyValue {
            value: Some(Value::IntValue(20)),
        };

        assert!(!evaluate_property_filter(&filter, &younger));
    }

    #[test]
    fn test_evaluate_property_filter_contains() {
        let filter = PropertyFilter {
            key: "description".to_string(),
            operator: PropertyFilterOperator::Contains as i32,
            value: Some(PropertyValue {
                value: Some(Value::StringValue("graph".to_string())),
            }),
        };

        let matching = PropertyValue {
            value: Some(Value::StringValue("This is a graph database".to_string())),
        };

        assert!(evaluate_property_filter(&filter, &matching));

        let not_matching = PropertyValue {
            value: Some(Value::StringValue("This is a document store".to_string())),
        };

        assert!(!evaluate_property_filter(&filter, &not_matching));
    }

    #[test]
    fn test_compare_property_values() {
        let a = PropertyValue {
            value: Some(Value::IntValue(10)),
        };
        let b = PropertyValue {
            value: Some(Value::IntValue(20)),
        };

        assert_eq!(compare_property_values(&a, &b), std::cmp::Ordering::Less);
        assert_eq!(compare_property_values(&b, &a), std::cmp::Ordering::Greater);
        assert_eq!(compare_property_values(&a, &a), std::cmp::Ordering::Equal);
    }
}
