// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Collection Strategy System
//!
//! Provides collection-specific flush and compaction strategies based on
//! the combination of indexing algorithm, storage engine, and search configuration.
//!
//! ## Strategy Combinations:
//! - **Standard Strategy**: HNSW + LSM + Standard Search
//! - **High-Performance Strategy**: IVF + LSM + Optimized Search
//! - **VIPER Strategy**: PQ + VIPER + ML-optimized Search
//! - **Custom Strategies**: User-defined combinations

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::{CollectionMetadata, WalEntry};

pub mod custom;
pub mod standard;
pub mod viper;

/// Collection flush strategy trait
#[async_trait]
pub trait CollectionStrategy: Send + Sync {
    /// Strategy name for identification
    fn strategy_name(&self) -> &'static str;

    /// Get strategy configuration
    fn get_config(&self) -> CollectionStrategyConfig;

    /// Initialize strategy for a collection
    async fn initialize(
        &mut self,
        collection_id: CollectionId,
        metadata: &CollectionMetadata,
    ) -> Result<()>;

    /// Flush WAL entries to collection-specific storage
    async fn flush_entries(
        &self,
        collection_id: &CollectionId,
        entries: Vec<WalEntry>,
    ) -> Result<FlushResult>;

    /// Compact collection data using strategy-specific algorithm
    async fn compact_collection(&self, collection_id: &CollectionId) -> Result<CompactionResult>;

    /// Search vectors using strategy-specific search engine
    async fn search_vectors(
        &self,
        collection_id: &CollectionId,
        query: Vec<f32>,
        k: usize,
        filter: Option<SearchFilter>,
    ) -> Result<Vec<SearchResult>>;

    /// Add vector to strategy-specific index
    async fn add_vector(
        &self,
        collection_id: &CollectionId,
        vector_record: &VectorRecord,
    ) -> Result<()>;

    /// Update vector in strategy-specific index
    async fn update_vector(
        &self,
        collection_id: &CollectionId,
        vector_record: &VectorRecord,
    ) -> Result<()>;

    /// Remove vector from strategy-specific index
    async fn remove_vector(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<bool>;

    /// Get strategy-specific statistics
    async fn get_stats(&self, collection_id: &CollectionId) -> Result<StrategyStats>;

    /// Optimize collection using strategy-specific algorithm
    async fn optimize_collection(&self, collection_id: &CollectionId)
        -> Result<OptimizationResult>;
}

/// Collection strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionStrategyConfig {
    /// Strategy type
    pub strategy_type: StrategyType,

    /// Indexing algorithm configuration
    pub indexing_config: IndexingConfig,

    /// Storage engine configuration
    pub storage_config: StorageConfig,

    /// Search engine configuration
    pub search_config: SearchConfig,

    /// Performance tuning parameters
    pub performance_config: PerformanceConfig,
}

/// Available strategy types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StrategyType {
    /// Standard strategy: HNSW + LSM + Standard Search
    Standard,
    /// High-performance strategy: IVF + LSM + Optimized Search
    HighPerformance,
    /// VIPER strategy: PQ + VIPER + ML-optimized Search
    Viper,
    /// Custom strategy with user-defined configuration
    Custom { name: String },
}

/// Indexing algorithm configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingConfig {
    /// Primary algorithm
    pub algorithm: IndexingAlgorithm,
    /// Algorithm-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
    /// Secondary indexing for hybrid approaches
    pub secondary_algorithm: Option<IndexingAlgorithm>,
}

/// Available indexing algorithms
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IndexingAlgorithm {
    /// Hierarchical Navigable Small World
    HNSW {
        m: usize,               // Number of connections
        ef_construction: usize, // Size of dynamic candidate list
        ef_search: usize,       // Size of search candidate list
    },
    /// Inverted File Index
    IVF {
        nlist: usize,  // Number of clusters
        nprobe: usize, // Number of clusters to search
    },
    /// Product Quantization
    PQ {
        m: usize,     // Number of subquantizers
        nbits: usize, // Number of bits per subquantizer
    },
    /// Flat (brute force) search
    Flat,
    /// Approximate Random Projection Trees
    AnnoyTrees { num_trees: usize, search_k: usize },
}

