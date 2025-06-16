// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! WAL Configuration with Smart Defaults for Performance

use serde::{Serialize, Deserialize};
use std::path::PathBuf;

/// Compression algorithms for WAL data
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CompressionAlgorithm {
    /// No compression (fastest)
    None,
    /// LZ4 - fast compression/decompression, good for CPU-bound workloads
    Lz4,
    /// Snappy - balanced compression/speed, good default
    Snappy,
    /// ZSTD - configurable compression levels
    Zstd { level: i32 },
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        // Smart default: Snappy provides good balance of compression ratio and speed
        // without significant CPU overhead, improving IOPS on most storage
        Self::Snappy
    }
}

/// Compression configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Algorithm to use
    pub algorithm: CompressionAlgorithm,
    
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

/// Performance configuration with smart defaults
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Memory table flush threshold (entries)
    pub memory_flush_threshold: usize,
    
    /// Memory table flush threshold (bytes)
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
    
    /// Maximum WAL age before forced flush (seconds)
    pub max_wal_age_secs: u64,
    
    /// Per-collection age override
    pub collection_max_age_overrides: std::collections::HashMap<String, u64>,
    
    /// Check interval for age-based flushes (seconds)
    pub age_check_interval_secs: u64,
    
    /// Sync mode for disk writes
    pub sync_mode: SyncMode,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            // Optimized for bulk inserts, large vectors, and metadata filtering
            memory_flush_threshold: 75_000,           // 75K entries for bulk processing
            memory_flush_size_bytes: 1024 * 1024 * 1024, // 1GB memory limit for large vectors
            disk_segment_size: 512 * 1024 * 1024,     // 512MB segments optimized for large vectors
            global_flush_threshold: 2048 * 1024 * 1024, // 2GB global limit for better batching
            write_buffer_size: 8 * 1024 * 1024,       // 8MB write buffer for large vector throughput
            concurrent_flushes: num_cpus::get().min(4), // Max 4 concurrent flushes to avoid I/O contention
            batch_threshold: 500,                       // Larger batches for bulk insert optimization
            mvcc_cleanup_interval_secs: 3600,          // Clean up old versions every hour
            ttl_cleanup_interval_secs: 300,            // Check TTL every 5 minutes
            max_wal_age_secs: 3600,                    // Flush WAL after 1 hour max age
            collection_max_age_overrides: std::collections::HashMap::new(),
            age_check_interval_secs: 300,              // Check age every 5 minutes
            sync_mode: SyncMode::PerBatch,             // Balance safety and bulk insert performance
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
    /// Data directories on different disks
    pub data_directories: Vec<PathBuf>,
    
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
            data_directories: vec![PathBuf::from("./data/wal")],
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
    
    /// Memory flush threshold per collection
    pub memory_flush_threshold: usize,
    
    /// Global memory limit across all collections
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
            memory_flush_threshold: 75_000,           // 75K entries for bulk inserts
            global_memory_limit: 2048 * 1024 * 1024,  // 2GB for large vectors and metadata
            mvcc_versions_retained: 3,                 // Keep last 3 versions for MVCC
            enable_concurrency: true,                  // Enable concurrent operations
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
            strategy_type: WalStrategyType::default(),  // Avro for schema evolution and recovery
            memtable: MemTableConfig::default(),        // ART for metadata filtering efficiency
            multi_disk: MultiDiskConfig::default(),     // LoadBalanced for bulk insert optimization
            compression: CompressionConfig::default(),  // Snappy for balanced performance
            performance: PerformanceConfig::default(),  // Optimized for large vectors and bulk processing
            enable_mvcc: true,  // Enable for consistency and document versioning
            enable_ttl: true,   // Enable for data lifecycle management
            enable_background_compaction: true, // Enable for maintenance and space reclamation
            collection_overrides: std::collections::HashMap::new(),
        }
    }
}

