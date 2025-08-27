// =============================================================================
// ORGANIZED STORAGE MODULE STRUCTURE
// =============================================================================

pub mod builder;
pub mod traits;
pub mod types;
pub mod validation;

// Common reusable components
pub mod common;

// Background operation context (optimization)
pub mod background_flush_context;

// Engine capabilities and supportability checks
pub mod engine_capabilities;

// Core storage engines (organized)
pub mod engines;

// Data persistence layer (organized)
pub mod persistence;

// Unified atomic operations
pub mod transaction_coordinator;

// Core modules
pub mod engine;
// Unified memtable system
pub mod memtable;
pub mod metadata;
// Quantization now handled by unified compute module
// Storage optimization utilities
pub mod optimization;
// Strategy module for collection lifecycle configuration
pub mod strategy;
// Specialized cache system with shared infrastructure
pub mod cache;

// Lock-free implementations have been integrated into the main implementations
// TransactionCoordinator now uses DashMap for active_operations
// StorageEngine now uses DashMap for lsm_trees and mmap_readers


// Main exports from organized structure
pub use builder::{StorageSystem, StorageSystemBuilder, StorageSystemConfig};
pub use types::StorageEngineType;
pub use validation::ConfigValidator;

// Strategy pattern exports
pub use traits::{
    CompactionParameters, CompactionResult, EngineHealth, EngineStatistics, FlushParameters,
    FlushResult as TraitFlushResult, StorageEngineStrategy, UnifiedStorageEngine,
};

// Engine exports
pub use engines::impls::sst::SstStorage;
// Temporarily disabled due to arrow-arith compilation conflicts - TODO: Re-enable when resolved
// pub use engines::viper::ViperEngine;

// Persistence exports
pub use persistence::{DiskManager, FilesystemConfig, FilesystemFactory};

// Atomic operations exports
pub use transaction_coordinator::{
    TransactionalOperationMetadata, TransactionalOperationStatus, StagingConfig, TransactionStageType,
    TransactionCoordinator, ViperTransactionalOperations, WalTransactionalOperations,
};


// Storage engine exports
pub use engine::StorageEngine;
// Write Buffer system exports
use crate::core::StorageError;
pub use metadata::{CollectionMetadata, MetadataStore, SystemMetadata};
pub use persistence::write_ahead_log::{BatchId, WALConfig, WriteAheadLogManager, WALOperation};

// ResultProcessor has naming conflicts, import explicitly when needed

pub type Result<T> = std::result::Result<T, StorageError>;

// Tests module
#[cfg(test)]
mod tests;
