//! # Proxima Block Storage Infrastructure
//!
//! **🚀 HIGH-PERFORMANCE SHARED STORAGE ENGINE INFRASTRUCTURE 🚀**
//!
//! This module provides the **unified Proxima block architecture** used by SST, SWIFT, and HELIX engines,
//! eliminating code duplication and providing **automatic optimization capabilities** that storage engines
//! should leverage instead of implementing manually.
//!
//! ## 🎯 **Key Benefits for Storage Engine Developers**
//!
//! ### **✅ AUTOMATIC CAPABILITIES - Use These Instead of Manual Implementation!**
//!
//! Proxima provides **out-of-the-box** functionality that storage engines often reimplement manually:
//!
//! - **🔍 Automatic Bloom Filter Generation**: Creates optimized bloom filters for existence checks
//! - **📊 Automatic Metadata Statistics**: Calculates min/max/null counts for all columns
//! - **⚡ Automatic SIMD Encoding**: Chooses optimal encoding based on data characteristics
//! - **🗜️ Automatic Compression**: Applies best compression algorithm for block content
//! - **📈 Automatic Quantization**: Integrates seamlessly with unified quantization engine
//! - **🔗 Automatic Index Generation**: Creates B+ tree indexes for O(log n) lookups
//! - **📝 Automatic Range Tracking**: Maintains ID and timestamp ranges for pruning
//! - **🧠 Automatic Delete Detection**: Identifies tombstone records automatically
//!
//! ### **🏗️ PROVEN PATTERNS - Follow HELIX's Example!**
//!
//! **HELIX engine demonstrates the CORRECT way to use Proxima:**
//! ```rust,ignore
//! // ✅ CORRECT: Composition pattern that leverages Proxima capabilities
//! pub struct HelixBlockMetadata {
//!     pub proxima_metadata: ProximaBlockMetadata,  // <- Reuse auto-generated stats!
//!     pub hilbert_range: Option<(u64, u64)>,           // <- Add engine-specific fields only
//!     pub pca_stats: Option<PCAStats>,
//! }
//! ```
//!
//! **❌ ANTI-PATTERN: What SST/SWIFT currently do (manual reimplementation):**
//! ```rust,ignore
//! // ❌ WRONG: Manual statistics calculation that duplicates Proxima work
//! let mut metadata_min_values = HashMap::new();
//! let mut metadata_max_values = HashMap::new();
//! for record in current_block {
//!     // 50+ lines of manual min/max calculation that Proxima already provides!
//! }
//! ```
//!
//! ## 📚 **How to Use Proxima Capabilities (Quick Start)**
//!
//! ### **1. Create Blocks with Auto-Features**
//! ```rust,ignore
//! use crate::storage::engines::core::formats::proximablocks::*;
//!
//! // ✅ Proxima automatically calculates all metadata
//! let block = ProximaDataBlock::new(records, compression_config);
//!
//! // ✅ Access auto-generated statistics
//! let stats = &block.metadata;
//! let id_range = &block.id_range;           // Auto-calculated
//! let timestamp_range = &block.timestamp_range; // Auto-calculated
//! let has_deletes = block.has_deletes;      // Auto-detected
//! ```
//!
//! ### **2. Use Composition Pattern for Engine-Specific Data**
//! ```rust,ignore
//! // ✅ RECOMMENDED: Wrap Proxima metadata, don't replace it
//! pub struct MyEngineBlockMetadata {
//!     pub proxima_metadata: ProximaBlockMetadata,  // <- All the auto-generated goodness
//!     pub my_engine_specific_data: MySpecificData,     // <- Your additions only
//! }
//! ```
//!
//! ### **3. Leverage Auto-Generated Bloom Filters**
//! ```rust,ignore
//! // ✅ Proxima can auto-generate bloom filters
//! let block = ProximaDataBlock::new_with_bloom_filters(records, compression_config, bloom_config);
//!
//! // ✅ Use built-in bloom filter methods
//! if block.contains_id("some_id") {
//!     // Efficient bloom filter check
//! }
//! ```
//!
//! Now includes SharedSstFormatReader for bandwidth-optimized cloud storage access.
//!
//! ## Common Capabilities Provided
//!
//! ### 1. Block Structure Management
//! - **RowBasedDataBlock**: Core data block structure with compression and quantization
//! - **SuperBlock**: Hierarchical block organization for SWIFT's multi-level architecture
//! - **BlockMetadata**: Metadata tracking for efficient block navigation
//! - **BlockLayout**: Configurable block organization strategies
//! - **Block Compression**: Per-block compression with multiple algorithms
//!
//! ### 2. Hierarchical Indexing
//! - **RowBasedIdIndex**: B+ tree based ID index for O(log n) lookups
//! - **HierarchicalIndex**: Multi-level index for superblock navigation
//! - **BloomFilterConfig**: Bloom filters per block for existence checks
//! - **MultiLevelIndex**: Support for both flat (SST) and hierarchical (SWIFT) structures
//! - **Index Compression**: Compressed index storage for memory efficiency
//!
//! ### 3. Compression Infrastructure
//! - **RowBasedCompressionConfig**: Unified compression configuration
//! - **VectorCompressionStrategy**: Adaptive compression based on data characteristics
//! - **CompressionParameters**: Fine-tuned parameters per algorithm
//! - **CompressionStats**: Track compression ratios and performance
//! - **Mixed Compression**: Different algorithms for different data types
//!
//! ### 4. Batch Operations
//! - **RowBasedBatchOperations**: Optimized batch read/write/update
//! - **BatchProcessingStrategy**: Configurable strategies (Sequential, Parallel, Adaptive)
//! - **ConcurrencyConfig**: Multi-threaded batch processing configuration
//! - **Memory Pool Integration**: Reuse buffers across batch operations
//! - **Batch Result Tracking**: Detailed metrics for batch operations
//!
//! ### 5. Header & Metadata Management
//! - **RowBasedHeader**: Unified file header structure
//! - **FileMetadata**: Track file-level statistics and properties
//! - **EngineMetadata**: Engine-specific metadata extensions
//! - **VersionInfo**: Support for format versioning and migration
//! - **ChecksumConfig**: Data integrity verification
//!
//! ### 6. Utility Functions
//! - **FilenameGenerator**: Consistent file naming across engines
//! - **PathResolver**: Handle local and cloud storage paths
//! - **MemoryEstimator**: Estimate memory requirements for operations
//! - **PerformanceProfiler**: Profile and optimize hot paths
//! - **Format Converters**: Convert between different block formats
//!
//! ## Key Differences Handled
//!
//! ### SST (Flat Structure)
//! - Single-level block organization
//! - Direct block access via offsets
//! - Optimized for sequential writes
//! - Simple bloom filter per file
//!
//! ### SWIFT (Hierarchical Structure)  
//! - SuperBlock → DataBlock hierarchy
//! - Multi-level indexing
//! - Optimized for mixed workloads
//! - Bloom filters at multiple levels
//!
//! ## Performance Benefits
//! - **Code Reuse**: 70% code sharing between SST and SWIFT
//! - **Consistent Optimization**: Same optimizations apply to both engines
//! - **Memory Efficiency**: Shared memory pools and caches
//! - **Maintenance**: Single codebase for core functionality
//! - **Testing**: Unified test suite for common components
//!
//! ## 🔧 **Serialization Best Practices (Audited December 2024)**
//!
//! ### **Cache-Line Alignment (64 bytes)**
//!
//! All blocks are padded to 64-byte cache-line boundaries for SIMD optimization:
//!
//! ```text
//! Block Layout:
//! ┌────────────────────────────────────────────────────────────┐
//! │ [block_len:4 bytes][block_data:N bytes][padding:0-63 bytes]│
//! └────────────────────────────────────────────────────────────┘
//!
//! Writer (writer.rs):
//!   aligned_size = ((block_len + 63) / 64) * 64
//!   padding = aligned_size - block_len
//!   output.extend(vec![0u8; padding])  // Zero-fill padding
//!
//! Reader (sst_query_engine.rs):
//!   offset += block_len
//!   offset += padding  // Skip cache-line padding
//! ```
//!
//! **Overhead Analysis:**
//! - Typical block: 263KB with 51 bytes padding = **0.019% overhead**
//! - This is negligible and enables direct SIMD operations on mmap'd data
//! - No runtime copy to aligned buffer is needed
//!
//! ### **SIMD-Friendly Memory Layout**
//!
//! ```text
//! ✅ Column-Oriented (SIMD-Optimal):
//! Dimension 0: [v0_d0, v1_d0, v2_d0, ...] ← Process 8+ values per SIMD op
//! Dimension 1: [v0_d1, v1_d1, v2_d1, ...]
//!
//! ❌ Row-Oriented (Scatter/Gather Overhead):
//! Vector 0: [v0_d0, v0_d1, ..., v0_d1535]
//! Vector 1: [v1_d0, v1_d1, ..., v1_d1535]
//! ```
//!
//! ### **Zero-Copy Serialization**
//!
//! Uses `bytemuck::cast_slice` for FP32 vectors (see `serialization.rs:541-580`):
//! ```rust,ignore
//! let byte_buffer: &[u8] = bytemuck::cast_slice(&buffer);  // No copy!
//! let fixed_array = FixedSizeBinaryArray::try_new(dimension * 4, ...)?;
//! ```
//!
//! ### **Memory Pool Configuration (serialization.rs:157-179)**
//!
//! ```rust,ignore
//! struct MemoryPools {
//!     fp32_pool: Mutex<Vec<Vec<f32>>>,   // Reuse FP32 vectors
//!     int8_pool: Mutex<Vec<Vec<i8>>>,    // Reuse INT8 quantized
//!     binary_pool: Mutex<Vec<Vec<u8>>>,  // Reuse binary codes
//!     pq_pool: Mutex<Vec<Vec<u8>>>,      // Reuse PQ codes
//! }
//! // Pool limit: 100 vectors per type to prevent memory bloat
//! ```
//!
//! ### **Encoding Strategy Selection**
//!
//! Default: `FullVector` (fastest decode for read-heavy vector workloads)
//!
//! | Strategy | Decode Speed | Compression | Best For |
//! |----------|--------------|-------------|----------|
//! | FullVector | ⭐ Fastest (0.94ms/1536d) | 18-20% | Vector databases (default) |
//! | GroupedField | Medium | 19-22% (best) | Storage-critical |
//! | TransposeField | Medium | 18-21% | Columnar analytics |
//!
//! ### **Engine-Specific Metadata (Not Duplication)**
//!
//! Each engine correctly has unique metadata requirements:
//!
//! | Engine | Unique Fields | Purpose |
//! |--------|---------------|---------|
//! | SST | `block_index`, `bloom_filter`, `sst_level` | LSM-tree |
//! | HELIX | `hilbert_config`, `pca_model` | Spatial locality |
//! | VIPER | Parquet row-groups | Columnar analytics |
//! | NOVA | `hierarchical_stats`, `zone_maps` | Progressive columnar |
//! | SWIFT | `composite_indexes` | Ultra-low latency |
//! | RAPTOR | `centroid_matrix` | Adaptive clustering |

