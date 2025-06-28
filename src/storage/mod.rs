// =============================================================================
// ORGANIZED STORAGE MODULE STRUCTURE
// =============================================================================

pub mod builder;
pub mod validation;
pub mod traits;

// Core storage engines (organized)
pub mod engines;

// Data persistence layer (organized)
pub mod persistence;

// Unified atomic operations
pub mod atomic;

// Legacy modules removed - use organized structure instead

// Other legacy modules
pub mod encoding;
pub mod engine;
pub mod mmap;
pub mod tiered;
pub mod atomicity;
pub mod memtable;
pub mod metadata;
#[deprecated(note = "Use indexing/ instead")]
pub mod search_index;
#[deprecated(note = "Use query/ instead")]
pub mod search;
pub mod strategy;

// Vector storage system removed - functionality integrated into engines/

// Main exports from organized structure
pub use builder::{StorageSystem, StorageSystemBuilder, StorageSystemConfig};
pub use validation::ConfigValidator;

// Strategy pattern exports
pub use traits::{
    UnifiedStorageEngine, StorageEngineStrategy,
    FlushParameters, CompactionParameters,
    FlushResult as TraitFlushResult, CompactionResult,
    EngineStatistics, EngineHealth
};

// Engine exports
pub use engines::{lsm::LsmTree, viper::ViperCoreEngine};

// Persistence exports  
pub use persistence::{FilesystemConfig, FilesystemFactory, DiskManager};

// Atomic operations exports
pub use atomic::{
    UnifiedAtomicCoordinator, ViperAtomicOperations, WalAtomicOperations,
    StagingConfig, StagingOperationType, AtomicOperationMetadata, AtomicOperationStatus
};

// Legacy exports (deprecated)
// Use engines directly instead of legacy StorageEngine
pub use engine::StorageEngine;
// WAL system exports
use crate::core::StorageError;
pub use atomicity::{
    AtomicOperation, AtomicityConfig, AtomicityManager, BulkOperation, BulkVectorOperation,
    OperationResult, TransactionContext, TransactionId, VectorDeleteOperation,
    VectorInsertOperation, VectorUpdateOperation,
};
pub use memtable::{Memtable, MemtableCollectionStats, MemtableEntry, MemtableOperation};
pub use metadata::{CollectionMetadata, MetadataStore, SystemMetadata};
// 🚨 DEPRECATED EXPORTS - Use unified vector storage system instead

// SearchIndexManager removed - use indexing/ instead

// VIPER exports removed - use engines/viper/ instead
pub use persistence::wal::avro::AvroWalStrategy;
pub use persistence::wal::bincode::BincodeWalStrategy;
pub use persistence::wal::{WalConfig, WalEntry, WalFactory, WalManager, WalOperation, WalStrategy};

// Vector storage system removed - functionality integrated into engines/
// ResultProcessor has naming conflicts, import explicitly when needed

pub type Result<T> = std::result::Result<T, StorageError>;
