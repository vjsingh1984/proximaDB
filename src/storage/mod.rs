pub mod lsm;
pub mod mmap;
pub mod disk_manager;
pub mod engine;
pub mod encoding;
pub mod tiered;
pub mod wal;
pub mod metadata;
pub mod search_index;
pub mod memtable;
pub mod viper;

pub use engine::StorageEngine;
pub use wal::{WalManager, WalEntry, WalConfig};
pub use metadata::{MetadataStore, CollectionMetadata, SystemMetadata};
pub use search_index::{SearchIndexManager, SearchRequest};
pub use memtable::{Memtable, MemtableEntry, MemtableOperation, MemtableCollectionStats};
pub use viper::{
    ViperStorageEngine, ViperConfig, ViperProgressiveSearchEngine, 
    SimdCompressionEngine, FilesystemPartitioner,
    ViperCompactionEngine, ViperVector, ViperSearchContext, ViperSearchResult
};
use crate::core::StorageError;

pub type Result<T> = std::result::Result<T, StorageError>;