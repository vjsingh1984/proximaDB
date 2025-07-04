// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! WAL Configuration with Smart Defaults for Performance

use serde::{Deserialize, Serialize};

// NOTE: CompressionAlgorithm moved to unified_types.rs
// WAL-specific configuration uses the unified type
pub use crate::core::CompressionAlgorithm;

/// Compression configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Algorithm to use
    pub algorithm: crate::core::CompressionAlgorithm,

    /// Enable compression for memory structures
    pub compress_memory: bool,

    /// Enable compression for disk storage
    pub compress_disk: bool,

    /// Minimum entry size to compress (bytes)
    pub min_compress_size: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgorithm::default(),
            compress_memory: false, // Keep memory uncompressed for fast metadata filtering
            compress_disk: true,    // Compress disk for space efficiency with large vectors
            min_compress_size: 1024, // Compress larger entries (vectors) for better disk IOPS
        }
    }
}

/// Performance configuration with smart defaults - size-based flush only
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Memory table flush threshold (bytes) - ONLY size-based trigger
    pub memory_flush_size_bytes: usize,

    /// Disk segment size (bytes) for each collection
    pub disk_segment_size: usize,

    /// Global WAL size threshold for forced flush (bytes)
    pub global_flush_threshold: usize,

    /// Write buffer size for disk operations
    pub write_buffer_size: usize,

    /// Number of concurrent flush operations
    pub concurrent_flushes: usize,

    /// Batch write optimization threshold
    pub batch_threshold: usize,

    /// MVCC cleanup interval (seconds)
    pub mvcc_cleanup_interval_secs: u64,

    /// TTL cleanup interval (seconds)
    pub ttl_cleanup_interval_secs: u64,

    /// Sync mode for disk writes
    pub sync_mode: SyncMode,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            // Optimized for write-triggered size-based flush only
            memory_flush_size_bytes: 1 * 1024 * 1024, // 1MB memory limit for testing metadata filtering performance
            disk_segment_size: 512 * 1024 * 1024, // 512MB segments optimized for large vectors
            global_flush_threshold: 512 * 1024 * 1024, // 512MB global limit for write-triggered flush
            write_buffer_size: 8 * 1024 * 1024, // 8MB write buffer for large vector throughput
            concurrent_flushes: num_cpus::get().min(4), // Max 4 concurrent flushes to avoid I/O contention
            batch_threshold: 500, // Larger batches for bulk insert optimization
            mvcc_cleanup_interval_secs: 3600, // Clean up old versions every hour
            ttl_cleanup_interval_secs: 300, // Check TTL every 5 minutes
            sync_mode: SyncMode::PerBatch, // Balance safety and bulk insert performance
        }
    }
}

/// Disk sync mode for durability vs performance trade-off
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SyncMode {
    /// Never sync (fastest, least durable)
    Never,
    /// Sync after each write (slowest, most durable)
    Always,
    /// Sync periodically (good balance)
    Periodic,
    /// Sync after each batch (good for batch workloads)
    PerBatch,
    /// Memory-only durability (no disk WAL, flush from memory to storage)
    MemoryOnly,
}

/// WAL strategy type selection
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WalStrategyType {
    /// Avro with schema evolution support
    Avro,
    /// Bincode for maximum native Rust performance
    Bincode,
}

impl Default for WalStrategyType {
    fn default() -> Self {
        // Default to Avro for schema evolution, robust recovery, and bulk insert efficiency
        Self::Avro
    }
}

/// Multi-disk configuration for WAL distribution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiDiskConfig {
    /// WAL directory URLs supporting multiple filesystem types
    /// Examples:
    /// - file:///path/to/wal1, file:///path/to/wal2 (local multi-disk)
    /// - s3://bucket1/wal, s3://bucket2/wal (S3 multi-bucket)
    /// - adls://account.dfs.core.windows.net/container1/wal (Azure)
    /// - gcs://bucket/wal (Google Cloud)
    pub data_directories: Vec<String>,

    /// Distribution strategy
    pub distribution_strategy: DiskDistributionStrategy,

    /// Enable collection affinity (collection stays on one disk)
    pub collection_affinity: bool,
}

/// Strategy for distributing collections across disks
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DiskDistributionStrategy {
    /// Round-robin distribution
    RoundRobin,
    /// Hash-based distribution (consistent)
    Hash,
    /// Load-balanced distribution (dynamic)
    LoadBalanced,
}

impl Default for MultiDiskConfig {
    fn default() -> Self {
        Self {
            data_directories: vec!["file:///workspace/data/wal".to_string()],
            distribution_strategy: DiskDistributionStrategy::LoadBalanced, // Optimal for bulk inserts
            collection_affinity: true, // Keep collection on one disk for sequential I/O
        }
    }
}

