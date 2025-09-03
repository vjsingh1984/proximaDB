//! Index storage and serialization

pub mod format_strategy;
pub mod ivf_posting_list_storage;
pub mod recovery;
pub mod serialization;
pub mod universal_index_storage;

// Re-export main types
pub use serialization::{
    DeltaManager, DeltaOperation, Index, IndexCheckpoint, IndexDelta, IndexMetadata,
    IndexSerializer, SerializableIndex,
};

pub use format_strategy::{
    FormatMigration, FormatRecommender, IndexFormatStrategy, IndexSerializationFormat,
};

pub use recovery::{IndexRecoveryManager, RecoveryResult, RecoveryStrategy};

pub use ivf_posting_list_storage::{PostingEntry, PostingList, PostingListStorage};

pub use universal_index_storage::{IndexStorageConfig, UniversalIndexStorage};
