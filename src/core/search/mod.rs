//! Search module for ProximaDB storage-aware search implementations

pub mod multi_tier_deduplication;
pub mod mvcc_resolution;
pub mod results;
pub mod unified_interface;
pub mod typesafe_filter;
pub mod index_based_filter;
pub mod progressive_quantization;
pub mod engine_benchmarks;
pub mod query_preprocessing;
pub mod metadata_filter_pushdown;
pub mod unified_progressive_pipeline;
pub mod smart_execution_strategy;
pub mod integrated_search_optimization;

#[cfg(test)]
mod early_termination_tests;
#[cfg(test)]
mod optimization_tests;

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Custom recall rates for progressive search stages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressiveRecalls {
    pub binary_recall: Option<f32>,
    pub int8_recall: Option<f32>,
    pub pq_recall: Option<f32>,
}

/// Unified search parameters for all storage engines
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchParams {
    // Core search parameters
    /// Query vectors for similarity search (supports single or batch search)
    pub query_vectors: Option<Vec<Vec<f32>>>,
    
    /// Single query vector (alternative to query_vectors for single queries)
    pub vector: Option<Vec<f32>>,
    
    /// Number of results to return
    pub top_k: Option<usize>,
    
    /// Distance metric to use for similarity calculation
    pub distance_metric: Option<crate::compute::distance_computation::DistanceMetric>,
    
    /// Unified metadata filter expression supporting AND, OR, NOT operators
    pub filter_expression: Option<FilterExpression>,
    
    /// Legacy filters field for backward compatibility  
    pub filters: Option<HashMap<String, serde_json::Value>>,
    
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
    
    /// Runtime optimization hints for search strategy selection
    pub runtime_hints: Option<crate::query::unified_search_optimizer::SearchHints>,
    
    /// Hint to enable/disable metadata filtering optimization
    pub enable_metadata_filtering_hint: Option<bool>,
    
    /// Custom optimization parameters
    pub custom_hints: Option<HashMap<String, serde_json::Value>>,
    
    /// Internal: Indicates if the query requires ordering (e.g., gRPC/REST always true, SQL with ORDER BY true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_ordering: Option<bool>,
    
    // Progressive search parameters
    /// Enable progressive quantization-aware search
    pub enable_progressive_search: Option<bool>,
    
    /// Progressive search scenario (high_recall, balanced, high_speed, low_memory)
    pub progressive_scenario: Option<String>,
    
    /// Custom recall rates for progressive stages
    pub progressive_recalls: Option<ProgressiveRecalls>,
    
    /// Optimization hint for search strategy
    pub optimization_hint: Option<String>,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            query_vectors: None,
            vector: None,
            top_k: Some(10),
            distance_metric: Some(crate::compute::distance_computation::DistanceMetric::Cosine),
            filter_expression: None,
            filters: None,
            accuracy_threshold: Some(0.95),
            include_expired: Some(false),
            timeout_ms: Some(5000),
            enable_two_stage: Some(true),
            quantization_hint: None,
            enable_clustering_hint: Some(true),
            enable_metadata_filtering_hint: Some(true),
            custom_hints: None,
            requires_ordering: None,
            runtime_hints: None,
            enable_progressive_search: Some(false),
            progressive_scenario: None,
            progressive_recalls: None,
            optimization_hint: None,
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
    /// SQL-style LIKE pattern matching (supports % and _ wildcards)
    Like,
}

// Re-export main types
pub use multi_tier_deduplication::{
    DeduplicationStats, MultiTierDeduplicator, StorageTier, TieredSearchCandidate, 
    DeduplicationStorageEngine, MetadataFilter,
};

