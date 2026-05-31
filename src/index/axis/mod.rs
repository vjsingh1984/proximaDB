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
pub mod indexes; // Index implementations (HNSW, IVF, LSH, Annoy)
pub mod integration;
pub mod management; // Management and orchestration
pub mod storage; // Storage and serialization // Integration with other systems

// TD-064: Filterable metadata for predicate-aware HNSW
pub mod filterable_metadata;

// Shared utilities and types
pub mod avro_analysis;
/// Cluster manager for IVF-based index partitioning.
pub mod cluster_manager;
pub mod clustering;
pub mod compact_vector;
pub mod eventlog;
pub mod flush_integration_simple;
pub mod index_factory;
pub mod pattern_analyzer;
/// Graph-tunneling predicate gate (LLD §4, GateANN arXiv 2603.21466).
pub mod tunnel;
pub mod types;
pub mod utils;
pub mod zero_overhead_vector;

// HMGI - Hierarchical Multi-modality Graph Indexing
pub mod hmgi;

// Test modules
#[cfg(test)]
pub mod annoy_index_tests;
#[cfg(test)]
pub mod flat_index_tests;
#[cfg(test)]
pub mod hybrid_index_tests;
#[cfg(test)]
pub mod pq_index_tests;
#[cfg(test)]
pub mod strategy_tests;
#[cfg(test)]
pub mod types_tests;

// Re-exports for convenience
pub use management::{
    // From adaptive_engine
    AccessFrequencyMetrics,
    AdaptiveIndexEngine,
    // From manager
    AxisManager,
    // From analyzer
    CollectionAnalyzer,
    CollectionCharacteristics,
    FilterOperator,
    HybridQuery,
    MetadataComplexity,
    MetadataFilter,
    MigrationStatus,
    PerformanceMetrics,
    QueryDistribution,
    QueryPatternAnalysis,
    QueryPatternType,
    QueryResult,
    ScoredResult,
    TemporalPattern,
    VectorQuery,
};

pub use proximadb_vector::{DisentangledVectorProjection, TransformProjectionSpec};

pub use indexes::{
    AnnoyStats,
    // Annoy
    AxisAnnoyConfig,
    AxisAnnoyIndex,
    // HNSW
    AxisHnswConfig,
    AxisHnswIndex,
    // LSH
    AxisLshConfig,
    AxisLshIndex,
    CentroidConfig,
    IvfStats,
    LshStats,
    PostingListConfig,
    // IVF
    IvfServingState,
    SerializableIvfColdTier,
    SerializableIvfConfig,
    SerializableIvfState,
    SerializableIvfStateV1,
    UnifiedIvfConfig,
    UnifiedIvfIndex,
    create_hnsw_index,
};

// Compatibility aliases for IVF (will remove after migration)
pub use indexes::{UnifiedIvfConfig as AxisIvfConfig, UnifiedIvfIndex as AxisIvfIndex};

pub use storage::{
    DeltaManager,
    DeltaOperation,
    FormatMigration,
    FormatRecommender,
    Index as SerializedIndex,
    IndexCheckpoint,
    IndexDelta,
    // Format strategy
    IndexFormatStrategy,
    IndexMetadata,
    // Recovery
    IndexRecoveryManager,
    IndexSerializationFormat,
    // Serialization
    IndexSerializer,
    RecoveryResult,
    RecoveryStrategy,
    SerializableIndex,
};

pub use integration::{
    AxisTieringConfig,
    // Tiering manager
    AxisTieringManager,
    CloudStorageType,
    // Collection state
    CollectionStateManager,
    CollectionTierState,
    EvictionReason,
    Index as MemTrackerIndex,
    IndexMemoryStatus,
    // Memory tracker
    IndexMemoryTracker,
    MemoryState,
    MemoryStats,
    TierLevel,
    TieringStats,
};

pub use types::{
    AlertThresholds, AxisConfig, Data, IndexAlgorithm, IndexSpecification, MigrationDecision,
    MigrationPriority, MigrationReason, MonitoringConfig, PerformanceThresholds, QueryCondition,
    ResultCombination,
};

pub use clustering::{
    AxisClusteringEngine, ClusterAssignment, ClusteringAlgorithm, ClusteringConfig,
    ClusteringMetrics, ClusteringModel, DBSCANConfig, HierarchicalConfig, KMeansConfig, KMeansInit,
    LinkageCriterion,
};
pub use index_factory::{AxisIndexCreationResult, AxisVectorIndex, IndexFactory, IndexStats};

// Migration helpers and monitor exports
pub use crate::query::query_optimizer::IndexCapabilities;
pub use management::migration_engine::{MigrationEngine, MigrationPhase, MigrationPlan};
pub use management::monitor::{AxisMonitor, MonitoringMetrics};
pub use management::strategy::{IndexStrategy, StrategyRecommendation, StrategySelector};

// HMGI exports
pub use hmgi::{
    ClusterMembership, ClusterNode, ClusterNodeId, CollectionTransition, DetectionResult,
    DistributedPartitionLocator, EnablementReason, HmgiMigrationEngine, HmgiMigrationPhase,
    HmgiPartitionKey, HmgiQueryCoordinator, HmgiRegistry, HmgiRouteStats, HmgiRouter,
    HmgiSearchRequest, HmgiTierPolicy, MigrationConfig, MigrationResult, MigrationState,
    ModalityDetector, ModalityExtractor, NetworkService, NodeState, PartitionMetadata,
    PartitionSet, ResultMerger, TierChangeReason, TierChangeRecommendation, TierChangeResult,
    VectorRecordSample,
};