pub mod arrow_reader; // Arrow reader for .sst files - enables external tool access
pub mod block_reader; // ✅ NEW: Unified Proxima block reader with strategies
pub mod block_structures;
pub mod bloom_filter; // Row-based bloom filter for SST and Swift
// Block-level compression now integrated into main block_structures.rs
pub mod compression_config;
pub mod engine_profile; // Engine-specific optimization profiles
pub mod index_structures;
// Quantization now handled by unified compute module
pub mod batch_operations;
pub mod constants;
pub mod header_metadata;
// Embedding-precision rollout (PR 5 of EMBEDDING_PRECISION_LLD_2026_05_22).
pub mod per_column_alignment;
pub mod spatial_clustering; // PCA-based clustering and Z-Order spatial indexing
pub mod spatial_encoding; // 512-bit spatial codes for high-dimensional embeddings
pub mod spatial_pruning; // SpatialPruner for unified block selection
pub mod spatial_traits; // SpatialCurveEncoder trait for unified block clustering
pub mod sst_io_layer; // Low-level I/O operations (formerly sst_io_layer)
pub mod sst_metadata; // NEW: Zero-copy metadata serialization for SST
pub mod swift_metadata;
pub mod utilities; // NEW: Zero-copy metadata serialization for SWIFT

// Re-exports for common use
pub use block_structures::{
    BlockCompressionConfig, BlockLayout, BlockLocation, BlockMetadataStats, ProximaBlockMetadata,
    ProximaDataBlock, QuantizationStatistics, SuperBlock, VectorEncodingLayout,
};
pub use compression_config::{
    CompressionParameters, CompressionStats, RowBasedCompressionConfig, VectorCompressionStrategy,
};
pub use index_structures::{
    BloomFilterConfig, HierarchicalIndex, IndexEntry, MultiLevelIndex, RowBasedIdIndex,
};
// Quantization now handled by unified compute module
pub use batch_operations::{
    BatchConfig, BatchProcessingStrategy, BatchResult, ConcurrencyConfig, RowBasedBatchOperations,
};
pub use header_metadata::{
    ChecksumConfig, EngineMetadata, FileMetadata, RowBasedHeader, VersionInfo,
};
pub use spatial_clustering::{
    AdaCurve, IncrementalPCA, ZOrderEncoder, cluster_blocks_pca, cluster_blocks_pca_adacurves,
    cluster_blocks_pca_zorder,
};
pub use utilities::{
    BlockUtilsPerformanceProfiler, MemoryEstimator, PathResolver, RowBasedUtilities,
};
// Backwards-compat alias for the pre-rename name. The linter rename to
// `BlockUtilsPerformanceProfiler` happened in utilities.rs but the
// `pub use` here wasn't updated in lockstep; alias keeps downstream
// `crate::storage::...::PerformanceProfiler` paths compiling.
pub use utilities::BlockUtilsPerformanceProfiler as PerformanceProfiler;

