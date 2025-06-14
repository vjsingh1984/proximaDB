// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Storage Layout Implementation
//! 
//! VIPER (Vector Intelligent Parquet Efficient Representation) is ProximaDB's
//! advanced storage layout optimized for vector similarity search with:
//! 
//! ## Core Features
//! - **Parquet-based storage**: Column-oriented format optimized for analytics
//! - **Dual storage modes**: 
//!   - Sparse vectors: Key-value pairs for efficient storage
//!   - Dense vectors: Row-based storage for better scan performance
//! - **Cluster-based partitioning**: Vectors organized by cluster ID for pruning
//! - **Bloom filter integration**: Fast false-positive elimination
//! - **Multi-tier storage**: Ultra-hot to archive tiers with automatic migration
//! 
//! ## Architecture
//! ```
//! ┌─────────────────────────────────────────┐
//! │            VIPER Layout                 │
//! ├─────────────────┬───────────────────────┤
//! │  Sparse Vectors │    Dense Vectors      │
//! │  (Key-Value)    │    (Row-based)        │
//! ├─────────────────┼───────────────────────┤
//! │  Cluster ID 1   │    Cluster ID 1       │
//! │  Cluster ID 2   │    Cluster ID 2       │
//! │  ...            │    ...                │
//! └─────────────────┴───────────────────────┘
//! ```

pub mod storage_engine;
pub mod compression;
pub mod partitioner;
pub mod search_engine; 
pub mod wal_manager;
pub mod compaction;
pub mod types;

pub use storage_engine::ViperStorageEngine;
pub use compression::{SimdCompressionEngine, CompressionRequest, CompressionResult, DecompressionResult};
pub use partitioner::{FilesystemPartitioner, PartitionConfig};
pub use search_engine::{ViperProgressiveSearchEngine, TierSearcher, TierCharacteristics};
pub use wal_manager::{ViperWalManager, WalEntry, WalRecoveryInfo, RecoveryStrategy};
pub use compaction::{ViperCompactionEngine, CompactionTask, CompactionResult};
pub use types::*;

use std::path::PathBuf;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// VIPER configuration for storage layout and optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViperConfig {
    /// Base directory for VIPER storage
    pub storage_root: PathBuf,
    
    /// Parquet file configuration
    pub parquet_config: ParquetConfig,
    
    /// Clustering configuration for vector organization
    pub clustering_config: ClusteringConfig,
    
    /// Compression settings
    pub compression_config: CompressionConfig,
    
    /// Storage tier configuration
    pub tier_config: TierConfig,
    
    /// Performance tuning parameters
    pub performance_config: PerformanceConfig,
}

/// Parquet file format configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParquetConfig {
    /// Row group size for Parquet files
    pub row_group_size: usize,
    
    /// Page size within row groups
    pub page_size: usize,
    
    /// Dictionary encoding for metadata columns
    pub use_dictionary_encoding: bool,
    
    /// Statistics collection for columns
    pub collect_statistics: bool,
    
    /// Bloom filter configuration
    pub bloom_filter_config: BloomFilterConfig,
}

/// Bloom filter configuration for fast pruning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilterConfig {
    /// Enable bloom filters for cluster IDs
    pub enable_cluster_bloom_filter: bool,
    
    /// False positive probability target
    pub false_positive_probability: f64,
    
    /// Expected number of items per bloom filter
    pub expected_items: usize,
    
    /// Hash function count
    pub hash_function_count: u32,
}

/// Vector clustering configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusteringConfig {
    /// Clustering algorithm to use
    pub algorithm: ClusteringAlgorithm,
    
    /// Number of clusters per partition
    pub clusters_per_partition: usize,
    
    /// Minimum vectors per cluster before splitting
    pub min_vectors_per_cluster: usize,
    
    /// Maximum vectors per cluster before splitting
    pub max_vectors_per_cluster: usize,
    
    /// Reclustering frequency (number of insertions)
    pub reclustering_threshold: usize,
}

/// Available clustering algorithms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusteringAlgorithm {
    /// K-means clustering
    KMeans { k: usize },
    
    /// Hierarchical clustering
    Hierarchical { linkage: LinkageType },
    
    /// Density-based clustering
    DBSCAN { eps: f32, min_points: usize },
    
    /// Locality-sensitive hashing
    LSH { hash_count: usize, bucket_count: usize },
}

/// Linkage types for hierarchical clustering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LinkageType {
    Single,
    Complete,
    Average,
    Ward,
}

/// Compression configuration for different data types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Vector data compression
    pub vector_compression: CompressionAlgorithm,
    
    /// Metadata compression  
    pub metadata_compression: CompressionAlgorithm,
    
    /// Cluster ID compression
    pub cluster_compression: CompressionAlgorithm,
    
    /// SIMD optimization settings
    pub simd_config: SimdConfig,
}

/// Available compression algorithms
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompressionAlgorithm {
    None,
    LZ4,
    Zstd { level: i32 },
    Snappy,
    Gzip { level: u32 },
}

