// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Comprehensive Storage System Builder Pattern
//! 
//! This module provides a unified builder for configuring all aspects of the
//! ProximaDB storage system including data storage, WAL, compression, indexing,
//! and storage layout strategies.

use std::sync::Arc;
use std::collections::HashMap;
use anyhow::Result;
use serde::{Serialize, Deserialize};

use crate::storage::filesystem::{FilesystemFactory, FilesystemConfig};
use super::wal::{WalConfig, WalFactory, WalManager, WalStrategy};
use super::wal::config::{WalStrategyType, MemTableType, CompressionAlgorithm};

/// Storage layout strategy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StorageLayoutStrategy {
    /// Traditional LSM-tree based storage
    Regular,
    /// VIPER - clustered vector storage with intelligent partitioning
    Viper,
    /// Hybrid - VIPER for vectors, Regular for metadata
    Hybrid,
}

impl Default for StorageLayoutStrategy {
    fn default() -> Self {
        Self::Regular // Default to regular for compatibility
    }
}

// NOTE: Indexing, distance metrics, and hardware acceleration have been moved to their respective modules:
// - src/indexing/builder.rs for IndexingConfig, IndexingAlgorithm, etc.
// - src/compute/builder.rs for HardwareAcceleration, distance metrics, etc.
// This builder now focuses solely on storage concerns.

