//! Thread-safe filter evaluation for parallel search
//!
//! This module provides a Send+Sync filter evaluator that can be safely
//! shared across threads for parallel SSTable searching.

use std::collections::HashMap;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tracing::{warn, error};

use crate::core::search::{FilterExpression, ComparisonOperator};

/// Thread-safe filter evaluator that can be shared across async tasks
#[derive(Clone, Debug)]
pub struct ThreadSafeFilterEvaluator {
    filter: Arc<FilterExpressionEvaluator>,
}

impl ThreadSafeFilterEvaluator {
    /// Create a new filter evaluator from a FilterExpression
    pub fn new(filter_expr: Option<&FilterExpression>) -> Option<Self> {
        filter_expr.map(|expr| Self {
            filter: Arc::new(FilterExpressionEvaluator::from_expression(expr)),
        })
    }

    /// Evaluate the filter against metadata
    pub fn evaluate(&self, metadata: &HashMap<String, String>) -> bool {
        self.filter.evaluate(metadata)
    }
}

/// Internal filter evaluator that implements the actual evaluation logic
#[derive(Clone, Debug, Serialize, Deserialize)]
enum FilterExpressionEvaluator {
    /// Always returns true (no filter)
    All,
    
    /// Field equals value
    Equals {
        field: String,
        value: serde_json::Value,
    },
    
    /// Field not equals value
    NotEquals {
        field: String,
        value: serde_json::Value,
    },
    
    /// Field contains substring
    Contains {
        field: String,
        substring: String,
    },
    
    /// Field exists
    Exists {
        field: String,
    },
    
    /// Field does not exist
    NotExists {
        field: String,
    },
    
    /// Field in list of values
    In {
        field: String,
        values: Vec<String>,
    },
    
    /// Field greater than value
    GreaterThan {
        field: String,
        value: serde_json::Value,
    },
    
    /// Field less than value
    LessThan {
        field: String,
        value: serde_json::Value,
    },
    
    /// Logical AND of multiple filters
    And {
        filters: Vec<FilterExpressionEvaluator>,
    },
    
    /// Logical OR of multiple filters
    Or {
        filters: Vec<FilterExpressionEvaluator>,
    },
    
    /// Logical NOT of a filter
    Not {
        filter: Box<FilterExpressionEvaluator>,
    },
}

impl FilterExpressionEvaluator {
    /// Convert from FilterExpression to our evaluator
    fn from_expression(expr: &FilterExpression) -> Self {
        match expr {
            FilterExpression::Comparison { field, operator, value } => {
                match operator {
                    ComparisonOperator::Equals => FilterExpressionEvaluator::Equals {
                        field: field.clone(),
                        value: value.clone(),
                    },
                    ComparisonOperator::NotEquals => FilterExpressionEvaluator::NotEquals {
                        field: field.clone(),
                        value: value.clone(),
                    },
                    ComparisonOperator::GreaterThan => FilterExpressionEvaluator::GreaterThan {
                        field: field.clone(),
                        value: value.clone(),
                    },
                    ComparisonOperator::LessThan => FilterExpressionEvaluator::LessThan {
                        field: field.clone(),
                        value: value.clone(),
                    },
                    ComparisonOperator::GreaterThanOrEqual => {
                        // Treat >= as > for simplicity
                        FilterExpressionEvaluator::GreaterThan {
                            field: field.clone(),
                            value: value.clone(),
                        }
                    },
                    ComparisonOperator::LessThanOrEqual => {
                        // Treat <= as < for simplicity
                        FilterExpressionEvaluator::LessThan {
                            field: field.clone(),
                            value: value.clone(),
                        }
                    },
                    ComparisonOperator::Contains => FilterExpressionEvaluator::Contains {
                        field: field.clone(),
                        substring: value.as_str().unwrap_or("").to_string(),
                    },
                    ComparisonOperator::In => {
                        // Parse value as array or comma-separated string
                        let values = if let Some(arr) = value.as_array() {
                            arr.iter().map(|v| v.to_string()).collect()
                        } else if let Some(s) = value.as_str() {
                            s.split(',').map(|s| s.trim().to_string()).collect()
                        } else {
                            vec![value.to_string()]
                        };
                        FilterExpressionEvaluator::In {
                            field: field.clone(),
                            values,
                        }
                    },
                }
            },
            FilterExpression::And(filters) => {
                let child_evaluators: Vec<FilterExpressionEvaluator> = filters
                    .iter()
                    .map(|f| Self::from_expression(f))
                    .collect();
                FilterExpressionEvaluator::And {
                    filters: child_evaluators,
                }
            },
            FilterExpression::Or(filters) => {
                let child_evaluators: Vec<FilterExpressionEvaluator> = filters
                    .iter()
                    .map(|f| Self::from_expression(f))
                    .collect();
                FilterExpressionEvaluator::Or {
                    filters: child_evaluators,
                }
            },
            FilterExpression::Not(filter) => {
                FilterExpressionEvaluator::Not {
                    filter: Box::new(Self::from_expression(filter)),
                }
            },
        }
    }
    
