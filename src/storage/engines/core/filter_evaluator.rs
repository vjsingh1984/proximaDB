//! Unified Filter Evaluator Module
//!
//! This module provides a comprehensive, thread-safe filter evaluation system
//! that can be used across all storage engines (SST, VIPER, HELIX, etc.).
//!
//! Features:
//! - Thread-safe evaluation with Send+Sync support
//! - All comparison operators from FilterExpression
//! - Automatic type conversions between String and serde_json::Value
//! - Performance optimizations with caching
//! - Consistent behavior across all engines

use anyhow::{Result, anyhow};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::cmp::Ordering;

use crate::core::search::{ComparisonOperator, FilterExpression};

/// Thread-safe filter evaluator that can be shared across async tasks
#[derive(Clone)]
pub struct UnifiedFilterEvaluator {
    /// The compiled filter for efficient evaluation
    filter: Arc<CompiledFilter>,
}

unsafe impl Send for UnifiedFilterEvaluator {}
unsafe impl Sync for UnifiedFilterEvaluator {}

impl UnifiedFilterEvaluator {
    /// Create a new evaluator from a FilterExpression
    pub fn new(expr: Option<&FilterExpression>) -> Result<Option<Self>> {
        match expr {
            None => Ok(None),
            Some(expr) => {
                let compiled = CompiledFilter::compile(expr)?;
                Ok(Some(Self {
                    filter: Arc::new(compiled),
                }))
            }
        }
    }

    /// Evaluate the filter against metadata with string values
    pub fn evaluate_strings(&self, metadata: &HashMap<String, String>) -> bool {
        // Convert string metadata to JSON values for evaluation
        let json_metadata: HashMap<String, Value> = metadata
            .iter()
            .map(|(k, v)| {
                // Try to parse as JSON value, fall back to string
                let value = if let Ok(parsed) = serde_json::from_str(v) {
                    parsed
                } else {
                    Value::String(v.clone())
                };
                (k.clone(), value)
            })
            .collect();

        self.filter.evaluate(&json_metadata)
    }

    /// Evaluate the filter against metadata with JSON values
    pub fn evaluate(&self, metadata: &HashMap<String, Value>) -> bool {
        self.filter.evaluate(metadata)
    }

    /// Evaluate against proto metadata items
    pub fn evaluate_proto(&self, metadata: &[crate::proto::proximadb_v1::MetadataItem]) -> bool {
        let json_metadata = crate::core::proto_metadata_helper::proto_metadata_to_json(metadata);
        self.filter.evaluate(&json_metadata)
    }

    /// Create a thread-safe closure for use in parallel operations
    pub fn as_closure(&self) -> Arc<dyn Fn(&HashMap<String, String>) -> bool + Send + Sync> {
        let evaluator = self.clone();
        Arc::new(move |metadata| evaluator.evaluate_strings(metadata))
    }

    /// Create a thread-safe closure for JSON metadata
    pub fn as_json_closure(&self) -> Arc<dyn Fn(&HashMap<String, Value>) -> bool + Send + Sync> {
        let evaluator = self.clone();
        Arc::new(move |metadata| evaluator.evaluate(metadata))
    }
}

/// Compiled filter for efficient evaluation
#[derive(Debug, Clone)]
enum CompiledFilter {
    /// Always returns true (no filter)
    All,

    /// Comparison operations
    Equals {
        field: String,
        value: Value,
    },
    NotEquals {
        field: String,
        value: Value,
    },
    GreaterThan {
        field: String,
        value: Value,
    },
    GreaterThanOrEqual {
        field: String,
        value: Value,
    },
    LessThan {
        field: String,
        value: Value,
    },
    LessThanOrEqual {
        field: String,
        value: Value,
    },

    /// String operations
    Contains {
        field: String,
        substring: String,
    },
    StartsWith {
        field: String,
        prefix: String,
    },
    EndsWith {
        field: String,
        suffix: String,
    },
    Like {
        field: String,
        pattern: String,
    },

    /// Set operations
    In {
        field: String,
        values: Vec<Value>,
    },
    NotIn {
        field: String,
        values: Vec<Value>,
    },

    /// Range operations
    Between {
        field: String,
        min: Value,
        max: Value,
    },

    /// Null checks
    IsNull {
        field: String,
    },
    IsNotNull {
        field: String,
    },

    /// Logical operations
    And {
        filters: Vec<CompiledFilter>,
    },
    Or {
        filters: Vec<CompiledFilter>,
    },
    Not {
        filter: Box<CompiledFilter>,
    },
}

