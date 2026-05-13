//! Typed configuration contracts shared across ProximaDB workspace crates.
//!
//! Keep this crate limited to serializable configuration shapes. Runtime conversion and service
//! bootstrap stay in platform/root layers until those boundaries are independently extracted.

use serde::{Deserialize, Serialize};

/// Hardware acceleration configuration controlling SIMD and GPU features.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareConfig {
    /// Enable automatic hardware detection.
    pub enable_detection: bool,

    /// Enable GPU acceleration if detected.
    pub enable_gpu_acceleration: bool,

    /// Enable SIMD acceleration if detected.
    pub enable_simd: bool,

    /// Enable AVX-512 if available.
    pub enable_avx512: bool,

    /// Enable GPU for SQL parsing.
    pub enable_gpu_parsing: bool,

    /// Enable GPU for distance calculations.
    pub enable_gpu_similarity: bool,

    /// Minimum vector size to use GPU.
    pub gpu_min_vector_size: usize,

    /// Minimum batch size to use GPU.
    pub gpu_min_batch_size: usize,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            enable_detection: true,
            enable_gpu_acceleration: true,
            enable_simd: true,
            enable_avx512: true,
            enable_gpu_parsing: true,
            enable_gpu_similarity: true,
            gpu_min_vector_size: 64,
            gpu_min_batch_size: 100,
        }
    }
}

/// WAL storage configuration supporting multiple directories and cloud storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalStorageConfig {
    /// Distribution strategy for collections across storage locations.
    pub distribution_strategy: WalDistributionStrategy,

    /// Whether to keep each collection on a single WAL directory.
    pub collection_affinity: bool,

    /// Memory flush threshold per collection (bytes).
    pub memory_flush_size_bytes: usize,

    /// Global WAL size threshold for forced flush (bytes).
    pub global_flush_threshold: usize,

    /// WAL strategy type (Avro vs Bincode).
    pub strategy_type: Option<String>,

    /// Memtable type for memory structure.
    pub memtable_type: Option<String>,

    /// Sync mode for durability vs performance tradeoff.
    pub sync_mode: Option<String>,

    /// Batch threshold for operations.
    pub batch_threshold: Option<usize>,

    /// Write buffer size in MB.
    pub write_buffer_size_mb: Option<usize>,

    /// Maximum concurrent flush operations.
    pub concurrent_flushes: Option<usize>,

    /// Shrink factor for global threshold management (percentage).
    pub global_shrink_factor: Option<f64>,

    /// Global manifest location (optional - explicit configuration).
    pub global_manifest_url: Option<String>,
}

/// Strategy for distributing WAL segments across multiple storage directories.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum WalDistributionStrategy {
    /// Round-robin across WAL directories.
    RoundRobin,
    /// Hash-based distribution (consistent).
    Hash,
    /// Load-balanced distribution (dynamic).
    #[default]
    LoadBalanced,
}

impl Default for WalStorageConfig {
    fn default() -> Self {
        Self {
            global_manifest_url: None,
            distribution_strategy: WalDistributionStrategy::LoadBalanced,
            collection_affinity: true,
            memory_flush_size_bytes: 10 * 1024 * 1024,
            global_flush_threshold: 4 * 1024 * 1024 * 1024,
            strategy_type: None,
            memtable_type: None,
            sync_mode: None,
            batch_threshold: None,
            write_buffer_size_mb: None,
            concurrent_flushes: None,
            global_shrink_factor: Some(0.4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wal_storage_defaults_match_root_runtime_expectations() {
        let config = WalStorageConfig::default();

        assert!(matches!(
            config.distribution_strategy,
            WalDistributionStrategy::LoadBalanced
        ));
        assert!(config.collection_affinity);
        assert_eq!(config.memory_flush_size_bytes, 10 * 1024 * 1024);
        assert_eq!(config.global_flush_threshold, 4 * 1024 * 1024 * 1024);
        assert_eq!(config.global_shrink_factor, Some(0.4));
    }

    #[test]
    fn hardware_defaults_match_root_runtime_expectations() {
        let config = HardwareConfig::default();

        assert!(config.enable_detection);
        assert!(config.enable_gpu_acceleration);
        assert!(config.enable_simd);
        assert!(config.enable_avx512);
        assert!(config.enable_gpu_parsing);
        assert!(config.enable_gpu_similarity);
        assert_eq!(config.gpu_min_vector_size, 64);
        assert_eq!(config.gpu_min_batch_size, 100);
    }
}