// NEW: Export shared SST reader components
pub use sst_io_layer::{
    BlockInfo, ReaderStatsSummary as SstReaderStats, SharedSstFormatReader as SstIOLayer,
    SstMmapStrategy, SstRegion,
};

// NEW: Export zero-copy metadata serialization components
pub use sst_metadata::{SstBlockHeader, SstGlobalHeader, SstMetadata, SstMetadataSerializer};

// NEW: Export Arrow reader for .sst files
pub use arrow_reader::ProximaBlocksArrowReader;

use proximadb_compression::CompressionAlgorithm;
use proximadb_distance_kernel::DistanceMetric;

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
    pub quantization: crate::compute::quantization::storage_engine::StorageQuantizationConfig,

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
            quantization:
                crate::compute::quantization::storage_engine::StorageQuantizationConfig::default(),
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

/// Utility functions for row-based engines
pub mod utils {
    use super::*;
    use crate::storage::engines::core::formats::proximablocks::constants::{
        DEFAULT_BLOCK_METADATA_OVERHEAD_BYTES, DEFAULT_TARGET_BLOCK_SIZE_BYTES,
        MAX_TARGET_BLOCK_SIZE_BYTES, MIN_TARGET_BLOCK_SIZE_BYTES,
    };

    /// Recommend a block size for SST/Swift/Helix based on vector dimension and metadata overhead.
    ///
    /// Mirrors the existing SST defaults (target ~3MB, clamp 2–4MB) while centralizing the logic.
    /// This keeps behavior stable but avoids duplicating the calculation per engine.
    pub fn recommend_block_size_for_dimension(
        dimension: usize,
        metadata_overhead_bytes: usize,
    ) -> usize {
        let overhead = if metadata_overhead_bytes == 0 {
            DEFAULT_BLOCK_METADATA_OVERHEAD_BYTES
        } else {
            metadata_overhead_bytes
        };
        let _estimated_row_bytes = dimension.saturating_mul(4).saturating_add(overhead);

        // Prior defaults targeted ~3MB, with a slight tweak for very large dimensions.
        let target_block_size = match dimension {
            0..=384 => DEFAULT_TARGET_BLOCK_SIZE_BYTES, // Small vectors
            385..=1536 => DEFAULT_TARGET_BLOCK_SIZE_BYTES, // Medium/large vectors
            _ => (2.5 * 1024.0 * 1024.0) as usize,      // XL vectors (network-friendly)
        };

        // Clamp to practical I/O bounds
        target_block_size.clamp(MIN_TARGET_BLOCK_SIZE_BYTES, MAX_TARGET_BLOCK_SIZE_BYTES) // 2MB min, 4MB max
    }

