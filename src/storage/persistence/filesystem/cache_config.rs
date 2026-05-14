//! Unified Cache Configuration
//!
//! Consolidated configuration for the unified caching filesystem that combines
//! settings from IntelligentFilesystem, ZeroCopyFilesystem, and ZeroCopyIOSystem.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// Re-export foundation compression type
pub use proximadb_compression_types::CompressionAlgorithm;

/// Unified cache configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UnifiedCacheConfig {
    /// Memory management settings
    pub memory: MemoryConfig,

    /// Disk cache settings
    pub disk: DiskCacheConfig,

    /// I/O optimization settings
    pub io: IOOptimizationConfig,

    /// Cache behavior settings
    pub behavior: CacheBehaviorConfig,

    /// Performance tuning settings
    pub performance: PerformanceConfig,
}

/// Memory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Total memory budget in MB for all caches
    pub total_budget_mb: usize,

    /// Percentage of memory for metadata cache
    pub metadata_percentage: u8,

    /// Percentage of memory for data cache
    pub data_percentage: u8,

    /// Percentage of memory for index cache
    pub index_percentage: u8,

    /// Percentage of memory for query result cache
    pub query_percentage: u8,
}

/// Disk cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskCacheConfig {
    /// Enable disk caching
    pub enabled: bool,

    /// Path to disk cache directory
    pub path: PathBuf,

    /// Maximum disk cache size in GB
    pub max_size_gb: usize,

    /// Maximum file size to cache in MB
    pub max_file_size_mb: usize,

    /// Storage tier for cache
    pub tier: StorageTier,
}

/// I/O optimization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IOOptimizationConfig {
    /// Enable range-based read optimization
    pub enable_range_optimization: bool,

    /// Enable predictive prefetching
    pub enable_prefetching: bool,

    /// Enable access pattern learning
    pub enable_pattern_learning: bool,

    /// Maximum concurrent I/O operations
    pub max_concurrent_io: usize,

    /// Threshold for range merging in bytes
    pub range_merge_threshold: usize,

    /// Minimum file size for range optimization in MB
    pub range_optimization_threshold_mb: usize,
}

/// Cache behavior configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheBehaviorConfig {
    /// Default TTL for cache entries in seconds
    pub default_ttl_secs: u64,

    /// Cache eviction policy
    pub eviction_policy: EvictionPolicy,

    /// Cache invalidation strategy
    pub invalidation_strategy: InvalidationStrategy,

    /// Enable cache warming on startup
    pub warming_enabled: bool,
}

/// Performance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Enable compression for cached data
    pub compression_enabled: bool,

    /// Compression algorithm to use
    pub compression_algorithm: CompressionAlgorithm,

    /// Enable memory-mapped I/O
    pub mmap_enabled: bool,

    /// Enable zero-copy operations
    pub zero_copy_enabled: bool,
}

/// Storage tier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageTier {
    /// L1 - Memory (fastest)
    Memory,
    /// L2 - SSD/NVMe (fast)
    SSD,
    /// L3 - HDD (slower)
    HDD,
    /// L4 - Network/Cloud (slowest)
    Network,
}

/// Cache eviction policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvictionPolicy {
    /// Least Recently Used
    LRU,
    /// Least Frequently Used
    LFU,
    /// Adaptive Replacement Cache
    ARC,
    /// First In First Out
    FIFO,
    /// Time-To-Live based
    TTL,
}

/// Cache invalidation strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InvalidationStrategy {
    /// Invalidate immediately
    Immediate,
    /// Invalidate after delay
    Delayed(u64),
    /// Invalidate on next access
    LazyInvalidation,
}

/// Workload type for preset configurations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkloadType {
    /// Maximum performance, aggressive caching
    HighPerformance,
    /// Balanced performance and resource usage
    Balanced,
    /// Minimize bandwidth and costs
    CostOptimized,
    /// Optimized for write-heavy workloads
    WriteHeavy,
    /// Optimized for read-heavy workloads
    ReadHeavy,
    /// Custom configuration
    Custom(Box<UnifiedCacheConfig>),
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            total_budget_mb: 1024,
            metadata_percentage: 40,
            data_percentage: 30,
            index_percentage: 20,
            query_percentage: 10,
        }
    }
}

