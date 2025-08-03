//! Search module for ProximaDB storage-aware search implementations

pub mod multi_tier_deduplication;
pub mod results;
pub mod unified_interface;
pub mod typesafe_filter;
pub mod index_based_filter;

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Unified search parameters for all storage engines
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchParams {
    // Core search parameters
    /// Query vectors for similarity search (supports single or batch search)
    pub query_vectors: Option<Vec<Vec<f32>>>,
    
    /// Number of results to return
    pub top_k: Option<usize>,
    
    /// Distance metric to use for similarity calculation
    pub distance_metric: Option<crate::compute::distance::DistanceMetric>,
    
    /// Unified metadata filter expression supporting AND, OR, NOT operators
    pub filter_expression: Option<FilterExpression>,
    
    /// Accuracy threshold for search (0.0-1.0)
    pub accuracy_threshold: Option<f32>,
    
    /// Include expired vectors in results
    pub include_expired: Option<bool>,
    
    /// Search timeout in milliseconds
    pub timeout_ms: Option<u64>,
    
    /// Enable two-stage search with quantization
    pub enable_two_stage: Option<bool>,
    
    // Optional optimization hints
    /// Preferred quantization level for search
    pub quantization_hint: Option<crate::compute::UnifiedQuantizationLevel>,
    
    /// Hint to enable/disable cluster optimization
    pub enable_clustering_hint: Option<bool>,
    
    /// Hint to enable/disable metadata filtering optimization
    pub enable_metadata_filtering_hint: Option<bool>,
    
    /// Custom optimization parameters
    pub custom_hints: Option<HashMap<String, serde_json::Value>>,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            query_vectors: None,
            top_k: Some(10),
            distance_metric: Some(crate::compute::distance::DistanceMetric::Cosine),
            filter_expression: None,
            accuracy_threshold: Some(0.95),
            include_expired: Some(false),
            timeout_ms: Some(5000),
            enable_two_stage: Some(true),
            quantization_hint: None,
            enable_clustering_hint: Some(true),
            enable_metadata_filtering_hint: Some(true),
            custom_hints: None,
        }
    }
}

impl SearchParams {
    /// Create search params for a single vector query
    pub fn single_vector(query_vector: Vec<f32>) -> Self {
        Self {
            query_vectors: Some(vec![query_vector]),
            ..Default::default()
        }
    }
    
    /// Create search params for batch vector query
    pub fn batch_vectors(query_vectors: Vec<Vec<f32>>) -> Self {
        Self {
            query_vectors: Some(query_vectors),
            ..Default::default()
        }
    }
    
    /// Get the first query vector (for single vector search)
    pub fn first_query_vector(&self) -> Option<&Vec<f32>> {
        self.query_vectors.as_ref()?.first()
    }
    
    /// Check if this is a batch search
    pub fn is_batch_search(&self) -> bool {
        self.query_vectors.as_ref().map_or(false, |v| v.len() > 1)
    }
    
    /// Create a filter expression from simple key-value pairs
    pub fn with_simple_filters(mut self, filters: HashMap<String, serde_json::Value>) -> Self {
        if filters.is_empty() {
            return self;
        }
        
        let conditions: Vec<FilterExpression> = filters.into_iter()
            .map(|(key, value)| FilterExpression::Comparison {
                field: key,
                operator: ComparisonOperator::Equals,
                value,
            })
            .collect();
        
        let filter_expr = if conditions.len() == 1 {
            conditions.into_iter().next().unwrap()
        } else {
            FilterExpression::And(conditions)
        };
        
        // Combine with existing filter if present
        self.filter_expression = match self.filter_expression {
            Some(existing) => Some(FilterExpression::And(vec![existing, filter_expr])),
            None => Some(filter_expr),
        };
        
        self
    }
}

/// Complex filter expression for advanced metadata filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilterExpression {
    /// Single comparison operation
    Comparison {
        field: String,
        operator: ComparisonOperator,
        value: serde_json::Value,
    },
    /// Logical AND of multiple expressions
    And(Vec<FilterExpression>),
    /// Logical OR of multiple expressions
    Or(Vec<FilterExpression>),
    /// Logical NOT of an expression
    Not(Box<FilterExpression>),
}

/// Comparison operators for metadata filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComparisonOperator {
    Equals,
    NotEquals,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    In,
    NotIn,
    Contains,
    StartsWith,
    EndsWith,
    Between,
    IsNull,
    IsNotNull,
}

// Re-export main types
pub use multi_tier_deduplication::{
    DeduplicationStats, MultiTierDeduplicator, StorageTier, TieredSearchCandidate, 
    DeduplicationStorageEngine, MetadataFilter,
};

// Filter types are already defined above, no need to re-export
pub use results::{SearchResult, SearchResultSet, SearchDebugInfo, QuantizationInfo, EngineStats};
pub use unified_interface::{
    UnifiedSearchEngine, UnifiedSearchOrchestrator, UnifiedSearchContext,
    CollectionConfig, FilterableColumn, ColumnDataType, StorageInfo, OptimizationHint,
};

