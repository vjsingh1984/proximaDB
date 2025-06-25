//! DEPRECATED: Unified Types for ProximaDB
//! 
//! ⚠️  WARNING: This module is OBSOLETE and will be removed in a future version.
//! All unified type definitions have been migrated to `crate::core::avro_unified` with
//! proper Avro schema support for zero-copy serialization and schema evolution.
//! 
//! ## Migration Path
//! - Use `crate::core::avro_unified::CompressionAlgorithm` instead of `CompressionAlgorithm`
//! - Use `crate::core::avro_unified::SearchResult` instead of `SearchResult` 
//! - Use `crate::core::avro_unified::CompactionConfig` instead of `CompactionConfig`
//! - Use `crate::core::avro_unified::DistanceMetric` instead of `DistanceMetric`
//! - All other types are available in `avro_unified` with better schema evolution support
//!
//! ## Removal Timeline
//! This file will be removed once all references are migrated to avro_unified types.
//!
//! This module consolidates duplicate type definitions found across the codebase.
//! It serves as the single source of truth for core data structures.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

/// Unified compression algorithm enum - replaces 10+ duplicates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CompressionAlgorithm {
    /// No compression
    None,
    /// LZ4 fast compression
    Lz4,
    /// LZ4 high compression
    Lz4Hc,
    /// Zstandard compression with configurable level
    Zstd { level: i32 },
    /// Snappy compression  
    Snappy,
    /// GZIP compression
    Gzip,
    /// Deflate compression
    Deflate,
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        // Smart default: Snappy provides good balance of compression ratio and speed
        Self::Snappy
    }
}

/// Unified compression configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Algorithm to use
    pub algorithm: CompressionAlgorithm,
    /// Compression level (1-9, algorithm dependent)
    pub level: u8,
    /// Enable compression for vectors
    pub compress_vectors: bool,
    /// Enable compression for metadata
    pub compress_metadata: bool,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgorithm::None,
            level: 3,
            compress_vectors: false,
            compress_metadata: false,
        }
    }
}

/// Unified search result structure - replaces 13+ duplicates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Vector identifier
    pub id: String,
    /// Similarity score
    pub score: f32,
    /// Vector data (optional)
    pub vector: Option<Vec<f32>>,
    /// Associated metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Distance from query (for debugging)
    pub distance: Option<f32>,
    /// Index path used (for debugging)
    pub index_path: Option<String>,
    /// Collection this result came from
    pub collection_id: Option<String>,
    /// Timestamp when vector was created
    pub created_at: Option<DateTime<Utc>>,
}

impl SearchResult {
    /// Create a basic search result
    pub fn new(id: String, score: f32) -> Self {
        Self {
            id,
            score,
            vector: None,
            metadata: HashMap::new(),
            distance: None,
            index_path: None,
            collection_id: None,
            created_at: None,
        }
    }

    /// Create search result with metadata
    pub fn with_metadata(id: String, score: f32, metadata: HashMap<String, serde_json::Value>) -> Self {
        Self {
            id,
            score,
            vector: None,
            metadata,
            distance: None,
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
    pub fn with_debug_info(mut self, distance: f32, index_path: String) -> Self {
        self.distance = Some(distance);
        self.index_path = Some(index_path);
        self
    }
}

/// Unified index type enum - replaces 4+ duplicates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IndexType {
    /// Dense vector index types
    Vector(VectorIndexType),
    /// Metadata index types
    Metadata(MetadataIndexType),
    /// Hybrid index combining multiple approaches
    Hybrid {
        vector_index: Box<VectorIndexType>,
        metadata_index: Box<MetadataIndexType>,
    },
}

/// Vector-specific index types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VectorIndexType {
    /// Flat (exhaustive) search
    Flat,
    /// Hierarchical Navigable Small World
    Hnsw {
        m: u32,
        ef_construction: u32,
        ef_search: u32,
    },
    /// Inverted File Index
    Ivf {
        nlist: u32,
        nprobe: u32,
    },
    /// Product Quantization
    Pq {
        m: u32,
        nbits: u32,
    },
    /// IVF + Product Quantization
    IvfPq {
        nlist: u32,
        nprobe: u32,
        m: u32,
        nbits: u32,
    },
    /// LSH (Locality Sensitive Hashing)
    Lsh {
        num_tables: u32,
        hash_length: u32,
    },
}

/// Metadata-specific index types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MetadataIndexType {
    /// B-tree for range queries
    BTree,
    /// Hash for equality queries
    Hash,
    /// Bloom filter for existence checks
    BloomFilter,
    /// Full-text search
    FullText,
    /// Inverted index
    Inverted,
}

/// Unified compaction configuration - replaces multiple CompactionConfig variants
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Enable automatic compaction
    pub enabled: bool,
    /// Strategy to use for compaction
    pub strategy: CompactionStrategy,
    /// Trigger threshold (ratio of deleted/total data)
    pub trigger_threshold: f32,
    /// Maximum parallelism for compaction
    pub max_parallelism: usize,
    /// Target file size after compaction
    pub target_file_size: u64,
    /// Minimum files to trigger compaction
    pub min_files_to_compact: usize,
    /// Maximum files to compact in one operation
    pub max_files_to_compact: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strategy: CompactionStrategy::SizeTiered,
            trigger_threshold: 0.5,
            max_parallelism: 2,
            target_file_size: 64 * 1024 * 1024, // 64MB
            min_files_to_compact: 3,
            max_files_to_compact: 10,
        }
    }
}

/// Compaction strategies
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CompactionStrategy {
    /// Size-tiered compaction (merge files of similar size)
    SizeTiered,
    /// Level-based compaction (LSM-tree style)
    Leveled,
    /// Universal compaction (merge all files)
    Universal,
    /// Adaptive based on workload patterns
    Adaptive,
}

/// Distance metrics for vector similarity
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DistanceMetric {
    /// Cosine similarity (1 - cosine_distance)
    Cosine,
    /// Euclidean (L2) distance
    Euclidean,
    /// Manhattan (L1) distance
    Manhattan,
    /// Dot product similarity
    DotProduct,
    /// Hamming distance (for binary vectors)
    Hamming,
}

impl Default for DistanceMetric {
    fn default() -> Self {
        Self::Cosine
    }
}

/// Storage engine types - unified storage identifiers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StorageEngine {
    /// VIPER (Vector-optimized Parquet) engine
    Viper,
    /// LSM (Log-Structured Merge) engine
    Lsm,
    /// Memory-mapped file engine
    Mmap,
    /// Hybrid approach combining multiple engines
    Hybrid,
}

impl Default for StorageEngine {
    fn default() -> Self {
        Self::Viper
    }
}