impl CompiledFilter {
    /// Compile a FilterExpression into an optimized form
    fn compile(expr: &FilterExpression) -> Result<Self> {
        match expr {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                match operator {
                    ComparisonOperator::Equals => Ok(CompiledFilter::Equals {
                        field: field.clone(),
                        value: value.clone(),
                    }),
                    ComparisonOperator::NotEquals => Ok(CompiledFilter::NotEquals {
                        field: field.clone(),
                        value: value.clone(),
                    }),
                    ComparisonOperator::GreaterThan => Ok(CompiledFilter::GreaterThan {
                        field: field.clone(),
                        value: value.clone(),
                    }),
                    ComparisonOperator::GreaterThanOrEqual => {
                        Ok(CompiledFilter::GreaterThanOrEqual {
                            field: field.clone(),
                            value: value.clone(),
                        })
                    }
                    ComparisonOperator::LessThan => Ok(CompiledFilter::LessThan {
                        field: field.clone(),
                        value: value.clone(),
                    }),
                    ComparisonOperator::LessThanOrEqual => Ok(CompiledFilter::LessThanOrEqual {
                        field: field.clone(),
                        value: value.clone(),
                    }),
                    ComparisonOperator::Contains => {
                        let substring = value
                            .as_str()
                            .ok_or_else(|| anyhow!("Contains requires string value"))?
                            .to_string();
                        Ok(CompiledFilter::Contains {
                            field: field.clone(),
                            substring,
                        })
                    }
                    ComparisonOperator::StartsWith => {
                        let prefix = value
                            .as_str()
                            .ok_or_else(|| anyhow!("StartsWith requires string value"))?
                            .to_string();
                        Ok(CompiledFilter::StartsWith {
                            field: field.clone(),
                            prefix,
                        })
                    }
                    ComparisonOperator::EndsWith => {
                        let suffix = value
                            .as_str()
                            .ok_or_else(|| anyhow!("EndsWith requires string value"))?
                            .to_string();
                        Ok(CompiledFilter::EndsWith {
                            field: field.clone(),
                            suffix,
                        })
                    }
                    ComparisonOperator::Like => {
                        let pattern = value
                            .as_str()
                            .ok_or_else(|| anyhow!("Like requires string value"))?
                            .to_string();
                        Ok(CompiledFilter::Like {
                            field: field.clone(),
                            pattern,
                        })
                    }
                    ComparisonOperator::In => {
                        let values = if let Some(arr) = value.as_array() {
                            arr.clone()
                        } else if let Some(s) = value.as_str() {
                            // Parse comma-separated values
                            s.split(',')
                                .map(|v| Value::String(v.trim().to_string()))
                                .collect()
                        } else {
                            vec![value.clone()]
                        };
                        Ok(CompiledFilter::In {
                            field: field.clone(),
                            values,
                        })
                    }
                    ComparisonOperator::NotIn => {
                        let values = if let Some(arr) = value.as_array() {
                            arr.clone()
                        } else if let Some(s) = value.as_str() {
                            s.split(',')
                                .map(|v| Value::String(v.trim().to_string()))
                                .collect()
                        } else {
                            vec![value.clone()]
                        };
                        Ok(CompiledFilter::NotIn {
                            field: field.clone(),
                            values,
                        })
                    }
                    ComparisonOperator::Between => {
                        // Between expects an array of [min, max]
                        let arr = value
                            .as_array()
                            .ok_or_else(|| anyhow!("Between requires array of [min, max]"))?;
                        if arr.len() != 2 {
                            return Err(anyhow!("Between requires exactly 2 values"));
                        }
                        Ok(CompiledFilter::Between {
                            field: field.clone(),
                            min: arr[0].clone(),
                            max: arr[1].clone(),
                        })
                    }
                    ComparisonOperator::IsNull => Ok(CompiledFilter::IsNull {
                        field: field.clone(),
                    }),
                    ComparisonOperator::IsNotNull => Ok(CompiledFilter::IsNotNull {
                        field: field.clone(),
                    }),
                }
            }
            FilterExpression::And(filters) => {
                let compiled: Result<Vec<_>> = filters.iter().map(Self::compile).collect();
                Ok(CompiledFilter::And { filters: compiled? })
            }
            FilterExpression::Or(filters) => {
                let compiled: Result<Vec<_>> = filters.iter().map(Self::compile).collect();
                Ok(CompiledFilter::Or { filters: compiled? })
            }
            FilterExpression::Not(filter) => Ok(CompiledFilter::Not {
                filter: Box::new(Self::compile(filter)?),
            }),
        }
    }

    /// Evaluate the compiled filter against metadata
    fn evaluate(&self, metadata: &HashMap<String, Value>) -> bool {
        match self {
            CompiledFilter::All => true,

            CompiledFilter::Equals { field, value } => metadata
                .get(field)
                .map(|v| Self::values_equal(v, value))
                .unwrap_or(false),

            CompiledFilter::NotEquals { field, value } => {
                metadata
                    .get(field)
                    .map(|v| !Self::values_equal(v, value))
                    .unwrap_or(true) // Field not present is "not equal"
            }

            CompiledFilter::GreaterThan { field, value } => metadata
                .get(field)
                .map(|v| Self::compare_values(v, value) == Some(std::cmp::Ordering::Greater))
                .unwrap_or(false),

            CompiledFilter::GreaterThanOrEqual { field, value } => metadata
                .get(field)
                .map(|v| {
                    let ord = Self::compare_values(v, value);
                    ord == Some(std::cmp::Ordering::Greater)
                        || ord == Some(std::cmp::Ordering::Equal)
                })
                .unwrap_or(false),

            CompiledFilter::LessThan { field, value } => metadata
                .get(field)
                .map(|v| Self::compare_values(v, value) == Some(std::cmp::Ordering::Less))
                .unwrap_or(false),

            CompiledFilter::LessThanOrEqual { field, value } => metadata
                .get(field)
                .map(|v| {
                    let ord = Self::compare_values(v, value);
                    ord == Some(std::cmp::Ordering::Less) || ord == Some(std::cmp::Ordering::Equal)
                })
                .unwrap_or(false),

            CompiledFilter::Contains { field, substring } => metadata
                .get(field)
                .and_then(|v| v.as_str())
                .map(|s| s.contains(substring))
                .unwrap_or(false),

            CompiledFilter::StartsWith { field, prefix } => metadata
                .get(field)
                .and_then(|v| v.as_str())
                .map(|s| s.starts_with(prefix))
                .unwrap_or(false),

            CompiledFilter::EndsWith { field, suffix } => metadata
                .get(field)
                .and_then(|v| v.as_str())
                .map(|s| s.ends_with(suffix))
                .unwrap_or(false),

            CompiledFilter::Like { field, pattern } => metadata
                .get(field)
                .and_then(|v| v.as_str())
                .map(|s| Self::like_match(s, pattern))
                .unwrap_or(false),

            CompiledFilter::In { field, values } => metadata
                .get(field)
                .map(|v| values.iter().any(|val| Self::values_equal(v, val)))
                .unwrap_or(false),

            CompiledFilter::NotIn { field, values } => metadata
                .get(field)
                .map(|v| !values.iter().any(|val| Self::values_equal(v, val)))
                .unwrap_or(true),

            CompiledFilter::Between { field, min, max } => metadata
                .get(field)
                .map(|v| {
                    Self::compare_values(v, min)
                        .map(|o| o != std::cmp::Ordering::Less)
                        .unwrap_or(false)
                        && Self::compare_values(v, max)
                            .map(|o| o != std::cmp::Ordering::Greater)
                            .unwrap_or(false)
                })
                .unwrap_or(false),

            CompiledFilter::IsNull { field } => {
                !metadata.contains_key(field) || metadata.get(field) == Some(&Value::Null)
            }

            CompiledFilter::IsNotNull { field } => {
                metadata.contains_key(field) && metadata.get(field) != Some(&Value::Null)
            }

            CompiledFilter::And { filters } => filters.iter().all(|f| f.evaluate(metadata)),

            CompiledFilter::Or { filters } => filters.iter().any(|f| f.evaluate(metadata)),

            CompiledFilter::Not { filter } => !filter.evaluate(metadata),
        }
    }

    /// Check if two JSON values are equal with type coercion
    fn values_equal(v1: &Value, v2: &Value) -> bool {
        // Use the existing json_comparison module for consistency
        crate::core::search::json_comparison::compare_json_values(v1, v2)
            == std::cmp::Ordering::Equal
    }

    /// Compare two JSON values with type coercion
    fn compare_values(v1: &Value, v2: &Value) -> Option<std::cmp::Ordering> {
        Some(crate::core::search::json_comparison::compare_json_values(
            v1, v2,
        ))
    }

    /// SQL LIKE pattern matching (% = any chars, _ = one char)
    fn like_match(text: &str, pattern: &str) -> bool {
        let mut text_chars = text.chars().peekable();
        let mut pattern_chars = pattern.chars().peekable();

        while let Some(&pattern_char) = pattern_chars.peek() {
            match pattern_char {
                '%' => {
                    pattern_chars.next();
                    if pattern_chars.peek().is_none() {
                        return true;
                    }
                    let remaining_pattern: String = pattern_chars.collect();
                    while text_chars.peek().is_some() {
                        let remaining_text: String = text_chars.clone().collect();
                        if Self::like_match(&remaining_text, &remaining_pattern) {
                            return true;
                        }
                        text_chars.next();
                    }
                    return false;
                }
                '_' => {
                    pattern_chars.next();
                    if text_chars.next().is_none() {
                        return false;
                    }
                }
                c => {
                    pattern_chars.next();
                    if text_chars.next() != Some(c) {
                        return false;
                    }
                }
            }
        }
        text_chars.peek().is_none()
    }
}