// Filter types are already defined above, no need to re-export
pub use results::{InternalSearchResult, SearchResultSet, SearchDebugInfo, QuantizationInfo, EngineStats};
// NOTE: Proto types (SearchResult, SearchVectorRecord) should NOT be re-exported here.
// They belong in the API layer only. Services should use InternalSearchResult
// and convert to proto types at the API boundary.
pub use unified_interface::{
    UnifiedSearchEngine, IntegratedSearchOptimizer, SearchPlan,
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
    fn like_pattern_match(text: &str, pattern: &str) -> bool {
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
                    (Some(Value::String(s)), ComparisonOperator::Like) => {
                        if let Value::String(pattern) = value {
                            // Simple LIKE implementation: % = any chars, _ = single char
                            // Convert SQL LIKE pattern to simple pattern matching without regex for performance
                            like_pattern_match(s, pattern)
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

/// Protocol Filter Conversion Utilities
/// 
/// This module provides conversion functions from protocol-specific filter types
/// to the unified FilterExpression type for consistent handling across all APIs
pub mod protocol_conversions {
    use crate::core::search::{FilterExpression, ComparisonOperator};
    use serde_json::Value;
    
    /// Convert gRPC proto MetadataFilter to unified FilterExpression
    /// Used by gRPC handlers to convert incoming proto filters
    pub fn from_proto_metadata_filter(
        proto_filter: &crate::proto::proximadb::MetadataFilter
    ) -> Result<FilterExpression, String> {
        if proto_filter.conditions.is_empty() {
            return Ok(FilterExpression::And(vec![]));
        }
        
        let conditions: Result<Vec<FilterExpression>, String> = proto_filter.conditions
            .iter()
            .map(|condition| {
                let field = condition.field_name.clone();
                let value = match &condition.value {
                    Some(v) => serde_json::to_value(v).map_err(|e| e.to_string())?,
                    None => return Err("Missing value in filter condition".to_string()),
                };
                
                let operator = match crate::proto::proximadb::FilterOperation::try_from(condition.operation) {
                    Ok(crate::proto::proximadb::FilterOperation::Equals) => ComparisonOperator::Equals,
                    Ok(crate::proto::proximadb::FilterOperation::NotEquals) => ComparisonOperator::NotEquals,
                    Ok(crate::proto::proximadb::FilterOperation::GreaterThan) => ComparisonOperator::GreaterThan,
                    Ok(crate::proto::proximadb::FilterOperation::GreaterThanOrEqual) => ComparisonOperator::GreaterThanOrEqual,
                    Ok(crate::proto::proximadb::FilterOperation::LessThan) => ComparisonOperator::LessThan,
                    Ok(crate::proto::proximadb::FilterOperation::LessThanOrEqual) => ComparisonOperator::LessThanOrEqual,
                    Ok(crate::proto::proximadb::FilterOperation::In) => ComparisonOperator::In,
                    Ok(crate::proto::proximadb::FilterOperation::NotIn) => ComparisonOperator::NotIn,
                    Ok(crate::proto::proximadb::FilterOperation::Contains) => ComparisonOperator::Contains,
                    Ok(crate::proto::proximadb::FilterOperation::StartsWith) => ComparisonOperator::StartsWith,
                    Ok(crate::proto::proximadb::FilterOperation::EndsWith) => ComparisonOperator::EndsWith,
                    _ => return Err(format!("Unknown proto filter operation: {}", condition.operation)),
                };
                
                Ok(FilterExpression::Comparison { field, operator, value })
            })
            .collect();
        
        match conditions {
            Ok(conds) => {
                if conds.len() == 1 {
                    Ok(conds.into_iter().next().unwrap())
                } else {
                    // Use AND logic by default for multiple conditions
                    Ok(FilterExpression::And(conds))
                }
            }
            Err(e) => Err(e)
        }
    }
    
    /// Convert REST JSON filter to unified FilterExpression
    /// Used by REST handlers to convert JSON filter objects
    pub fn from_rest_json_filter(
        json_filter: &serde_json::Value
    ) -> Result<FilterExpression, String> {
        match json_filter {
            Value::Object(obj) => {
                // Handle different REST filter formats
                if let Some(conditions) = obj.get("conditions") {
                    // Array of conditions with logic operator
                    if let Value::Array(cond_array) = conditions {
                        let logic = obj.get("logic").and_then(|v| v.as_str()).unwrap_or("and");
                        let expressions: Result<Vec<FilterExpression>, String> = cond_array
                            .iter()
                            .map(|c| parse_rest_condition(c))
                            .collect();
                        
                        match expressions {
                            Ok(exprs) => {
                                if logic == "or" {
                                    Ok(FilterExpression::Or(exprs))
                                } else {
                                    Ok(FilterExpression::And(exprs))
                                }
                            }
                            Err(e) => Err(e)
                        }
                    } else {
                        Err("conditions must be an array".to_string())
                    }
                } else {
                    // Single condition object
                    parse_rest_condition(json_filter)
                }
            }
            _ => Err("Filter must be an object".to_string())
        }
    }
    
    /// Parse a single REST condition into FilterExpression
    fn parse_rest_condition(condition: &Value) -> Result<FilterExpression, String> {
        if let Value::Object(obj) = condition {
            let field = obj.get("field")
                .and_then(|v| v.as_str())
                .ok_or("Missing field name")?
                .to_string();
            
            let operator = obj.get("operator")
                .and_then(|v| v.as_str())
                .ok_or("Missing operator")?;
            
            let value = obj.get("value")
                .ok_or("Missing value")?
                .clone();
            
            let op = match operator {
                "eq" | "equals" => ComparisonOperator::Equals,
                "ne" | "not_equals" => ComparisonOperator::NotEquals,
                "gt" | "greater_than" => ComparisonOperator::GreaterThan,
                "gte" | "greater_than_or_equal" => ComparisonOperator::GreaterThanOrEqual,
                "lt" | "less_than" => ComparisonOperator::LessThan,
                "lte" | "less_than_or_equal" => ComparisonOperator::LessThanOrEqual,
                "in" => ComparisonOperator::In,
                "not_in" => ComparisonOperator::NotIn,
                "contains" => ComparisonOperator::Contains,
                "starts_with" => ComparisonOperator::StartsWith,
                "ends_with" => ComparisonOperator::EndsWith,
                "between" => ComparisonOperator::Between,
                "is_null" => ComparisonOperator::IsNull,
                "is_not_null" => ComparisonOperator::IsNotNull,
                "like" => ComparisonOperator::Like,
                _ => return Err(format!("Unknown operator: {}", operator))
            };
            
            Ok(FilterExpression::Comparison { field, operator: op, value })
        } else {
            Err("Condition must be an object".to_string())
        }
    }
    
    /// Convert SQL Condition (from parser) to unified FilterExpression
    /// Used by SQL engine to convert parsed WHERE conditions
    pub fn from_sql_condition(
        condition: &crate::query::sql_engine::parser::Condition
    ) -> Result<FilterExpression, String> {
        use crate::query::sql_engine::parser::{Condition, ComparisonOp, Value};
        
        match condition {
            Condition::Comparison { field, operator, value } => {
                let json_value = sql_value_to_json(value)?;
                let comp_op = match operator {
                    ComparisonOp::Eq => ComparisonOperator::Equals,
                    ComparisonOp::Ne => ComparisonOperator::NotEquals,
                    ComparisonOp::Lt => ComparisonOperator::LessThan,
                    ComparisonOp::Le => ComparisonOperator::LessThanOrEqual,
                    ComparisonOp::Gt => ComparisonOperator::GreaterThan,
                    ComparisonOp::Ge => ComparisonOperator::GreaterThanOrEqual,
                    ComparisonOp::Like => ComparisonOperator::Like,
                    ComparisonOp::In => ComparisonOperator::In,
                };
                
                Ok(FilterExpression::Comparison {
                    field: field.clone(),
                    operator: comp_op,
                    value: json_value,
                })
            }
            Condition::And(left, right) => {
                let left_expr = from_sql_condition(left)?;
                let right_expr = from_sql_condition(right)?;
                Ok(FilterExpression::And(vec![left_expr, right_expr]))
            }
            Condition::Or(left, right) => {
                let left_expr = from_sql_condition(left)?;
                let right_expr = from_sql_condition(right)?;
                Ok(FilterExpression::Or(vec![left_expr, right_expr]))
            }
            Condition::Not(inner) => {
                let inner_expr = from_sql_condition(inner)?;
                Ok(FilterExpression::Not(Box::new(inner_expr)))
            }
            Condition::In { field, values } => {
                let json_values: Result<Vec<serde_json::Value>, String> = values
                    .iter()
                    .map(sql_value_to_json)
                    .collect();
                
                Ok(FilterExpression::Comparison {
                    field: field.clone(),
                    operator: ComparisonOperator::In,
                    value: serde_json::Value::Array(json_values?),
                })
            }
            Condition::Between { field, low, high } => {
                let low_json = sql_value_to_json(low)?;
                let high_json = sql_value_to_json(high)?;
                
                Ok(FilterExpression::Comparison {
                    field: field.clone(),
                    operator: ComparisonOperator::Between,
                    value: serde_json::Value::Array(vec![low_json, high_json]),
                })
            }
        }
    }
    
    /// Convert SQL Value to JSON Value
    fn sql_value_to_json(value: &crate::query::sql_engine::parser::Value) -> Result<serde_json::Value, String> {
        use crate::query::sql_engine::parser::Value;
        
        match value {
            Value::String(s) => Ok(serde_json::Value::String(s.clone())),
            Value::Number(n) => Ok(serde_json::json!(*n)),
            Value::Bool(b) => Ok(serde_json::Value::Bool(*b)),
            Value::Null => Ok(serde_json::Value::Null),
            Value::Vector(v) => Ok(serde_json::json!(v)),
            Value::List(list) => {
                let json_list: Result<Vec<serde_json::Value>, String> = list
                    .iter()
                    .map(sql_value_to_json)
                    .collect();
                Ok(serde_json::Value::Array(json_list?))
            }
        }
    }
    
    /// Convert legacy HashMap filters to unified FilterExpression
    /// Used for backward compatibility with existing filter formats
    pub fn from_legacy_hashmap_filter(
        filters: &std::collections::HashMap<String, serde_json::Value>
    ) -> FilterExpression {
        if filters.is_empty() {
            return FilterExpression::And(vec![]);
        }
        
        let conditions: Vec<FilterExpression> = filters
            .iter()
            .map(|(key, value)| FilterExpression::Comparison {
                field: key.clone(),
                operator: ComparisonOperator::Equals,
                value: value.clone(),
            })
            .collect();
        
        if conditions.len() == 1 {
            conditions.into_iter().next().unwrap()
        } else {
            FilterExpression::And(conditions)
        }
    }
}

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
    /// assert_eq!(conditions.get(key), Some(&serde_json::json!(2)));
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
