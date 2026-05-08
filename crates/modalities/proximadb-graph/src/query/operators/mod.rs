//! Graph query operators implementing the Volcano execution model.

use crate::query::execution::{ColumnSpec, PhysicalOperator, QueryValue, ResultTuple, ValueType};
use proximadb_proto::proximadb_v1::{
    PropertyFilter, PropertyFilterOperator, PropertyValue, property_value::Value,
};

pub mod expand;
pub mod filter;
pub mod limit;
pub mod project;
pub mod scan;

pub use expand::ExpandOperator;
pub use filter::{ComparisonOperator, FilterExpression, FilterOperator, FilterValue};
pub use limit::LimitOperator;
pub use project::{ProjectOperator, ProjectionSpec};
pub use scan::NodeScanOperator;

/// Edge direction for expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDirection {
    /// Outgoing edges (source → target).
    Outgoing,
    /// Incoming edges (source ← target).
    Incoming,
    /// Both incoming and outgoing edges.
    Bidirectional,
}

/// Helper function to evaluate a proto property filter against a concrete value.
pub fn evaluate_property_filter(filter: &PropertyFilter, actual_value: &PropertyValue) -> bool {
    let expected_value = match &filter.value {
        Some(v) => v,
        None => return false,
    };

    let operator =
        PropertyFilterOperator::try_from(filter.operator).unwrap_or(PropertyFilterOperator::Equals);

    match operator {
        PropertyFilterOperator::Equals => {
            compare_property_values(actual_value, expected_value) == std::cmp::Ordering::Equal
        }
        PropertyFilterOperator::NotEquals => {
            compare_property_values(actual_value, expected_value) != std::cmp::Ordering::Equal
        }
        PropertyFilterOperator::GreaterThan => {
            compare_property_values(actual_value, expected_value) == std::cmp::Ordering::Greater
        }
        PropertyFilterOperator::LessThan => {
            compare_property_values(actual_value, expected_value) == std::cmp::Ordering::Less
        }
        PropertyFilterOperator::GreaterEqual => {
            let cmp = compare_property_values(actual_value, expected_value);
            cmp == std::cmp::Ordering::Greater || cmp == std::cmp::Ordering::Equal
        }
        PropertyFilterOperator::LessEqual => {
            let cmp = compare_property_values(actual_value, expected_value);
            cmp == std::cmp::Ordering::Less || cmp == std::cmp::Ordering::Equal
        }
        PropertyFilterOperator::Contains => {
            if let (Some(actual_str), Some(expected_str)) = (
                get_string_value(actual_value),
                get_string_value(expected_value),
            ) {
                actual_str.contains(expected_str)
            } else {
                false
            }
        }
        PropertyFilterOperator::StartsWith => {
            if let (Some(actual_str), Some(expected_str)) = (
                get_string_value(actual_value),
                get_string_value(expected_value),
            ) {
                actual_str.starts_with(expected_str)
            } else {
                false
            }
        }
        PropertyFilterOperator::EndsWith => {
            if let (Some(actual_str), Some(expected_str)) = (
                get_string_value(actual_value),
                get_string_value(expected_value),
            ) {
                actual_str.ends_with(expected_str)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn compare_property_values(a: &PropertyValue, b: &PropertyValue) -> std::cmp::Ordering {
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

fn get_string_value(value: &PropertyValue) -> Option<&str> {
    match &value.value {
        Some(Value::StringValue(s)) => Some(s.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_property_filter_supports_equals_and_contains() {
        let equals_filter = PropertyFilter {
            key: "name".to_string(),
            operator: PropertyFilterOperator::Equals as i32,
            value: Some(PropertyValue {
                value: Some(Value::StringValue("Alice".to_string())),
            }),
        };
        let contains_filter = PropertyFilter {
            key: "description".to_string(),
            operator: PropertyFilterOperator::Contains as i32,
            value: Some(PropertyValue {
                value: Some(Value::StringValue("graph".to_string())),
            }),
        };

        assert!(evaluate_property_filter(
            &equals_filter,
            &PropertyValue {
                value: Some(Value::StringValue("Alice".to_string())),
            }
        ));
        assert!(evaluate_property_filter(
            &contains_filter,
            &PropertyValue {
                value: Some(Value::StringValue("This is a graph database".to_string())),
            }
        ));
        assert!(!evaluate_property_filter(
            &equals_filter,
            &PropertyValue {
                value: Some(Value::StringValue("Bob".to_string())),
            }
        ));
    }

    #[test]
    fn compare_property_values_orders_numeric_values() {
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