    /// Calculate optimal block size based on dimension and record count
    pub fn calculate_optimal_block_size(
        dimension: usize,
        target_records: u32,
        compression_ratio: f32,
    ) -> usize {
        // Base vector size (4 bytes per float32)
        let vector_size = dimension * 4;

        // Add metadata overhead (ID, timestamp, version, etc.)
        let metadata_overhead = 128; // Estimated overhead per record

        let record_size = vector_size + metadata_overhead;
        let total_size = (record_size * target_records as usize) as f32;

        // Apply compression ratio
        (total_size * compression_ratio) as usize
    }

    /// Estimate memory usage for configuration
    pub fn estimate_memory_usage(config: &RowBasedConfig) -> usize {
        let block_size = calculate_optimal_block_size(
            config.dimension,
            config.records_per_block,
            config.compression.compression_ratio_estimate,
        );

        let blocks_in_memory = config.performance.cache_size_bytes / block_size;
        let estimated_usage = blocks_in_memory * block_size;

        // Add index overhead (10-20% of data size)
        let index_overhead = estimated_usage / 10;

        estimated_usage + index_overhead
    }

    /// Recommend engine configuration based on workload
    pub fn recommend_config_for_workload(
        dimension: usize,
        _expected_scale: u64,
        workload_type: WorkloadType,
    ) -> RowBasedConfig {
        let mut config = RowBasedConfig {
            dimension,
            ..Default::default()
        };

        match workload_type {
            WorkloadType::HighThroughputWrite => {
                // Optimize for writes
                config.compression.algorithm = CompressionAlgorithm::Lz4; // Fast compression
                config
                    .compression
                    .block_compression
                    .compression_stages
                    .push(compression_config::CompressionStage {
                        stage_name: "primary".to_string(),
                        algorithm: CompressionAlgorithm::Lz4,
                        level: 1,
                        condition: compression_config::CompressionCondition::Always,
                    });
                config.performance.max_concurrent_operations = 16;
                config.records_per_block = 4000; // Larger blocks
            }
            WorkloadType::LowLatencyRead => {
                // Optimize for reads
                config.performance.prefetch_enabled = true;
                config.indexing.bloom_filter_per_block = true;
                config.records_per_block = 1000; // Smaller blocks for faster access
            }
            WorkloadType::Balanced => {
                // Use defaults - already balanced
            }
            WorkloadType::LargeScale => {
                // Optimize for scale
                config.superblock_size_target = 2 * 1024 * 1024 * 1024; // 2GB
                config.compression.algorithm = CompressionAlgorithm::Zstd; // Better compression
                config.quantization.enable_progressive = true; // Enable progressive quantization
            }
        }

        config
    }