impl Default for DiskCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: PathBuf::from("/tmp/proximadb_cache"),
            max_size_gb: 10,
            max_file_size_mb: 100,
            tier: StorageTier::SSD,
        }
    }
}

impl Default for IOOptimizationConfig {
    fn default() -> Self {
        Self {
            enable_range_optimization: true,
            enable_prefetching: true,
            enable_pattern_learning: true,
            max_concurrent_io: 8,
            range_merge_threshold: 4096,
            range_optimization_threshold_mb: 10,
        }
    }
}

impl Default for CacheBehaviorConfig {
    fn default() -> Self {
        Self {
            default_ttl_secs: 300,
            eviction_policy: EvictionPolicy::LRU,
            invalidation_strategy: InvalidationStrategy::Immediate,
            warming_enabled: false,
        }
    }
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            compression_enabled: false,
            compression_algorithm: CompressionAlgorithm::Lz4,
            mmap_enabled: true,
            zero_copy_enabled: true,
        }
    }
}

impl UnifiedCacheConfig {
    /// Create configuration from workload type
    pub fn from_workload(workload: WorkloadType) -> Self {
        match workload {
            WorkloadType::HighPerformance => Self::high_performance(),
            WorkloadType::Balanced => Self::balanced(),
            WorkloadType::CostOptimized => Self::cost_optimized(),
            WorkloadType::WriteHeavy => Self::write_heavy(),
            WorkloadType::ReadHeavy => Self::read_heavy(),
            WorkloadType::Custom(config) => *config,
        }
    }

    /// High performance configuration
    pub fn high_performance() -> Self {
        Self {
            memory: MemoryConfig {
                total_budget_mb: 2048,
                metadata_percentage: 50,
                data_percentage: 30,
                index_percentage: 15,
                query_percentage: 5,
            },
            disk: DiskCacheConfig {
                enabled: true,
                path: PathBuf::from("/tmp/proximadb_cache"),
                max_size_gb: 50,
                max_file_size_mb: 500,
                tier: StorageTier::SSD,
            },
            io: IOOptimizationConfig {
                enable_range_optimization: true,
                enable_prefetching: true,
                enable_pattern_learning: true,
                max_concurrent_io: 16,
                range_merge_threshold: 8192,
                range_optimization_threshold_mb: 5,
            },
            behavior: CacheBehaviorConfig {
                default_ttl_secs: 600,
                eviction_policy: EvictionPolicy::ARC,
                invalidation_strategy: InvalidationStrategy::Immediate,
                warming_enabled: true,
            },
            performance: PerformanceConfig {
                compression_enabled: false,
                compression_algorithm: CompressionAlgorithm::None,
                mmap_enabled: true,
                zero_copy_enabled: true,
            },
        }
    }

    /// Balanced configuration
    pub fn balanced() -> Self {
        Self::default()
    }

    /// Cost-optimized configuration
    pub fn cost_optimized() -> Self {
        Self {
            memory: MemoryConfig {
                total_budget_mb: 512,
                metadata_percentage: 60,
                data_percentage: 20,
                index_percentage: 15,
                query_percentage: 5,
            },
            disk: DiskCacheConfig {
                enabled: true,
                path: PathBuf::from("/tmp/proximadb_cache"),
                max_size_gb: 5,
                max_file_size_mb: 50,
                tier: StorageTier::HDD,
            },
            io: IOOptimizationConfig {
                enable_range_optimization: true,
                enable_prefetching: false,
                enable_pattern_learning: true,
                max_concurrent_io: 4,
                range_merge_threshold: 16384,
                range_optimization_threshold_mb: 20,
            },
            behavior: CacheBehaviorConfig {
                default_ttl_secs: 3600,
                eviction_policy: EvictionPolicy::LFU,
                invalidation_strategy: InvalidationStrategy::LazyInvalidation,
                warming_enabled: false,
            },
            performance: PerformanceConfig {
                compression_enabled: true,
                compression_algorithm: CompressionAlgorithm::Zstd,
                mmap_enabled: false,
                zero_copy_enabled: false,
            },
        }
    }

