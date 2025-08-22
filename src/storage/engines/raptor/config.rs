use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    
    // Index settings
    pub enable_hnsw: bool,
    pub hnsw_m: usize,
    pub hnsw_ef_construction: usize,
    pub hnsw_ef_search: usize,
    
    // Metadata settings
    pub enable_complex_types: bool,
    pub enable_bloom_filters: bool,
    pub bloom_fpp: f64,
    pub enable_statistics: bool,
    
    // Vector settings
    pub vector_dimension: Option<usize>,  // Deprecated - use dimension
    pub dimension: usize,  // Required dimension from collection config
    
    // Compaction settings
    pub compaction_threshold_files: usize,
    pub compaction_min_size_mb: usize,
    pub enable_hnsw_aware_compaction: bool,
    pub compaction_config: Option<CompactionConfig>,
    pub hnsw_config: Option<HnswConfig>,
    
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
pub struct HnswConfig {
    pub num_entry_points: usize,
    pub max_connections: usize,
    pub ef_construction: usize,
    pub ef_search: usize,
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
            rowgroup_size: 1000,
            
            // Compression optimized for vector data:
            // - Zstd level 3 gives 2-3x compression with fast decompression
            // - Applied per-column for selective decompression
            // - Graph edges use dictionary encoding
            compression: CompressionCodec::Zstd(3),
            compression_level: 3,
            use_fastlanes_encoding: true,  // Enable FastLanes for SIMD-optimized encoding
            
            enable_simd: true,
            simd_lanes: 16,
            
            enable_range_reads: true,
            prefetch_size_mb: 32,
            cache_size_mb: 1024,
            cache_eviction_policy: EvictionPolicy::Cost,
            
            // HNSW configuration for hybrid global+local graphs:
            // - M=16: Balanced connectivity (16 connections per node)
            // - ef_construction=200: High quality graph building
            // - ef_search=100: Fast approximate search
            // These work with both global graph and local rowgroup subgraphs
            enable_hnsw: true,
            hnsw_m: 16,
            hnsw_ef_construction: 200,
            hnsw_ef_search: 100,
            
            enable_complex_types: true,
            enable_bloom_filters: true,
            bloom_fpp: 0.01,
            enable_statistics: true,
            
            // Vector settings
            vector_dimension: None,  // Deprecated
            dimension: 768,  // Default to common embedding dimension, will be overridden // Will be determined from data
            
            // Aggressive compaction for HNSW graph consistency:
            // - Trigger at 2 files to maintain single navigable graph
            // - No size threshold - compact immediately
            // - Single level (L0) to avoid graph fragmentation
            // - Supports 100GB+ files through columnar streaming
            compaction_threshold_files: 2,  // Compact immediately when we have 2 files
            compaction_min_size_mb: 1,       // Compact even small files to maintain single graph
            enable_hnsw_aware_compaction: true,
            compaction_config: Some(CompactionConfig {
                max_level: 0,                 // Only L0 for RAPTOR (single level)
                l0_trigger_file_count: 2,     // Trigger when we have 2+ files
                target_file_size: usize::MAX, // Single large file (100GB+ supported)
            }),
            hnsw_config: Some(HnswConfig {
                num_entry_points: 5,
                max_connections: 32,
                ef_construction: 200,
                ef_search: 100,
            }),
            
            max_parallel_reads: 8,
            buffer_pool_size_mb: 512,
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