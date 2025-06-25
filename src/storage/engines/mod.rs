//! Storage Engines Module
//!
//! This module contains all storage engine implementations following the Strategy Pattern.
//! VIPER is the default strategy, with LSM as an alternative for comparison.

pub mod lsm;
pub mod viper;
// pub mod hybrid; // Future implementation

// Re-export main engine types
pub use lsm::LsmTree;
pub use viper::ViperCoreEngine;

// Strategy pattern exports
pub use crate::storage::traits::{
    UnifiedStorageEngine, StorageEngineStrategy,
    FlushParameters, CompactionParameters,
    FlushResult, CompactionResult,
    EngineStatistics, EngineHealth
};