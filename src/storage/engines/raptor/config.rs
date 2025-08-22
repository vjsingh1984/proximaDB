use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use super::constants;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorConfig {
    // Storage settings
    pub rowgroup_size: usize,
    pub compression: CompressionCodec,
    pub compression_level: u32,
    pub use_fastlanes_encoding: bool,  // Enable FastLanes SIMD encoding
    
    // SIMD settings
    pub enable_simd: bool,
    pub simd_lanes: usize,
    
    // Cloud I/O settings
    pub enable_range_reads: bool,
    pub prefetch_size_mb: usize,
    pub cache_size_mb: usize,
    pub cache_eviction_policy: EvictionPolicy,
    
    // IVF clustering settings for RAPTOR's p²+k×p algorithm
    pub enable_clustering: bool,
    pub num_clusters: Option<usize>,  // k value, defaults to √n if not specified
    pub target_rowgroup_size: Option<usize>,  // p value, auto-calculated if not specified
    pub use_component_boosting: bool,  // Enable distance component boosting
    
    // Metadata settings
    pub enable_complex_types: bool,
    pub enable_bloom_filters: bool,
    pub bloom_fpp: f64,
    pub enable_statistics: bool,
    
    // Vector settings
    pub dimension: usize,  // Required dimension from collection config
    
    // Compaction settings
    pub compaction_threshold_files: usize,
    pub compaction_min_size_mb: usize,
    pub enable_clustering_aware_compaction: bool,
    pub compaction_config: Option<CompactionConfig>,
    pub clustering_config: Option<ClusteringConfig>,
    
    // Performance settings
    pub max_parallel_reads: usize,
    pub buffer_pool_size_mb: usize,
    pub enable_prefetching: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionCodec {
    None,
    Lz4,
    Zstd(i32), // compression level
    Snappy,
    Gzip(u32), // compression level
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvictionPolicy {
    Lru,
    Lfu,
    Arc,
    Cost, // Cost-aware eviction based on I/O cost
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    pub max_level: usize,              // For RAPTOR: always 0 (single level)
    pub l0_trigger_file_count: usize,  // For RAPTOR: 2 (compact when > 1 file)
    pub target_file_size: usize,       // For RAPTOR: usize::MAX (single file)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusteringConfig {
    pub num_clusters: usize,  // k value in p²+k×p
    pub rowgroup_size: usize,  // p value in p²+k×p
    pub boosting_alpha_own: f32,  // α₁ weight for own centroid
    pub boosting_alpha_inter: f32,  // α₂ weight for inter-centroid
    pub boosting_alpha_variance: f32,  // α₃ weight for variance
}

impl Default for RaptorConfig {
    fn default() -> Self {
        // Smart defaults optimized for HNSW + Columnar architecture
        Self {
            // RowGroup size optimized for typical k<10 queries:
            // - 1000 vectors balances I/O efficiency vs wasted reads
            // - At k=10, worst case reads 1000 vectors for 10 results (1% efficiency)
            // - At k=100, may need 2-3 rowgroups (acceptable)
            // - Memory: ~4MB per rowgroup @ 1024-dim (fits in L3 cache)
            // - HNSW local graph: ~16K edges (1000 nodes * 16 connections)
            // - Sweet spot: minimizes wasted I/O while maintaining locality
            rowgroup_size: constants::clustering::DEFAULT_ROWGROUP_SIZE,
            
            // Compression optimized for vector data:
            // - Zstd level 3 gives 2-3x compression with fast decompression
            // - Applied per-column for selective decompression
            // - Graph edges use dictionary encoding
            compression: CompressionCodec::Zstd(constants::compression::DEFAULT_ZSTD_LEVEL),
            compression_level: constants::compression::DEFAULT_ZSTD_LEVEL as u32,
            use_fastlanes_encoding: true,  // Enable FastLanes for SIMD-optimized encoding
            
            enable_simd: true,
            simd_lanes: constants::memory::DEFAULT_SIMD_LANES,
            
            enable_range_reads: true,
            prefetch_size_mb: constants::memory::DEFAULT_PREFETCH_SIZE_MB,
            cache_size_mb: constants::memory::DEFAULT_CACHE_SIZE_MB,
            cache_eviction_policy: EvictionPolicy::Cost,
            
            // IVF clustering for RAPTOR's p²+k×p algorithm
            // Automatically calculates optimal k and p values based on dataset size
            enable_clustering: true,
            num_clusters: None,  // Auto-calculate as √n
            target_rowgroup_size: None,  // Auto-calculate based on L3 cache
            use_component_boosting: true,  // Enable advanced distance components
            
            enable_complex_types: true,
            enable_bloom_filters: true,
            bloom_fpp: constants::compression::DEFAULT_BLOOM_FPP,
            enable_statistics: true,
            
            // Vector settings
            dimension: constants::dimensions::DEFAULT,  // Default to common embedding dimension, will be overridden by collection config
            
            // Aggressive compaction for HNSW graph consistency:
            // - Trigger at 2 files to maintain single navigable graph
            // - No size threshold - compact immediately
            // - Single level (L0) to avoid graph fragmentation
            // - Supports 100GB+ files through columnar streaming
            compaction_threshold_files: constants::io::COMPACTION_THRESHOLD_FILES,
            compaction_min_size_mb: constants::io::COMPACTION_MIN_SIZE_MB,
            enable_clustering_aware_compaction: true,
            compaction_config: Some(CompactionConfig {
                max_level: constants::io::MAX_LSM_LEVEL,
                l0_trigger_file_count: constants::io::COMPACTION_THRESHOLD_FILES,
                target_file_size: constants::io::TARGET_FILE_SIZE,
            }),
            clustering_config: Some(ClusteringConfig {
                num_clusters: constants::clustering::DEFAULT_CLUSTER_COUNT,
                rowgroup_size: constants::clustering::DEFAULT_ROWGROUP_SIZE,
                boosting_alpha_own: constants::boosting::ALPHA_OWN_DEFAULT,
                boosting_alpha_inter: constants::boosting::ALPHA_INTER_DEFAULT,
                boosting_alpha_variance: constants::boosting::ALPHA_VARIANCE_DEFAULT,
            }),
            
            max_parallel_reads: constants::memory::DEFAULT_MAX_PARALLEL_READS,
            buffer_pool_size_mb: constants::memory::DEFAULT_BUFFER_POOL_SIZE_MB,
            enable_prefetching: true,
        }
    }
}

impl RaptorConfig {
    /// Configuration optimized for small k queries (k<10)
    pub fn for_small_k() -> Self {
        let mut config = Self::default();
        config.rowgroup_size = 500;  // Minimize wasted reads
        config.cache_size_mb = 4096; // Cache more rowgroups
        config
    }
    
    /// Configuration optimized for medium k queries (k~100)
    pub fn for_medium_k() -> Self {
        let mut config = Self::default();
        config.rowgroup_size = 2000;  // Balance I/O and efficiency
        config
    }
    
    /// Configuration optimized for large k queries (k>100)
    pub fn for_large_k() -> Self {
        let mut config = Self::default();
        config.rowgroup_size = 5000;  // Maximize throughput
        config.enable_prefetching = true;
        config.prefetch_size_mb = 128;
        config
    }
    
    pub fn for_cloud() -> Self {
        let mut config = Self::default();
        config.enable_range_reads = true;
        config.prefetch_size_mb = 64;
        config.cache_size_mb = 2048;
        config.compression = CompressionCodec::Zstd(6);
        config.rowgroup_size = 1000;  // Default for cloud
        config
    }
    
    pub fn for_local_ssd() -> Self {
        let mut config = Self::default();
        config.enable_range_reads = false;
        config.prefetch_size_mb = 16;
        config.cache_size_mb = 512;
        config.compression = CompressionCodec::Lz4;
        config.max_parallel_reads = 16;
        config
    }
    
    pub fn for_high_performance() -> Self {
        let mut config = Self::default();
        config.rowgroup_size = 5000;
        config.compression = CompressionCodec::None;
        config.enable_simd = true;
        config.simd_lanes = 32;
        config.cache_size_mb = 4096;
        config.max_parallel_reads = 32;
        config.buffer_pool_size_mb = 2048;
        config
    }
}