//! Type-safe metadata filtering using collection metadata
//!
//! This module provides type-aware filtering that leverages collection metadata
//! to perform comparisons without cast operators, improving performance for both
//! SST and VIPER engines.

use crate::core::search::{ComparisonOperator, FilterExpression};
use crate::proto::proximadb::{FilterableColumnSpec, FilterableDataType};
use crate::proto::proximadb::{MetadataItem, metadata_item::Value as MetadataValue};
use std::cmp::Ordering;
use std::collections::HashMap;

/// Type-safe filter evaluator that uses collection metadata for datatype information
pub struct TypeSafeFilterEvaluator {
    /// Column metadata from collection configuration
    column_types: HashMap<String, FilterableDataType>,
}

impl TypeSafeFilterEvaluator {
    /// Create a new evaluator from collection metadata
    pub fn new(filterable_columns: &[FilterableColumnSpec]) -> Self {
        let column_types: HashMap<String, FilterableDataType> = filterable_columns
            .iter()
            .filter_map(|col| {
                FilterableDataType::try_from(col.data_type)
                    .ok()
                    .map(|dt| (col.name.clone(), dt))
            })
            .collect();

        Self { column_types }
    }

    /// Evaluate a filter expression against MetadataItem values with type safety
    pub fn evaluate(&self, expr: &FilterExpression, metadata: &[MetadataItem]) -> bool {
        // Convert MetadataItem slice to a HashMap for efficient lookup
        let metadata_map: HashMap<&str, &MetadataItem> = metadata
            .iter()
            .map(|item| (item.key.as_str(), item))
            .collect();

        self.evaluate_recursive(expr, &metadata_map)
    }

