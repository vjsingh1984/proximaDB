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

// Legacy modules (being reorganized)
#[deprecated(note = "Use engines/lsm/ instead")]
pub mod lsm;
#[deprecated(note = "Use persistence/filesystem/ instead")]
pub mod filesystem;
#[deprecated(note = "Use persistence/disk_manager/ instead")]
pub mod disk_manager;
#[deprecated(note = "Use persistence/wal/ instead")]
pub mod wal;
#[deprecated(note = "Use engines/viper/ instead")]
pub mod viper;

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

// 🔥 NEW: Unified Vector Storage System (Phase 1-4 Consolidation)
pub mod vector;

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

// Legacy exports (deprecated)
#[deprecated(note = "Use engines directly instead")]
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

#[deprecated(
    since = "0.1.0",
    note = "Use `UnifiedIndexManager` from `proximadb::storage::vector::indexing` instead"
)]
pub use search_index::{SearchIndexManager, SearchRequest};

// Legacy VIPER exports (all deprecated in favor of unified vector storage)
#[deprecated(
    since = "0.1.0",
    note = "Use `ViperCoreEngine` from `proximadb::storage::vector::engines` instead"
)]
pub use viper::ViperStorageEngine;

#[deprecated(
    since = "0.1.0",
    note = "Use `ViperPipeline` from `proximadb::storage::vector::engines` instead"
)]
pub use viper::ViperParquetFlusher;

#[deprecated(
    since = "0.1.0",
    note = "Use `ViperFactory` from `proximadb::storage::vector::engines` instead"
)]
pub use viper::ViperSchemaFactory;

#[deprecated(
    since = "0.1.0",
    note = "Use unified vector storage configuration from `proximadb::storage::vector` instead"
)]
pub use viper::{
    FlushResult as ViperFlushResult, SearchStrategy, VectorRecordProcessor, VectorRecordSchemaAdapter, ViperConfig,
    ViperSchemaBuilder, ViperSchemaStrategy,
};
pub use persistence::wal::avro::AvroWalStrategy;
pub use persistence::wal::bincode::BincodeWalStrategy;
pub use persistence::wal::{WalConfig, WalEntry, WalFactory, WalManager, WalOperation, WalStrategy};

// 🔥 NEW: Unified Vector Storage System Exports
// TODO: Re-enable after fixing compilation issues
/*
pub use vector::{
    SearchContext, SearchResult, SearchStrategy as UnifiedSearchStrategy, MetadataFilter, 
    FieldCondition, VectorOperation, OperationResult as VectorOperationResult,
    VectorStorage, SearchAlgorithm, VectorIndex, IndexType, StorageTier, DistanceMetric,
    SearchDebugInfo, StorageCapabilities, StorageStatistics, IndexStatistics, HealthStatus,
    UnifiedSearchEngine, UnifiedSearchConfig, SearchEngineFactory, SearchCostModel,
    TierSearcher, TierCharacteristics,
    UnifiedIndexManager, UnifiedIndexConfig, IndexManagerFactory, MultiIndex, 
    IndexBuilder, IndexSpec, HnswIndex, HnswConfig, FlatIndex, IvfIndex, IvfConfig,
    eq_filter, range_filter, and_filters, or_filters,
};
*/
// ResultProcessor has naming conflicts, import explicitly when needed

pub type Result<T> = std::result::Result<T, StorageError>;