    /// Write-heavy workload configuration
    pub fn write_heavy() -> Self {
        Self {
            memory: MemoryConfig {
                total_budget_mb: 256,
                metadata_percentage: 70,
                data_percentage: 10,
                index_percentage: 15,
                query_percentage: 5,
            },
            disk: DiskCacheConfig {
                enabled: false, // Minimize caching for writes
                path: PathBuf::from("/tmp/proximadb_cache"),
                max_size_gb: 1,
                max_file_size_mb: 10,
                tier: StorageTier::Memory,
            },
            io: IOOptimizationConfig {
                enable_range_optimization: false,
                enable_prefetching: false,
                enable_pattern_learning: false,
                max_concurrent_io: 2,
                range_merge_threshold: 1024,
                range_optimization_threshold_mb: 50,
            },
            behavior: CacheBehaviorConfig {
                default_ttl_secs: 60,
                eviction_policy: EvictionPolicy::FIFO,
                invalidation_strategy: InvalidationStrategy::Immediate,
                warming_enabled: false,
            },
            performance: PerformanceConfig {
                compression_enabled: false,
                compression_algorithm: CompressionAlgorithm::None,
                mmap_enabled: false,
                zero_copy_enabled: false,
            },
        }
    }

    /// Read-heavy workload configuration
    pub fn read_heavy() -> Self {
        Self {
            memory: MemoryConfig {
                total_budget_mb: 4096,
                metadata_percentage: 40,
                data_percentage: 40,
                index_percentage: 15,
                query_percentage: 5,
            },
            disk: DiskCacheConfig {
                enabled: true,
                path: PathBuf::from("/tmp/proximadb_cache"),
                max_size_gb: 100,
                max_file_size_mb: 1000,
                tier: StorageTier::SSD,
            },
            io: IOOptimizationConfig {
                enable_range_optimization: true,
                enable_prefetching: true,
                enable_pattern_learning: true,
                max_concurrent_io: 32,
                range_merge_threshold: 2048,
                range_optimization_threshold_mb: 1,
            },
            behavior: CacheBehaviorConfig {
                default_ttl_secs: 1800,
                eviction_policy: EvictionPolicy::ARC,
                invalidation_strategy: InvalidationStrategy::LazyInvalidation,
                warming_enabled: true,
            },
            performance: PerformanceConfig {
                compression_enabled: true,
                compression_algorithm: CompressionAlgorithm::Lz4,
                mmap_enabled: true,
                zero_copy_enabled: true,
            },
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Check memory percentages sum to 100
        let total = self.memory.metadata_percentage
            + self.memory.data_percentage
            + self.memory.index_percentage
            + self.memory.query_percentage;

        if total != 100 {
            return Err(ConfigError::InvalidMemoryDistribution(total));
        }

        // Check paths exist if disk cache is enabled
        if self.disk.enabled && !self.disk.path.exists() {
            std::fs::create_dir_all(&self.disk.path)
                .map_err(|e| ConfigError::InvalidPath(self.disk.path.clone(), e))?;
        }

        Ok(())
    }
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Memory percentages must sum to 100, got {0}")]
    InvalidMemoryDistribution(u8),

    #[error("Invalid cache path {0}: {1}")]
    InvalidPath(PathBuf, std::io::Error),
}

// Migration from old configs is intentionally removed
// The old IntelligentFilesystem and ZeroCopyFilesystem are deprecated
// and should not be used. All new code should use UnifiedCacheConfig directly.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        let config = UnifiedCacheConfig::default();
        assert!(config.validate().is_ok());

        let mut bad_config = config.clone();
        bad_config.memory.metadata_percentage = 50;
        bad_config.memory.data_percentage = 60; // Sum > 100
        assert!(bad_config.validate().is_err());
    }

    #[test]
    fn test_workload_presets() {
        let high_perf = UnifiedCacheConfig::high_performance();
        assert_eq!(high_perf.memory.total_budget_mb, 2048);
        assert!(high_perf.io.enable_prefetching);

        let cost_opt = UnifiedCacheConfig::cost_optimized();
        assert_eq!(cost_opt.memory.total_budget_mb, 512);
        assert!(!cost_opt.io.enable_prefetching);

        let write_heavy = UnifiedCacheConfig::write_heavy();
        assert!(!write_heavy.disk.enabled);
    }
}