/// Memtable configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemTableConfig {
    /// Memtable strategy type
    pub memtable_type: MemTableType,

    /// Global memory limit across all collections (size-based only)
    pub global_memory_limit: usize,

    /// MVCC version retention count
    pub mvcc_versions_retained: usize,

    /// Enable concurrent operations
    pub enable_concurrency: bool,
}

/// Memtable strategy type selection
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MemTableType {
    /// Skip List - High write throughput, ordered data (RocksDB/LevelDB default)
    SkipList,
    /// B+ Tree - Stable inserts/queries, general use, memory efficient
    BTree,
    /// ART - Concurrent Adaptive Radix Tree, high performance for range queries
    Art,
    /// Hash Map - Write-heavy, unordered ingestion, point lookups only
    HashMap,
}

impl Default for MemTableType {
    fn default() -> Self {
        // ART (Adaptive Radix Tree) for efficient metadata filtering and space efficiency
        Self::Art
    }
}

impl Default for MemTableConfig {
    fn default() -> Self {
        Self {
            memtable_type: MemTableType::default(),
            global_memory_limit: 512 * 1024 * 1024, // 512MB for write-triggered flush
            mvcc_versions_retained: 3,              // Keep last 3 versions for MVCC
            enable_concurrency: true,               // Enable concurrent operations
        }
    }
}

/// Comprehensive WAL configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalConfig {
    /// Strategy type to use
    pub strategy_type: WalStrategyType,

    /// Memtable configuration
    pub memtable: MemTableConfig,

    /// Multi-disk configuration
    pub multi_disk: MultiDiskConfig,

    /// Compression settings
    pub compression: CompressionConfig,

    /// Performance tuning
    pub performance: PerformanceConfig,

    /// Enable MVCC versioning
    pub enable_mvcc: bool,

    /// Enable TTL support
    pub enable_ttl: bool,

    /// Enable background compaction
    pub enable_background_compaction: bool,

    /// Collection-specific overrides
    pub collection_overrides: std::collections::HashMap<String, CollectionWalConfig>,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            strategy_type: WalStrategyType::default(), // Avro for schema evolution and recovery
            memtable: MemTableConfig::default(),       // ART for metadata filtering efficiency
            multi_disk: MultiDiskConfig::default(),    // LoadBalanced for bulk insert optimization
            compression: CompressionConfig::default(), // Snappy for balanced performance
            performance: PerformanceConfig::default(), // Optimized for large vectors and bulk processing
            enable_mvcc: true, // Enable for consistency and document versioning
            enable_ttl: true,  // Enable for data lifecycle management
            enable_background_compaction: true, // Enable for maintenance and space reclamation
            collection_overrides: std::collections::HashMap::new(),
        }
    }
}

/// Collection-specific WAL configuration overrides
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionWalConfig {
    /// Override memory flush size threshold for this collection (bytes)
    pub memory_flush_size_bytes: Option<usize>,

    /// Override disk segment size for this collection
    pub disk_segment_size: Option<usize>,

    /// Override compression settings
    pub compression: Option<CompressionConfig>,

    /// Override TTL settings
    pub default_ttl_days: Option<u32>,
}

// Conversion from core config to WAL config
impl From<&crate::core::config::WalStorageConfig> for WalConfig {
    fn from(core_config: &crate::core::config::WalStorageConfig) -> Self {
        let mut wal_config = WalConfig::default();
        
        // Convert URLs to MultiDiskConfig
        wal_config.multi_disk.data_directories = core_config.wal_urls.clone();
        wal_config.multi_disk.distribution_strategy = match core_config.distribution_strategy {
            crate::core::config::WalDistributionStrategy::RoundRobin => DiskDistributionStrategy::RoundRobin,
            crate::core::config::WalDistributionStrategy::Hash => DiskDistributionStrategy::Hash,
            crate::core::config::WalDistributionStrategy::LoadBalanced => DiskDistributionStrategy::LoadBalanced,
        };
        wal_config.multi_disk.collection_affinity = core_config.collection_affinity;
        
        // Set performance thresholds
        wal_config.performance.memory_flush_size_bytes = core_config.memory_flush_size_bytes;
        wal_config.performance.global_flush_threshold = core_config.global_flush_threshold;
        
        // Apply optional configuration overrides from config.toml
        if let Some(strategy_type) = &core_config.strategy_type {
            wal_config.strategy_type = match strategy_type.as_str() {
                "Avro" => WalStrategyType::Avro,
                "Bincode" => WalStrategyType::Bincode,
                _ => WalStrategyType::default(),
            };
        }
        
        if let Some(memtable_type) = &core_config.memtable_type {
            wal_config.memtable.memtable_type = match memtable_type.as_str() {
                "BTree" => MemTableType::BTree,
                "HashMap" => MemTableType::HashMap,
                "SkipList" => MemTableType::SkipList,
                "Art" => MemTableType::Art,
                _ => MemTableType::default(),
            };
        }
        
        if let Some(sync_mode) = &core_config.sync_mode {
            wal_config.performance.sync_mode = match sync_mode.as_str() {
                "Always" => SyncMode::Always,
                "PerBatch" => SyncMode::PerBatch,
                "Periodic" => SyncMode::Periodic,
                "Never" => SyncMode::Never,
                "MemoryOnly" => SyncMode::MemoryOnly,
                _ => SyncMode::PerBatch, // Default to balanced mode
            };
        }
        
        if let Some(batch_threshold) = core_config.batch_threshold {
            wal_config.performance.batch_threshold = batch_threshold;
        }
        
        if let Some(write_buffer_mb) = core_config.write_buffer_size_mb {
            wal_config.performance.write_buffer_size = write_buffer_mb * 1024 * 1024; // Convert MB to bytes
        }
        
        if let Some(concurrent_flushes) = core_config.concurrent_flushes {
            wal_config.performance.concurrent_flushes = concurrent_flushes;
        }
        
        wal_config
    }
}

