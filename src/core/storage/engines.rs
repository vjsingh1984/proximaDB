//! Storage engines and distance metrics

use serde::{Deserialize, Serialize};

/// Unified distance metric enum - used across storage engines  
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DistanceMetric {
    /// Cosine similarity (1 - cosine distance)
    Cosine,
    /// Euclidean distance (L2)
    Euclidean,
    /// Manhattan distance (L1)
    Manhattan,
    /// Dot product similarity
    DotProduct,
    /// Hamming distance for binary vectors
    Hamming,
}

impl Default for DistanceMetric {
    fn default() -> Self {
        Self::Cosine
    }
}

impl DistanceMetric {
    /// Get all available distance metrics
    pub fn all() -> &'static [DistanceMetric] {
        &[
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::Manhattan,
            DistanceMetric::DotProduct,
            DistanceMetric::Hamming,
        ]
    }
    
    /// Check if metric is suitable for normalized vectors
    pub fn supports_normalized_vectors(&self) -> bool {
        matches!(self, DistanceMetric::Cosine | DistanceMetric::DotProduct)
    }
}

/// Unified storage engine enum - supports multiple backends
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum StorageEngine {
    /// VIPER - Vector-optimized Parquet storage
    Viper,
    /// LSM Tree based storage
    Lsm,
    /// Memory-mapped file storage
    Mmap,
    /// Hybrid storage combining multiple engines
    Hybrid,
    /// In-memory only (for testing/caching)
    Memory,
    /// Object store backend (S3, Azure, GCS)
    ObjectStore,
}

impl Default for StorageEngine {
    fn default() -> Self {
        Self::Viper
    }
}

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