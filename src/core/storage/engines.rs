//! Storage engines and distance metrics

// Use the canonical DistanceMetric from compute distance module
pub use crate::compute::distance_computation::DistanceMetric;

// Use the canonical StorageEngine from proto instead of duplicate enum
pub use crate::proto::proximadb_v1::StorageEngine;

impl StorageEngine {
    /// Get all available storage engines
    pub fn all() -> &'static [StorageEngine] {
        &[
            StorageEngine::Viper,
            StorageEngine::Sst,
            StorageEngine::Mmap,
            StorageEngine::Hybrid,
        ]
    }

    /// Check if engine supports compression
    pub fn supports_compression(&self) -> bool {
        matches!(self, StorageEngine::Viper | StorageEngine::Sst)
    }

    /// Check if engine supports transactions
    pub fn supports_transactions(&self) -> bool {
        matches!(self, StorageEngine::Sst | StorageEngine::Hybrid)
    }

    /// Check if engine is persistent
    pub fn is_persistent(&self) -> bool {
        // All current engines are persistent
        true
    }
}
