//! Storage engines and distance metrics

use serde::{Deserialize, Serialize};

// Use the canonical DistanceMetric from compute distance module
pub use crate::compute::distance::DistanceMetric;

// Use the canonical StorageEngine from proto instead of duplicate enum
pub use crate::proto::proximadb::StorageEngine;

impl StorageEngine {
    /// Get all available storage engines
    pub fn all() -> &'static [StorageEngine] {
        &[
            StorageEngine::Viper,
            StorageEngine::Lsm,
            StorageEngine::Mmap,
            StorageEngine::Hybrid,
            StorageEngine::Memory,
            StorageEngine::ObjectStore,
        ]
    }
    
    /// Check if engine supports compression
    pub fn supports_compression(&self) -> bool {
        matches!(
            self, 
            StorageEngine::Viper | StorageEngine::Lsm | StorageEngine::ObjectStore
        )
    }
    
    /// Check if engine supports transactions
    pub fn supports_transactions(&self) -> bool {
        matches!(self, StorageEngine::Lsm | StorageEngine::Hybrid)
    }
    
    /// Check if engine is persistent
    pub fn is_persistent(&self) -> bool {
        !matches!(self, StorageEngine::Memory)
    }
}