/// Helper function to create a filter closure from an optional FilterExpression
/// This is the primary API for storage engines to use
pub fn create_filter_fn(
    expr: Option<&FilterExpression>,
) -> Option<Arc<dyn Fn(&HashMap<String, String>) -> bool + Send + Sync>> {
    UnifiedFilterEvaluator::new(expr)
        .ok()
        .flatten()
        .map(|evaluator| evaluator.as_closure())
}

/// Create a filter closure for JSON metadata
pub fn create_json_filter_fn(
    expr: Option<&FilterExpression>,
) -> Option<Arc<dyn Fn(&HashMap<String, Value>) -> bool + Send + Sync>> {
    UnifiedFilterEvaluator::new(expr)
        .ok()
        .flatten()
        .map(|evaluator| evaluator.as_json_closure())
}

/// Direct evaluation function for backward compatibility
pub fn evaluate_filter(expr: &FilterExpression, metadata: &HashMap<String, Value>) -> bool {
    CompiledFilter::compile(expr)
        .map(|filter| filter.evaluate(metadata))
        .unwrap_or(false)
}

/// Evaluate filter with string metadata
pub fn evaluate_filter_strings(
    expr: &FilterExpression,
    metadata: &HashMap<String, String>,
) -> bool {
    UnifiedFilterEvaluator::new(Some(expr))
        .ok()
        .flatten()
        .map(|evaluator| evaluator.evaluate_strings(metadata))
        .unwrap_or(false)
}

