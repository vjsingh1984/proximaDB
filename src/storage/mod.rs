pub mod builder;
pub mod disk_manager;
pub mod encoding;
pub mod engine;
pub mod filesystem;
pub mod lsm;
pub mod mmap;
pub mod tiered;
pub mod validation;
// Comprehensive WAL system with strategy pattern
pub mod atomicity;
pub mod memtable;
pub mod metadata;
pub mod search;      // 🚨 OBSOLETE: Use vector/search/ instead (Phase 1.2)
pub mod search_index; // 🚨 OBSOLETE: Use vector/indexing/ instead (Phase 2)
pub mod strategy;
// pub mod unified_engine;  // MOVED: obsolete/storage/unified_engine.rs
pub mod viper;       // 🚨 OBSOLETE: Use vector/engines/viper.rs instead (Phase 4)
pub mod wal;

// 🔥 NEW: Unified Vector Storage System (Phase 1-4 Consolidation)
// TODO: Re-enable after fixing compilation issues
// pub mod vector;

pub use builder::{StorageSystem, StorageSystemBuilder, StorageSystemConfig};
pub use engine::StorageEngine;
pub use filesystem::{FilesystemConfig, FilesystemFactory};
pub use validation::ConfigValidator;
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
    FlushResult, SearchStrategy, VectorRecordProcessor, VectorRecordSchemaAdapter, ViperConfig,
    ViperSchemaBuilder, ViperSchemaStrategy,
};
pub use wal::avro::AvroWalStrategy;
pub use wal::bincode::BincodeWalStrategy;
pub use wal::{WalConfig, WalEntry, WalFactory, WalManager, WalOperation, WalStrategy};

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