impl WalConfig {
    /// Create configuration optimized for high-throughput writes
    pub fn high_throughput() -> Self {
        let mut config = Self::default();
        config.strategy_type = WalStrategyType::Bincode; // Faster serialization
        config.memtable.memtable_type = MemTableType::HashMap; // Fastest writes for unordered data
        config.compression.algorithm = CompressionAlgorithm::Lz4; // Faster compression
        config.performance.memory_flush_size_bytes = 256 * 1024 * 1024; // 256MB
        config.performance.batch_threshold = 500; // Larger batches
        config.performance.sync_mode = SyncMode::PerBatch; // Less frequent syncing
        config
    }

    /// Create configuration optimized for low-latency reads
    pub fn low_latency() -> Self {
        let mut config = Self::default();
        config.memtable.memtable_type = MemTableType::HashMap; // Fastest point lookups
        config.compression.compress_memory = false; // Faster memory access
        config.compression.compress_disk = false; // Faster disk reads
        config.performance.memory_flush_size_bytes = 32 * 1024 * 1024; // 32MB smaller memory footprint
        config.performance.sync_mode = SyncMode::Always; // Immediate consistency
        config
    }

    /// Create configuration optimized for storage efficiency
    pub fn storage_optimized() -> Self {
        let mut config = Self::default();
        config.memtable.memtable_type = MemTableType::BTree; // Most memory-efficient
        config.compression.algorithm = CompressionAlgorithm::Zstd; // Better compression
        config.compression.compress_memory = true; // Compress everything
        config.compression.min_compress_size = 64; // Compress smaller entries
        config.performance.disk_segment_size = 512 * 1024 * 1024; // Larger segments
        config
    }

    /// Create configuration optimized for range queries and analytics
    pub fn range_query_optimized() -> Self {
        let mut config = Self::default();
        config.memtable.memtable_type = MemTableType::BTree; // Excellent range scan performance
        config.strategy_type = WalStrategyType::Avro; // Schema evolution for analytics
        config.compression.algorithm = CompressionAlgorithm::Snappy; // Balanced compression
        config.performance.memory_flush_size_bytes = 64 * 1024 * 1024; // 64MB moderate memory usage
        config
    }

    /// Create configuration optimized for high concurrency and string keys
    pub fn high_concurrency() -> Self {
        let mut config = Self::default();
        config.memtable.memtable_type = MemTableType::Art; // Excellent concurrency
        config.strategy_type = WalStrategyType::Bincode; // Fast serialization
        config.compression.algorithm = CompressionAlgorithm::Lz4; // Fast compression
        config.memtable.enable_concurrency = true;
        config
    }

    /// Get effective configuration for a collection (with overrides)
    pub fn effective_config_for_collection(
        &self,
        collection_id: &str,
    ) -> CollectionEffectiveConfig {
        let overrides = self.collection_overrides.get(collection_id);

        CollectionEffectiveConfig {
            memory_flush_size_bytes: overrides
                .and_then(|o| o.memory_flush_size_bytes)
                .unwrap_or(self.performance.memory_flush_size_bytes),
            disk_segment_size: overrides
                .and_then(|o| o.disk_segment_size)
                .unwrap_or(self.performance.disk_segment_size),
            compression: overrides
                .and_then(|o| o.compression.clone())
                .unwrap_or_else(|| self.compression.clone()),
            default_ttl_days: overrides.and_then(|o| o.default_ttl_days),
        }
    }
}

/// Effective configuration for a specific collection
#[derive(Debug, Clone)]
pub struct CollectionEffectiveConfig {
    pub disk_segment_size: usize,
    pub compression: CompressionConfig,
    pub default_ttl_days: Option<u32>,
    /// Size-based flush threshold (bytes) - derived from memory_flush_size_bytes
    pub memory_flush_size_bytes: usize,
}