/// SIMD optimization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimdConfig {
    /// Enable SIMD for compression/decompression
    pub enable_simd_compression: bool,
    
    /// Enable SIMD for vector operations
    pub enable_simd_distance: bool,
    
    /// Target SIMD instruction set
    pub instruction_set: SimdInstructionSet,
}

/// Available SIMD instruction sets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SimdInstructionSet {
    Auto,
    SSE4_2,
    AVX2,
    AVX512,
    NEON,
}

/// Storage tier configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    /// Tier definitions
    pub tiers: HashMap<String, TierDefinition>,
    
    /// Default tier for new data
    pub default_tier: String,
    
    /// Migration policies between tiers
    pub migration_policies: Vec<MigrationPolicy>,
}

/// Individual tier definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierDefinition {
    /// Tier level (Ultra-Hot, Hot, Warm, Cold, Archive)
    pub level: TierLevel,
    
    /// Storage backend for this tier
    pub backend: StorageBackend,
    
    /// Compression settings for this tier
    pub compression: CompressionAlgorithm,
    
    /// Access pattern optimization
    pub access_pattern: AccessPattern,
}

/// Storage tier levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TierLevel {
    UltraHot,
    Hot, 
    Warm,
    Cold,
    Archive,
}

/// Storage backend implementations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageBackend {
    /// Memory-mapped local files
    MMAP { directory: PathBuf },
    
    /// Local SSD storage
    LocalSSD { directory: PathBuf },
    
    /// Local HDD storage  
    LocalHDD { directory: PathBuf },
    
    /// S3-compatible object storage
    S3 { bucket: String, prefix: String },
    
    /// Azure Blob storage
    AzureBlob { container: String, prefix: String },
    
    /// Google Cloud Storage
    GCS { bucket: String, prefix: String },
}

/// Access pattern optimization hints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccessPattern {
    /// Random access pattern
    Random,
    
    /// Sequential scan pattern
    Sequential,
    
    /// Write-heavy pattern
    WriteHeavy,
    
    /// Read-heavy pattern
    ReadHeavy,
    
    /// Mixed workload
    Mixed,
}

/// Data migration policies between tiers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationPolicy {
    /// Source tier
    pub from_tier: String,
    
    /// Destination tier
    pub to_tier: String,
    
    /// Migration trigger conditions
    pub triggers: Vec<MigrationTrigger>,
    
    /// Migration strategy
    pub strategy: MigrationStrategy,
}

/// Migration trigger conditions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationTrigger {
    /// Age-based migration
    Age { days: u32 },
    
    /// Access frequency-based migration
    AccessFrequency { 
        threshold: f64,
        window_days: u32,
    },
    
    /// Storage capacity-based migration
    StorageCapacity { 
        threshold_percent: f32,
    },
    
    /// Cost optimization migration
    CostOptimization {
        target_cost_reduction_percent: f32,
    },
}

/// Migration execution strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationStrategy {
    /// Immediate migration
    Immediate,
    
    /// Background migration during low load
    Background {
        max_concurrent_migrations: usize,
        low_load_threshold: f32,
    },
    
    /// Scheduled migration at specific times
    Scheduled {
        schedule: String, // Cron-like schedule
    },
}

/// Performance tuning configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Prefetch configuration for better cache performance
    pub prefetch_config: PrefetchConfig,
    
    /// Parallel processing configuration
    pub parallel_config: ParallelConfig,
    
    /// Memory management settings
    pub memory_config: MemoryConfig,
    
    /// I/O optimization settings
    pub io_config: IoConfig,
}

/// Prefetch configuration for cache optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchConfig {
    /// Enable prefetching for sequential access
    pub enable_sequential_prefetch: bool,
    
    /// Prefetch distance (number of pages ahead)
    pub prefetch_distance: usize,
    
    /// Enable adaptive prefetching based on access patterns
    pub enable_adaptive_prefetch: bool,
}

/// Parallel processing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelConfig {
    /// Number of I/O threads for parallel operations
    pub io_thread_count: usize,
    
    /// Number of compute threads for vector operations
    pub compute_thread_count: usize,
    
    /// Enable work-stealing for load balancing
    pub enable_work_stealing: bool,
}

/// Memory management configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Page cache size for MMAP operations
    pub page_cache_size_mb: usize,
    
    /// Buffer pool size for I/O operations
    pub buffer_pool_size_mb: usize,
    
    /// Enable memory-mapped file management
    pub enable_mmap_management: bool,
    
    /// Memory pressure handling strategy
    pub memory_pressure_strategy: MemoryPressureStrategy,
}

/// Strategies for handling memory pressure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryPressureStrategy {
    /// Aggressive cache eviction
    Aggressive,
    
    /// Conservative cache eviction
    Conservative,
    
    /// Adaptive based on system memory
    Adaptive,
    
    /// Offload to lower tiers
    Offload,
}

/// I/O optimization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoConfig {
    /// Enable direct I/O bypassing OS cache
    pub enable_direct_io: bool,
    
    /// I/O scheduler hints
    pub io_scheduler: IoScheduler,
    
    /// Read-ahead configuration
    pub read_ahead_kb: usize,
    
    /// Write batching configuration
    pub write_batch_size: usize,
}

