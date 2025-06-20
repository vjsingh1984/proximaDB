// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Indexing strategies for AXIS

use super::MigrationPriority;
use serde::{Deserialize, Serialize};

/// Index strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStrategy {
    /// Primary index type
    pub primary_index_type: IndexType,

    /// Secondary indexes to maintain
    pub secondary_indexes: Vec<IndexType>,

    /// Optimization configuration
    pub optimization_config: OptimizationConfig,

    /// Migration priority
    pub migration_priority: MigrationPriority,

    /// Resource requirements
    pub resource_requirements: ResourceRequirements,
}

impl Default for IndexStrategy {
    fn default() -> Self {
        Self {
            primary_index_type: IndexType::GlobalIdOnly,
            secondary_indexes: vec![IndexType::Metadata],
            optimization_config: OptimizationConfig::default(),
            migration_priority: MigrationPriority::Low,
            resource_requirements: ResourceRequirements::low(),
        }
    }
}

/// Types of indexes available in AXIS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IndexType {
    /// Global ID index only (minimal indexing)
    GlobalIdOnly,

    /// Global ID index (always present)
    GlobalId,

    /// Metadata index for filtering
    Metadata,

    /// Dense vector index (HNSW)
    DenseVector,

    /// Sparse vector index (LSM + MinHash)
    SparseVector,

    /// Join engine for multi-index queries
    JoinEngine,

    /// Lightweight HNSW for small collections
    LightweightHNSW,

    /// Standard HNSW index
    HNSW,

    /// Partitioned HNSW for large collections
    PartitionedHNSW,

    /// Product quantization for compression
    ProductQuantization,

    /// Vector compression
    VectorCompression,

    /// LSM tree for sparse vectors
    LSMTree,

    /// MinHash LSH for sparse similarity
    MinHashLSH,

    /// Inverted index for sparse vectors
    InvertedIndex,

    /// Sparse optimized index
    SparseOptimized,

    /// Basic sparse index
    SparseBasic,

    /// Hybrid index for small collections
    HybridSmall,

    /// Full AXIS deployment
    FullAXIS,
}

/// Optimization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationConfig {
    /// Enable caching
    pub enable_caching: bool,

    /// Cache size in MB
    pub cache_size_mb: usize,

    /// Enable prefetching
    pub enable_prefetching: bool,

    /// Batch size for operations
    pub batch_size: usize,

    /// Enable SIMD optimizations
    pub enable_simd: bool,

    /// Enable GPU acceleration
    pub enable_gpu: bool,

    /// Compression settings
    pub compression: CompressionConfig,
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        Self {
            enable_caching: true,
            cache_size_mb: 1024,
            enable_prefetching: true,
            batch_size: 1000,
            enable_simd: true,
            enable_gpu: false,
            compression: CompressionConfig::default(),
        }
    }
}

impl OptimizationConfig {
    /// Minimal optimization for small collections
    pub fn minimal() -> Self {
        Self {
            enable_caching: true,
            cache_size_mb: 256,
            enable_prefetching: false,
            batch_size: 100,
            enable_simd: false,
            enable_gpu: false,
            compression: CompressionConfig::none(),
        }
    }

    /// Balanced optimization
    pub fn balanced() -> Self {
        Self::default()
    }

    /// High performance optimization
    pub fn high_performance() -> Self {
        Self {
            enable_caching: true,
            cache_size_mb: 4096,
            enable_prefetching: true,
            batch_size: 10000,
            enable_simd: true,
            enable_gpu: true,
            compression: CompressionConfig::fast(),
        }
    }

    /// Sparse optimized configuration
    pub fn sparse_optimized() -> Self {
        Self {
            enable_caching: true,
            cache_size_mb: 2048,
            enable_prefetching: true,
            batch_size: 5000,
            enable_simd: true,
            enable_gpu: false,
            compression: CompressionConfig::high(),
        }
    }

    /// Adaptive configuration
    pub fn adaptive() -> Self {
        Self {
            enable_caching: true,
            cache_size_mb: 2048,
            enable_prefetching: true,
            batch_size: 1000,
            enable_simd: true,
            enable_gpu: false,
            compression: CompressionConfig::adaptive(),
        }
    }
}

/// Compression configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Enable compression
    pub enabled: bool,

    /// Compression algorithm
    pub algorithm: CompressionAlgorithm,

    /// Compression level (1-9)
    pub level: u8,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            algorithm: CompressionAlgorithm::Zstd,
            level: 3,
        }
    }
}

impl CompressionConfig {
    /// No compression
    pub fn none() -> Self {
        Self {
            enabled: false,
            algorithm: CompressionAlgorithm::None,
            level: 0,
        }
    }

    /// Fast compression
    pub fn fast() -> Self {
        Self {
            enabled: true,
            algorithm: CompressionAlgorithm::Lz4,
            level: 1,
        }
    }

    /// High compression
    pub fn high() -> Self {
        Self {
            enabled: true,
            algorithm: CompressionAlgorithm::Zstd,
            level: 6,
        }
    }

    /// Adaptive compression
    pub fn adaptive() -> Self {
        Self {
            enabled: true,
            algorithm: CompressionAlgorithm::Adaptive,
            level: 3,
        }
    }
}

/// Compression algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionAlgorithm {
    None,
    Lz4,
    Zstd,
    Snappy,
    Adaptive,
}

/// Resource requirements for indexing strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRequirements {
    /// Memory requirement in GB
    pub memory_gb: f64,

    /// CPU cores required
    pub cpu_cores: f64,

    /// Disk space in GB
    pub disk_gb: f64,

    /// Network bandwidth in Mbps
    pub network_mbps: f64,
}

impl ResourceRequirements {
    /// Low resource requirements
    pub fn low() -> Self {
        Self {
            memory_gb: 1.0,
            cpu_cores: 1.0,
            disk_gb: 10.0,
            network_mbps: 10.0,
        }
    }

    /// Medium resource requirements
    pub fn medium() -> Self {
        Self {
            memory_gb: 4.0,
            cpu_cores: 2.0,
            disk_gb: 50.0,
            network_mbps: 100.0,
        }
    }

    /// High resource requirements
    pub fn high() -> Self {
        Self {
            memory_gb: 16.0,
            cpu_cores: 8.0,
            disk_gb: 200.0,
            network_mbps: 1000.0,
        }
    }

    /// Very high resource requirements
    pub fn very_high() -> Self {
        Self {
            memory_gb: 64.0,
            cpu_cores: 16.0,
            disk_gb: 1000.0,
            network_mbps: 10000.0,
        }
    }
}
