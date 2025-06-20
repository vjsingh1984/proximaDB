// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Indexing System
//!
//! 🔥 NEW: This module consolidates all indexing functionality from:
//! - src/storage/search_index.rs (SearchIndexManager)
//! - src/storage/viper/index.rs (ViperIndexManager, VectorIndex trait)
//!
//! ## Key Features
//! - **Unified Interface**: Single index manager for all index types
//! - **Multi-Index Support**: Multiple indexes per collection for different query patterns
//! - **Algorithm Registry**: Pluggable index algorithms (HNSW, IVF, Flat, LSH)
//! - **Intelligent Selection**: Automatic index type selection based on data characteristics
//! - **Performance Optimization**: Automatic maintenance and optimization scheduling
//! - **Comprehensive Monitoring**: Detailed statistics and performance tracking

pub mod manager;
pub mod hnsw;
pub mod algorithms;

// Re-export main components
pub use manager::{
    UnifiedIndexManager, UnifiedIndexConfig, MultiIndex, IndexBuilderRegistry,
    IndexMaintenanceScheduler, IndexStatsCollector, IndexOptimizer,
    CollectionIndexStats, GlobalIndexStats, IndexBuilder, MetadataIndexBuilder,
    MetadataIndex, BuildCostEstimate, OptimizationStrategy, OptimizationRecommendation,
    MaintenanceTask, MaintenanceTaskType, MaintenancePriority,
};

// Re-export IndexSpec from types
pub use super::types::IndexSpec;

pub use hnsw::{
    HnswIndex, HnswConfig, HnswIndexBuilder,
};

pub use algorithms::{
    FlatIndex, FlatIndexBuilder, IvfIndex, IvfConfig, IvfIndexBuilder,
    LshIndex, LshConfig,
};

/// Index manager factory for creating configured instances
pub struct IndexManagerFactory;

impl IndexManagerFactory {
    /// Create a new unified index manager with default configuration
    pub async fn create_default() -> anyhow::Result<UnifiedIndexManager> {
        let config = UnifiedIndexConfig::default();
        UnifiedIndexManager::new(config).await
    }
    
    /// Create a new unified index manager with custom configuration
    pub async fn create_with_config(config: UnifiedIndexConfig) -> anyhow::Result<UnifiedIndexManager> {
        UnifiedIndexManager::new(config).await
    }
    
    /// Create a performance-optimized index manager
    pub async fn create_performance_optimized() -> anyhow::Result<UnifiedIndexManager> {
        let config = UnifiedIndexConfig {
            max_concurrent_operations: 8,
            enable_auto_optimization: true,
            maintenance_interval_secs: 1800, // 30 minutes
            index_cache_size_mb: 1024,
            enable_multi_index: true,
            performance_monitoring: true,
            ..Default::default()
        };
        UnifiedIndexManager::new(config).await
    }
    
    /// Create a memory-optimized index manager
    pub async fn create_memory_optimized() -> anyhow::Result<UnifiedIndexManager> {
        let config = UnifiedIndexConfig {
            max_concurrent_operations: 2,
            enable_auto_optimization: false,
            maintenance_interval_secs: 7200, // 2 hours
            index_cache_size_mb: 128,
            enable_multi_index: false,
            performance_monitoring: false,
            default_index_type: crate::storage::vector::IndexType::Flat,
            ..Default::default()
        };
        UnifiedIndexManager::new(config).await
    }
    
    /// Create an accuracy-optimized index manager
    pub async fn create_accuracy_optimized() -> anyhow::Result<UnifiedIndexManager> {
        let config = UnifiedIndexConfig {
            max_concurrent_operations: 4,
            enable_auto_optimization: true,
            maintenance_interval_secs: 900, // 15 minutes
            index_cache_size_mb: 2048,
            enable_multi_index: true,
            performance_monitoring: true,
            default_index_type: crate::storage::vector::IndexType::HNSW { 
                m: 32, 
                ef_construction: 400, 
                ef_search: 100 
            },
            ..Default::default()
        };
        UnifiedIndexManager::new(config).await
    }
}

// IndexSpec is now defined in types.rs and re-exported