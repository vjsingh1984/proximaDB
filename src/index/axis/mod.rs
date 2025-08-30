// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # AXIS - Adaptive eXtensible Indexing System
//!
//! AXIS is ProximaDB's intelligent indexing layer that provides high-performance vector similarity
//! search through multiple indexing algorithms. It automatically adapts to collection characteristics
//! and query patterns, providing zero-downtime migration between strategies as data evolves.
//!
//! ## Role in ProximaDB Architecture
//!
//! AXIS serves as the primary indexing layer for vector similarity search:
//! ```text
//! Search Request → AXIS → Storage Engines
//!        ↓           ↓
//!   Index Selection  Vector Retrieval
//!        ↓           ↓
//!   Algorithm Exec   Result Ranking
//! ```
//!
//! ## Key Features
//!
//! 1. **Multiple Index Algorithms**:
//!    - **HNSW**: Hierarchical Navigable Small World graphs for high recall
//!    - **IVF**: Inverted File indexing for large-scale datasets
//!    - **LSH**: Locality Sensitive Hashing for approximate search
//!    - **Annoy**: Approximate Nearest Neighbors for static datasets
//!    - **PQ**: Product Quantization for memory-efficient indexing
//!    - **Flat**: Brute-force search for exact results
//!
//! 2. **Adaptive Intelligence**:
//!    - Automatic index selection based on data characteristics
//!    - Query pattern analysis for optimization
//!    - Zero-downtime migration between index types
//!    - Performance monitoring and tuning
//!
//! 3. **Integration Features**:
//!    - Seamless integration with storage engines
//!    - Event-driven updates via EventLog
//!    - Flush coordination with WAL system
//!    - Compaction-aware index maintenance
//!
//! ## Module Organization
//!
//! - **`indexes/`**: Core index implementations (HNSW, IVF, LSH, etc.)
//! - **`management/`**: Index lifecycle management and adaptation
//! - **`storage/`**: Index persistence and serialization
//! - **`integration/`**: Integration with storage, WAL, and compaction
//! - **`eventlog/`**: Event-driven index updates
//!
//! ## Performance Characteristics
//!
//! - **Query Latency**: < 10ms for 1M vectors (HNSW)
//! - **Index Build**: 100K vectors/sec (parallel construction)
//! - **Memory Usage**: Configurable with quantization support
//! - **Accuracy**: 95%+ recall with proper tuning
//!
//! ## Adaptive Strategy Selection
//!
//! AXIS automatically selects the optimal index based on:
//! - Collection size and dimensionality
//! - Query patterns (range, k-NN, filtered)
//! - Available memory and compute resources
//! - Accuracy requirements
//!
//! ## Zero-Downtime Migration
//!
//! When data characteristics change, AXIS can:
//! 1. Build new index in background
//! 2. Gradually shift traffic to new index
//! 3. Validate performance improvements
//! 4. Atomically switch and cleanup old index

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