// =============================================================================
// ORGANIZED STORAGE MODULE STRUCTURE
// =============================================================================

pub mod builder;
pub mod traits;
pub mod types;
pub mod validation;

// Background operation context (optimization)
pub mod background_flush_context;

// Engine capabilities and supportability checks
pub mod engine_capabilities;

// Core storage engines (organized)
pub mod engines;

// Data persistence layer (organized)
pub mod persistence;

// Unified atomic operations
pub mod atomic;

// Core modules
pub mod engine;
// Unified memtable system
pub mod memtable;
pub mod metadata;
// Storage optimization utilities
pub mod optimization;
// Strategy module for collection lifecycle configuration
pub mod strategy;
// Unified cross-engine cache system for performance optimization
// Restored to improve query performance through intelligent caching
pub mod unified_cache;

// Lock-free implementations have been integrated into the main implementations
// UnifiedAtomicCoordinator now uses DashMap for active_operations
// StorageEngine now uses DashMap for lsm_trees and mmap_readers


// Main exports from organized structure
pub use builder::{StorageSystem, StorageSystemBuilder, StorageSystemConfig};
pub use types::StorageEngineType;
pub use validation::ConfigValidator;

// Strategy pattern exports
pub use traits::{
    CompactionParameters, CompactionResult, EngineHealth, EngineStatistics, FlushParameters,
    FlushResult as TraitFlushResult, StorageEngineStrategy, UnifiedStorageEngine,
};

// Engine exports
pub use engines::{sst::SstStorage, viper::ViperEngine};

// Persistence exports
pub use persistence::{DiskManager, FilesystemConfig, FilesystemFactory};

// Atomic operations exports
pub use atomic::{
    AtomicOperationMetadata, AtomicOperationStatus, StagingConfig, StagingOperationType,
    UnifiedAtomicCoordinator, ViperAtomicOperations, WalAtomicOperations,
};

// 🔴 UNUSED EXPORTS - COMMENTED OUT FOR REMOVAL  
// Cache system never integrated, only used in test files
// Unified cache system exports (Phase 2 optimization)
// pub use unified_cache::{
//     UnifiedCrossEngineCache, UnifiedCacheConfig, CacheKey, CacheDataType,
//     MemoryPressure, CrossEngineMetrics,
// };

// Storage engine exports
pub use engine::StorageEngine;
// Write Buffer system exports
use crate::core::StorageError;
pub use metadata::{CollectionMetadata, MetadataStore, SystemMetadata};
pub use persistence::write_buffer::{BatchId, WriteBufferConfig, WriteBufferManager, WriteBufferOperation};

// ResultProcessor has naming conflicts, import explicitly when needed

pub type Result<T> = std::result::Result<T, StorageError>;

// Tests module
#[cfg(test)]
mod tests;