/// Data storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataStorageConfig {
    /// Storage URLs for data files
    pub data_urls: Vec<String>,
    
    /// Storage layout strategy
    pub layout_strategy: StorageLayoutStrategy,
    
    /// Compression configuration for data
    pub compression: DataCompressionConfig,
    
    /// Segment size for data files
    pub segment_size: u64,
    
    /// Enable memory-mapped I/O
    pub enable_mmap: bool,
    
    /// Cache size (MB)
    pub cache_size_mb: usize,
    
    /// Compaction settings
    pub compaction_config: CompactionConfig,
    
    /// Tiering configuration
    pub tiering_config: Option<DataTieringConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataCompressionConfig {
    /// Enable compression for vector data
    pub compress_vectors: bool,
    
    /// Enable compression for metadata
    pub compress_metadata: bool,
    
    /// Compression algorithm for vectors
    pub vector_compression: VectorCompressionAlgorithm,
    
    /// Compression algorithm for metadata
    pub metadata_compression: CompressionLevel,
    
    /// Compression level (1-9, higher = better compression, slower)
    pub compression_level: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorCompressionAlgorithm {
    /// No compression (fastest)
    None,
    /// Product Quantization
    PQ,
    /// Optimized Product Quantization
    OPQ,
    /// Scalar Quantization
    SQ,
    /// Binary Quantization
    BQ,
    /// Half-precision floats
    FP16,
    /// 8-bit quantization
    INT8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionLevel {
    /// No compression
    None,
    /// Fast compression (low CPU, moderate compression)
    Fast,
    /// High compression (higher CPU, better compression)
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Enable automatic compaction
    pub enable_auto_compaction: bool,
    
    /// Compaction trigger threshold (ratio of deleted data)
    pub trigger_threshold: f32,
    
    /// Max compaction parallelism
    pub max_parallelism: usize,
    
    /// Compaction strategy
    pub strategy: CompactionStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactionStrategy {
    /// Size-tiered compaction
    SizeTiered,
    /// Level-based compaction
    Leveled,
    /// Universal compaction
    Universal,
    /// Adaptive based on workload
    Adaptive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataTieringConfig {
    /// Hot tier configuration
    pub hot_tier: TierConfig,
    
    /// Warm tier configuration  
    pub warm_tier: TierConfig,
    
    /// Cold tier configuration
    pub cold_tier: TierConfig,
    
    /// Auto-tiering policies
    pub auto_tier_policies: AutoTierPolicies,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    /// Storage URLs for this tier
    pub urls: Vec<String>,
    
    /// Compression settings
    pub compression: DataCompressionConfig,
    
    /// Cache size for this tier
    pub cache_size_mb: usize,
    
    /// Access pattern optimization
    pub access_pattern: AccessPattern,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccessPattern {
    /// Optimized for random access
    Random,
    /// Optimized for sequential access
    Sequential,
    /// Mixed access pattern
    Mixed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoTierPolicies {
    /// Move to warm tier after this many hours of no access
    pub hot_to_warm_hours: u64,
    
    /// Move to cold tier after this many hours of no access
    pub warm_to_cold_hours: u64,
    
    /// Access frequency threshold for keeping in hot tier
    pub hot_tier_access_threshold: u32,
    
    /// Enable predictive tiering based on access patterns
    pub enable_predictive_tiering: bool,
}

/// Storage-focused system configuration (storage, WAL, filesystem only)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSystemConfig {
    /// Data storage configuration
    pub data_storage: DataStorageConfig,
    
    /// WAL system configuration
    pub wal_system: WalConfig,
    
    /// Filesystem configuration
    pub filesystem: FilesystemConfig,
    
    /// Storage-specific performance settings
    pub storage_performance: StoragePerformanceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePerformanceConfig {
    /// Number of I/O threads for storage operations
    pub io_threads: usize,
    
    /// Memory pool size for storage operations (MB)
    pub memory_pool_mb: usize,
    
    /// Storage-specific batch configuration
    pub batch_config: BatchConfig,
    
    /// Enable zero-copy optimizations for storage
    pub enable_zero_copy: bool,
    
    /// Buffer sizes for storage operations
    pub buffer_config: StorageBufferConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageBufferConfig {
    /// Read buffer size (bytes)
    pub read_buffer_size: usize,
    
    /// Write buffer size (bytes)
    pub write_buffer_size: usize,
    
    /// Compaction buffer size (bytes)
    pub compaction_buffer_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Default batch size for operations
    pub default_batch_size: usize,
    
    /// Maximum batch size
    pub max_batch_size: usize,
    
    /// Batch timeout (ms)
    pub batch_timeout_ms: u64,
    
    /// Enable adaptive batching
    pub enable_adaptive_batching: bool,
}

impl Default for StorageSystemConfig {
    fn default() -> Self {
        Self {
            data_storage: DataStorageConfig {
                data_urls: vec!["file://./data/storage".to_string()],
                layout_strategy: StorageLayoutStrategy::default(),
                compression: DataCompressionConfig {
                    compress_vectors: true,
                    compress_metadata: true,
                    vector_compression: VectorCompressionAlgorithm::PQ,
                    metadata_compression: CompressionLevel::Fast,
                    compression_level: 3,
                },
                segment_size: 128 * 1024 * 1024, // 128MB
                enable_mmap: true,
                cache_size_mb: 512,
                compaction_config: CompactionConfig {
                    enable_auto_compaction: true,
                    trigger_threshold: 0.3,
                    max_parallelism: 4,
                    strategy: CompactionStrategy::Adaptive,
                },
                tiering_config: None,
            },
            wal_system: WalConfig::default(),
            filesystem: FilesystemConfig::default(),
            storage_performance: StoragePerformanceConfig {
                io_threads: num_cpus::get(),
                memory_pool_mb: 2048,
                batch_config: BatchConfig {
                    default_batch_size: 1000,
                    max_batch_size: 10000,
                    batch_timeout_ms: 100,
                    enable_adaptive_batching: true,
                },
                enable_zero_copy: true,
                buffer_config: StorageBufferConfig {
                    read_buffer_size: 2 * 1024 * 1024, // 2MB
                    write_buffer_size: 1 * 1024 * 1024, // 1MB
                    compaction_buffer_size: 4 * 1024 * 1024, // 4MB
                },
            },
        }
    }
}

/// Comprehensive storage system builder
pub struct StorageSystemBuilder {
    config: StorageSystemConfig,
}

impl StorageSystemBuilder {
    /// Create a new storage system builder
    pub fn new() -> Self {
        Self {
            config: StorageSystemConfig::default(),
        }
    }
    
    /// Set storage layout strategy
    pub fn with_storage_layout(mut self, strategy: StorageLayoutStrategy) -> Self {
        self.config.data_storage.layout_strategy = strategy;
        self
    }
    
    /// Enable VIPER storage layout
    pub fn with_viper_layout(mut self) -> Self {
        self.config.data_storage.layout_strategy = StorageLayoutStrategy::Viper;
        self
    }
    
    /// Enable hybrid storage layout (VIPER for vectors, Regular for metadata)
    pub fn with_hybrid_layout(mut self) -> Self {
        self.config.data_storage.layout_strategy = StorageLayoutStrategy::Hybrid;
        self
    }
    
    /// Configure data storage URLs
    pub fn with_data_storage_urls(mut self, urls: Vec<String>) -> Self {
        self.config.data_storage.data_urls = urls;
        self
    }
    
    /// Configure multi-disk data storage
    pub fn with_multi_disk_data_storage(mut self, disk_paths: Vec<String>) -> Self {
        let urls = disk_paths.into_iter()
            .map(|path| format!("file://{}", path))
            .collect();
        self.config.data_storage.data_urls = urls;
        self
    }
    
    /// Configure S3 data storage
    pub fn with_s3_data_storage(mut self, buckets: Vec<String>) -> Self {
        let urls = buckets.into_iter()
            .map(|bucket| format!("s3://{}/data", bucket))
            .collect();
        self.config.data_storage.data_urls = urls;
        self
    }
    
    /// Set data segment size
    pub fn with_data_segment_size(mut self, size: u64) -> Self {
        self.config.data_storage.segment_size = size;
        self
    }
    
    /// Configure data compression
    pub fn with_data_compression(mut self, config: DataCompressionConfig) -> Self {
        self.config.data_storage.compression = config;
        self
    }
    
    /// Enable high compression for data
    pub fn with_high_data_compression(mut self) -> Self {
        self.config.data_storage.compression = DataCompressionConfig {
            compress_vectors: true,
            compress_metadata: true,
            vector_compression: VectorCompressionAlgorithm::OPQ,
            metadata_compression: CompressionLevel::High,
            compression_level: 6,
        };
        self
    }
    
    /// Configure fast data compression
    pub fn with_fast_data_compression(mut self) -> Self {
        self.config.data_storage.compression = DataCompressionConfig {
            compress_vectors: true,
            compress_metadata: true,
            vector_compression: VectorCompressionAlgorithm::PQ,
            metadata_compression: CompressionLevel::Fast,
            compression_level: 1,
        };
        self
    }
    
    /// Disable data compression
    pub fn without_data_compression(mut self) -> Self {
        self.config.data_storage.compression = DataCompressionConfig {
            compress_vectors: false,
            compress_metadata: false,
            vector_compression: VectorCompressionAlgorithm::None,
            metadata_compression: CompressionLevel::None,
            compression_level: 0,
        };
        self
    }
    
    /// Configure WAL system
    pub fn with_wal_config(mut self, config: WalConfig) -> Self {
        self.config.wal_system = config;
        self
    }
    
    /// Set WAL strategy (Avro for schema evolution, Bincode for performance)
    pub fn with_wal_strategy(mut self, strategy: WalStrategyType) -> Self {
        self.config.wal_system.strategy_type = strategy;
        self
    }
    
    /// Set WAL memtable type (SkipList, BTree, ART, HashMap)
    pub fn with_wal_memtable(mut self, memtable_type: MemTableType) -> Self {
        self.config.wal_system.memtable.memtable_type = memtable_type;
        self
    }
    
    /// Set WAL segment size
    pub fn with_wal_segment_size(mut self, size: usize) -> Self {
        self.config.wal_system.performance.disk_segment_size = size;
        self
    }
    
    /// Configure WAL compression
    pub fn with_wal_compression(mut self, algorithm: CompressionAlgorithm) -> Self {
        self.config.wal_system.compression.algorithm = algorithm;
        self
    }
    
    /// Configure high-throughput WAL
    pub fn with_high_throughput_wal(mut self) -> Self {
        self.config.wal_system = WalConfig::high_throughput();
        self
    }
    
    /// Configure low-latency WAL
    pub fn with_low_latency_wal(mut self) -> Self {
        self.config.wal_system = WalConfig::low_latency();
        self
    }
    
    /// Configure storage-optimized WAL
    pub fn with_storage_optimized_wal(mut self) -> Self {
        self.config.wal_system = WalConfig::storage_optimized();
        self
    }
    
    /// Configure range-query optimized WAL
    pub fn with_range_query_wal(mut self) -> Self {
        self.config.wal_system = WalConfig::range_query_optimized();
        self
    }
    
    /// Configure high-concurrency WAL
    pub fn with_high_concurrency_wal(mut self) -> Self {
        self.config.wal_system = WalConfig::high_concurrency();
        self
    }
    
    // NOTE: Indexing configuration methods have been moved to src/indexing/builder.rs
    // Use IndexingBuilder for configuring search algorithms, distance metrics, etc.
    
    /// Enable tiered storage
    pub fn with_tiered_data_storage(mut self, config: DataTieringConfig) -> Self {
        self.config.data_storage.tiering_config = Some(config);
        self
    }
    
    /// Configure storage performance settings
    pub fn with_storage_performance_config(mut self, config: StoragePerformanceConfig) -> Self {
        self.config.storage_performance = config;
        self
    }
    
    /// Enable high-performance storage mode
    pub fn with_high_performance_storage_mode(mut self) -> Self {
        self.config.storage_performance = StoragePerformanceConfig {
            io_threads: num_cpus::get() * 2,
            memory_pool_mb: 8192,
            batch_config: BatchConfig {
                default_batch_size: 5000,
                max_batch_size: 50000,
                batch_timeout_ms: 50,
                enable_adaptive_batching: true,
            },
            enable_zero_copy: true,
            buffer_config: StorageBufferConfig {
                read_buffer_size: 8 * 1024 * 1024, // 8MB
                write_buffer_size: 4 * 1024 * 1024, // 4MB
                compaction_buffer_size: 16 * 1024 * 1024, // 16MB
            },
        };
        self
    }
    
    /// Configure storage memory settings
    pub fn with_storage_memory_config(mut self, cache_mb: usize, pool_mb: usize) -> Self {
        self.config.data_storage.cache_size_mb = cache_mb;
        self.config.storage_performance.memory_pool_mb = pool_mb;
        self
    }
    
    /// Configure storage buffer sizes
    pub fn with_storage_buffer_config(mut self, config: StorageBufferConfig) -> Self {
        self.config.storage_performance.buffer_config = config;
        self
    }
    
    /// Enable zero-copy optimizations for storage
    pub fn with_zero_copy_storage(mut self) -> Self {
        self.config.storage_performance.enable_zero_copy = true;
        self
    }
    
    /// Build the storage system
    pub async fn build(self) -> Result<StorageSystem> {
        tracing::info!("🏗️ Building storage system");
        tracing::info!("📊 Storage layout: {:?}", self.config.data_storage.layout_strategy);
        tracing::info!("🗃️ WAL strategy: {:?}", self.config.wal_system.strategy_type);
        tracing::info!("💾 Storage URLs: {:?}", self.config.data_storage.data_urls);
        
        // Validate configuration before building
        tracing::info!("🔍 Validating storage configuration...");
        super::validation::ConfigValidator::validate_storage_system(&self.config)?;
        tracing::info!("✅ Configuration validation passed");
        
        // Generate recommendations
        let recommendations = super::validation::ConfigValidator::generate_recommendations(&self.config);
        if !recommendations.is_empty() {
            tracing::info!("💡 Configuration recommendations:");
            for rec in &recommendations {
                tracing::info!("   - {}", rec);
            }
        }
        
        // Initialize filesystem factory
        let filesystem = Arc::new(FilesystemFactory::new(self.config.filesystem.clone()).await?);
        tracing::info!("✅ Filesystem factory initialized");
        
        // Build WAL system using new factory pattern
        tracing::info!("🔧 Creating WAL strategy: {:?} with memtable: {:?}", 
                      self.config.wal_system.strategy_type, 
                      self.config.wal_system.memtable.memtable_type);
        
        let wal_strategy = WalFactory::create_from_config(&self.config.wal_system, filesystem.clone()).await?;
        let wal_manager = WalManager::new(wal_strategy, self.config.wal_system.clone()).await?;
        tracing::info!("✅ WAL system initialized with {:?} strategy", 
                      self.config.wal_system.strategy_type);
        
        // TODO: Initialize data storage engines based on layout strategy
        // TODO: Initialize tiered storage based on configuration
        // TODO: Initialize compaction strategies
        
        let system = StorageSystem {
            config: self.config,
            filesystem,
            wal_manager: Arc::new(wal_manager),
        };
        
        tracing::info!("🎉 Storage system build complete");
        
        Ok(system)
    }
    
    /// Get current configuration (for inspection)
    pub fn config(&self) -> &StorageSystemConfig {
        &self.config
    }
}

impl Default for StorageSystemBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Fully configured storage system
pub struct StorageSystem {
    config: StorageSystemConfig,
    filesystem: Arc<FilesystemFactory>,
    wal_manager: Arc<WalManager>,
}

impl StorageSystem {
    /// Get configuration
    pub fn config(&self) -> &StorageSystemConfig {
        &self.config
    }
    
    /// Get filesystem factory
    pub fn filesystem(&self) -> &Arc<FilesystemFactory> {
        &self.filesystem
    }
    
    /// Get WAL manager
    pub fn wal_manager(&self) -> &Arc<WalManager> {
        &self.wal_manager
    }
    
    /// Get current storage layout strategy
    pub fn storage_layout(&self) -> &StorageLayoutStrategy {
        &self.config.data_storage.layout_strategy
    }
    
    /// Get storage performance configuration
    pub fn storage_performance(&self) -> &StoragePerformanceConfig {
        &self.config.storage_performance
    }
    
    /// Get data storage configuration
    pub fn data_storage_config(&self) -> &DataStorageConfig {
        &self.config.data_storage
    }
}

impl std::fmt::Debug for StorageSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageSystem")
            .field("storage_layout", &self.config.data_storage.layout_strategy)
            .field("wal_strategy", &self.config.wal_system.strategy_type)
            .field("wal_memtable", &self.config.wal_system.memtable.memtable_type)
            .field("data_urls_count", &self.config.data_storage.data_urls.len())
            .field("compression_enabled", &self.config.data_storage.compression.compress_vectors)
            .field("tiering_enabled", &self.config.data_storage.tiering_config.is_some())
            .field("zero_copy_enabled", &self.config.storage_performance.enable_zero_copy)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_storage_system_builder() {
        let builder = StorageSystemBuilder::new()
            .with_viper_layout()
            .with_wal_strategy(WalStrategyType::Avro)
            .with_wal_memtable(MemTableType::BTree)
            .with_high_data_compression();
        
        assert_eq!(builder.config.data_storage.layout_strategy, StorageLayoutStrategy::Viper);
        assert_eq!(builder.config.wal_system.strategy_type, WalStrategyType::Avro);
        assert_eq!(builder.config.wal_system.memtable.memtable_type, MemTableType::BTree);
        assert!(builder.config.data_storage.compression.compress_vectors);
    }
    
    #[tokio::test]
    async fn test_multi_disk_configuration() {
        let builder = StorageSystemBuilder::new()
            .with_multi_disk_data_storage(vec![
                "/ssd1/data".to_string(),
                "/ssd2/data".to_string(),
                "/ssd3/data".to_string(),
            ])
            .with_high_performance_storage_mode();
        
        assert_eq!(builder.config.data_storage.data_urls.len(), 3);
        assert!(builder.config.storage_performance.enable_zero_copy);
        assert_eq!(builder.config.storage_performance.io_threads, num_cpus::get() * 2);
    }
    
    #[tokio::test]
    async fn test_storage_performance_configuration() {
        let builder = StorageSystemBuilder::new()
            .with_storage_buffer_config(StorageBufferConfig {
                read_buffer_size: 8 * 1024 * 1024,
                write_buffer_size: 4 * 1024 * 1024,
                compaction_buffer_size: 16 * 1024 * 1024,
            })
            .with_zero_copy_storage();
        
        assert_eq!(builder.config.storage_performance.buffer_config.read_buffer_size, 8 * 1024 * 1024);
        assert_eq!(builder.config.storage_performance.buffer_config.write_buffer_size, 4 * 1024 * 1024);
        assert!(builder.config.storage_performance.enable_zero_copy);
    }
}