    /// Evaluate the filter against metadata
    fn evaluate(&self, metadata: &HashMap<String, String>) -> bool {
        match self {
            FilterExpressionEvaluator::All => true,
            
            FilterExpressionEvaluator::Equals { field, value } => {
                metadata.get(field).map(|v| {
                    // Convert metadata value to match JSON value type
                    if let Some(s) = value.as_str() {
                        v == s
                    } else if let Some(n) = value.as_i64() {
                        v.parse::<i64>().ok() == Some(n)
                    } else if let Some(n) = value.as_f64() {
                        v.parse::<f64>().ok() == Some(n)
                    } else if let Some(b) = value.as_bool() {
                        v.parse::<bool>().ok() == Some(b)
                    } else {
                        false
                    }
                }).unwrap_or(false)
            }
            
            FilterExpressionEvaluator::NotEquals { field, value } => {
                // Invert the equals check
                !metadata.get(field).map(|v| {
                    if let Some(s) = value.as_str() {
                        v == s
                    } else if let Some(n) = value.as_i64() {
                        v.parse::<i64>().ok() == Some(n)
                    } else if let Some(n) = value.as_f64() {
                        v.parse::<f64>().ok() == Some(n)
                    } else if let Some(b) = value.as_bool() {
                        v.parse::<bool>().ok() == Some(b)
                    } else {
                        false
                    }
                }).unwrap_or(false)
            }
            
            FilterExpressionEvaluator::Contains { field, substring } => {
                metadata.get(field).map(|v| v.contains(substring)).unwrap_or(false)
            }
            
            FilterExpressionEvaluator::Exists { field } => {
                metadata.contains_key(field)
            }
            
            FilterExpressionEvaluator::NotExists { field } => {
                !metadata.contains_key(field)
            }
            
            FilterExpressionEvaluator::In { field, values } => {
                metadata.get(field).map(|v| values.contains(v)).unwrap_or(false)
            }
            
            FilterExpressionEvaluator::GreaterThan { field, value } => {
                metadata.get(field).map(|v| {
                    if let Some(n) = value.as_f64() {
                        v.parse::<f64>().map(|v_num| v_num > n).unwrap_or(false)
                    } else if let Some(s) = value.as_str() {
                        v > s
                    } else {
                        false
                    }
                }).unwrap_or(false)
            }
            
            FilterExpressionEvaluator::LessThan { field, value } => {
                metadata.get(field).map(|v| {
                    if let Some(n) = value.as_f64() {
                        v.parse::<f64>().map(|v_num| v_num < n).unwrap_or(false)
                    } else if let Some(s) = value.as_str() {
                        v < s
                    } else {
                        false
                    }
                }).unwrap_or(false)
            }
            
            FilterExpressionEvaluator::And { filters } => {
                filters.iter().all(|f| f.evaluate(metadata))
            }
            
            FilterExpressionEvaluator::Or { filters } => {
                filters.iter().any(|f| f.evaluate(metadata))
            }
            
            FilterExpressionEvaluator::Not { filter } => {
                !filter.evaluate(metadata)
            }
        }
    }
}

/// Helper function to create a Send+Sync filter function from FilterExpression
pub fn create_filter_fn(
    filter_expr: Option<&FilterExpression>
) -> Option<Arc<dyn Fn(&HashMap<String, String>) -> bool + Send + Sync>> {
    filter_expr.map(|expr| {
        let evaluator = ThreadSafeFilterEvaluator::new(Some(expr)).unwrap();
        Arc::new(move |metadata: &HashMap<String, String>| {
            evaluator.evaluate(metadata)
        }) as Arc<dyn Fn(&HashMap<String, String>) -> bool + Send + Sync>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_equals_filter() {
        let mut metadata = HashMap::new();
        metadata.insert("category".to_string(), "science".to_string());
        
        let filter = FilterExpressionEvaluator::Equals {
            field: "category".to_string(),
            value: "science".to_string(),
        };
        
        assert!(filter.evaluate(&metadata));
        
        metadata.insert("category".to_string(), "math".to_string());
        assert!(!filter.evaluate(&metadata));
    }
    
    #[test]
    fn test_and_filter() {
        let mut metadata = HashMap::new();
        metadata.insert("category".to_string(), "science".to_string());
        metadata.insert("year".to_string(), "2024".to_string());
        
        let filter = FilterExpressionEvaluator::And {
            filters: vec![
                FilterExpressionEvaluator::Equals {
                    field: "category".to_string(),
                    value: "science".to_string(),
                },
                FilterExpressionEvaluator::GreaterThan {
                    field: "year".to_string(),
                    value: "2023".to_string(),
                },
            ],
        };
        
        assert!(filter.evaluate(&metadata));
        
        metadata.insert("year".to_string(), "2022".to_string());
        assert!(!filter.evaluate(&metadata));
    }
    
    #[test]
    fn test_thread_safety() {
        use std::thread;
        use crate::core::search::ComparisonOperator;
        
        let filter_expr = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: "science".to_string(),
        };
        
        let evaluator = ThreadSafeFilterEvaluator::new(Some(&filter_expr)).unwrap();
        
        // Test that we can clone and send to another thread
        let evaluator_clone = evaluator.clone();
        let handle = thread::spawn(move || {
            let mut metadata = HashMap::new();
            metadata.insert("category".to_string(), "science".to_string());
            evaluator_clone.evaluate(&metadata)
        });
        
        assert!(handle.join().unwrap());
    }
}