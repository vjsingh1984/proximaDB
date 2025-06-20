// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Search System
//!
//! 🔥 NEW: This module consolidates all search functionality from:
//! - src/storage/search/mod.rs (SearchEngine trait, cost optimization)
//! - src/storage/viper/search_engine.rs (Progressive search, tier management)
//! - src/storage/viper/search_impl.rs (VIPER-specific search logic)
//!
//! ## Key Features
//! - **Unified Interface**: Single search engine for all storage types
//! - **Cost Optimization**: Algorithm selection based on performance data
//! - **Progressive Search**: Tier-aware searching across storage layers
//! - **Pluggable Algorithms**: Registry-based algorithm management
//! - **Smart Caching**: Feature selection and result caching

pub mod engine;

// Re-export main components
pub use engine::{
    UnifiedSearchEngine, UnifiedSearchConfig, SearchCostModel, TierSearcher,
    TierCharacteristics, TierConstraints, ResultProcessor, MergeStrategy,
    ResultPostProcessor, ScoreNormalizer, FeatureSelectionCache,
    AlgorithmPerformanceData, CollectionCostFactors, DataDistribution,
    AccessPatterns, CostWeights,
};

/// Search engine factory for creating configured instances
pub struct SearchEngineFactory;

impl SearchEngineFactory {
    /// Create a new unified search engine with default configuration
    pub async fn create_default() -> anyhow::Result<UnifiedSearchEngine> {
        let config = UnifiedSearchConfig::default();
        UnifiedSearchEngine::new(config).await
    }
    
    /// Create a new unified search engine with custom configuration
    pub async fn create_with_config(config: UnifiedSearchConfig) -> anyhow::Result<UnifiedSearchEngine> {
        UnifiedSearchEngine::new(config).await
    }
    
    /// Create a performance-optimized search engine
    pub async fn create_performance_optimized() -> anyhow::Result<UnifiedSearchEngine> {
        let config = UnifiedSearchConfig {
            max_concurrent_searches: 16,
            enable_cost_optimization: true,
            enable_progressive_search: true,
            enable_feature_selection: true,
            cache_size: 5000,
            tier_search_parallelism: 8,
            ..Default::default()
        };
        UnifiedSearchEngine::new(config).await
    }
    
    /// Create a memory-optimized search engine
    pub async fn create_memory_optimized() -> anyhow::Result<UnifiedSearchEngine> {
        let config = UnifiedSearchConfig {
            max_concurrent_searches: 4,
            enable_cost_optimization: false,
            enable_progressive_search: false,
            enable_feature_selection: false,
            cache_size: 100,
            tier_search_parallelism: 2,
            ..Default::default()
        };
        UnifiedSearchEngine::new(config).await
    }
}