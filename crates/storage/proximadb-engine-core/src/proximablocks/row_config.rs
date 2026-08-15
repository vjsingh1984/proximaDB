// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Row-based engine configuration types (RowBasedConfig, BlockIndexConfiguration,
//! PerformanceConfiguration, search/filter modes, capabilities trait) — hoisted from
//! root `formats/proximablocks/mod.rs` (TD-DECOMP-72).

use proximadb_compression::CompressionAlgorithm;
use proximadb_distance_kernel::DistanceMetric;
use proximadb_quantization_model::StorageQuantizationConfig;

use crate::proximablocks::compression_config::RowBasedCompressionConfig;

/// Common configuration for row-based storage engines
#[derive(Debug, Clone)]
pub struct RowBasedConfig {
    /// Engine identification
    pub engine_name: String,
    pub engine_version: String,

    /// Storage configuration
    pub dimension: usize,
    pub distance_metric: DistanceMetric,
    pub collection_id: String,

    /// Block organization
    pub records_per_block: u32,
    pub blocks_per_superblock: u32,
    pub superblock_size_target: u64, // Target size in bytes

    /// Compression configuration
    pub compression: RowBasedCompressionConfig,

    /// Quantization configuration
    pub quantization: StorageQuantizationConfig,

    /// Index configuration
    pub indexing: BlockIndexConfiguration,

    /// Performance tuning
    pub performance: PerformanceConfiguration,
}

/// Backwards-compat alias for [`BlockIndexConfiguration`].
pub type IndexConfiguration = BlockIndexConfiguration;

/// Index configuration shared between SST and SWIFT
#[derive(Debug, Clone)]
pub struct BlockIndexConfiguration {
    /// Bloom filter settings
    pub bloom_filter_enabled: bool,
    pub bloom_filter_false_positive_rate: f64,
    pub bloom_filter_per_block: bool,

    /// ID index settings
    pub id_index_type: IdIndex,
    pub id_index_compression: bool,

    /// Hierarchical indexing
    pub enable_hierarchical_index: bool,
    pub index_levels: u8,

    /// Metadata indexing
    pub enable_metadata_index: bool,
    pub filterable_columns: Vec<String>,
}

/// Type of ID indexing strategy
#[derive(Debug, Clone, PartialEq)]
pub enum IdIndex {
    /// B+ tree for sorted access
    BTree,
    /// Hash map for O(1) lookup
    HashMap,
    /// Hybrid approach (B+ tree + hash)
    Hybrid,
    /// Dense array for sequential IDs
    Dense,
}

/// Performance configuration
#[derive(Debug, Clone)]
pub struct PerformanceConfiguration {
    /// Memory management
    pub memory_pool_enabled: bool,
    pub max_memory_per_operation: usize,
    pub cache_size_bytes: usize,

    /// Concurrency settings
    pub max_concurrent_operations: usize,
    pub batch_size_optimization: bool,

    /// I/O optimization
    pub prefetch_enabled: bool,
    pub async_io_enabled: bool,
    pub io_buffer_size: usize,

    /// Hardware acceleration
    pub simd_enabled: bool,
    pub hardware_detection: bool,
}

/// Search mode for row-based engines
#[derive(Debug, Clone)]
pub enum RowBasedSearchMode {
    /// AXIS returns IDs, lookup full vectors
    IndexDriven {
        ids: Vec<String>,
        include_vectors: bool,
    },

    /// Full similarity search without AXIS
    IndexFree {
        query: Vec<f32>,
        top_k: usize,
        filter: Option<BlockMetadataFilter>,
    },

    /// Hybrid mode - combine AXIS with local refinement
    Hybrid {
        axis_ids: Vec<String>,
        query: Vec<f32>,
        rerank_factor: f32,
        local_search_k: usize,
    },
}

/// Backwards-compat alias for [`BlockMetadataFilter`].
pub type MetadataFilter = BlockMetadataFilter;

/// Metadata filter for queries
#[derive(Debug, Clone)]
pub struct BlockMetadataFilter {
    pub conditions: Vec<FilterCondition>,
    pub logic: FilterLogic,
}

#[derive(Debug, Clone)]
pub enum FilterLogic {
    And,
    Or,
    Not,
}

#[derive(Debug, Clone)]
pub enum FilterCondition {
    Equals(String, serde_json::Value),
    NotEquals(String, serde_json::Value),
    Range(String, serde_json::Value, serde_json::Value),
    In(String, Vec<serde_json::Value>),
    NotIn(String, Vec<serde_json::Value>),
    IsNull(String),
    IsNotNull(String),
    Contains(String, String),
    StartsWith(String, String),
    EndsWith(String, String),
}

/// Operation statistics
#[derive(Debug, Clone)]
pub struct OperationStats {
    pub records_processed: u64,
    pub bytes_processed: u64,
    pub duration_ms: u64,
    pub memory_peak: usize,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub compression_ratio: f32,
    pub quantization_savings: f32,
}

/// Engine capabilities shared between SST and SWIFT
pub trait RowBasedEngineCapabilities {
    /// Get engine configuration
    fn get_config(&self) -> &RowBasedConfig;

    /// Supports dual-mode operation (ID lookup + similarity search)
    fn supports_dual_mode(&self) -> bool {
        true
    }

    /// Supports progressive search refinement
    fn supports_progressive_search(&self) -> bool {
        true
    }

    /// Supports quantization for memory savings
    fn supports_quantization(&self) -> bool {
        true
    }

    /// Supports hierarchical block structure
    fn supports_hierarchical_blocks(&self) -> bool {
        true
    }

    /// Get supported distance metrics
    fn supported_distance_metrics(&self) -> Vec<DistanceMetric>;

    /// Get supported compression algorithms
    fn supported_compression_algorithms(&self) -> Vec<CompressionAlgorithm>;
}

impl Default for RowBasedConfig {
    fn default() -> Self {
        Self {
            engine_name: "row_based".to_string(),
            engine_version: "1.0.0".to_string(),
            dimension: 768,
            distance_metric: DistanceMetric::Cosine,
            collection_id: "default".to_string(),
            records_per_block: 2000,
            blocks_per_superblock: 64,
            superblock_size_target: 1024 * 1024 * 1024, // 1GB
            compression: RowBasedCompressionConfig::default(),
            quantization: StorageQuantizationConfig::default(),
            indexing: BlockIndexConfiguration::default(),
            performance: PerformanceConfiguration::default(),
        }
    }
}

impl Default for BlockIndexConfiguration {
    fn default() -> Self {
        Self {
            bloom_filter_enabled: true,
            bloom_filter_false_positive_rate: 0.01, // 1%
            bloom_filter_per_block: true,
            id_index_type: IdIndex::Hybrid,
            id_index_compression: true,
            enable_hierarchical_index: true,
            index_levels: 3,
            enable_metadata_index: true,
            filterable_columns: vec![
                "category".to_string(),
                "timestamp".to_string(),
                "version".to_string(),
            ],
        }
    }
}

impl Default for PerformanceConfiguration {
    fn default() -> Self {
        Self {
            memory_pool_enabled: true,
            max_memory_per_operation: 512 * 1024 * 1024, // 512MB
            cache_size_bytes: 1024 * 1024 * 1024,        // 1GB
            max_concurrent_operations: 8,
            batch_size_optimization: true,
            prefetch_enabled: true,
            async_io_enabled: true,
            io_buffer_size: 64 * 1024, // 64KB
            simd_enabled: true,
            hardware_detection: true,
        }
    }
}
