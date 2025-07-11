// =============================================================================
// ORGANIZED STORAGE MODULE STRUCTURE
// =============================================================================

pub mod assignment_service;
pub mod builder;
pub mod traits;
pub mod types;
pub mod validation;

// Core storage engines (organized)
pub mod engines;

// Data persistence layer (organized)
pub mod persistence;

// Unified atomic operations
pub mod atomic;

// Legacy modules removed - use organized structure instead

// Other legacy modules
// atomicity module moved to obsolete - use atomic module instead
pub mod encoding;
pub mod engine;
pub mod mmap;
// Unified memtable system
pub mod memtable;
pub mod metadata;
// search and search_index modules moved to obsolete
// Use query/ and indexing/ instead
// Strategy module for collection lifecycle configuration
pub mod strategy;

// Vector storage system removed - functionality integrated into engines/

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
pub use engines::{lsm::LsmTree, viper::ViperEngine};

// Persistence exports
pub use persistence::{DiskManager, FilesystemConfig, FilesystemFactory};

// Atomic operations exports
pub use atomic::{
    AtomicOperationMetadata, AtomicOperationStatus, StagingConfig, StagingOperationType,
    UnifiedAtomicCoordinator, ViperAtomicOperations, WalAtomicOperations,
};

// Legacy exports (deprecated)
// Use engines directly instead of legacy StorageEngine
pub use engine::StorageEngine;
// WAL system exports
use crate::core::StorageError;
pub use metadata::{CollectionMetadata, MetadataStore, SystemMetadata};
pub use persistence::wal::{BatchId, WalConfig, WalManager, WalOperation};

// Vector storage system removed - functionality integrated into engines/
// ResultProcessor has naming conflicts, import explicitly when needed

pub type Result<T> = std::result::Result<T, StorageError>;
