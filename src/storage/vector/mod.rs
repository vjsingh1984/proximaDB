// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Vector Storage System
//!
//! This module provides a consolidated, clean architecture for all vector storage
//! operations in ProximaDB. It replaces the fragmented system that was spread
//! across 21+ files with a well-organized, unified approach.
//!
//! ## Architecture Overview
//!
//! - **types.rs**: Unified type definitions (single source of truth)
//! - **coordinator.rs**: Central coordination and orchestration
//! - **search/**: Consolidated search engines and algorithms
//! - **indexing/**: Unified index management
//! - **engines/**: Storage engine implementations (VIPER, LSM, etc.)
//! - **operations/**: Common operations (insert, update, delete, search)
//!
//! ## Key Benefits
//!
//! 1. **Single Source of Truth**: All vector types defined in one place
//! 2. **Clear Separation**: Clean boundaries between concerns
//! 3. **Reduced Duplication**: Eliminates 3 search engines, multiple result types
//! 4. **Better Performance**: Unified data paths, fewer conversions
//! 5. **Easier Maintenance**: Centralized logic, consistent interfaces

pub mod types;

// Phase 1.2: Unified Search System (COMPLETE)
pub mod search;

// Phase 2: Unified Indexing System (COMPLETE)
pub mod indexing;

// Phase 3: Storage Coordinator Pattern (COMPLETE)
pub mod coordinator;

// Phase 3: Optimized Operations (COMPLETE)
pub mod operations;

// Phase 4: Consolidated Storage Engines (COMPLETE)
pub mod engines;

// Storage policy module (replaces storage tiers)
pub mod storage_policy;

// Re-export key types for easy access
pub use types::{
    SearchContext, SearchResult, SearchStrategy, MetadataFilter, FieldCondition,
    VectorOperation, OperationResult, VectorStorage, SearchAlgorithm, VectorIndex,
    IndexType, DistanceMetric, SearchDebugInfo, StorageCapabilities,
    StorageStatistics, IndexStatistics, HealthStatus, IndexSpec,
};

// Re-export storage policy types
pub use storage_policy::{
    StoragePolicy, StorageLifecycle, CompressionPolicy, StorageLocation,
    StoragePolicyManager,
};

// Re-export search system
pub use search::{
    UnifiedSearchEngine, UnifiedSearchConfig, SearchEngineFactory,
    SearchCostModel, TierSearcher, TierCharacteristics,
    ResultProcessor as SearchResultProcessor,
};

// Re-export indexing system
pub use indexing::{
    UnifiedIndexManager, UnifiedIndexConfig, IndexManagerFactory,
    MultiIndex, IndexBuilder, HnswIndex, HnswConfig,
    FlatIndex, IvfIndex, IvfConfig, CollectionIndexStats,
};

// Re-export coordinator system
pub use coordinator::{
    VectorStorageCoordinator, CoordinatorConfig, OperationRouter, PerformanceMonitor,
    RoutingStrategy, RoutingRule, RoutingCondition, VectorOperationType,
    PerformanceProfile, EngineLoad, CoordinatorStats, PerformanceAnalytics,
};

// Re-export operations system
pub use operations::{
    VectorInsertHandler, InsertConfig, VectorSearchHandler, SearchConfig,
    QueryPlanner, ResultProcessor as OperationResultProcessor,
    InsertValidator, InsertOptimizer,
};

// Re-export storage engines
pub use engines::{
    ViperCoreEngine, ViperCoreConfig, ViperCollectionMetadata,
    CompressionStats, CompressionConfig, SchemaConfig, AtomicOperationsConfig,
    VectorStorageFormat, CompressionAlgorithm, ViperCoreStats,
};

// Re-export helper functions
pub use types::{eq_filter, range_filter, and_filters, or_filters};

/// Version of the unified vector storage system
pub const VECTOR_STORAGE_VERSION: &str = "1.0.0";

/// Current phase of consolidation implementation
pub const CONSOLIDATION_PHASE: &str = "Phase 3: Storage Coordinator Pattern";