/// Storage engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Storage engine type
    pub engine_type: StorageEngineType,
    /// Engine-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
    /// Compression settings
    pub compression: CompressionConfig,
    /// Tiering configuration
    pub tiering: Option<TieringConfig>,
}

/// Available storage engine types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StorageEngineType {
    /// LSM Tree storage
    LSM,
    /// VIPER columnar storage
    VIPER,
    /// Memory-mapped files
    MMAP,
    /// Custom storage engine
    Custom { name: String },
}

/// Search engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Distance metric
    pub distance_metric: DistanceMetric,
    /// Search-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
    /// Enable search optimization
    pub enable_optimization: bool,
    /// Search caching configuration
    pub caching: Option<SearchCacheConfig>,
}

/// Distance metrics
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DistanceMetric {
    Cosine,
    Euclidean,
    DotProduct,
    Manhattan,
    Hamming,
    Jaccard,
}

/// Performance tuning configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Parallel processing threads
    pub num_threads: usize,
    /// Memory limits
    pub memory_limit_mb: usize,
    /// Enable SIMD optimizations
    pub enable_simd: bool,
    /// Enable GPU acceleration
    pub enable_gpu: bool,
    /// Batch processing configuration
    pub batch_config: BatchConfig,
}

/// Batch processing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Batch size for operations
    pub batch_size: usize,
    /// Batch timeout in milliseconds
    pub batch_timeout_ms: u64,
    /// Enable adaptive batching
    pub adaptive_batching: bool,
}

/// Compression configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Enable compression
    pub enabled: bool,
    /// Compression algorithm
    pub algorithm: String,
    /// Compression level
    pub level: u8,
}

/// Tiering configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieringConfig {
    /// Enable automatic tiering
    pub enabled: bool,
    /// Tier levels configuration
    pub tiers: Vec<TierConfig>,
}

/// Individual tier configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    /// Tier name
    pub name: String,
    /// Access frequency threshold
    pub access_threshold: f64,
    /// Storage type for this tier
    pub storage_type: String,
}

/// Search cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCacheConfig {
    /// Enable caching
    pub enabled: bool,
    /// Cache size in MB
    pub cache_size_mb: usize,
    /// Cache TTL in seconds
    pub ttl_seconds: u64,
}

/// Search filter for vector queries
pub struct SearchFilter {
    /// Metadata filters
    pub metadata_filters: HashMap<String, serde_json::Value>,
    /// Custom filter function
    pub custom_filter: Option<Box<dyn Fn(&VectorRecord) -> bool + Send + Sync>>,
}

impl std::fmt::Debug for SearchFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchFilter")
            .field("metadata_filters", &self.metadata_filters)
            .field("custom_filter", &self.custom_filter.is_some())
            .finish()
    }
}

impl Clone for SearchFilter {
    fn clone(&self) -> Self {
        Self {
            metadata_filters: self.metadata_filters.clone(),
            custom_filter: None, // Function trait objects can't be cloned
        }
    }
}

/// Search result with score and metadata
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Vector ID
    pub id: VectorId,
    /// Similarity score
    pub score: f32,
    /// Vector data (optional)
    pub vector: Option<Vec<f32>>,
    /// Metadata (optional)
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Collection ID
    pub collection_id: CollectionId,
}

/// Flush operation result
#[derive(Debug, Clone)]
pub struct FlushResult {
    /// Number of entries flushed
    pub entries_flushed: usize,
    /// Bytes written to storage
    pub bytes_written: u64,
    /// Time taken for flush operation
    pub flush_time_ms: u64,
    /// Strategy-specific metadata
    pub strategy_metadata: HashMap<String, serde_json::Value>,
}

/// Compaction operation result
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// Number of entries compacted
    pub entries_compacted: usize,
    /// Space reclaimed in bytes
    pub space_reclaimed: u64,
    /// Time taken for compaction
    pub compaction_time_ms: u64,
    /// Compaction efficiency metrics
    pub efficiency_metrics: HashMap<String, f64>,
}

