//! Index storage and serialization

pub mod serialization;
pub mod format_strategy;
pub mod recovery;
pub mod ivf_posting_list_storage;
pub mod universal_index_storage;

// Re-export main types
pub use serialization::{
    IndexSerializer, IndexMetadata, IndexCheckpoint, IndexDelta, 
    DeltaManager, Index, DeltaOperation, SerializableIndex,
};

pub use format_strategy::{
    IndexFormatStrategy, IndexSerializationFormat, 
    FormatMigration, FormatRecommender,
};

pub use recovery::{
    IndexRecoveryManager, RecoveryResult, RecoveryStrategy,
};

pub use ivf_posting_list_storage::{
    PostingListStorage, PostingList, PostingEntry,
};

pub use universal_index_storage::{
    UniversalIndexStorage, IndexStorageConfig,
};