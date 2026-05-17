// Common traits and types for the Zero-Copy I/O System
// Consolidated from previous implementations to avoid duplication

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use proximadb_kernel::error::ProximaDBError;

/// Engine-specific metadata serialization trait
///
/// Each storage engine (SST, Parquet, etc.) implements this trait to provide
/// optimized metadata serialization for zero-copy caching.
pub trait MetadataSerializer: Send + Sync {
    /// Engine identifier (e.g., "SST", "PARQUET", "SWIFT")
    fn engine_id(&self) -> &'static str;

    /// Serialize file metadata to bytes using optimal format
    /// Fixed-size data uses bytemuck, variable-size uses bincode
    fn serialize_metadata(
        &self,
        file_path: &str,
        collection_id: &str,
    ) -> Result<Vec<u8>, ProximaDBError>;

    /// Deserialize metadata from cached bytes
    fn deserialize_metadata(&self, data: &[u8]) -> Result<Box<dyn EngineMetadata>, ProximaDBError>;

    /// Check if entire file can be skipped based on metadata
    fn can_skip_file(&self, metadata: &dyn EngineMetadata, query_context: &QueryContext) -> bool;

    /// Get specific data ranges to read (None = read entire file)
    fn get_required_ranges(
        &self,
        metadata: &dyn EngineMetadata,
        query_context: &QueryContext,
    ) -> Option<Vec<DataRange>>;

    /// Estimate selectivity for the given query (0.0 = no matches, 1.0 = all matches)
    fn estimate_selectivity(
        &self,
        metadata: &dyn EngineMetadata,
        query_context: &QueryContext,
    ) -> f32 {
        metadata.estimated_selectivity(query_context)
    }

    /// Helper method to hash strings for bloom filters
    fn hash_string(&self, s: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }

    /// Helper method to hash bytes for bloom filters
    fn hash_bytes(&self, bytes: &[u8]) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    }

    /// Simple bloom filter check helper
    fn check_bloom_simple(&self, bloom_data: &[u8], key: &[u8]) -> bool {
        if bloom_data.is_empty() {
            return false; // No bloom filter data
        }
        let hash = self.hash_bytes(key);
        let index = (hash % bloom_data.len() as u64) as usize;
        bloom_data[index] != 0
    }
}

/// Engine-agnostic metadata interface
///
/// Provides common operations that all engine metadata types must support
pub trait EngineMetadata: Send + Sync {
    /// Get total file size
    fn file_size(&self) -> u64;

    /// Estimate query selectivity based on metadata
    fn estimated_selectivity(&self, query_context: &QueryContext) -> f32;

    /// Get memory footprint of this metadata structure
    fn memory_footprint(&self) -> usize;

    /// Get creation timestamp if available
    fn creation_timestamp(&self) -> Option<u64> {
        None
    }

    /// Get compression ratio if available
    fn compression_ratio(&self) -> Option<f32> {
        None
    }

    /// Check if metadata supports given query type
    fn supports_query_type(&self, query_type: &QueryType) -> bool {
        match query_type {
            QueryType::IdLookup => true,         // All engines support ID lookup
            QueryType::SimilaritySearch => true, // All engines support similarity
            QueryType::MetadataFilter => true,   // All engines support metadata
            QueryType::Batch => true,            // All engines support batch
            QueryType::VectorSearch => true,     // All engines support vector search
            QueryType::FullScan => true,         // All engines support full scan
        }
    }

    /// Enable downcasting to concrete types
    fn as_any(&self) -> &dyn std::any::Any;

    /// Clone the metadata (manual implementation since we can't use dyn_clone)
    fn clone_box(&self) -> Box<dyn EngineMetadata>;
}

/// Query context for filtering and optimization decisions
#[derive(Debug, Clone)]
pub struct QueryContext {
    /// Vector similarity query
    pub query_vector: Option<Vec<f32>>,

    /// Metadata filters
    pub metadata_filters: HashMap<String, String>,

    /// ID lookups
    pub id_lookups: Vec<String>,

    /// Top-K requirement for similarity search
    pub top_k: Option<usize>,

    /// Distance threshold for filtering
    pub distance_threshold: Option<f32>,

    /// Query type classification
    pub query_type: QueryType,

    /// Collection-specific context
    pub collection_context: Option<CollectionContext>,

    /// Request priority for batching and scheduling
    pub priority: RequestPriority,

    /// Estimated result size for memory planning
    pub estimated_result_size: Option<usize>,

    /// Selectivity hint for query optimization (0.0 = very selective, 1.0 = all records)
    pub selectivity_hint: Option<f32>,