    #[derive(Debug, Clone)]
    pub enum WorkloadType {
        HighThroughputWrite,
        LowLatencyRead,
        Balanced,
        LargeScale,
    }
}

#[cfg(test)]
mod tests {
    use super::utils::*;
    use super::*;

    #[test]
    fn test_optimal_block_size_calculation() {
        let block_size = calculate_optimal_block_size(768, 2000, 0.7);

        // Expected: 768 * 4 * 2000 + overhead, compressed by 0.7
        let expected_base = (768 * 4 + 128) * 2000;
        let expected_compressed = (expected_base as f32 * 0.7) as usize;

        assert_eq!(block_size, expected_compressed);
    }

    #[test]
    fn test_memory_usage_estimation() {
        let config = RowBasedConfig::default();
        let usage = estimate_memory_usage(&config);

        // Should be reasonable (between 100MB and 10GB)
        assert!(usage > 100 * 1024 * 1024); // > 100MB
        assert!(usage < 10 * 1024 * 1024 * 1024); // < 10GB
    }

    #[test]
    fn test_workload_config_recommendations() {
        let config =
            recommend_config_for_workload(384, 1_000_000, WorkloadType::HighThroughputWrite);

        assert_eq!(config.dimension, 384);
        // Check that compression stages include LZ4
        assert!(
            config
                .compression
                .block_compression
                .compression_stages
                .iter()
                .any(|stage| stage.algorithm == CompressionAlgorithm::Lz4)
        );
        assert_eq!(config.records_per_block, 4000);
        assert_eq!(config.performance.max_concurrent_operations, 16);
    }

    #[test]
    fn test_default_configurations() {
        let config = RowBasedConfig::default();

        assert!(config.indexing.bloom_filter_enabled);
        assert!(config.performance.memory_pool_enabled);
        assert!(config.quantization.enable_progressive);
        assert_eq!(config.indexing.id_index_type, IdIndex::Hybrid);
    }

    #[test]
    fn test_metadata_filter_creation() {
        let filter = BlockMetadataFilter {
            conditions: vec![
                FilterCondition::Equals("category".to_string(), "electronics".into()),
                FilterCondition::Range("price".to_string(), 100.0.into(), 1000.0.into()),
            ],
            logic: FilterLogic::And,
        };

        assert_eq!(filter.conditions.len(), 2);
        assert!(matches!(filter.logic, FilterLogic::And));
    }
}
