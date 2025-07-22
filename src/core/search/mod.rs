//! Search module for ProximaDB storage-aware search implementations

// pub mod lsm_search; // Obsolete - uses old storage_aware interface
pub mod multi_tier_deduplication;
pub mod results;
// pub mod storage_aware; // Obsolete - replaced by unified search
pub mod unified_interface;
// pub mod viper_search; // Obsolete - uses old storage_aware interface

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
    
    /// Simple metadata filters (for backward compatibility)
    pub filters: Option<HashMap<String, serde_json::Value>>,
    
    /// Complex filter expression supporting AND, OR, NOT operators
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
            filters: None,
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
    DeduplicationStats, MultiTierDeduplicator, StorageTier, TieredSearchResult, 
    DeduplicationStorageEngine, MetadataFilter,
};

// Filter types are already defined above, no need to re-export
// Obsolete storage_aware exports - replaced by unified search
// pub use storage_aware::{
//     ClusteringHints, QuantizationLevel, SearchCapabilities, SearchEngineFactory,
//     SearchMetrics, SearchValidator, StorageSearchEngine,
// };
pub use results::{SearchResult, SearchResultSet, SearchDebugInfo, QuantizationInfo, EngineStats};
pub use unified_interface::{
    UnifiedSearchEngine, UnifiedSearchOrchestrator, UnifiedSearchContext,
    CollectionConfig, FilterableColumn, ColumnDataType, StorageInfo, OptimizationHint,
};