/// I/O scheduler optimization hints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IoScheduler {
    /// No specific hints
    Default,
    
    /// Optimize for sequential I/O
    Sequential,
    
    /// Optimize for random I/O
    Random,
    
    /// Deadline-based scheduling
    Deadline,
    
    /// Completely fair queuing
    CFQ,
}

impl Default for ViperConfig {
    fn default() -> Self {
        Self {
            storage_root: PathBuf::from("./data/viper"),
            parquet_config: ParquetConfig::default(),
            clustering_config: ClusteringConfig::default(),
            compression_config: CompressionConfig::default(),
            tier_config: TierConfig::default(),
            performance_config: PerformanceConfig::default(),
        }
    }
}

impl Default for ParquetConfig {
    fn default() -> Self {
        Self {
            row_group_size: 10_000,
            page_size: 1024 * 1024, // 1MB
            use_dictionary_encoding: true,
            collect_statistics: true,
            bloom_filter_config: BloomFilterConfig::default(),
        }
    }
}

impl Default for BloomFilterConfig {
    fn default() -> Self {
        Self {
            enable_cluster_bloom_filter: true,
            false_positive_probability: 0.01, // 1% false positive rate
            expected_items: 10_000,
            hash_function_count: 3,
        }
    }
}

impl Default for ClusteringConfig {
    fn default() -> Self {
        Self {
            algorithm: ClusteringAlgorithm::KMeans { k: 100 },
            clusters_per_partition: 10,
            min_vectors_per_cluster: 100,
            max_vectors_per_cluster: 10_000,
            reclustering_threshold: 50_000,
        }
    }
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            vector_compression: CompressionAlgorithm::Zstd { level: 3 },
            metadata_compression: CompressionAlgorithm::LZ4,
            cluster_compression: CompressionAlgorithm::LZ4,
            simd_config: SimdConfig::default(),
        }
    }
}

impl Default for SimdConfig {
    fn default() -> Self {
        Self {
            enable_simd_compression: true,
            enable_simd_distance: true,
            instruction_set: SimdInstructionSet::Auto,
        }
    }
}

impl Default for TierConfig {
    fn default() -> Self {
        let mut tiers = HashMap::new();
        
        tiers.insert("ultra_hot".to_string(), TierDefinition {
            level: TierLevel::UltraHot,
            backend: StorageBackend::MMAP { 
                directory: PathBuf::from("./data/viper/ultra_hot") 
            },
            compression: CompressionAlgorithm::None,
            access_pattern: AccessPattern::Random,
        });
        
        tiers.insert("hot".to_string(), TierDefinition {
            level: TierLevel::Hot,
            backend: StorageBackend::LocalSSD { 
                directory: PathBuf::from("./data/viper/hot") 
            },
            compression: CompressionAlgorithm::LZ4,
            access_pattern: AccessPattern::Mixed,
        });
        
        tiers.insert("warm".to_string(), TierDefinition {
            level: TierLevel::Warm,
            backend: StorageBackend::LocalHDD { 
                directory: PathBuf::from("./data/viper/warm") 
            },
            compression: CompressionAlgorithm::Zstd { level: 1 },
            access_pattern: AccessPattern::Sequential,
        });
        
        Self {
            tiers,
            default_tier: "hot".to_string(),
            migration_policies: vec![
                MigrationPolicy {
                    from_tier: "ultra_hot".to_string(),
                    to_tier: "hot".to_string(),
                    triggers: vec![
                        MigrationTrigger::Age { days: 1 },
                        MigrationTrigger::AccessFrequency { 
                            threshold: 0.1, 
                            window_days: 7 
                        },
                    ],
                    strategy: MigrationStrategy::Background {
                        max_concurrent_migrations: 2,
                        low_load_threshold: 0.7,
                    },
                },
                MigrationPolicy {
                    from_tier: "hot".to_string(),
                    to_tier: "warm".to_string(),
                    triggers: vec![
                        MigrationTrigger::Age { days: 30 },
                        MigrationTrigger::AccessFrequency { 
                            threshold: 0.01, 
                            window_days: 30 
                        },
                    ],
                    strategy: MigrationStrategy::Scheduled {
                        schedule: "0 2 * * *".to_string(), // Daily at 2 AM
                    },
                },
            ],
        }
    }
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            prefetch_config: PrefetchConfig {
                enable_sequential_prefetch: true,
                prefetch_distance: 4,
                enable_adaptive_prefetch: true,
            },
            parallel_config: ParallelConfig {
                io_thread_count: num_cpus::get(),
                compute_thread_count: num_cpus::get(),
                enable_work_stealing: true,
            },
            memory_config: MemoryConfig {
                page_cache_size_mb: 1024, // 1GB
                buffer_pool_size_mb: 512,  // 512MB
                enable_mmap_management: true,
                memory_pressure_strategy: MemoryPressureStrategy::Adaptive,
            },
            io_config: IoConfig {
                enable_direct_io: false,
                io_scheduler: IoScheduler::Default,
                read_ahead_kb: 128,
                write_batch_size: 1024 * 1024, // 1MB
            },
        }
    }
}