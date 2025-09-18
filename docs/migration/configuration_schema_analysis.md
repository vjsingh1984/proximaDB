# Configuration Schema Analysis

## Current Configuration Structures

### 1. IntelligentFilesystem Configuration

**Location**: `storage/persistence/filesystem/intelligent_filesystem.rs`

```rust
pub struct CacheConfig {
    pub max_memory_mb: usize,        // Default: 512
    pub max_disk_gb: usize,           // Default: 10
    pub metadata_ttl_secs: u64,       // Default: 300
    pub enable_prefetch: bool,        // Default: true
    pub enable_learning: bool,        // Default: true
}

pub enum CacheStrategy {
    Adaptive,       // Default configuration
    Aggressive,     // 1GB memory, 50GB disk, 600s TTL
    Minimal,        // 128MB memory, 1GB disk, 60s TTL
    Custom(CacheConfig),
}
```

### 2. ZeroCopyIOSystem Configuration

**Location**: `storage/engines/core/io/zero_copy/config.rs`

```rust
pub struct ZeroCopyIOConfig {
    pub metadata_cache: MetadataCacheConfig,
    pub download_optimizer: DownloadOptimizerConfig,
    pub access_prediction: AccessPredictionConfig,
    pub background_tasks: BackgroundTaskConfig,
    pub performance: PerformanceConfig,
    pub cost_optimization: CostOptimizationConfig,
    pub integration: IntegrationConfig,
    pub collection_isolation: CollectionIsolation,
}

pub struct MetadataCacheConfig {
    pub max_entries: usize,           // Default: 50,000
    pub max_size_mb: usize,           // Default: 1024
    pub ttl_seconds: u64,             // Default: 300
    pub enable_mmap: bool,            // Default: true
    pub cache_directory: PathBuf,     // Default: /tmp/proximadb_cache
    pub eviction_policy: EvictionPolicy,
}

pub struct DownloadOptimizerConfig {
    pub enable_range_optimization: bool,     // Default: true
    pub range_merge_threshold_bytes: usize,  // Default: 4096
    pub max_concurrent_downloads: usize,     // Default: 8
    pub bandwidth_limit_mbps: Option<usize>, // Default: None
    pub size_thresholds: SizeBasedThresholds,
}

pub enum WorkloadType {
    HighPerformance,  // Minimize latency
    CostOptimized,    // Minimize bandwidth
    Balanced,         // Balance both
    Custom(ZeroCopyIOConfig),
}
```

### 3. CrossCacheOrchestrator Configuration

**Location**: `storage/cache/config.rs`

```rust
pub struct CacheConfig {
    pub global: GlobalCacheConfig,
    pub vector_data: VectorCacheConfig,
    pub query_result: QueryCacheConfig,
    pub filter_bitmap: FilterCacheConfig,
    pub index_structure: IndexCacheConfig,
    pub metadata: MetadataStoreConfig,
    pub coordination: CoordinationConfig,
    pub monitoring: MonitoringConfig,
}

pub struct GlobalCacheConfig {
    pub total_memory_mb: usize,       // Default: 1024
    pub enabled: bool,                // Default: true
    pub default_ttl_seconds: u64,     // Default: 3600
    pub default_eviction_policy: EvictionPolicy,
    pub enable_tiered_storage: bool,  // Default: true
    pub l2_storage_path: Option<String>,
    pub l3_storage_endpoint: Option<String>,
    pub compression: bool,            // Default: true
    pub enable_warming: bool,         // Default: false
    pub warming_source: Option<String>,
}
```

## Configuration Overlap Analysis

### Duplicated Settings

| Setting | IntelligentFilesystem | ZeroCopyIOSystem | CrossCacheOrchestrator |
|---------|---------------------|------------------|----------------------|
| Memory Limit | max_memory_mb (512) | max_size_mb (1024) | total_memory_mb (1024) |
| TTL | metadata_ttl_secs (300) | ttl_seconds (300) | default_ttl_seconds (3600) |
| Eviction | Implicit LRU | eviction_policy | default_eviction_policy |
| Prefetch | enable_prefetch | enable_range_optimization | N/A |
| Learning | enable_learning | access_prediction | N/A |
| Disk Cache | max_disk_gb | cache_directory | l2_storage_path |

### Conflicting Defaults
- Memory: 512MB vs 1024MB vs 1024MB
- TTL: 300s vs 300s vs 3600s
- Different eviction policies

### Missing Unification
- No single source of truth for cache configuration
- No coordination between different cache layers
- No unified memory budget management

## Proposed Unified Configuration

