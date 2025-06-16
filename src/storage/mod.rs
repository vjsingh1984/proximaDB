pub mod builder;
pub mod filesystem;
pub mod validation;
pub mod lsm;
pub mod mmap;
pub mod disk_manager;
pub mod engine;
pub mod encoding;
pub mod tiered;
// Comprehensive WAL system with strategy pattern
pub mod wal;
pub mod metadata;
pub mod search_index;
pub mod memtable;
pub mod viper;
pub mod strategy;
pub mod unified_engine;

pub use builder::{StorageSystemBuilder, StorageSystem, StorageSystemConfig};
pub use filesystem::{FilesystemFactory, FilesystemConfig};
pub use validation::ConfigValidator;
pub use engine::StorageEngine;
// WAL system exports
pub use wal::{WalManager, WalEntry, WalOperation, WalConfig, WalStrategy, WalFactory};
pub use wal::avro::AvroWalStrategy;
pub use wal::bincode::BincodeWalStrategy;
pub use metadata::{MetadataStore, CollectionMetadata, SystemMetadata};
pub use search_index::{SearchIndexManager, SearchRequest};
pub use memtable::{Memtable, MemtableEntry, MemtableOperation, MemtableCollectionStats};
pub use viper::{
    ViperConfig, ViperSchemaBuilder, ViperSchemaStrategy, 
    VectorRecordProcessor, VectorRecordSchemaAdapter,
    ViperSchemaFactory, ViperParquetFlusher, FlushResult,
    SearchStrategy, ViperStorageEngine
};
pub use unified_engine::{CollectionConfig, StorageLayoutStrategy, UnifiedStorageEngine};
use crate::core::StorageError;

pub type Result<T> = std::result::Result<T, StorageError>;