/// JSON Value Comparison Utilities
/// 
/// This module provides centralized JSON value comparison logic that handles
/// numeric type coercion correctly across integer and floating-point values.
pub mod json_comparison {
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
        match (n1.as_i64(), n2.as_i64()) {
            (Some(i1), Some(i2)) => return i1 == i2,
            _ => {}
        }
        
        // Try unsigned integer comparison for large positive numbers
        match (n1.as_u64(), n2.as_u64()) {
            (Some(u1), Some(u2)) => return u1 == u2,
            _ => {}
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
            _ => false
        }
    }
    
    /// Compare JSON values for ordering (supports all JSON types)
    /// 
    /// Type precedence: Null < Bool < Number < String < Array < Object
    pub fn compare_json_values(a: &Value, b: &Value) -> Ordering {
        match (a, b) {
            (Value::Number(n1), Value::Number(n2)) => {
                // Try integer comparison first for precision
                match (n1.as_i64(), n2.as_i64()) {
                    (Some(i1), Some(i2)) => return i1.cmp(&i2),
                    _ => {}
                }
                
                // Try unsigned comparison for large numbers
                match (n1.as_u64(), n2.as_u64()) {
                    (Some(u1), Some(u2)) => return u1.cmp(&u2),
                    _ => {}
                }
                
                // Fall back to float comparison
                let f1 = n1.as_f64().unwrap_or(0.0);
                let f2 = n2.as_f64().unwrap_or(0.0);
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
    
    /// Evaluate a filter expression against metadata
    /// 
    /// This is the centralized filter evaluation logic used by all storage engines
    pub fn evaluate_filter(
        expr: &crate::core::search::FilterExpression,
        metadata: &std::collections::HashMap<String, Value>,
    ) -> bool {
        use crate::core::search::{FilterExpression, ComparisonOperator};
        
        match expr {
            FilterExpression::And(exprs) => {
                exprs.iter().all(|e| evaluate_filter(e, metadata))
            }
            FilterExpression::Or(exprs) => {
                exprs.iter().any(|e| evaluate_filter(e, metadata))
            }
            FilterExpression::Not(e) => {
                !evaluate_filter(e, metadata)
            }
            FilterExpression::Comparison { field, operator, value } => {
                let field_value = metadata.get(field);
                match (field_value, operator) {
                    (Some(field_val), ComparisonOperator::Equals) => {
                        // Add debug output for filter evaluation
                        #[cfg(feature = "debug-filters")]
                        debug!("    🔍 Evaluating filter: field={}, metadata_val={:?}, filter_val={:?}", 
                            field, field_val, value);
                        
                        // For numbers, use type-aware numeric comparison
                        if let (Value::Number(n1), Value::Number(n2)) = (field_val, value) {
                            let result = compare_json_numbers(n1, n2);
                            #[cfg(feature = "debug-filters")]
                            debug!("      Number comparison: {} vs {} = {}", n1, n2, result);
                            result
                        } else {
                            let result = field_val == value;
                            #[cfg(feature = "debug-filters")]
                            debug!("      Direct comparison: {:?} == {:?} = {}", field_val, value, result);
                            result
                        }
                    }
                    (Some(field_val), ComparisonOperator::NotEquals) => {
                        if let (Value::Number(n1), Value::Number(n2)) = (field_val, value) {
                            !compare_json_numbers(n1, n2)
                        } else {
                            field_val != value
                        }
                    }
                    (Some(field_val), ComparisonOperator::LessThan) => {
                        compare_json_values(field_val, value) == Ordering::Less
                    }
                    (Some(field_val), ComparisonOperator::LessThanOrEqual) => {
                        let ord = compare_json_values(field_val, value);
                        ord == Ordering::Less || ord == Ordering::Equal
                    }
                    (Some(field_val), ComparisonOperator::GreaterThan) => {
                        compare_json_values(field_val, value) == Ordering::Greater
                    }
                    (Some(field_val), ComparisonOperator::GreaterThanOrEqual) => {
                        let ord = compare_json_values(field_val, value);
                        ord == Ordering::Greater || ord == Ordering::Equal
                    }
                    (Some(Value::Array(arr)), ComparisonOperator::In) => {
                        arr.contains(value)
                    }
                    (Some(field_val), ComparisonOperator::In) => {
                        if let Value::Array(values) = value {
                            values.iter().any(|v| {
                                if let (Value::Number(n1), Value::Number(n2)) = (field_val, v) {
                                    compare_json_numbers(n1, n2)
                                } else {
                                    field_val == v
                                }
                            })
                        } else {
                            false
                        }
                    }
                    (Some(field_val), ComparisonOperator::NotIn) => {
                        if let Value::Array(values) = value {
                            !values.iter().any(|v| {
                                if let (Value::Number(n1), Value::Number(n2)) = (field_val, v) {
                                    compare_json_numbers(n1, n2)
                                } else {
                                    field_val == v
                                }
                            })
                        } else {
                            true
                        }
                    }
                    (Some(Value::String(s)), ComparisonOperator::Contains) => {
                        if let Value::String(pattern) = value {
                            s.contains(pattern)
                        } else {
                            false
                        }
                    }
                    (Some(Value::String(s)), ComparisonOperator::StartsWith) => {
                        if let Value::String(pattern) = value {
                            s.starts_with(pattern)
                        } else {
                            false
                        }
                    }
                    (Some(Value::String(s)), ComparisonOperator::EndsWith) => {
                        if let Value::String(pattern) = value {
                            s.ends_with(pattern)
                        } else {
                            false
                        }
                    }
                    (Some(field_val), ComparisonOperator::Between) => {
                        if let Value::Array(bounds) = value {
                            if bounds.len() == 2 {
                                let ge_lower = compare_json_values(field_val, &bounds[0]) != Ordering::Less;
                                let le_upper = compare_json_values(field_val, &bounds[1]) != Ordering::Greater;
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
}

/// JSON value serialization for index statistics
pub mod json_value_serde;

/// Centralized metadata filter extraction utilities
pub mod filter_extraction {
    use std::collections::{HashMap, HashSet};
    use crate::core::search::{FilterExpression, ComparisonOperator};
    
    /// Extract simple equality conditions from filter expressions
    /// 
    /// This extracts field/value pairs from FilterExpression for efficient metadata filtering
    /// Used consistently across SST, VIPER, and Write Buffer engines
    /// 
    /// # Examples
    /// ```rust,ignore
    /// use proximadb::core::search::{FilterExpression, ComparisonOperator};
    /// use proximadb::core::search::filter_extraction::extract_metadata_conditions;
    /// 
    /// let filter = FilterExpression::Comparison {
    ///     field: "batch".to_string(),
    ///     operator: ComparisonOperator::Equals,
    ///     value: serde_json::json!(2),
    /// };
    /// let conditions = extract_metadata_conditions(&filter);
    /// assert_eq!(conditions.get("batch"), Some(&serde_json::json!(2)));
    /// ```
    pub fn extract_metadata_conditions(filter_expr: &FilterExpression) -> HashMap<String, serde_json::Value> {
        let mut conditions = HashMap::new();
        extract_conditions_recursive(filter_expr, &mut conditions);
        conditions
    }

    /// Recursively extract metadata conditions from filter expressions
    fn extract_conditions_recursive(expr: &FilterExpression, conditions: &mut HashMap<String, serde_json::Value>) {
        match expr {
            FilterExpression::Comparison { field, operator, value } => {
                // Only extract equality conditions for metadata filtering
                if matches!(operator, ComparisonOperator::Equals) {
                    conditions.insert(field.clone(), value.clone());
                }
            }
            FilterExpression::And(exprs) => {
                // For AND expressions, extract all conditions
                for expr in exprs {
                    extract_conditions_recursive(expr, conditions);
                }
            }
            FilterExpression::Or(_) | FilterExpression::Not(_) => {
                // OR and NOT expressions are too complex for simple metadata filtering
                // These will be handled by full expression evaluation
            }
        }
    }

    /// Extract column names referenced in a filter expression
    /// 
    /// Used for determining which columns need to be loaded/indexed
    /// 
    /// # Examples
    /// ```rust,ignore
    /// use proximadb::core::search::{FilterExpression, ComparisonOperator};
    /// use proximadb::core::search::filter_extraction::extract_filter_columns;
    /// 
    /// let filter = FilterExpression::And(vec![
    ///     FilterExpression::Comparison { 
    ///         field: "batch".to_string(), 
    ///         operator: ComparisonOperator::Equals,
    ///         value: serde_json::json!(1),
    ///     },
    ///     FilterExpression::Comparison { 
    ///         field: "category".to_string(), 
    ///         operator: ComparisonOperator::Equals,
    ///         value: serde_json::json!("A"),
    ///     },
    /// ]);
    /// let columns = extract_filter_columns(&filter);
    /// assert!(columns.contains("batch"));
    /// assert!(columns.contains("category"));
    /// ```
    pub fn extract_filter_columns(expr: &FilterExpression) -> HashSet<String> {
        let mut columns = HashSet::new();
        extract_columns_recursive(expr, &mut columns);
        columns
    }

    /// Recursively extract column names from filter expressions
    fn extract_columns_recursive(expr: &FilterExpression, columns: &mut HashSet<String>) {
        match expr {
            FilterExpression::Comparison { field, .. } => {
                columns.insert(field.clone());
            }
            FilterExpression::And(exprs) | FilterExpression::Or(exprs) => {
                for expr in exprs {
                    extract_columns_recursive(expr, columns);
                }
            }
            FilterExpression::Not(expr) => {
                extract_columns_recursive(expr, columns);
            }
        }
    }
}
