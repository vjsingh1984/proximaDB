//! Global WAL Manifest System
//!
//! Unified manifest module providing centralized tracking of WAL files across all collections.
//!
//! ## Architecture
//!
//! - **Global Manifest**: Single source of truth for all WAL files
//! - **Global LSN**: Monotonic sequence number across all collections
//! - **Singleton Service**: High-performance write-behind queue
//! - **Crash Safety**: Write-ahead staging and atomic updates
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::persistence::write_ahead_log::manifest;
//!
//! // Initialize during server startup
//! manifest::init(&wal_config).await?;
//!
//! // Append entry asynchronously
//! manifest::append_async(entry).await?;
//!
//! // Get entries for recovery
//! let entries = manifest::get_active_entries().await;
//! ```

mod service;
mod singleton;
mod types;

// Re-export all public types and functions
pub use service::{GlobalManifestService, GlobalManifestServiceConfig};
pub use singleton::{
    // Convenience functions
    append_async,
    append_sync,
    cleanup_checkpointed,
    create_checkpoint,
    get_active_entries,
    get_all_entries,
    get_collection_entries,
    get_or_init,
    get_service,
    init,
    mark_flushed,
    mark_flushed_and_delete_files,
    reset,
    shutdown,
};
pub use types::{
    CheckpointCollectionState, GlobalCheckpoint, GlobalLsnAllocator, GlobalManifestEntry,
    WalEntryStatus,
};
