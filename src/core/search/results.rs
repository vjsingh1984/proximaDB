//! Search result types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::compute::unified_distance::SimilarityResult;
use crate::compute::unified_quantization::UnifiedQuantizationLevel;

/// Unified search result structure - replaces 13+ duplicates across schema_types and other files
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Vector/document identifier
    pub id: String,
    /// Alternative identifier field for compatibility with schema_types
    pub vector_id: Option<String>,
    /// Similarity score (higher = more similar)
    pub score: f32,
    /// Distance value (lower = more similar, if different from score)  
    pub distance: Option<f32>,
    /// Result rank (1-based)
    pub rank: Option<i32>,
    /// Original vector data (optional for bandwidth optimization)
    pub vector: Option<Vec<f32>>,
    /// Associated metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Debug information for result
    pub debug_info: Option<SearchDebugInfo>,
    
    // Unified search pipeline integration
    /// Semantic distance information with metric awareness (replaces multiple adapters)
    pub semantic_distance: Option<SimilarityResult>,
    /// Quantization information if applicable
    pub quantization_info: Option<QuantizationInfo>,
    /// Engine-specific optimization stats (replaces multiple result types)
    pub engine_stats: Option<EngineStats>,
    
    // Additional fields for compatibility with existing code
    /// Index path for result tracking
    pub index_path: Option<String>,
    /// Collection identifier
    pub collection_id: Option<String>,
    /// Creation timestamp
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Debug information for search results
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchDebugInfo {
    /// Algorithm used for this result
    pub algorithm: String,
    /// Number of candidates evaluated
    pub candidates_evaluated: u32,
    /// Time spent processing this result (microseconds)
    pub processing_time_us: u64,
}

/// Quantization information for search results
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuantizationInfo {
    /// Quantization level used
    pub level: UnifiedQuantizationLevel,
    /// Compression ratio achieved
    pub compression_ratio: f32,
    /// Accuracy retained (percentage)
    pub accuracy_retained: f32,
    /// Column name in Parquet file
    pub column_name: Option<String>,
}

/// Engine-specific optimization statistics
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EngineStats {
    /// Strategy used (DirectArrow, MetadataFiltered, QuantizedTwoStage, Hybrid)
    pub strategy_used: String,
    /// Bytes read from storage
    pub bytes_read: usize,
    /// Number of seek operations
    pub seek_operations: usize,
    /// Number of HTTP range requests (cloud storage)
    pub range_requests: usize,
    /// Cache hit/miss statistics
    pub cache_hits: usize,
    pub cache_misses: usize,
    /// Deduplication savings
    pub deduplication_savings: usize,
}

impl SearchResult {
    /// Create a basic search result
    pub fn new(id: String, score: f32) -> Self {
        Self {
            id,
            vector_id: None,
            score,
            distance: None,
            rank: None,
            vector: None,
            metadata: HashMap::new(),
            debug_info: None,
            semantic_distance: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
            collection_id: None,
            created_at: None,
        }
    }
    
    /// Create search result with metadata
    pub fn with_metadata(
        id: String,
        score: f32,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            id,
            vector_id: None,
            score,
            distance: None,
            rank: None,
            vector: None,
            metadata,
            debug_info: None,
            semantic_distance: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
            collection_id: None,
            created_at: None,
        }
    }
    
    /// Add vector data to result
    pub fn with_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(vector);
        self
    }
    
    /// Add debug information
    pub fn with_debug_info(mut self, debug_info: SearchDebugInfo) -> Self {
        self.debug_info = Some(debug_info);
        self
    }
    
    /// Add semantic distance information (eliminates adapter conversions)
    pub fn with_semantic_distance(mut self, semantic_distance: SimilarityResult) -> Self {
        self.semantic_distance = Some(semantic_distance);
        // Update core score/distance fields for compatibility
        self.score = semantic_distance.normalized_score;
        self.distance = Some(semantic_distance.rank_value);
        self
    }
    
    /// Add quantization information for Parquet column integration
    pub fn with_quantization_info(mut self, quantization_info: QuantizationInfo) -> Self {
        self.quantization_info = Some(quantization_info);
        self
    }
    
    /// Add engine-specific optimization statistics (eliminates multiple result types)
    pub fn with_engine_stats(mut self, engine_stats: EngineStats) -> Self {
        self.engine_stats = Some(engine_stats);
        self
    }
    
    /// Create search result directly from semantic distance computation
    pub fn from_semantic_distance(
        id: String,
        vector_id: Option<String>,
        semantic_distance: SimilarityResult,
        vector: Option<Vec<f32>>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            id,
            vector_id,
            score: semantic_distance.normalized_score,
            distance: Some(semantic_distance.rank_value),
            rank: None,
            vector,
            metadata,
            debug_info: None,
            semantic_distance: Some(semantic_distance),
            quantization_info: None,
            engine_stats: None,
            index_path: None,
            collection_id: None,
            created_at: None,
        }
    }
}

/// Collection of search results with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultSet {
    /// Individual search results
    pub results: Vec<SearchResult>,
    /// Total number of matching documents (before pagination)
    pub total_count: u64,
    /// Query that generated these results
    pub query_id: Option<String>,
    /// Processing time for entire query (microseconds)
    pub processing_time_us: u64,
    /// Algorithm used for search
    pub algorithm: String,
    /// Additional query metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

// BaseResult trait implementation removed - not needed for unified architecture