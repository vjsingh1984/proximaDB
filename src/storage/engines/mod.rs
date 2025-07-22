//! Storage Engines Module
//!
//! This module contains all storage engine implementations following the Strategy Pattern.
//! VIPER is the default strategy, with LSM as an alternative for comparison.

pub mod lsm;
pub mod viper;
// pub mod hybrid; // Future implementation

// Re-export main engine types
pub use lsm::LsmTree;
pub use viper::ViperEngine;

// Strategy pattern exports
pub use crate::storage::traits::{
    CompactionParameters, CompactionResult, EngineHealth, EngineStatistics, FlushParameters,
    FlushResult, StorageEngineStrategy, UnifiedStorageEngine,
};
