//! Search module for ProximaDB storage-aware search implementations

pub mod lsm_search;
pub mod multi_tier_deduplication;
pub mod storage_aware;
pub mod viper_search;

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Unified search parameters for all storage engines
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchParams {
    // Required parameters
    /// Number of results to return
    pub top_k: Option<usize>,
    
    /// Metadata filters to apply
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
    
    /// Hint to enable/disable metadata filtering optimization
    pub enable_metadata_filtering_hint: Option<bool>,
    
    /// Custom optimization parameters
    pub custom_hints: Option<HashMap<String, serde_json::Value>>,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            top_k: Some(10),
            filters: None,
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

// Re-export main types
pub use multi_tier_deduplication::{
    DeduplicationStats, MultiTierDeduplicator, StorageTier, TieredSearchResult, 
    DeduplicationStorageEngine, MetadataFilter,
};
pub use storage_aware::{
    ClusteringHints, QuantizationLevel, SearchCapabilities, SearchEngineFactory,
    SearchMetrics, SearchValidator, StorageSearchEngine,
};
