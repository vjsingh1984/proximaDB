// =============================================================================
// ORGANIZED STORAGE MODULE STRUCTURE
// =============================================================================

pub mod assignment_service;
pub mod builder;
pub mod traits;
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
pub mod strategy;

// Vector storage system removed - functionality integrated into engines/

// Main exports from organized structure
pub use builder::{StorageSystem, StorageSystemBuilder, StorageSystemConfig};
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
// atomicity exports removed - use atomic module instead
// pub use engines::lsm::memtable::{Memtable, MemtableCollectionStats, MemtableEntry, MemtableOperation}; // Moved to obsolete - using unified memtable system
pub use metadata::{CollectionMetadata, MetadataStore, SystemMetadata};
// 🚨 DEPRECATED EXPORTS - Use unified vector storage system instead

// SearchIndexManager removed - use indexing/ instead

// VIPER exports removed - use engines/viper/ instead
// Legacy strategies removed - use modern batch strategies:
// pub use persistence::wal::avro_batch::AvroWalBatchStrategy;
// pub use persistence::wal::bincode_batch::BincodeWalBatchStrategy;
pub use persistence::wal::{
    BatchId, WalConfig, WalManager, WalOperation,
    // WalEntry removed - use WalVectorBatch for batch-oriented operations instead
    // WalStrategy removed - use WalBatchStrategy with single-entry batches for individual operations
    // WalFactory removed - use WalBatchFactory for modern implementations
};

// Vector storage system removed - functionality integrated into engines/
// ResultProcessor has naming conflicts, import explicitly when needed

pub type Result<T> = std::result::Result<T, StorageError>;