/// Check if a field is filterable based on collection configuration
pub fn is_filterable_field(field: &str, filterable_columns: &[String]) -> bool {
    filterable_columns.contains(&field.to_string())
}

/// Evaluate a field value considering both filterable columns and extra_meta
pub fn get_field_value(
    field: &str,
    metadata: &HashMap<String, Value>,
    extra_meta: Option<&HashMap<String, String>>,
    filterable_columns: &[String],
) -> Option<Value> {
    if is_filterable_field(field, filterable_columns) {
        // Fast path: direct column access
        metadata.get(field).cloned()
    } else {
        // Slow path: check extra_meta Map
        extra_meta.and_then(|map| {
            map.get(field).map(|s| Value::String(s.clone()))
        })
    }
}

/// Evaluate filter with awareness of filterable columns
/// This function optimizes metadata filtering by checking filterable columns first
pub fn evaluate_filter_with_config(
    expr: &FilterExpression,
    metadata: &HashMap<String, Value>,
    extra_meta: Option<&HashMap<String, String>>,
    filterable_columns: &[String],
) -> bool {
    match expr {
        FilterExpression::Comparison { field, operator, value } => {
            let field_value = get_field_value(field, metadata, extra_meta, filterable_columns);

            if let Some(metadata_value) = field_value {
                evaluate_comparison_op(&metadata_value, operator, value)
            } else {
                // Field not present - handle NULL comparison
                match operator {
                    ComparisonOperator::Equals => value.is_null(),
                    ComparisonOperator::NotEquals => !value.is_null(),
                    _ => false,
                }
            }
        }
        FilterExpression::And(exprs) => {
            exprs.iter().all(|e| evaluate_filter_with_config(e, metadata, extra_meta, filterable_columns))
        }
        FilterExpression::Or(exprs) => {
            exprs.iter().any(|e| evaluate_filter_with_config(e, metadata, extra_meta, filterable_columns))
        }
        FilterExpression::Not(expr) => {
            !evaluate_filter_with_config(expr, metadata, extra_meta, filterable_columns)
        }
    }
}

