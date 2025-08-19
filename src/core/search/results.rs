//! Search result types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use crate::compute::distance_computation::engine::SimilarityResult;
use crate::compute::quantization::unified::UnifiedQuantizationLevel;

/// Unified search result structure - replaces 13+ duplicates across schema_types and other files
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SearchResult {
    /// Vector/document identifier
    pub id: String,
    /// Alternative identifier field for compatibility with schema_types
    pub vector_id: Option<String>,
    /// Similarity score (higher = more similar)
    pub score: f32,
    /// Distance value (lower = more similar, if different from score)  
    pub similarity: Option<f32>,
    /// Result rank (1-based) - use u16 since ranks are typically small
    /// Original vector data (optional for bandwidth optimization)
    pub vector: Option<Vec<f32>>,
    /// Associated metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Debug information for result
    pub debug_info: Option<SearchDebugInfo>,
    /// Version for MVCC (multi-version concurrency control) - use u32 to match proto VectorRecord  
    pub version: Option<u32>,
    /// Record timestamp for version resolution (earliest wins for same version) - use u32 for seconds since epoch (unsigned)
    pub timestamp: Option<u32>,
    
    // Unified search pipeline integration
    /// Semantic distance information with metric awareness (replaces multiple adapters)
    pub semantic_similarity: Option<SimilarityResult>,
    /// Quantization information if applicable
    pub quantization_info: Option<QuantizationInfo>,
    /// Engine-specific optimization stats (replaces multiple result types)
    pub engine_stats: Option<EngineStats>,
    
    // Additional fields for compatibility with existing code
    /// Index path for result tracking
    pub index_path: Option<String>,
    // Creation timestamp (as DateTime) - removed as duplicate, use the u32 timestamp instead
    // pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
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
    pub name: Option<String>,
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
    pub fn new(id: String, similarity: f32) -> Self {
        Self {
            id,
            vector_id: None,
            score: similarity,
            similarity: None,
            // rank removed -  None,
            vector: None,
            metadata: HashMap::new(),
            debug_info: None,
            version: None,
            timestamp: None,
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
            // timestamp: None, // Duplicate field removed
        }
    }
    
    /// Create search result with metadata
    pub fn with_metadata(
        id: String,
        similarity: f32,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            id,
            vector_id: None,
            score: similarity,
            similarity: None,
            // rank removed -  None,
            vector: None,
            metadata,
            debug_info: None,
            version: None,
            timestamp: None,
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
            // timestamp: None, // Duplicate field removed
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
        self.semantic_similarity = Some(semantic_distance.clone());
        // Update core score/distance fields for compatibility
        self.score = semantic_distance.normalized_score;
        self.similarity = Some(semantic_distance.rank_value);
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
        semantic_similarity: SimilarityResult,
        vector: Option<Vec<f32>>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            id,
            vector_id,
            score: semantic_similarity.normalized_score,
            similarity: Some(semantic_similarity.rank_value),
            // rank removed -  None,
            vector,
            metadata,
            debug_info: None,
            version: None,
            timestamp: None,
            semantic_similarity: Some(semantic_similarity),
            quantization_info: None,
            engine_stats: None,
            index_path: None,
        }
    }
    
    /// Create a simple search result with just ID, score and defaults for other fields
    pub fn simple(id: String, similarity: f32) -> Self {
        Self {
            id,
            vector_id: None,
            score: similarity,
            similarity: None,
            // rank removed -  None,
            vector: None,
            metadata: HashMap::new(),
            debug_info: None,
            version: None,
            timestamp: None,
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
            // timestamp: None, // Duplicate field removed
        }
    }
}

/// Collection of search results with metadata
/// Using Arc<[SearchResult]> for immutable, zero-copy sharing of results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultSet {
    /// Individual search results - immutable for performance
    #[serde(with = "arc_slice_serde")]
    pub results: Arc<[SearchResult]>,
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

impl SearchResultSet {
    /// Create a SearchResultSet from a Vec<SearchResult>
    pub fn from_vec(
        results: Vec<SearchResult>,
        total_count: u64,
        query_id: Option<String>,
        processing_time_us: u64,
        algorithm: String,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            results: Arc::from(results.into_boxed_slice()),
            total_count,
            query_id,
            processing_time_us,
            algorithm,
            metadata,
        }
    }

    /// Create an empty SearchResultSet
    pub fn empty(algorithm: String) -> Self {
        Self {
            results: Arc::from(Vec::new().into_boxed_slice()),
            total_count: 0,
            query_id: None,
            processing_time_us: 0,
            algorithm,
            metadata: HashMap::new(),
        }
    }
}

/// Helper module for serializing Arc<[T]>
mod arc_slice_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;
    use super::SearchResult;

    pub fn serialize<S>(results: &Arc<[SearchResult]>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        results.as_ref().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Arc<[SearchResult]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec = Vec::<SearchResult>::deserialize(deserializer)?;
        Ok(Arc::from(vec.into_boxed_slice()))
    }
}

// Manual trait implementations for ordering (HashMap doesn't implement Ord)
impl Eq for SearchResult {}

impl PartialOrd for SearchResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SearchResult {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Order by score in REVERSE order (higher scores first)
        // For distance metrics, lower is better, so this gives us better results first
        other.score.partial_cmp(&self.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| self.id.cmp(&other.id)) // Tie-break by ID for consistency
    }
}