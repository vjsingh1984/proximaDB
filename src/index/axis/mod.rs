// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! AXIS - Adaptive eXtensible Indexing System
//!
//! A sophisticated indexing system that automatically adapts to collection
//! characteristics and query patterns, providing zero-downtime migration
//! between indexing strategies as data evolves.

// Core modules
pub mod indexes;      // Index implementations (HNSW, IVF, LSH, Annoy)
pub mod management;   // Management and orchestration
pub mod storage;      // Storage and serialization
pub mod integration;  // Integration with other systems

// Shared utilities and types
pub mod types;
pub mod utils;
pub mod clustering;
pub mod cluster_manager;
pub mod zero_overhead_vector;
pub mod compact_vector;
pub mod avro_analysis;
pub mod pattern_analyzer;
pub mod index_factory;
pub mod eventlog;
pub mod flush_integration_simple;

// Test modules
#[cfg(test)]
pub mod types_tests;
#[cfg(test)]
pub mod strategy_tests;
#[cfg(test)]
pub mod annoy_index_tests;
#[cfg(test)]
pub mod pq_index_tests;
#[cfg(test)]
pub mod flat_index_tests;
#[cfg(test)]
pub mod hybrid_index_tests;

// Re-exports for convenience
pub use management::{
    // From adaptive_engine
    AccessFrequencyMetrics, AdaptiveIndexEngine, CollectionCharacteristics, MetadataComplexity,
    PerformanceMetrics, QueryDistribution, QueryPatternAnalysis, QueryPatternType, TemporalPattern,
    // From analyzer
    CollectionAnalyzer,
    // From manager
    AxisManager, FilterOperator, HybridQuery, MetadataFilter, MigrationStatus, QueryResult,
    ScoredResult, VectorQuery,
};

pub use indexes::{
    // HNSW
    AxisHnswConfig, AxisHnswIndex, create_hnsw_index,
    // Annoy
    AxisAnnoyConfig, AxisAnnoyIndex, AnnoyStats,
    // IVF
    UnifiedIvfConfig, UnifiedIvfIndex, IvfStats, CentroidConfig, PostingListConfig,
    // LSH
    AxisLshConfig, AxisLshIndex, LshStats,
};

// Compatibility aliases for IVF (will remove after migration)
pub use indexes::{UnifiedIvfConfig as AxisIvfConfig, UnifiedIvfIndex as AxisIvfIndex};

pub use storage::{
    // Serialization
    IndexSerializer, IndexMetadata, IndexCheckpoint, IndexDelta, DeltaManager,
    Index as SerializedIndex, DeltaOperation, SerializableIndex,
    // Format strategy
    IndexFormatStrategy, IndexSerializationFormat, FormatMigration, FormatRecommender,
    // Recovery
    IndexRecoveryManager, RecoveryResult, RecoveryStrategy,
};

pub use integration::{
    // Memory tracker
    IndexMemoryTracker, IndexMemoryStatus, Index as MemTrackerIndex,
    MemoryState, EvictionReason, MemoryStats,
    // Collection state
    CollectionStateManager, CollectionTierState, TierLevel, CloudStorageType,
    // Tiering manager
    AxisTieringManager, AxisTieringConfig, TieringStats,
};

pub use types::{
    AxisConfig, PerformanceThresholds, IndexAlgorithm, Data, 
    IndexSpecification, QueryCondition, ResultCombination,
    MigrationDecision, MigrationReason, MigrationPriority,
    AlertThresholds, MonitoringConfig,
};

pub use index_factory::{AxisIndexCreationResult, AxisVectorIndex, IndexFactory, IndexStats};
pub use clustering::{
    AxisClusteringEngine, ClusterAssignment, ClusteringAlgorithm, ClusteringConfig,
    ClusteringMetrics, ClusteringModel, DBSCANConfig, HierarchicalConfig, KMeansConfig,
    KMeansInit, LinkageCriterion,
};

// Migration helpers and monitor exports
pub use management::migration_engine::{MigrationEngine, MigrationPlan, MigrationPhase};
pub use management::monitor::{AxisMonitor, MonitoringMetrics};
pub use crate::query::unified_query_optimizer::IndexCapabilities;
pub use management::strategy::{IndexStrategy, StrategySelector, StrategyRecommendation};