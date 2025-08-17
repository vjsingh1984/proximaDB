use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorConfig {
    // Storage settings
    pub rowgroup_size: usize,
    pub compression: CompressionCodec,
    pub compression_level: u32,
    
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
    
    // Compaction settings
    pub compaction_threshold_files: usize,
    pub compaction_min_size_mb: usize,
    pub enable_hnsw_aware_compaction: bool,
    
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

impl Default for RaptorConfig {
    fn default() -> Self {
        Self {
            rowgroup_size: 10000,
            compression: CompressionCodec::Zstd(3),
            compression_level: 3,
            
            enable_simd: true,
            simd_lanes: 16,
            
            enable_range_reads: true,
            prefetch_size_mb: 32,
            cache_size_mb: 1024,
            cache_eviction_policy: EvictionPolicy::Cost,
            
            enable_hnsw: true,
            hnsw_m: 16,
            hnsw_ef_construction: 200,
            hnsw_ef_search: 100,
            
            enable_complex_types: true,
            enable_bloom_filters: true,
            bloom_fpp: 0.01,
            enable_statistics: true,
            
            compaction_threshold_files: 5,
            compaction_min_size_mb: 128,
            enable_hnsw_aware_compaction: true,
            
            max_parallel_reads: 8,
            buffer_pool_size_mb: 512,
            enable_prefetching: true,
        }
    }
}

impl RaptorConfig {
    pub fn for_cloud() -> Self {
        let mut config = Self::default();
        config.enable_range_reads = true;
        config.prefetch_size_mb = 64;
        config.cache_size_mb = 2048;
        config.compression = CompressionCodec::Zstd(6);
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