/// Strategy performance statistics
#[derive(Debug, Clone)]
pub struct StrategyStats {
    /// Strategy type
    pub strategy_type: StrategyType,
    /// Total operations performed
    pub total_operations: u64,
    /// Average operation latency
    pub avg_latency_ms: f64,
    /// Index build time
    pub index_build_time_ms: u64,
    /// Search accuracy metrics
    pub search_accuracy: f64,
    /// Memory usage in bytes
    pub memory_usage_bytes: u64,
    /// Storage efficiency ratio
    pub storage_efficiency: f64,
}

/// Optimization operation result
#[derive(Debug, Clone)]
pub struct OptimizationResult {
    /// Performance improvement ratio
    pub improvement_ratio: f64,
    /// Time taken for optimization
    pub optimization_time_ms: u64,
    /// Optimization type performed
    pub optimization_type: String,
    /// Metrics before optimization
    pub before_metrics: HashMap<String, f64>,
    /// Metrics after optimization
    pub after_metrics: HashMap<String, f64>,
}

/// Collection strategy registry
pub struct StrategyRegistry {
    /// Registered strategies by collection ID
    strategies: HashMap<CollectionId, Arc<dyn CollectionStrategy>>,
    /// Default strategy factory
    default_strategy_factory:
        Box<dyn Fn(&CollectionMetadata) -> Result<Arc<dyn CollectionStrategy>> + Send + Sync>,
}

impl StrategyRegistry {
    /// Create new strategy registry
    pub fn new() -> Self {
        Self {
            strategies: HashMap::new(),
            default_strategy_factory: Box::new(|metadata| {
                // Default to standard strategy
                Ok(Arc::new(standard::StandardStrategy::new(metadata)?))
            }),
        }
    }

    /// Register strategy for collection
    pub fn register_strategy(
        &mut self,
        collection_id: CollectionId,
        strategy: Arc<dyn CollectionStrategy>,
    ) {
        self.strategies.insert(collection_id, strategy);
    }

    /// Get strategy for collection
    pub fn get_strategy(
        &self,
        collection_id: &CollectionId,
    ) -> Option<Arc<dyn CollectionStrategy>> {
        self.strategies.get(collection_id).cloned()
    }

    /// Get or create strategy for collection
    pub fn get_or_create_strategy(
        &mut self,
        collection_id: &CollectionId,
        metadata: &CollectionMetadata,
    ) -> Result<Arc<dyn CollectionStrategy>> {
        if let Some(strategy) = self.strategies.get(collection_id) {
            Ok(strategy.clone())
        } else {
            let strategy = (self.default_strategy_factory)(metadata)?;
            self.strategies
                .insert(collection_id.clone(), strategy.clone());
            Ok(strategy)
        }
    }

    /// Remove strategy for collection
    pub fn remove_strategy(
        &mut self,
        collection_id: &CollectionId,
    ) -> Option<Arc<dyn CollectionStrategy>> {
        self.strategies.remove(collection_id)
    }

    /// List all registered collections
    pub fn list_collections(&self) -> Vec<CollectionId> {
        self.strategies.keys().cloned().collect()
    }
}

impl Default for CollectionStrategyConfig {
    fn default() -> Self {
        Self {
            strategy_type: StrategyType::Standard,
            indexing_config: IndexingConfig {
                algorithm: IndexingAlgorithm::HNSW {
                    m: 16,
                    ef_construction: 200,
                    ef_search: 50,
                },
                parameters: HashMap::new(),
                secondary_algorithm: None,
            },
            storage_config: StorageConfig {
                engine_type: StorageEngineType::LSM,
                parameters: HashMap::new(),
                compression: CompressionConfig {
                    enabled: true,
                    algorithm: "lz4".to_string(),
                    level: 1,
                },
                tiering: None,
            },
            search_config: SearchConfig {
                distance_metric: DistanceMetric::Cosine,
                parameters: HashMap::new(),
                enable_optimization: true,
                caching: Some(SearchCacheConfig {
                    enabled: true,
                    cache_size_mb: 256,
                    ttl_seconds: 300,
                }),
            },
            performance_config: PerformanceConfig {
                num_threads: num_cpus::get(),
                memory_limit_mb: 1024,
                enable_simd: true,
                enable_gpu: false,
                batch_config: BatchConfig {
                    batch_size: 1000,
                    batch_timeout_ms: 100,
                    adaptive_batching: true,
                },
            },
        }
    }
}