    /// Collection ID for context-specific optimizations
    pub collection_id: String,

    /// Number of concurrent queries for resource planning
    pub concurrent_queries: Option<usize>,

    /// Cache temperature hint (hot/warm/cold access patterns)
    pub cache_temperature: CacheTemperature,
}

/// Type of query for pattern analysis and optimization
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueryType {
    /// Single or batch ID lookup
    IdLookup,
    /// Vector similarity search
    SimilaritySearch,
    /// Metadata-only filtering
    MetadataFilter,
    /// Batch operations (mixed)
    Batch,
    /// Vector search (alias for SimilaritySearch)
    VectorSearch,
    /// Full scan of all data
    FullScan,
}

/// Collection-specific context for optimization
#[derive(Debug, Clone)]
pub struct CollectionContext {
    /// Collection ID
    pub collection_id: String,

    /// Vector dimension
    pub dimension: usize,

    /// Distance metric used
    pub distance_metric: String,

    /// Typical query patterns
    pub query_patterns: Vec<QueryType>,

    /// Access frequency
    pub access_frequency: AccessFrequency,
}

/// Access frequency classification
#[derive(Debug, Clone)]
pub enum AccessFrequency {
    VeryHigh, // > 1000 ops/hour
    High,     // 100-1000 ops/hour
    Medium,   // 10-100 ops/hour
    Low,      // 1-10 ops/hour
    VeryLow,  // < 1 op/hour
}

/// Cache temperature for access pattern optimization
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheTemperature {
    Hot,  // Frequently accessed, keep in memory
    Warm, // Moderately accessed, consider caching
    Cold, // Rarely accessed, avoid caching
}

/// Data range to read from file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataRange {
    /// Byte offset in file
    pub offset: u64,

    /// Number of bytes to read
    pub length: u64,

    /// Priority for ordering (255 = highest priority)
    pub priority: u8,
}

impl DataRange {
    /// Create a new data range
    pub fn new(offset: u64, length: u64, priority: u8) -> Self {
        Self {
            offset,
            length,
            priority,
        }
    }

    /// Get end offset
    pub fn end_offset(&self) -> u64 {
        self.offset + self.length
    }

    /// Check if this range overlaps with another
    pub fn overlaps_with(&self, other: &DataRange) -> bool {
        self.offset < other.end_offset() && other.offset < self.end_offset()
    }

    /// Check if this range is contiguous with another
    pub fn is_contiguous_with(&self, other: &DataRange) -> bool {
        self.end_offset() == other.offset || other.end_offset() == self.offset
    }

    /// Merge with another range if possible
    pub fn try_merge(&self, other: &DataRange) -> Option<DataRange> {
        if self.overlaps_with(other) || self.is_contiguous_with(other) {
            let start = self.offset.min(other.offset);
            let end = self.end_offset().max(other.end_offset());
            let priority = self.priority.max(other.priority);

            Some(DataRange::new(start, end - start, priority))
        } else {
            None
        }
    }
}

/// File access request for batch optimization
#[derive(Debug, Clone)]
pub struct FileAccessRequest {
    /// File path (local or cloud)
    pub file_path: String,

    /// Collection ID
    pub collection_id: String,

    /// Engine type
    pub engine_type: String,

    /// Query context
    pub query_context: QueryContext,

    /// Request priority
    pub priority: RequestPriority,
}

/// Request priority for batching and scheduling
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestPriority {
    Critical = 4,
    High = 3,
    Normal = 2,
    Low = 1,
    Background = 0,
}

/// Result of metadata analysis
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MetadataAnalysisResult {
    /// Can the entire file be skipped?
    pub can_skip_file: bool,

    /// Required data ranges (None = full file)
    pub required_ranges: Option<Vec<DataRange>>,

    /// Estimated selectivity
    pub estimated_selectivity: f32,

    /// Confidence in the analysis
    pub confidence: f32,

    /// Analysis rationale
    pub rationale: String,
}

impl Default for QueryContext {
    fn default() -> Self {
        Self {
            query_vector: None,
            metadata_filters: HashMap::new(),
            id_lookups: Vec::new(),
            top_k: None,
            distance_threshold: None,
            query_type: QueryType::SimilaritySearch,
            collection_context: None,
            priority: RequestPriority::Normal,
            estimated_result_size: None,
            selectivity_hint: None,
            collection_id: String::new(),
            concurrent_queries: None,
            cache_temperature: CacheTemperature::Warm,
        }
    }
}

impl QueryContext {
    /// Create a new query context for ID lookup
    pub fn for_id_lookup(ids: Vec<String>) -> Self {
        Self {
            id_lookups: ids,
            query_type: QueryType::IdLookup,
            ..Default::default()
        }
    }