/// Collection-specific WAL configuration overrides
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionWalConfig {
    /// Override memory flush threshold for this collection
    pub memory_flush_threshold: Option<usize>,
    
    /// Override disk segment size for this collection
    pub disk_segment_size: Option<usize>,
    
    /// Override compression settings
    pub compression: Option<CompressionConfig>,
    
    /// Override TTL settings
    pub default_ttl_days: Option<u32>,
}

impl WalConfig {
    /// Create configuration optimized for high-throughput writes
    pub fn high_throughput() -> Self {
        let mut config = Self::default();
        config.strategy_type = WalStrategyType::Bincode; // Faster serialization
        config.memtable.memtable_type = MemTableType::HashMap; // Fastest writes for unordered data
        config.compression.algorithm = CompressionAlgorithm::Lz4; // Faster compression
        config.performance.memory_flush_threshold = 50_000; // Larger memory buffers
        config.performance.memory_flush_size_bytes = 128 * 1024 * 1024; // 128MB
        config.performance.batch_threshold = 500; // Larger batches
        config.performance.sync_mode = SyncMode::PerBatch; // Less frequent syncing
        config
    }
    
    /// Create configuration optimized for low-latency reads
    pub fn low_latency() -> Self {
        let mut config = Self::default();
        config.memtable.memtable_type = MemTableType::HashMap; // Fastest point lookups
        config.compression.compress_memory = false; // Faster memory access
        config.compression.compress_disk = false;   // Faster disk reads
        config.performance.memory_flush_threshold = 5_000; // Smaller memory footprint
        config.performance.sync_mode = SyncMode::Always;   // Immediate consistency
        config
    }
    
    /// Create configuration optimized for storage efficiency
    pub fn storage_optimized() -> Self {
        let mut config = Self::default();
        config.memtable.memtable_type = MemTableType::BTree; // Most memory-efficient
        config.compression.algorithm = CompressionAlgorithm::Zstd { level: 3 }; // Better compression
        config.compression.compress_memory = true;  // Compress everything
        config.compression.min_compress_size = 64;  // Compress smaller entries
        config.performance.disk_segment_size = 512 * 1024 * 1024; // Larger segments
        config
    }
    
    /// Create configuration optimized for range queries and analytics
    pub fn range_query_optimized() -> Self {
        let mut config = Self::default();
        config.memtable.memtable_type = MemTableType::BTree; // Excellent range scan performance
        config.strategy_type = WalStrategyType::Avro; // Schema evolution for analytics
        config.compression.algorithm = CompressionAlgorithm::Snappy; // Balanced compression
        config.performance.memory_flush_threshold = 20_000; // Moderate memory usage
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
    pub fn effective_config_for_collection(&self, collection_id: &str) -> CollectionEffectiveConfig {
        let overrides = self.collection_overrides.get(collection_id);
        
        CollectionEffectiveConfig {
            memory_flush_threshold: overrides
                .and_then(|o| o.memory_flush_threshold)
                .unwrap_or(self.performance.memory_flush_threshold),
            disk_segment_size: overrides
                .and_then(|o| o.disk_segment_size)
                .unwrap_or(self.performance.disk_segment_size),
            compression: overrides
                .and_then(|o| o.compression.clone())
                .unwrap_or_else(|| self.compression.clone()),
            default_ttl_days: overrides.and_then(|o| o.default_ttl_days),
        }
    }
    
    /// Get effective max age for a collection (with overrides)
    pub fn effective_max_age_for_collection(&self, collection_id: &str) -> u64 {
        self.performance.collection_max_age_overrides
            .get(collection_id)
            .copied()
            .unwrap_or(self.performance.max_wal_age_secs)
    }
}

/// Effective configuration for a specific collection
#[derive(Debug, Clone)]
pub struct CollectionEffectiveConfig {
    pub memory_flush_threshold: usize,
    pub disk_segment_size: usize,
    pub compression: CompressionConfig,
    pub default_ttl_days: Option<u32>,
}