fn evaluate_comparison_op(record_value: &Value, operator: &ComparisonOperator, expected: &Value) -> bool {
    match operator {
        ComparisonOperator::Equals => {
            if let (Value::Number(n1), Value::Number(n2)) = (record_value, expected) {
                // Use numeric comparison for numbers
                n1.as_f64() == n2.as_f64()
            } else {
                record_value == expected
            }
        }
        ComparisonOperator::NotEquals => {
            if let (Value::Number(n1), Value::Number(n2)) = (record_value, expected) {
                n1.as_f64() != n2.as_f64()
            } else {
                record_value != expected
            }
        }
        ComparisonOperator::GreaterThan => {
            compare_json_values(record_value, expected) == Ordering::Greater
        }
        ComparisonOperator::LessThan => {
            compare_json_values(record_value, expected) == Ordering::Less
        }
        ComparisonOperator::GreaterThanOrEqual => {
            let ord = compare_json_values(record_value, expected);
            ord == Ordering::Greater || ord == Ordering::Equal
        }
        ComparisonOperator::LessThanOrEqual => {
            let ord = compare_json_values(record_value, expected);
            ord == Ordering::Less || ord == Ordering::Equal
        }
        _ => false,
    }
}

fn compare_json_values(v1: &Value, v2: &Value) -> Ordering {
    match (v1, v2) {
        (Value::Number(n1), Value::Number(n2)) => {
            n1.as_f64().partial_cmp(&n2.as_f64()).unwrap_or(Ordering::Equal)
        }
        (Value::String(s1), Value::String(s2)) => s1.cmp(s2),
        (Value::Bool(b1), Value::Bool(b2)) => b1.cmp(b2),
        _ => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_equals_filter() {
        let mut metadata = HashMap::new();
        metadata.insert("name".to_string(), json!("Alice"));
        metadata.insert("age".to_string(), json!(30));

        let expr = FilterExpression::Comparison {
            field: "name".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("Alice"),
        };

        assert!(evaluate_filter(&expr, &metadata));
    }

    #[test]
    fn test_numeric_comparison() {
        let mut metadata = HashMap::new();
        metadata.insert("score".to_string(), json!(85));

        let expr = FilterExpression::Comparison {
            field: "score".to_string(),
            operator: ComparisonOperator::GreaterThan,
            value: json!(80),
        };

        assert!(evaluate_filter(&expr, &metadata));
    }

    #[test]
    fn test_between_filter() {
        let mut metadata = HashMap::new();
        metadata.insert("age".to_string(), json!(25));

        let expr = FilterExpression::Comparison {
            field: "age".to_string(),
            operator: ComparisonOperator::Between,
            value: json!([20, 30]),
        };

        assert!(evaluate_filter(&expr, &metadata));
    }

    #[test]
    fn test_like_pattern() {
        let mut metadata = HashMap::new();
        metadata.insert("email".to_string(), json!("user@example.com"));

        let expr = FilterExpression::Comparison {
            field: "email".to_string(),
            operator: ComparisonOperator::Like,
            value: json!("%@example.%"),
        };

        assert!(evaluate_filter(&expr, &metadata));
    }

    #[test]
    fn test_thread_safe_evaluator() {
        let expr = FilterExpression::Comparison {
            field: "status".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("active"),
        };

        let evaluator = UnifiedFilterEvaluator::new(Some(&expr)).unwrap().unwrap();
        let closure = evaluator.as_closure();

        let mut metadata = HashMap::new();
        metadata.insert("status".to_string(), "active".to_string());

        assert!(closure(&metadata));
    }

    #[test]
    fn test_complex_and_filter() {
        let mut metadata = HashMap::new();
        metadata.insert("age".to_string(), json!(25));
        metadata.insert("city".to_string(), json!("NYC"));

        let expr = FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "age".to_string(),
                operator: ComparisonOperator::GreaterThanOrEqual,
                value: json!(21),
            },
            FilterExpression::Comparison {
                field: "city".to_string(),
                operator: ComparisonOperator::In,
                value: json!(["NYC", "LA", "SF"]),
            },
        ]);

        assert!(evaluate_filter(&expr, &metadata));
    }
}