    /// Create a new query context for ID lookup with collection ID
    pub fn for_id_lookup_with_collection(ids: Vec<String>, collection_id: String) -> Self {
        Self {
            id_lookups: ids,
            query_type: QueryType::IdLookup,
            collection_id,
            ..Default::default()
        }
    }

    /// Create a new query context for similarity search
    pub fn for_similarity_search(query_vector: Vec<f32>, top_k: usize) -> Self {
        Self {
            query_vector: Some(query_vector),
            top_k: Some(top_k),
            query_type: QueryType::SimilaritySearch,
            ..Default::default()
        }
    }

    /// Create a new query context for similarity search with collection ID
    pub fn for_similarity_search_with_collection(
        query_vector: Vec<f32>,
        top_k: usize,
        collection_id: String,
    ) -> Self {
        Self {
            query_vector: Some(query_vector),
            top_k: Some(top_k),
            query_type: QueryType::SimilaritySearch,
            collection_id,
            ..Default::default()
        }
    }

    /// Create a new query context for metadata filtering
    pub fn for_metadata_filter(filters: HashMap<String, String>) -> Self {
        Self {
            metadata_filters: filters,
            query_type: QueryType::MetadataFilter,
            ..Default::default()
        }
    }

    /// Create a new query context for metadata filtering with collection ID
    pub fn for_metadata_filter_with_collection(
        filters: HashMap<String, String>,
        collection_id: String,
    ) -> Self {
        Self {
            metadata_filters: filters,
            query_type: QueryType::MetadataFilter,
            collection_id,
            ..Default::default()
        }
    }

    /// Check if this is a point query (single ID lookup)
    pub fn is_point_query(&self) -> bool {
        self.query_type == QueryType::IdLookup && self.id_lookups.len() == 1
    }

    /// Check if this is a batch query
    pub fn is_batch_query(&self) -> bool {
        self.query_type == QueryType::Batch
            || (self.query_type == QueryType::IdLookup && self.id_lookups.len() > 1)
    }

    /// Get query complexity score (0.0 = simple, 1.0 = complex)
    pub fn complexity_score(&self) -> f32 {
        let mut score = 0.0;

        // Vector similarity adds complexity
        if self.query_vector.is_some() {
            score += 0.3;
            if let Some(top_k) = self.top_k {
                score += (top_k as f32 / 1000.0).min(0.2); // Up to 0.2 for top_k
            }
        }

        // Multiple ID lookups add complexity
        if self.id_lookups.len() > 1 {
            score += (self.id_lookups.len() as f32 / 100.0).min(0.3);
        }

        // Metadata filters add complexity
        if !self.metadata_filters.is_empty() {
            score += (self.metadata_filters.len() as f32 / 10.0).min(0.2);
        }

        score.min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_range_operations() {
        let range1 = DataRange::new(0, 100, 255);
        let range2 = DataRange::new(100, 50, 128);
        let range3 = DataRange::new(200, 100, 200);

        // Test contiguous ranges
        assert!(range1.is_contiguous_with(&range2));
        assert!(!range1.is_contiguous_with(&range3));

        // Test merging
        let merged = range1.try_merge(&range2).unwrap();
        assert_eq!(merged.offset, 0);
        assert_eq!(merged.length, 150);
        assert_eq!(merged.priority, 255);

        // Test non-mergeable
        assert!(range1.try_merge(&range3).is_none());
    }

    #[test]
    fn test_query_context_builders() {
        let id_context = QueryContext::for_id_lookup(vec!["id1".to_string(), "id2".to_string()]);
        assert_eq!(id_context.query_type, QueryType::IdLookup);
        assert!(id_context.is_batch_query());

        let similarity_context = QueryContext::for_similarity_search(vec![1.0, 2.0, 3.0], 10);
        assert_eq!(similarity_context.query_type, QueryType::SimilaritySearch);
        assert!(!similarity_context.is_point_query());

        let mut filters = HashMap::new();
        filters.insert("category".to_string(), "electronics".to_string());
        let filter_context = QueryContext::for_metadata_filter(filters);
        assert_eq!(filter_context.query_type, QueryType::MetadataFilter);
    }

    #[test]
    fn test_complexity_scoring() {
        let simple_context = QueryContext::for_id_lookup(vec!["id1".to_string()]);
        assert!(simple_context.complexity_score() < 0.1);

        let complex_context = QueryContext::for_similarity_search(vec![1.0; 768], 1000);
        assert!(complex_context.complexity_score() > 0.3);
    }
}