```rust
pub struct UnifiedCacheConfig {
    // Memory Management
    pub memory: MemoryConfig,

    // Disk Cache
    pub disk: DiskCacheConfig,

    // I/O Optimization
    pub io: IOOptimizationConfig,

    // Cache Behavior
    pub behavior: CacheBehaviorConfig,

    // Performance Tuning
    pub performance: PerformanceConfig,

    // Workload Preset
    pub workload: WorkloadType,
}

pub struct MemoryConfig {
    pub total_budget_mb: usize,       // Total memory for all caches
    pub metadata_percentage: u8,      // % for metadata cache
    pub data_percentage: u8,          // % for data cache
    pub index_percentage: u8,         // % for index cache
    pub query_percentage: u8,         // % for query cache
}

pub struct DiskCacheConfig {
    pub enabled: bool,
    pub path: PathBuf,
    pub max_size_gb: usize,
    pub tier: StorageTier,            // L1 (Memory), L2 (SSD), L3 (HDD)
}

pub struct IOOptimizationConfig {
    pub enable_range_optimization: bool,
    pub enable_prefetching: bool,
    pub enable_pattern_learning: bool,
    pub max_concurrent_io: usize,
    pub range_merge_threshold: usize,
}

pub struct CacheBehaviorConfig {
    pub default_ttl_secs: u64,
    pub eviction_policy: EvictionPolicy,
    pub invalidation_strategy: InvalidationStrategy,
    pub warming_enabled: bool,
}

pub struct PerformanceConfig {
    pub compression_enabled: bool,
    pub compression_algorithm: CompressionAlgorithm,
    pub mmap_enabled: bool,
    pub zero_copy_enabled: bool,
}

pub enum WorkloadType {
    HighPerformance,   // Max cache, aggressive prefetch, short TTL
    Balanced,          // Default settings
    CostOptimized,     // Min bandwidth, selective caching, long TTL
    WriteHeavy,        // Optimized for writes, minimal caching
    ReadHeavy,         // Optimized for reads, max caching
    Custom(Box<UnifiedCacheConfig>),
}
```

## Migration Mapping

### From IntelligentFilesystem
```rust
impl From<intelligent_filesystem::CacheConfig> for UnifiedCacheConfig {
    fn from(old: CacheConfig) -> Self {
        UnifiedCacheConfig {
            memory: MemoryConfig {
                total_budget_mb: old.max_memory_mb,
                metadata_percentage: 40,
                data_percentage: 40,
                index_percentage: 10,
                query_percentage: 10,
            },
            disk: DiskCacheConfig {
                enabled: old.max_disk_gb > 0,
                path: PathBuf::from("/tmp/proximadb_cache"),
                max_size_gb: old.max_disk_gb,
                tier: StorageTier::L2,
            },
            behavior: CacheBehaviorConfig {
                default_ttl_secs: old.metadata_ttl_secs,
                eviction_policy: EvictionPolicy::LRU,
                invalidation_strategy: InvalidationStrategy::Immediate,
                warming_enabled: false,
            },
            io: IOOptimizationConfig {
                enable_prefetching: old.enable_prefetch,
                enable_pattern_learning: old.enable_learning,
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
```

### From ZeroCopyIOConfig
```rust
impl From<ZeroCopyIOConfig> for UnifiedCacheConfig {
    fn from(old: ZeroCopyIOConfig) -> Self {
        UnifiedCacheConfig {
            memory: MemoryConfig {
                total_budget_mb: old.metadata_cache.max_size_mb,
                // Distribute based on old config
                ..Default::default()
            },
            io: IOOptimizationConfig {
                enable_range_optimization: old.download_optimizer.enable_range_optimization,
                max_concurrent_io: old.download_optimizer.max_concurrent_downloads,
                range_merge_threshold: old.download_optimizer.range_merge_threshold_bytes,
                ..Default::default()
            },
            performance: PerformanceConfig {
                mmap_enabled: old.metadata_cache.enable_mmap,
                zero_copy_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
```

## Configuration Priority

During migration, when multiple configurations exist:

1. **Explicit User Config**: Always takes precedence
2. **Workload Preset**: Applied if no explicit config
3. **Engine Defaults**: Engine-specific optimizations
4. **System Defaults**: Fallback values

## Environment Variables

Support environment variable overrides for operational tuning:

```bash
PROXIMADB_CACHE_MEMORY_MB=2048
PROXIMADB_CACHE_TTL_SECS=600
PROXIMADB_CACHE_DISK_GB=50
PROXIMADB_CACHE_WORKLOAD=HighPerformance
```

## Configuration Validation

```rust
impl UnifiedCacheConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Memory percentages must sum to 100
        let total = self.memory.metadata_percentage +
                   self.memory.data_percentage +
                   self.memory.index_percentage +
                   self.memory.query_percentage;
        if total != 100 {
            return Err(ConfigError::InvalidMemoryDistribution);
        }

        // Validate paths exist
        if self.disk.enabled && !self.disk.path.exists() {
            return Err(ConfigError::InvalidDiskCachePath);
        }

        Ok(())
    }
}
```

## Phase 1 Progress
- ✅ P1.1: Filesystem usage documented
- ✅ P1.2: Metadata cache dependencies mapped
- ✅ P1.3: Compatibility test suite created
- ✅ P1.4: Configuration schema documented
- Next: P1.5 - Identify performance critical paths