    /// Recursively evaluate filter expressions
    fn evaluate_recursive(
        &self,
        expr: &FilterExpression,
        metadata: &HashMap<&str, &MetadataItem>,
    ) -> bool {
        match expr {
            FilterExpression::And(exprs) => {
                exprs.iter().all(|e| self.evaluate_recursive(e, metadata))
            }
            FilterExpression::Or(exprs) => {
                exprs.iter().any(|e| self.evaluate_recursive(e, metadata))
            }
            FilterExpression::Not(e) => !self.evaluate_recursive(e, metadata),
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                // Get the field's expected data type from collection metadata
                let expected_type = self.column_types.get(field).copied();

                // Get the actual metadata value
                let metadata_item = metadata.get(field.as_str());

                match (metadata_item, operator) {
                    (Some(item), ComparisonOperator::Equals) => {
                        self.compare_typed_values(&item.value, value, expected_type)
                            == Some(Ordering::Equal)
                    }
                    (Some(item), ComparisonOperator::NotEquals) => {
                        self.compare_typed_values(&item.value, value, expected_type)
                            != Some(Ordering::Equal)
                    }
                    (Some(item), ComparisonOperator::LessThan) => {
                        matches!(
                            self.compare_typed_values(&item.value, value, expected_type),
                            Some(Ordering::Less)
                        )
                    }
                    (Some(item), ComparisonOperator::LessThanOrEqual) => {
                        matches!(
                            self.compare_typed_values(&item.value, value, expected_type),
                            Some(Ordering::Less) | Some(Ordering::Equal)
                        )
                    }
                    (Some(item), ComparisonOperator::GreaterThan) => {
                        matches!(
                            self.compare_typed_values(&item.value, value, expected_type),
                            Some(Ordering::Greater)
                        )
                    }
                    (Some(item), ComparisonOperator::GreaterThanOrEqual) => {
                        matches!(
                            self.compare_typed_values(&item.value, value, expected_type),
                            Some(Ordering::Greater) | Some(Ordering::Equal)
                        )
                    }
                    (Some(item), ComparisonOperator::In) => {
                        if let serde_json::Value::Array(values) = value {
                            values.iter().any(|v| {
                                self.compare_typed_values(&item.value, v, expected_type)
                                    == Some(Ordering::Equal)
                            })
                        } else {
                            false
                        }
                    }
                    (Some(item), ComparisonOperator::NotIn) => {
                        if let serde_json::Value::Array(values) = value {
                            !values.iter().any(|v| {
                                self.compare_typed_values(&item.value, v, expected_type)
                                    == Some(Ordering::Equal)
                            })
                        } else {
                            true
                        }
                    }
                    (Some(item), ComparisonOperator::Contains) => {
                        if let (
                            Some(MetadataValue::StringValue(s)),
                            serde_json::Value::String(pattern),
                        ) = (&item.value, value)
                        {
                            s.contains(pattern)
                        } else {
                            false
                        }
                    }
                    (Some(item), ComparisonOperator::StartsWith) => {
                        if let (
                            Some(MetadataValue::StringValue(s)),
                            serde_json::Value::String(pattern),
                        ) = (&item.value, value)
                        {
                            s.starts_with(pattern)
                        } else {
                            false
                        }
                    }
                    (Some(item), ComparisonOperator::EndsWith) => {
                        if let (
                            Some(MetadataValue::StringValue(s)),
                            serde_json::Value::String(pattern),
                        ) = (&item.value, value)
                        {
                            s.ends_with(pattern)
                        } else {
                            false
                        }
                    }
                    (Some(item), ComparisonOperator::Between) => {
                        if let serde_json::Value::Array(bounds) = value {
                            if bounds.len() == 2 {
                                let ge_lower = matches!(
                                    self.compare_typed_values(
                                        &item.value,
                                        &bounds[0],
                                        expected_type
                                    ),
                                    Some(Ordering::Greater) | Some(Ordering::Equal)
                                );
                                let le_upper = matches!(
                                    self.compare_typed_values(
                                        &item.value,
                                        &bounds[1],
                                        expected_type
                                    ),
                                    Some(Ordering::Less) | Some(Ordering::Equal)
                                );
                                ge_lower && le_upper
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                    (None, ComparisonOperator::IsNull) => true,
                    (Some(_), ComparisonOperator::IsNull) => false,
                    (None, ComparisonOperator::IsNotNull) => false,
                    (Some(_), ComparisonOperator::IsNotNull) => true,
                    _ => false,
                }
            }
        }
    }

    /// Compare MetadataItem value with JSON value using type information
    fn compare_typed_values(
        &self,
        metadata_value: &Option<MetadataValue>,
        json_value: &serde_json::Value,
        expected_type: Option<FilterableDataType>,
    ) -> Option<Ordering> {
        match (metadata_value, expected_type) {
            // String comparison
            (Some(MetadataValue::StringValue(s)), Some(FilterableDataType::FilterableString)) => {
                if let serde_json::Value::String(js) = json_value {
                    Some(s.cmp(js))
                } else {
                    None
                }
            }

            // Number comparison - no casting needed, direct type comparison
            (
                Some(MetadataValue::NumberValue(n)),
                Some(FilterableDataType::FilterableInteger)
                | Some(FilterableDataType::FilterableFloat),
            ) => {
                match json_value {
                    serde_json::Value::Number(jn) => {
                        // Direct numeric comparison without conversion
                        if let Some(ji) = jn.as_i64() {
                            Some((*n as i64).cmp(&ji))
                        } else if let Some(jf) = jn.as_f64() {
                            n.partial_cmp(&jf)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }

            // Boolean comparison
            (Some(MetadataValue::BoolValue(b)), Some(FilterableDataType::FilterableBoolean)) => {
                if let serde_json::Value::Bool(jb) = json_value {
                    Some(b.cmp(jb))
                } else {
                    None
                }
            }

            // Cross-type conversions when metadata doesn't match expected type
            // This can happen with flexible schemas or type mismatches
            (Some(MetadataValue::StringValue(s)), Some(FilterableDataType::FilterableInteger)) => {
                // Try to parse string as integer
                if let (Ok(si), serde_json::Value::Number(jn)) = (s.parse::<i64>(), json_value) {
                    if let Some(ji) = jn.as_i64() {
                        Some(si.cmp(&ji))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }

            (Some(MetadataValue::StringValue(s)), Some(FilterableDataType::FilterableFloat)) => {
                // Try to parse string as float
                if let (Ok(sf), serde_json::Value::Number(jn)) = (s.parse::<f64>(), json_value) {
                    if let Some(jf) = jn.as_f64() {
                        sf.partial_cmp(&jf)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }

            (Some(MetadataValue::NumberValue(n)), Some(FilterableDataType::FilterableString)) => {
                // Convert number to string for comparison
                if let serde_json::Value::String(js) = json_value {
                    n.to_string().as_str().cmp(js).into()
                } else {
                    None
                }
            }

            _ => None,
        }
    }
}

/// Convert MetadataItem values to serde_json for compatibility
/// This is used as a fallback when type-safe filtering isn't available
pub fn metadata_items_to_json(items: &[MetadataItem]) -> HashMap<String, serde_json::Value> {
    let mut map = HashMap::new();
    for item in items {
        let value = match &item.value {
            Some(MetadataValue::StringValue(s)) => serde_json::Value::String(s.clone()),
            Some(MetadataValue::NumberValue(n)) => {
                if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 {
                    serde_json::Value::Number(serde_json::Number::from(*n as i64))
                } else {
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(*n)
                            .unwrap_or_else(|| serde_json::Number::from(0)),
                    )
                }
            }
            Some(MetadataValue::BoolValue(b)) => serde_json::Value::Bool(*b),
            None => serde_json::Value::Null,
        };
        map.insert(item.key.clone(), value);
    }
    map
}

/// Extract metadata conditions for efficient filtering
pub fn extract_typed_conditions(
    expr: &FilterExpression,
    column_types: &HashMap<String, FilterableDataType>,
) -> HashMap<String, TypedCondition> {
    let mut conditions = HashMap::new();
    extract_conditions_recursive(expr, column_types, &mut conditions);
    conditions
}

/// Typed condition for optimized filtering
#[derive(Debug, Clone)]
pub struct TypedCondition {
    pub operator: ComparisonOperator,
    pub value: MetadataValue,
}

fn extract_conditions_recursive(
    expr: &FilterExpression,
    column_types: &HashMap<String, FilterableDataType>,
    conditions: &mut HashMap<String, TypedCondition>,
) {
    match expr {
        FilterExpression::Comparison {
            field,
            operator,
            value,
        } => {
            if let Some(&data_type) = column_types.get(field) {
                // Convert JSON value to typed MetadataValue
                let typed_value = match (data_type, value) {
                    (FilterableDataType::FilterableString, serde_json::Value::String(s)) => {
                        Some(MetadataValue::StringValue(s.clone()))
                    }
                    (
                        FilterableDataType::FilterableInteger | FilterableDataType::FilterableFloat,
                        serde_json::Value::Number(n),
                    ) => n.as_f64().map(MetadataValue::NumberValue),
                    (FilterableDataType::FilterableBoolean, serde_json::Value::Bool(b)) => {
                        Some(MetadataValue::BoolValue(*b))
                    }
                    _ => None,
                };

                if let Some(tv) = typed_value {
                    conditions.insert(
                        field.clone(),
                        TypedCondition {
                            operator: operator.clone(),
                            value: tv,
                        },
                    );
                }
            }
        }
        FilterExpression::And(exprs) => {
            for expr in exprs {
                extract_conditions_recursive(expr, column_types, conditions);
            }
        }
        _ => {} // OR and NOT are too complex for simple extraction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_safe_string_comparison() {
        let columns = vec![FilterableColumnSpec {
            name: "category".to_string(),
            data_type: FilterableDataType::FilterableString as i32,
            indexed: true,
            supports_range: false,
            estimated_cardinality: Some(100),
            encoding_hint: None,
        }];

        let evaluator = TypeSafeFilterEvaluator::new(&columns);

        let metadata = vec![MetadataItem {
            key: "category".to_string(),
            value: Some(MetadataValue::StringValue("electronics".to_string())),
        }];

        let filter = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("electronics"),
        };

        assert!(evaluator.evaluate(&filter, &metadata));
    }

    #[test]
    fn test_type_safe_numeric_comparison() {
        let columns = vec![FilterableColumnSpec {
            name: "price".to_string(),
            data_type: FilterableDataType::FilterableFloat as i32,
            indexed: true,
            supports_range: true,
            estimated_cardinality: None,
            encoding_hint: None,
        }];

        let evaluator = TypeSafeFilterEvaluator::new(&columns);

        let metadata = vec![MetadataItem {
            key: "price".to_string(),
            value: Some(MetadataValue::NumberValue(99.99)),
        }];

        let filter = FilterExpression::Comparison {
            field: "price".to_string(),
            operator: ComparisonOperator::LessThan,
            value: serde_json::json!(100.0),
        };

        assert!(evaluator.evaluate(&filter, &metadata));
    }
}
