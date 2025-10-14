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
//! ```rust
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

mod types;
mod service;
mod singleton;

// Re-export all public types and functions
pub use types::{
    GlobalManifestEntry, GlobalCheckpoint, CheckpointCollectionState,
    GlobalLsnAllocator, WalEntryStatus,
};
pub use service::{GlobalManifestService, GlobalManifestServiceConfig};
pub use singleton::{
    init, get_service, get_or_init, shutdown,
    // Convenience functions
    append_async, append_sync,
    get_active_entries, get_collection_entries, get_all_entries,
    create_checkpoint, cleanup_checkpointed, mark_flushed,
};
