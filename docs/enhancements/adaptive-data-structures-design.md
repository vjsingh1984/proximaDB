# Adaptive Data Structures Design for ProximaDB

## Executive Summary

ProximaDB's shared infrastructure requires a sophisticated data structure design that can handle diverse workload patterns across indexes and caches. This document presents a comprehensive **Adaptive Data Structures Architecture** that optimizes performance based on workload characteristics while maintaining a unified interface.

## Problem Statement

### Current Challenges

1. **Diverse Workload Patterns**:
   - **Indexes**: Bulk append during compactions/flushes, occasional deletes/upserts
   - **Caches**: Read-heavy with invalidation bursts, memory pressure handling
   - **Mixed**: Unpredictable patterns requiring adaptive behavior

2. **Performance Requirements**:
   - Lock-free operations for high concurrency
   - Memory-efficient under pressure
   - Cascade invalidation for cache consistency
   - Bulk operation optimization

3. **Architectural Constraints**:
   - Single refactoring opportunity (avoid future rewrites)
   - Must handle all scenarios comprehensively
   - Maintain unified interface across use cases

## Workload Characteristics Analysis

### Index Workloads
```
Pattern: Bulk append-heavy → Moderate reads → Occasional deletes
┌─────────────────┬─────────────────┬─────────────────┐
│   Compaction    │   Search Ops    │   Cleanup       │
│   (Bulk Write)  │   (Read Heavy)  │   (Deletes)     │
│                 │                 │                 │
│ ████████████    │ ████████        │ ██              │
│ 80% of I/O      │ 15% of I/O      │ 5% of I/O       │
└─────────────────┴─────────────────┴─────────────────┘

Concurrency: High write bursts, moderate read concurrency
Memory Pressure: Predictable during compactions
```

### Cache Workloads
```
Pattern: Read-heavy → Write bursts → Invalidation cascades
┌─────────────────┬─────────────────┬─────────────────┐
│   Cache Hits    │  Memory Press.  │  Invalidation   │
│   (Read Heavy)  │  (Evictions)    │   (Cascades)    │
│                 │                 │                 │
│ ████████████    │ █████           │ ███             │
│ 70% of I/O      │ 20% of I/O      │ 10% of I/O      │
└─────────────────┴─────────────────┴─────────────────┘

Concurrency: High concurrent reads, bursty concurrent writes
Memory Pressure: Unpredictable, needs automatic eviction
```

## Architectural Design

### Core Principles

1. **Workload-Aware Optimization**: Different storage backends for different patterns
2. **Unified Interface**: Single API regardless of underlying implementation
3. **Adaptive Behavior**: Runtime adaptation based on access patterns
4. **Memory Safety**: Automatic memory management with configurable policies
5. **Lock-Free Performance**: Minimize blocking operations
6. **Multi-Tier Durability**: Guaranteed data persistence across memory, filesystem, and cloud
7. **Cross-Collection Scalability**: Global resource management beyond individual collections

### Key Architectural Insights

#### Why Multi-Tier Storage is Essential

You are absolutely correct to emphasize multi-tier storage. The design addresses several critical production requirements:

1. **Data Durability**: Cache and index data that cannot be lost must have guaranteed persistence paths to filesystem and cloud storage.

2. **Capacity Scaling**: When memory limits are exceeded across collections, the system needs automatic promotion/demotion strategies that preserve data integrity.

3. **Cross-Collection Resource Management**: A global memory manager is essential because:
   - Collections compete for shared system resources
   - Memory pressure affects all collections simultaneously  
   - Priority-based eviction ensures critical collections maintain performance
   - Resource rebalancing prevents any single collection from starving others

4. **Shared vs. Collection-Specific Logic**: The design provides both:
   - **Shared Infrastructure**: `TierManager`, `GlobalMemoryManager`, `CloudStorageTier` - common across all collections
   - **Collection-Specific Policies**: Each collection can have different tier policies, priority levels, and access patterns

#### Implementation Strategy

```rust
// Shared infrastructure that all collections use
pub struct SharedTierInfrastructure {
    /// Global filesystem provider
    filesystem: Arc<dyn FilesystemProvider>,
    /// Global cloud storage
    cloud_storage: Arc<dyn CloudStorageProvider>,
    /// Cross-collection memory management
    global_memory_manager: Arc<GlobalMemoryManager>,
}

// Collection-specific configuration
pub struct CollectionTierConfig {
    /// Collection ID
    collection_id: String,
    /// Collection priority (affects eviction order)
    priority: CollectionPriority,
    /// Memory allocation limits
    memory_budget: MemoryBudget,
    /// Tier promotion/demotion policies
    tier_policies: TierPolicies,
}
```

This approach ensures that:
- **Index modules** get write-optimized tier management with bulk operation support
- **Cache modules** get read-optimized tier management with fast promotion/demotion
- **Both share** the same filesystem and cloud infrastructure, preventing code duplication
- **Cross-collection coordination** prevents resource starvation and enables global optimization

### High-Level Architecture

```rust
┌─────────────────────────────────────────────────────────────┐
│                 AdaptiveStore<K, V>                         │
│                   (Unified Interface)                       │
├─────────────────────────────────────────────────────────────┤
│  insert() │ get() │ remove() │ handle_memory_pressure()     │
│  batch_insert() │ invalidate_cascade() │ get_metrics()     │
└─────────────┬───────────────┬───────────────┬───────────────┘
              │               │               │
    ┌─────────▼─────────┐ ┌───▼───────────▼───┐ ┌─────▼─────────┐
    │  IndexBackend     │ │  CacheBackend     │ │ HybridBackend │
    │                   │ │                   │ │               │
    │ DashMap<K,V>      │ │ Moka<K,Arc<V>>   │ │ Hot: Moka     │
    │ WriteBuffer       │ │ InvalidationSet   │ │ Cold: DashMap │
    │ FlushThreshold    │ │ PressureHandler   │ │ AutoPromotion │
    └───────────────────┘ └───────────────────┘ └───────────────┘
```

### Global Shared Infrastructure with Per-Collection Policies (FINAL ARCHITECTURE)

**Key Insights**: 
1. **ONE shared infrastructure instance per server** - not per collection
2. **Per-collection policies** determine storage constraints based on `base_location/{collection_id}/indexes/`
3. **Hierarchical constraints**: Collection's durable baseline determines maximum acceleration tiers available
4. **Never evict indexes**: Index workloads promote to persistent storage, cache workloads can evict

**Architecture Overview**:
- **GlobalTierManager**: Single instance per server managing all collections
- **CollectionStorageConfig**: Per-collection constraints parsed from metadata base_location
- **SmartTierPolicy**: Collection-specific policies within server constraints
- **Storage Hierarchy**: Memory → NVMe → HDD → CloudExpress → CloudStandard → CloudIA → CloudArchive

**Example Collection Constraints**:
```
s3://bucket/collection1/     → Baseline: CloudStandard, Max Acceleration: HDD
/mnt/disk/collection2/       → Baseline: HDD, Max Acceleration: Memory  
/mnt/nvme/collection3/       → Baseline: NVMe, Max Acceleration: Memory
/tmp/cache_collection/       → Baseline: Memory, No Acceleration
```

```rust
/// Global tier manager - ONE INSTANCE PER SERVER
/// Manages storage tiers for ALL collections with per-collection policies
pub struct GlobalTierManager {
    /// All available storage tiers on this server (detected at startup)
    available_tiers: Vec<StorageTier>,
    
    /// Global tier configurations (capacity, cost, latency per server)
    tier_configs: HashMap<StorageTier, TierConfig>,
    
    /// Per-collection policies and constraints (many per server)
    collection_policies: HashMap<String, SmartTierPolicy>,
    
    /// Global memory management across ALL collections
    global_memory_manager: GlobalMemoryManager,
    
    /// Cross-collection metrics aggregation
    metrics_collector: GlobalMetricsCollector,
}

/// Per-collection storage configuration from collection metadata
pub struct CollectionStorageConfig {
    /// Collection ID
    collection_id: String,
    
    /// Base storage URL: {base_location}/{collection_id}/indexes/
    base_location: String,
    
    /// Durable baseline tier (indexes can use faster tiers above this)
    durable_baseline: StorageTier,
    
    /// Maximum acceleration tier allowed for this collection
    max_acceleration_tier: Option<StorageTier>,
    
    /// Collection-specific resource limits
    storage_limits: CollectionStorageLimits,
}

/// Usage example:
impl GlobalTierManager {
    pub fn register_collection(
        &mut self,
        collection_id: String,
        base_location: String, // "s3://bucket/collection1/" or "/mnt/disk/collection2/"
        workload_type: WorkloadType,
    ) -> Result<()> {
        // Parse collection constraints from base_location
        let collection_config = CollectionStorageConfig::from_base_location(
            collection_id.clone(), 
            base_location
        )?;
        
        // Create collection-specific policy within server constraints
        let policy = SmartTierPolicy::for_workload_constrained(
            workload_type,
            collection_config,
            &self.available_tiers, // Server's detected tiers
            &self.tier_configs,    // Server's tier configurations
        );
        
        self.collection_policies.insert(collection_id, policy);
        Ok(())
    }
}

/// Configurable policies for different storage types
pub trait TierPolicyEngine<K, V>: Send + Sync {
    /// Determine optimal tier placement
    fn determine_placement(&self, key: &K, value: &V, access_pattern: AccessPattern) -> TierPlacement;
    
    /// Handle memory pressure (different strategies per store type)
    fn handle_memory_pressure(&self, current_usage: MemoryUsage) -> TierPressureResponse;
    
    /// Data eviction policy (caches can evict, indexes cannot)
    fn can_evict_data(&self, key: &K, access_info: &AccessInfo) -> bool;
    
    /// Promotion/demotion thresholds
    fn get_promotion_threshold(&self) -> u64;
    fn get_demotion_threshold(&self) -> u64;
}

/// Flexible tier policies with multiple criteria and storage providers
#[derive(Debug, Clone)]
pub struct FlexibleTierPolicy {
    /// Eviction criteria (size, age, access patterns)
    eviction_criteria: Vec<EvictionCriterion>,
    
    /// Promotion criteria (frequency, recency, size, business priority)
    promotion_criteria: Vec<PromotionCriterion>,
    
    /// Storage provider configurations
    storage_providers: StorageProviderConfig,
    
    /// Policy type (Index = never evict, Cache = can evict, Hybrid = adaptive)
    policy_type: TierPolicyType,
}

#[derive(Debug, Clone)]
pub enum EvictionCriterion {
    /// Size-based: evict if object larger than threshold
    Size { max_size_bytes: usize },
    
    /// Age-based: evict if older than threshold
    Age { max_age_days: u32 },
    
    /// Access-based: evict if accessed less than threshold
    AccessFrequency { min_accesses_per_day: u32 },
    
    /// Recency-based: evict if not accessed recently
    LastAccess { max_idle_hours: u32 },
    
    /// Business priority: evict low-priority data first
    Priority { min_priority_level: u8 },
    
    /// Memory pressure: evict when memory utilization exceeds threshold
    MemoryPressure { utilization_threshold: f64 },
    
    /// Collection-specific: different rules per collection
    CollectionSpecific { collection_rules: HashMap<String, Box<EvictionCriterion>> },
}

#[derive(Debug, Clone)]
pub enum PromotionCriterion {
    /// Frequency: promote frequently accessed data to faster tiers
    Frequency { access_threshold: u32, window_hours: u32 },
    
    /// Size optimization: promote small objects to memory, large to cloud
    SizeOptimized { memory_max_kb: usize, disk_max_mb: usize },
    
    /// Cost optimization: balance storage cost vs access speed
    CostOptimized { max_cost_per_gb_per_month: f64 },
    
    /// Latency SLA: promote data needed for low-latency access
    LatencySLA { max_access_time_ms: u32 },
    
    /// Geographic: promote data closer to access regions
    Geographic { preferred_regions: Vec<String> },
    
    /// Predictive: promote data likely to be accessed soon
    Predictive { ml_confidence_threshold: f64 },
}

#[derive(Debug, Clone)]
pub struct StorageProviderConfig {
    /// Cloud storage providers with different tiers
    cloud_providers: Vec<CloudStorageProvider>,
    
    /// Local filesystem configurations
    local_storage: LocalStorageConfig,
    
    /// Cross-provider replication settings
    replication: ReplicationConfig,
}

#[derive(Debug, Clone)]
pub enum CloudStorageProvider {
    /// AWS S3 with multiple storage classes
    AwsS3 {
        bucket: String,
        region: String,
        storage_class: S3StorageClass, // Standard, IA, Glacier, Deep Archive
        lifecycle_policies: Vec<S3LifecycleRule>,
    },
    
    /// Azure Blob Storage with access tiers  
    AzureBlob {
        account: String,
        container: String,
        access_tier: AzureAccessTier, // Hot, Cool, Archive
        geo_replication: bool,
    },
    
    /// Google Cloud Storage with storage classes
    GoogleCloud {
        bucket: String,
        location: String,
        storage_class: GcsStorageClass, // Standard, Nearline, Coldline, Archive
        auto_class: bool,
    },
    
    /// Multi-cloud with automatic failover
    MultiCloud {
        primary: Box<CloudStorageProvider>,
        secondaries: Vec<CloudStorageProvider>,
        failover_strategy: FailoverStrategy,
    },
}

#[derive(Debug, Clone)]
pub enum S3StorageClass {
    Standard,           // Most frequently accessed
    StandardIA,         // Infrequently accessed (30+ days)
    OneZoneIA,          // Single AZ, infrequent access
    Glacier,            // Long-term archive (90+ days)
    GlacierDeepArchive, // Rarely accessed (180+ days)
    IntelligentTiering, // Automatic optimization
}

/// Example: Index policy that never evicts, uses cost-optimized cloud tiering
impl TierPolicyEngine<K, V> for IndexTierPolicy {
    fn can_evict_data(&self, _key: &K, _access_info: &AccessInfo) -> bool {
        false // Indexes NEVER evict data - always promote to persistent storage
    }
    
    fn determine_placement(&self, key: &K, value: &V, access_pattern: AccessPattern) -> TierPlacement {
        let size = self.estimate_size(value);
        let predicted_access = self.predict_access_frequency(key, access_pattern);
        
        match (size, predicted_access) {
            // Small, frequently accessed -> Memory
            (s, freq) if s < 1024 * 1024 && freq > 100 => TierPlacement::Memory,
            
            // Medium size, moderate access -> Local SSD
            (s, freq) if s < 100 * 1024 * 1024 && freq > 10 => TierPlacement::LocalDisk,
            
            // Large or infrequent -> Cloud with appropriate tier
            (s, freq) if s > 100 * 1024 * 1024 || freq < 1 => {
                if freq < 0.1 {
                    // Very rare access -> Archive tier (Glacier/Archive)
                    TierPlacement::CloudArchive(CloudStorageProvider::AwsS3 {
                        bucket: self.archive_bucket.clone(),
                        region: self.region.clone(),
                        storage_class: S3StorageClass::GlacierDeepArchive,
                        lifecycle_policies: vec![],
                    })
                } else {
                    // Occasional access -> Standard cloud tier
                    TierPlacement::CloudStandard(CloudStorageProvider::AwsS3 {
                        bucket: self.standard_bucket.clone(),
                        region: self.region.clone(),
                        storage_class: S3StorageClass::Standard,
                        lifecycle_policies: vec![],
                    })
                }
            }
            
            _ => TierPlacement::LocalDisk, // Default fallback
        }
    }
    
    fn handle_memory_pressure(&self, usage: MemoryUsage) -> TierPressureResponse {
        // Index policy: Promote to persistent storage based on age + size
        let mut promotion_candidates = usage.get_promotion_candidates();
        
        // Sort by: 1) Size (largest first), 2) Age (oldest first), 3) Access frequency (least first)
        promotion_candidates.sort_by(|a, b| {
            b.size.cmp(&a.size)
                .then(b.age.cmp(&a.age))
                .then(a.access_frequency.cmp(&b.access_frequency))
        });
        
        TierPressureResponse::PromoteToStorage {
            candidates: promotion_candidates,
            target_bytes: usage.excess_memory(),
            strategy: PromotionStrategy::SizeAndAgeBased,
            destination: self.determine_optimal_storage_tier(&promotion_candidates),
        }
    }
}

/// Example: Cache policy with aggressive eviction but smart local disk usage
impl TierPolicyEngine<K, V> for CacheTierPolicy {
    fn can_evict_data(&self, key: &K, access_info: &AccessInfo) -> bool {
        // Multi-criteria eviction decision
        self.eviction_criteria.iter().any(|criterion| {
            match criterion {
                EvictionCriterion::Size { max_size_bytes } => 
                    access_info.size_bytes > *max_size_bytes,
                EvictionCriterion::Age { max_age_days } => 
                    access_info.age_days() > *max_age_days,
                EvictionCriterion::AccessFrequency { min_accesses_per_day } => 
                    access_info.accesses_per_day() < *min_accesses_per_day,
                EvictionCriterion::LastAccess { max_idle_hours } => 
                    access_info.hours_since_last_access() > *max_idle_hours,
                EvictionCriterion::MemoryPressure { utilization_threshold } => 
                    self.current_memory_utilization() > *utilization_threshold,
                _ => false,
            }
        })
    }
    
    fn handle_memory_pressure(&self, usage: MemoryUsage) -> TierPressureResponse {
        // Cache policy: Smart tiering based on access patterns and costs
        if self.has_local_disk_capacity() {
            TierPressureResponse::PromoteToFilesystem {
                // Promote frequently accessed large objects to local disk
                target_bytes: usage.excess_memory() * 60 / 100, // 60% to disk
                strategy: PromotionStrategy::FrequencyAndSizeBased,
            }
        } else {
            TierPressureResponse::Mixed {
                // 20% to cloud (for later retrieval), 80% evict (can regenerate)
                promote_to_cloud: usage.excess_memory() * 20 / 100,
                evict: usage.excess_memory() * 80 / 100,
                cloud_provider: CloudStorageProvider::GoogleCloud {
                    bucket: self.cache_backup_bucket.clone(),
                    location: "us-central1".to_string(),
                    storage_class: GcsStorageClass::Nearline, // Cheap but accessible
                    auto_class: true,
                },
            }
        }
    }
}
```

### Storage Backend Implementations

#### 1. IndexBackend - Write-Optimized with Universal Tiering

```rust
pub struct IndexBackend<K, V> {
    // SHARED INFRASTRUCTURE: Universal tiering with index policies
    tier_manager: UniversalTierManager<K, V>,
    
    // INDEX-SPECIFIC: Write optimization - batch writes before committing
    write_buffer: RwLock<Vec<(K, V)>>,
    flush_threshold: usize,
    
    // INDEX-SPECIFIC: Metrics for bulk operations
    bulk_write_count: AtomicU64,
    individual_write_count: AtomicU64,
}

impl<K, V> IndexBackend<K, V> {
    pub fn new(config: IndexConfig) -> Result<Self> {
        // Create index-specific tier policy
        let policy = Box::new(IndexTierPolicy {
            // Index policy: NEVER evict, always promote to persistent storage
            eviction_policy: EvictionPolicy::NeverEvict,
            promotion_strategy: PromotionStrategy::LeastRecentlyUsed,
            filesystem_buffer_size: config.filesystem_buffer_mb * 1024 * 1024,
            cloud_archive_threshold_days: config.cloud_archive_days,
        });
        
        // Create universal tier manager with index policy
        let tier_manager = UniversalTierManager::new(policy, config.resource_limits)?;
        
        Ok(Self {
            tier_manager,
            write_buffer: RwLock::new(Vec::new()),
            flush_threshold: config.flush_threshold,
            bulk_write_count: AtomicU64::new(0),
            individual_write_count: AtomicU64::new(0),
        })
    }

    pub fn insert(&self, key: K, value: V) -> Result<()> {
        let mut buffer = self.write_buffer.write().unwrap();
        buffer.push((key, value));
        
        if buffer.len() >= self.flush_threshold {
            // Bulk flush using SHARED TIER INFRASTRUCTURE
            let batch: Vec<_> = buffer.drain(..).collect();
            drop(buffer); // Release lock early
            
            for (k, v) in batch {
                // Universal tier manager handles placement (memory/disk/cloud)
                self.tier_manager.insert_with_policy(k, v, AccessPattern::BulkWrite)?;
            }
            self.bulk_write_count.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
    
    pub fn get(&self, key: &K) -> Option<V> {
        // Check write buffer first (most recent writes)
        if let Some(value) = self.write_buffer.read().unwrap()
            .iter()
            .rev()  // Search from most recent
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone()) {
            return Some(value);
        }
        
        // Use SHARED TIER INFRASTRUCTURE for multi-tier lookup
        // Will check: memory -> local disk -> cloud (based on index policy)
        self.tier_manager.get_with_promotion(key, AccessPattern::IndexRead)
    }
    
    pub fn handle_memory_pressure(&self) -> Result<usize> {
        // Delegate to SHARED TIER INFRASTRUCTURE with index-specific policy
        let response = self.tier_manager.handle_memory_pressure()?;
        
        match response {
            TierPressureResponse::PromoteToFilesystem { items_moved, .. } => {
                // Index policy: promote to disk instead of evicting
                Ok(items_moved)
            }
            TierPressureResponse::PromoteToCloud { items_moved, .. } => {
                // Very high pressure: promote to cloud
                Ok(items_moved)
            }
            TierPressureResponse::Evict { .. } => {
                // Index policy should NEVER evict - this indicates a policy bug
                Err(anyhow!("Index backend should never evict data"))
            }
        }
    }
    
    pub fn force_flush(&self) -> usize {
        let mut buffer = self.write_buffer.write().unwrap();
        let count = buffer.len();
        
        let batch: Vec<_> = buffer.drain(..).collect();
        drop(buffer);
        
        for (k, v) in batch {
            // Use shared tiering for immediate persistence
            let _ = self.tier_manager.insert_with_policy(k, v, AccessPattern::FlushOperation);
        }
        
        count
    }
}
```

**Performance Characteristics**:
- **Bulk Insert**: Excellent (batched writes reduce contention)
- **Single Insert**: Good (buffered, deferred to bulk operation)
- **Read Performance**: Good (two-tier lookup: buffer → primary)
- **Memory Efficiency**: Good (bounded buffer, configurable threshold)
- **Concurrency**: Excellent (lock-free primary, minimal buffer locking)

#### 2. CacheBackend - Read-Optimized with Automatic Eviction

```rust
pub struct CacheBackend<K, V> {
    // Primary cache with automatic eviction
    l1: Moka<K, Arc<V>>,
    
    // Invalidation tracking for cascade operations
    invalidation_tracker: DashSet<K>,
    
    // Memory pressure callback
    memory_pressure_handler: Arc<dyn Fn() + Send + Sync>,
    
    // Cache-specific metrics
    hit_count: AtomicU64,
    miss_count: AtomicU64,
    eviction_count: AtomicU64,
    invalidation_count: AtomicU64,
}

impl<K, V> CacheBackend<K, V> {
    pub fn insert(&self, key: K, value: V) -> Result<()> {
        self.l1.insert(key, Arc::new(value));
        Ok(())
    }
    
    pub fn get(&self, key: &K) -> Option<V> {
        match self.l1.get(key) {
            Some(arc_value) => {
                self.hit_count.fetch_add(1, Ordering::Relaxed);
                Some((*arc_value).clone())
            },
            None => {
                self.miss_count.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }
    
    pub fn invalidate_cascade(&self, keys: &[K]) -> Result<usize> {
        let mut invalidated = 0;
        
        for key in keys {
            self.l1.invalidate(key);
            self.invalidation_tracker.insert(key.clone());
            invalidated += 1;
        }
        
        self.invalidation_count.fetch_add(invalidated as u64, Ordering::Relaxed);
        Ok(invalidated)
    }
    
    pub fn handle_memory_pressure(&self) -> Result<usize> {
        // Moka handles this automatically, but we can trigger explicit cleanup
        (self.memory_pressure_handler)();
        
        // Return approximate number of entries after cleanup
        Ok(self.l1.entry_count() as usize)
    }
    
    pub fn get_hit_rate(&self) -> f64 {
        let hits = self.hit_count.load(Ordering::Relaxed);
        let misses = self.miss_count.load(Ordering::Relaxed);
        
        if hits + misses == 0 {
            0.0
        } else {
            hits as f64 / (hits + misses) as f64
        }
    }
}
```

**Performance Characteristics**:
- **Insert**: Excellent (optimized for cache workloads)
- **Read Performance**: Excellent (Moka's optimized lock-free reads)
- **Memory Efficiency**: Excellent (automatic eviction, memory-aware)
- **Eviction Support**: Automatic (LRU, LFU, or custom policies)
- **Invalidation**: Cascade-aware with tracking

#### 3. HybridBackend - Multi-Tier Storage with Filesystem and Cloud Support

```rust
pub struct HybridBackend<K, V> {
    // Memory tiers (fastest access)
    hot_tier: Moka<K, Arc<V>>,           // L1: Most frequently accessed
    warm_tier: DashMap<K, Arc<V>>,       // L2: Moderately accessed
    
    // Persistent tiers (guaranteed durability)
    local_storage: LocalFilesystemTier<K, V>,    // L3: Local SSD/HDD storage
    cloud_storage: CloudStorageTier<K, V>,       // L4: Cloud object storage
    
    // Tier management
    tier_manager: TierManager<K, V>,
    
    // Adaptive thresholds per tier
    hot_promotion_threshold: AtomicU64,
    warm_promotion_threshold: AtomicU64,
    cold_demotion_threshold: AtomicU64,
    
    // Cross-collection data management
    collection_metadata: DashMap<String, CollectionTierState>,
    global_memory_tracker: Arc<GlobalMemoryTracker>,
    
    // Access pattern tracking
    access_frequency: DashMap<K, AtomicU64>,
    access_recency: DashMap<K, AtomicU64>,
    
    // Performance metrics per tier
    tier_metrics: TierMetrics,
}

impl<K, V> HybridBackend<K, V> {
    pub fn insert(&self, key: K, value: V) -> Result<()> {
        // Adaptive placement based on access patterns
        if self.is_hot_key(&key) {
            self.hot_tier.insert(key, Arc::new(value));
        } else {
            self.cold_tier.insert(key, value);
        }
        Ok(())
    }
    
    pub fn get(&self, key: &K) -> Option<V> {
        // Check hot tier first
        if let Some(value) = self.hot_tier.get(key) {
            self.hot_hits.fetch_add(1, Ordering::Relaxed);
            self.record_access(key);
            return Some((*value).clone());
        }
        
        // Check cold tier, consider promotion
        if let Some(entry) = self.cold_tier.get(key) {
            let value = entry.value().clone();
            self.cold_hits.fetch_add(1, Ordering::Relaxed);
            
            // Record access and check for promotion
            let access_count = self.record_access(key);
            let promotion_threshold = self.promotion_threshold.load(Ordering::Relaxed);
            
            if access_count >= promotion_threshold {
                // Promote to hot tier
                self.hot_tier.insert(key.clone(), Arc::new(value.clone()));
                self.cold_tier.remove(key);
                self.promotions.fetch_add(1, Ordering::Relaxed);
            }
            
            return Some(value);
        }
        
        None
    }
    
    fn record_access(&self, key: &K) -> u64 {
        self.access_frequency
            .entry(key.clone())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed) + 1
    }
    
    fn is_hot_key(&self, key: &K) -> bool {
        self.access_frequency
            .get(key)
            .map(|freq| freq.load(Ordering::Relaxed) >= self.promotion_threshold.load(Ordering::Relaxed))
            .unwrap_or(false)
    }
    
    pub fn handle_memory_pressure(&self) -> Result<usize> {
        // Strategy 1: Clear access frequency tracking
        let freq_entries = self.access_frequency.len();
        self.access_frequency.clear();
        
        // Strategy 2: Demote from hot to cold (Moka handles hot tier automatically)
        // The demotion happens naturally through Moka's eviction
        
        // Strategy 3: Adjust thresholds to be more aggressive
        let current_promotion = self.promotion_threshold.load(Ordering::Relaxed);
        self.promotion_threshold.store(current_promotion * 2, Ordering::Relaxed);
        
        Ok(freq_entries)
    }
}
```

### Multi-Tier Storage Architecture

The hybrid backend supports comprehensive multi-tier storage spanning memory, local storage, and cloud storage to ensure data durability and handle capacity constraints across collections.

#### Tier Hierarchy and Data Flow

```rust
/// Comprehensive tier management for data that cannot be lost
pub struct TierManager<K, V> {
    /// Tier configuration
    config: MultiTierConfig,
    
    /// Filesystem integration
    filesystem: Arc<dyn FilesystemProvider>,
    
    /// Cloud storage integration  
    cloud_provider: Arc<dyn CloudStorageProvider>,
    
    /// Cross-collection memory management
    global_memory_manager: Arc<GlobalMemoryManager>,
    
    /// Promotion/demotion policies
    tier_policies: TierPolicies,
}

/// Multi-tier data flow strategy
impl<K, V> TierManager<K, V> {
    /// Comprehensive promotion/demotion logic
    pub async fn manage_tier_placement(&self, key: &K, value: &V, access_pattern: AccessPattern) -> Result<TierPlacement> {
        match self.analyze_optimal_placement(key, value, access_pattern) {
            TierPlacement::Hot => {
                // High-frequency access: keep in memory (Moka L1)
                self.ensure_hot_tier_capacity().await?;
                Ok(TierPlacement::Hot)
            }
            TierPlacement::Warm => {
                // Moderate access: memory with potential eviction (DashMap L2)
                self.ensure_warm_tier_capacity().await?;
                Ok(TierPlacement::Warm)
            }
            TierPlacement::Cold => {
                // Infrequent access: local filesystem (SSD/HDD L3)
                self.promote_to_local_storage(key, value).await?;
                Ok(TierPlacement::Cold)
            }
            TierPlacement::Archive => {
                // Very rare access: cloud storage (S3/GCS/Azure L4)  
                self.archive_to_cloud_storage(key, value).await?;
                Ok(TierPlacement::Archive)
            }
        }
    }
    
    /// Cross-collection memory pressure handling
    pub async fn handle_global_memory_pressure(&self) -> Result<MemoryReclamationReport> {
        let mut report = MemoryReclamationReport::new();
        
        // Strategy 1: Demote from hot to warm tier
        let hot_demotions = self.demote_least_accessed_hot_items(1000).await?;
        report.add_demotions("hot_to_warm", hot_demotions);
        
        // Strategy 2: Demote from warm to local filesystem  
        let warm_demotions = self.demote_warm_to_local_storage(5000).await?;
        report.add_demotions("warm_to_local", warm_demotions);
        
        // Strategy 3: Archive old local data to cloud
        let archive_count = self.archive_old_local_data().await?;
        report.add_demotions("local_to_cloud", archive_count);
        
        // Strategy 4: Cross-collection balancing
        let balanced_collections = self.rebalance_across_collections().await?;
        report.add_rebalancing(balanced_collections);
        
        Ok(report)
    }
}
```

#### Filesystem Integration

```rust
/// Local filesystem tier for guaranteed persistence
pub struct LocalFilesystemTier<K, V> {
    /// Base storage path
    storage_path: PathBuf,
    
    /// Serialization strategy
    serializer: Arc<dyn TierSerializer<K, V>>,
    
    /// File organization strategy  
    file_manager: FileManager,
    
    /// Local caching for recent filesystem access
    filesystem_cache: Moka<K, CachedFileEntry<V>>,
    
    /// Compression for storage efficiency
    compression_engine: CompressionEngine,
}

impl<K, V> LocalFilesystemTier<K, V> 
where
    K: Serialize + DeserializeOwned + Hash + Eq + Clone,
    V: Serialize + DeserializeOwned + Clone,
{
    /// Store data with guaranteed persistence
    pub async fn store_persistent(&self, key: K, value: V, collection_id: &str) -> Result<()> {
        let storage_key = self.generate_storage_key(&key, collection_id);
        let file_path = self.file_manager.get_file_path(&storage_key);
        
        // Ensure directory exists
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        
        // Serialize and compress
        let serialized = self.serializer.serialize(&key, &value)?;
        let compressed = self.compression_engine.compress(&serialized)?;
        
        // Atomic write with backup
        self.atomic_write_with_backup(&file_path, &compressed).await?;
        
        // Update filesystem cache
        self.filesystem_cache.insert(key.clone(), CachedFileEntry {
            value: value.clone(),
            file_path: file_path.clone(),
            last_modified: SystemTime::now(),
        });
        
        Ok(())
    }
    
    /// Retrieve with automatic promotion to memory if frequently accessed
    pub async fn retrieve_with_promotion(&self, key: &K, collection_id: &str) -> Result<Option<V>> {
        // Check filesystem cache first
        if let Some(cached) = self.filesystem_cache.get(key) {
            // Check if file was modified externally
            if self.is_file_current(&cached.file_path, cached.last_modified).await? {
                return Ok(Some(cached.value.clone()));
            }
        }
        
        // Load from disk
        let storage_key = self.generate_storage_key(key, collection_id);
        let file_path = self.file_manager.get_file_path(&storage_key);
        
        if !file_path.exists() {
            return Ok(None);
        }
        
        let compressed_data = tokio::fs::read(&file_path).await?;
        let serialized_data = self.compression_engine.decompress(&compressed_data)?;
        let (_, value) = self.serializer.deserialize(&serialized_data)?;
        
        // Update filesystem cache
        self.filesystem_cache.insert(key.clone(), CachedFileEntry {
            value: value.clone(),
            file_path,
            last_modified: SystemTime::now(),
        });
        
        Ok(Some(value))
    }
}
```

#### Cloud Storage Integration

```rust
/// Cloud storage tier for archival and cross-region replication
pub struct CloudStorageTier<K, V> {
    /// Cloud provider implementation
    provider: Arc<dyn CloudStorageProvider>,
    
    /// Bucket/container configuration
    storage_config: CloudStorageConfig,
    
    /// Serialization with cloud-optimized compression
    serializer: Arc<dyn CloudSerializer<K, V>>,
    
    /// Local cache for cloud data
    cloud_cache: Moka<K, CloudCacheEntry<V>>,
    
    /// Asynchronous upload/download queue
    transfer_queue: Arc<TransferQueue>,
}

impl<K, V> CloudStorageTier<K, V> 
where
    K: Serialize + DeserializeOwned + Hash + Eq + Clone,
    V: Serialize + DeserializeOwned + Clone,
{
    /// Archive data to cloud with redundancy
    pub async fn archive_with_redundancy(&self, key: K, value: V, collection_id: &str) -> Result<CloudArchiveResult> {
        let cloud_key = self.generate_cloud_key(&key, collection_id);
        
        // Serialize with cloud-optimized compression
        let serialized = self.serializer.serialize_for_cloud(&key, &value)?;
        
        // Multi-region upload for redundancy
        let upload_tasks = self.storage_config.regions.iter().map(|region| {
            let provider = self.provider.clone();
            let key = cloud_key.clone();
            let data = serialized.clone();
            let region = region.clone();
            
            async move {
                provider.upload_to_region(&key, &data, &region).await
            }
        });
        
        // Wait for majority of uploads to succeed
        let results = futures::future::join_all(upload_tasks).await;
        let successful_uploads = results.iter().filter(|r| r.is_ok()).count();
        
        if successful_uploads >= (self.storage_config.regions.len() / 2 + 1) {
            // Update cloud cache
            self.cloud_cache.insert(key.clone(), CloudCacheEntry {
                value: value.clone(),
                cloud_key: cloud_key.clone(),
                regions: self.storage_config.regions.clone(),
                uploaded_at: SystemTime::now(),
            });
            
            Ok(CloudArchiveResult {
                key: cloud_key,
                regions_uploaded: successful_uploads,
                total_regions: self.storage_config.regions.len(),
                redundancy_level: successful_uploads as f64 / self.storage_config.regions.len() as f64,
            })
        } else {
            Err(anyhow!("Failed to achieve minimum redundancy for cloud archive"))
        }
    }
    
    /// Retrieve from cloud with automatic failover
    pub async fn retrieve_with_failover(&self, key: &K, collection_id: &str) -> Result<Option<V>> {
        // Check cloud cache first
        if let Some(cached) = self.cloud_cache.get(key) {
            return Ok(Some(cached.value.clone()));
        }
        
        let cloud_key = self.generate_cloud_key(key, collection_id);
        
        // Try regions in order of preference (latency-based)
        for region in &self.storage_config.regions {
            match self.provider.download_from_region(&cloud_key, region).await {
                Ok(data) => {
                    let (_, value) = self.serializer.deserialize_from_cloud(&data)?;
                    
                    // Update cloud cache
                    self.cloud_cache.insert(key.clone(), CloudCacheEntry {
                        value: value.clone(),
                        cloud_key: cloud_key.clone(),
                        regions: self.storage_config.regions.clone(),
                        uploaded_at: SystemTime::now(), // Approximate
                    });
                    
                    return Ok(Some(value));
                }
                Err(e) => {
                    tracing::warn!("Failed to retrieve from region {}: {}", region, e);
                    continue;
                }
            }
        }
        
        Ok(None)
    }
}
```

#### Cross-Collection Memory Management

```rust
/// Global memory management across all collections
pub struct GlobalMemoryManager {
    /// Total memory budget
    total_memory_budget: AtomicUsize,
    
    /// Current memory usage by collection
    collection_usage: DashMap<String, AtomicUsize>,
    
    /// Memory allocation strategy
    allocation_strategy: MemoryAllocationStrategy,
    
    /// Priority-based collection ranking
    collection_priorities: DashMap<String, CollectionPriority>,
}

impl GlobalMemoryManager {
    /// Rebalance memory across collections when pressure occurs
    pub async fn rebalance_collections(&self, required_memory: usize) -> Result<RebalanceResult> {
        let mut rebalance_result = RebalanceResult::new();
        
        // Get current usage by collection
        let mut collection_stats: Vec<_> = self.collection_usage
            .iter()
            .map(|entry| {
                let collection_id = entry.key().clone();
                let usage = entry.value().load(Ordering::Relaxed);
                let priority = self.collection_priorities.get(&collection_id)
                    .map(|p| p.value().clone())
                    .unwrap_or_default();
                CollectionMemoryStats { collection_id, usage, priority }
            })
            .collect();
            
        // Sort by priority (low priority collections evicted first)
        collection_stats.sort_by_key(|stats| stats.priority.value());
        
        let mut memory_reclaimed = 0;
        for stats in collection_stats {
            if memory_reclaimed >= required_memory {
                break;
            }
            
            // Calculate how much to reclaim from this collection
            let target_reclaim = (required_memory - memory_reclaimed).min(stats.usage / 2);
            
            // Perform collection-specific memory reclaim
            let reclaimed = self.reclaim_from_collection(&stats.collection_id, target_reclaim).await?;
            memory_reclaimed += reclaimed;
            
            rebalance_result.add_collection_reclaim(stats.collection_id, reclaimed);
        }
        
        Ok(rebalance_result)
    }
}
```

**Enhanced Performance Characteristics**:
- **Insert**: Excellent (intelligent tier placement)
- **Read Performance**: Excellent (multi-tier caching with promotion)
- **Memory Efficiency**: Excellent (automatic tier management)
- **Data Durability**: Guaranteed (filesystem + cloud redundancy)
- **Cross-Collection Scalability**: Excellent (global memory management)
- **Adaptability**: Excellent (runtime tier optimization)
- **Complex Workloads**: Optimal (handles any scale and pattern)

## Unified Interface Implementation

### Core AdaptiveStore Structure

```rust
pub struct AdaptiveStore<K, V> 
where 
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    // Storage backend
    backend: StorageBackend<K, V>,
    
    // Workload pattern detection
    pattern: WorkloadPattern,
    workload_detector: AtomicWorkloadDetector,
    
    // Unified metrics
    metrics: WorkloadMetrics,
    
    // Configuration
    config: AdaptiveConfig,
}

pub enum StorageBackend<K, V> {
    Index(IndexBackend<K, V>),
    Cache(CacheBackend<K, V>),
    Hybrid(HybridBackend<K, V>),
}

pub enum WorkloadPattern {
    IndexStore,    // Bulk append-heavy with occasional deletes
    CacheStore,    // Read-heavy with invalidation bursts  
    HybridStore,   // Mixed workload with adaptive behavior
    AutoDetect,    // Runtime detection and adaptation
}

pub struct WorkloadMetrics {
    // Operation counters
    read_count: AtomicU64,
    write_count: AtomicU64,
    delete_count: AtomicU64,
    batch_operation_count: AtomicU64,
    
    // Performance metrics
    avg_read_latency_ns: AtomicU64,
    avg_write_latency_ns: AtomicU64,
    
    // Memory and cache metrics
    memory_pressure_events: AtomicU64,
    invalidation_cascades: AtomicU64,
    
    // Hit rates (for cache backends)
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
}

pub struct AdaptiveConfig {
    // Index backend config
    write_buffer_size: usize,
    flush_threshold: usize,
    
    // Cache backend config
    cache_capacity: u64,
    eviction_policy: EvictionPolicy,
    memory_limit_mb: usize,
    
    // Hybrid backend config
    hot_tier_capacity: u64,
    promotion_threshold: u64,
    demotion_threshold: u64,
    
    // Adaptive behavior config
    workload_detection_window_ms: u64,
    adaptation_threshold: f64,
}
```

### Unified API Implementation

```rust
impl<K, V> AdaptiveStore<K, V>
where 
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    // Constructor with workload pattern specification
    pub fn new(pattern: WorkloadPattern, config: AdaptiveConfig) -> Result<Self> {
        let backend = match pattern {
            WorkloadPattern::IndexStore => {
                StorageBackend::Index(IndexBackend::new(config.clone())?)
            },
            WorkloadPattern::CacheStore => {
                StorageBackend::Cache(CacheBackend::new(config.clone())?)
            },
            WorkloadPattern::HybridStore => {
                StorageBackend::Hybrid(HybridBackend::new(config.clone())?)
            },
            WorkloadPattern::AutoDetect => {
                // Start with hybrid, adapt based on usage
                StorageBackend::Hybrid(HybridBackend::new(config.clone())?)
            },
        };
        
        Ok(Self {
            backend,
            pattern,
            workload_detector: AtomicWorkloadDetector::new(),
            metrics: WorkloadMetrics::new(),
            config,
        })
    }
    
    // Core operations with unified interface
    pub fn insert(&self, key: K, value: V) -> Result<()> {
        let start = std::time::Instant::now();
        
        let result = match &self.backend {
            StorageBackend::Index(backend) => backend.insert(key, value),
            StorageBackend::Cache(backend) => backend.insert(key, value),
            StorageBackend::Hybrid(backend) => backend.insert(key, value),
        };
        
        // Update metrics
        self.metrics.write_count.fetch_add(1, Ordering::Relaxed);
        let latency = start.elapsed().as_nanos() as u64;
        self.update_write_latency(latency);
        
        // Update workload detection
        self.workload_detector.record_write();
        
        result
    }
    
    pub fn get(&self, key: &K) -> Option<V> {
        let start = std::time::Instant::now();
        
        let result = match &self.backend {
            StorageBackend::Index(backend) => backend.get(key),
            StorageBackend::Cache(backend) => backend.get(key),
            StorageBackend::Hybrid(backend) => backend.get(key),
        };
        
        // Update metrics
        self.metrics.read_count.fetch_add(1, Ordering::Relaxed);
        let latency = start.elapsed().as_nanos() as u64;
        self.update_read_latency(latency);
        
        // Update cache hit/miss metrics
        match result {
            Some(_) => self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed),
            None => self.metrics.cache_misses.fetch_add(1, Ordering::Relaxed),
        };
        
        // Update workload detection
        self.workload_detector.record_read();
        
        result
    }
    
    pub fn batch_insert(&self, items: Vec<(K, V)>) -> Result<usize> {
        let start = std::time::Instant::now();
        
        let result = match &self.backend {
            StorageBackend::Index(backend) => {
                // Optimized for bulk operations
                for (key, value) in items.iter() {
                    backend.insert(key.clone(), value.clone())?;
                }
                backend.force_flush(); // Ensure immediate visibility
                Ok(items.len())
            },
            StorageBackend::Cache(backend) => {
                // Individual inserts for cache
                let mut inserted = 0;
                for (key, value) in items {
                    backend.insert(key, value)?;
                    inserted += 1;
                }
                Ok(inserted)
            },
            StorageBackend::Hybrid(backend) => {
                // Mixed approach
                let mut inserted = 0;
                for (key, value) in items {
                    backend.insert(key, value)?;
                    inserted += 1;
                }
                Ok(inserted)
            },
        };
        
        // Update metrics
        self.metrics.batch_operation_count.fetch_add(1, Ordering::Relaxed);
        self.workload_detector.record_batch_write(items.len());
        
        result
    }
    
    pub fn remove(&self, key: &K) -> Option<V> {
        let result = match &self.backend {
            StorageBackend::Index(backend) => backend.remove(key),
            StorageBackend::Cache(backend) => backend.remove(key),
            StorageBackend::Hybrid(backend) => backend.remove(key),
        };
        
        if result.is_some() {
            self.metrics.delete_count.fetch_add(1, Ordering::Relaxed);
        }
        
        result
    }
    
    // Memory pressure handling
    pub fn handle_memory_pressure(&self) -> Result<MemoryPressureReport> {
        let start = std::time::Instant::now();
        
        let result = match &self.backend {
            StorageBackend::Index(backend) => {
                let flushed = backend.force_flush();
                MemoryPressureReport {
                    items_affected: flushed,
                    bytes_freed: 0, // Approximate
                    strategy: "flush_write_buffer".to_string(),
                }
            },
            StorageBackend::Cache(backend) => {
                let remaining = backend.handle_memory_pressure()?;
                MemoryPressureReport {
                    items_affected: remaining,
                    bytes_freed: 0, // Moka handles this internally
                    strategy: "automatic_eviction".to_string(),
                }
            },
            StorageBackend::Hybrid(backend) => {
                let affected = backend.handle_memory_pressure()?;
                MemoryPressureReport {
                    items_affected: affected,
                    bytes_freed: 0,
                    strategy: "tier_demotion".to_string(),
                }
            },
        };
        
        self.metrics.memory_pressure_events.fetch_add(1, Ordering::Relaxed);
        
        Ok(result)
    }
    
    // Cascade invalidation for cache workloads
    pub fn invalidate_cascade(&self, keys: &[K]) -> Result<usize> {
        let result = match &self.backend {
            StorageBackend::Cache(backend) => {
                backend.invalidate_cascade(keys)
            },
            StorageBackend::Hybrid(backend) => {
                backend.invalidate_cascade(keys)
            },
            StorageBackend::Index(_) => {
                // For index workloads, just remove
                let mut removed = 0;
                for key in keys {
                    if self.remove(key).is_some() {
                        removed += 1;
                    }
                }
                Ok(removed)
            },
        };
        
        self.metrics.invalidation_cascades.fetch_add(1, Ordering::Relaxed);
        result
    }
    
    // Performance metrics and monitoring
    pub fn get_metrics(&self) -> WorkloadMetricsSnapshot {
        WorkloadMetricsSnapshot {
            read_count: self.metrics.read_count.load(Ordering::Relaxed),
            write_count: self.metrics.write_count.load(Ordering::Relaxed),
            delete_count: self.metrics.delete_count.load(Ordering::Relaxed),
            batch_operation_count: self.metrics.batch_operation_count.load(Ordering::Relaxed),
            
            avg_read_latency_ns: self.metrics.avg_read_latency_ns.load(Ordering::Relaxed),
            avg_write_latency_ns: self.metrics.avg_write_latency_ns.load(Ordering::Relaxed),
            
            cache_hit_rate: self.get_cache_hit_rate(),
            memory_pressure_events: self.metrics.memory_pressure_events.load(Ordering::Relaxed),
            invalidation_cascades: self.metrics.invalidation_cascades.load(Ordering::Relaxed),
            
            backend_specific: self.get_backend_metrics(),
        }
    }
    
    fn get_cache_hit_rate(&self) -> f64 {
        let hits = self.metrics.cache_hits.load(Ordering::Relaxed);
        let misses = self.metrics.cache_misses.load(Ordering::Relaxed);
        
        if hits + misses == 0 {
            0.0
        } else {
            hits as f64 / (hits + misses) as f64
        }
    }
    
    // Adaptive behavior - runtime optimization
    pub fn adapt_to_workload(&mut self) -> Result<AdaptationReport> {
        let current_pattern = self.workload_detector.detect_pattern();
        
        if current_pattern != self.pattern && self.should_adapt(&current_pattern) {
            // Perform backend migration
            let migration_result = self.migrate_backend(current_pattern)?;
            
            Ok(AdaptationReport {
                old_pattern: self.pattern.clone(),
                new_pattern: current_pattern,
                migration_success: migration_result.success,
                items_migrated: migration_result.items_migrated,
                migration_duration_ms: migration_result.duration_ms,
            })
        } else {
            Ok(AdaptationReport::no_change())
        }
    }
}

// Supporting types
pub struct MemoryPressureReport {
    pub items_affected: usize,
    pub bytes_freed: usize,
    pub strategy: String,
}

pub struct WorkloadMetricsSnapshot {
    pub read_count: u64,
    pub write_count: u64,
    pub delete_count: u64,
    pub batch_operation_count: u64,
    pub avg_read_latency_ns: u64,
    pub avg_write_latency_ns: u64,
    pub cache_hit_rate: f64,
    pub memory_pressure_events: u64,
    pub invalidation_cascades: u64,
    pub backend_specific: BackendMetrics,
}

pub enum BackendMetrics {
    Index { 
        flush_count: u64, 
        buffer_utilization: f64 
    },
    Cache { 
        eviction_count: u64, 
        memory_utilization: f64 
    },
    Hybrid { 
        promotions: u64, 
        demotions: u64, 
        hot_tier_hit_rate: f64 
    },
}
```

## Implementation Roadmap

### Phase 1: Foundation (Week 1)
- [ ] Implement basic `AdaptiveStore` structure
- [ ] Create `IndexBackend` with DashMap + write buffer
- [ ] Add unified metrics collection with `MetricsUpdate` integration
- [ ] Basic unit tests for index workloads
- [x] **Metrics Framework Integration**: Complete integration specification with existing metrics system

### Phase 2: Cache Backend (Week 2)
- [ ] Implement `CacheBackend` with Moka
- [ ] Add invalidation cascade support with metrics tracking
- [ ] Memory pressure handling with performance monitoring
- [ ] Unit tests for cache workloads
- [ ] **Metrics Integration**: Implement CacheMetricsSnapshot compatibility

### Phase 3: Hybrid Backend (Week 3)
- [ ] Implement `HybridBackend` with dual tiers
- [ ] Add adaptive promotion/demotion logic with metrics-driven decisions
- [ ] Access pattern tracking with real-time analytics
- [ ] Unit tests for mixed workloads
- [ ] **Metrics Integration**: Multi-tier performance tracking

### Phase 4: Integration (Week 4)  
- [ ] Integrate with existing HNSW, Annoy, IVF, LSH indexes

## Final Architecture: Rule-Based Tier Management (2025-08-08)

### Scaling Concerns Addressed

After analyzing the per-collection policy approach, it became clear that storing individual `SmartTierPolicy` instances for each collection would cause significant scaling issues. Instead, we've implemented a **Rule-Based Tier Policy** system that:

1. **Eliminates Per-Collection Storage**: No hash maps storing policies for thousands of collections
2. **Uses Server-Wide Rules**: Single set of rules applied based on data characteristics  
3. **Configurable via Server Config**: Rules can be configured through `config.toml` (not exposed via API initially)
4. **Default Disk Paths**: Collections get predictable paths like `/tmp/{collection_id}/` by default
5. **Future API Extensibility**: Architecture supports exposing configuration through Collection Create API if needed

### Rule-Based Architecture

```
                    ProximaDB Server
    ┌─────────────────────────────────────────────────────┐
    │              GlobalTierManager                      │
    │  ┌─────────────────────────────────────────────────┐│
    │  │           RuleBasedTierPolicy                   ││
    │  │                                                 ││
    │  │  Rules:                      Target Tiers:     ││
    │  │  • >100 accesses/day    →    Memory (L1)       ││
    │  │  • 10-100 accesses/day  →    NVMe (L2)         ││
    │  │  • 1-10 accesses/day    →    HDD (L3)          ││
    │  │  • <1 access/day        →    Evict/Archive     ││
    │  │                                                 ││
    │  │  Paths: /tmp/{collection_id}/{tier}/            ││
    │  └─────────────────────────────────────────────────┘│
    └─────────────────────────────────────────────────────┘
                          │
              ┌───────────┼───────────┐
              │           │           │
        Collection1   Collection2  Collection3
         (any tier)    (any tier)   (any tier)
```

### Data Flow Diagrams

#### Index Workload - Data Promotion/Demotion (REVISED)

```
Index Cache Lifecycle (CAN EVICT - Durability Guaranteed by AXIS Storage)

CRITICAL INSIGHT: AXIS indexes maintain durability at {baseurl}/{collectionid}/indexes/
Therefore, cache/memory tiers can safely evict since data can be reloaded from AXIS storage!

Hot Data (>100 accesses/day)
┌─────────────┐    Memory Pressure    ┌─────────────┐    Disk Full    ┌─────────────┐
│   Memory    │ ───────────────────→  │    NVMe     │ ─────────────→  │     HDD     │
│    (L1)     │   (Demote or Evict)   │    (L2)     │  (Demote/Evict) │    (L3)     │
│ Cache Tier  │                       │ Cache Tier  │                 │ Cache Tier  │
└─────────────┘                       └─────────────┘                 └─────────────┘
      ↑ ↓                                   ↑ ↓                             ↑ ↓
      │ │ Promotion/Demotion                │ │                             │ │
      │ │ Based on Access                   │ │                             │ │
      │ │                                   │ │                             │ │
┌─────────────┐                       ┌─────────────┐                 ┌─────────────┐
│   EVICTED   │                       │   EVICTED   │                 │ AXIS Source │
│ (Can Reload │                       │ (Can Reload │                 │{baseurl}/   │
│ from AXIS)  │                       │ from AXIS)  │                 │{collection}/│
└─────────────┘                       └─────────────┘                 │indexes/     │
                                                                       └─────────────┘
                            ↑
                            │ Source of Truth
                            │ (WAL/Flush/Compaction Updates)
                            │
                    ┌───────────────────┐
                    │   AXIS Index       │
                    │  Durable Storage   │
                    │ Always Consistent  │
                    └───────────────────┘

Rule: Index cache tiers CAN evict since AXIS maintains durability at {baseurl}/{collectionid}/indexes/
```

#### Cache Workload - Data Eviction/Promotion

```
Cache Data Lifecycle (Can Evict or Promote Based on Access Patterns)

Hot Data (>50 accesses/day)
┌─────────────┐    Memory Pressure    ┌─────────────┐    Disk Full    ┌─────────────┐
│   Memory    │ ───────────────────→  │    NVMe     │ ─────────────→  │     HDD     │
│    (L1)     │   (Promote if hot)    │    (L2)     │  (Promote)      │    (L3)     │
│ /tmp/c1/mem │                       │/tmp/c1/nvme │                 │ /tmp/c1/hdd │
└─────────────┘                       └─────────────┘                 └─────────────┘
      ↓                                      ↓                               ↓
      │ Low Access Frequency                 │ Very Low Access               │ No Access
      │ (<10 accesses/day)                   │ (<1 access/day)               │ (>90 days)
      │                                      │                                │
      ↓                                      ↓                               ↓
┌─────────────┐                       ┌─────────────┐                 ┌─────────────┐
│   EVICTED   │                       │   EVICTED   │                 │   EVICTED   │
│  (Memory    │                       │ (Disk Space │                 │ (Archived   │
│  Reclaimed) │                       │  Reclaimed) │                 │  or Purged) │
└─────────────┘                       └─────────────┘                 └─────────────┘

Rule: Cache backends can evict data to reclaim resources
```

#### Hybrid Workload - Adaptive Behavior

```
Hybrid Data Lifecycle (Switches Between Index and Cache Behaviors)

              Workload Pattern Detection
                        │
            ┌───────────┼───────────┐
            │                       │
     Read-Heavy (>80%)      Write-Heavy (>50%)
     Cache Behavior          Index Behavior
            │                       │
            ↓                       ↓
    ┌─────────────┐           ┌─────────────┐
    │ Moka Cache  │           │  DashMap    │
    │ + Eviction  │           │ + Promotion │
    │ + LRU/TTL   │           │ + Buffering │
    └─────────────┘           └─────────────┘
            │                       │
            │     Mixed Pattern     │
            │     (40-60% each)     │
            └───────────┼───────────┘
                        │
                        ↓
              ┌─────────────┐
              │ Dual-Tier   │
              │ Hot: Moka   │
              │ Cold: Disk  │
              └─────────────┘

Rule: Hybrid backends adapt based on detected workload patterns
```

### Directory Structure Examples

```bash
# Default server configuration (base_disk_path = "/tmp")
/tmp/
├── collection_user_vectors/
│   ├── nvme/          # L2 tier (if NVMe available)
│   │   ├── index_data
│   │   └── cache_data
│   ├── hdd/           # L3 tier  
│   │   ├── index_data
│   │   └── cache_data
│   └── archive/       # L4+ tiers (future cloud integration)
│
├── collection_product_embeddings/
│   ├── nvme/
│   ├── hdd/
│   └── archive/
│
└── collection_search_cache/
    ├── nvme/
    ├── hdd/
    └── archive/

# Custom server configuration (base_disk_path = "/mnt/proximadb")  
/mnt/proximadb/
├── collection1/
│   ├── nvme/
│   ├── hdd/
│   └── archive/
└── collection2/
    ├── nvme/
    ├── hdd/
    └── archive/
```

### Implementation Examples

#### Creating Adaptive Stores

```rust
use proximadb::common::adaptive_structures::*;
use proximadb::common::tier_policy_engine::*;

// Server startup - create global tier manager
let server_config = ServerTierConfig {
    base_disk_path: "/mnt/proximadb".to_string(),
    base_nvme_path: Some("/mnt/nvme".to_string()),
    max_memory_bytes: 8 * 1024 * 1024 * 1024, // 8GB
    enable_cloud_storage: false, // Disabled initially
    default_cloud_provider: None,
};

let global_tier_manager = Arc::new(GlobalTierManager::with_config(server_config));
let universal_tier_manager = Arc::new(UniversalTierManager::new(global_tier_manager).await?);
let factory = AdaptiveStoreFactory::new(universal_tier_manager);

// Create index store for HNSW
let index_config = AdaptiveStoreConfig {
    collection_id: "user_vectors".to_string(),
    backend_type: BackendType::Index {
        structure: IndexStructure::DashMap {
            initial_capacity: 10000,
            memory_limit_mb: Some(512),
        },
        tier_policy: IndexTierPolicy {
            min_tier: StorageTier::Memory,
            promotion_threshold: 100, // 100 accesses/day  
            max_acceleration_tier: StorageTier::NvmeSsd,
        },
    },
    // ... tier and metrics config
};

let index_store = factory.create_store::<String, VectorRecord>(
    "user_vectors".to_string(), 
    Some(index_config)
).await?;

// Create cache store for query results
let cache_config = AdaptiveStoreConfig {
    collection_id: "query_cache".to_string(),
    backend_type: BackendType::Cache {
        structure: CacheStructure::Moka {
            max_capacity: 100000,
            time_to_live: Some(Duration::from_secs(3600)),
            time_to_idle: Some(Duration::from_secs(1800)),
        },
        tier_policy: CacheTierPolicy {
            eviction_policy: EvictionPolicy::Lru { max_entries: 100000 },
            // ... promotion/demotion criteria
        },
    },
    // ... tier and metrics config
};

let cache_store = factory.create_store::<String, QueryResult>(
    "query_cache".to_string(),
    Some(cache_config)
).await?;

// Use the stores
index_store.insert("vector1".to_string(), vector_record).await?;
let result = cache_store.get(&query_key).await;

// Trigger tier rebalancing
index_store.rebalance_tiers().await?;
cache_store.rebalance_tiers().await?;
```

#### Rule-Based Tier Decisions

```rust
// The RuleBasedTierPolicy determines tier placement automatically
let rule_policy = RuleBasedTierPolicy::default();

// For a frequently accessed vector (120 accesses/day, 2 days old)
let target_tier = rule_policy.determine_tier(&WorkloadPattern::ReadHeavy, 120.0, 2);
assert_eq!(target_tier, 1); // Memory tier

// For a moderately accessed vector (25 accesses/day, 10 days old) 
let target_tier = rule_policy.determine_tier(&WorkloadPattern::Mixed, 25.0, 10);
assert_eq!(target_tier, 2); // NVMe tier

// For a cold vector (0.5 accesses/day, 45 days old)
let target_tier = rule_policy.determine_tier(&WorkloadPattern::WriteHeavy, 0.5, 45);
assert_eq!(target_tier, 3); // HDD tier

// Get storage path for collection
let path = rule_policy.get_collection_path("user_vectors", 2);
assert_eq!(path, "/tmp/nvme/user_vectors");
```

### Intelligent Tiering with Durability-Aware Promotion

#### Core Principle: Never Duplicate, Always Accelerate
The tiering system intelligently avoids redundant storage by understanding where durable data exists and only promoting to tiers that provide acceleration benefits.

**Key Design Decisions:**
1. **Eviction is configurable** - Not always mandatory, depends on durability location
2. **Smart promotion** - Only promote to tiers faster than durable storage location
3. **Ephemeral by design** - Shared infrastructure can be lost without data loss
4. **Flexible policies** - Support eviction, non-eviction, promotion, demotion, or any combination
5. **Cost-aware** - Avoid double storage costs by not duplicating durable data

#### Scenario-Based Tiering Strategies

```
SCENARIO 1: Durable Storage on Local Disk
─────────────────────────────────────────
Durable Location: /mnt/ssd/{collection}/indexes/

Tiering Strategy:
┌─────────────┐
│   Memory    │ ← Buffer hot data (EVICT ON PRESSURE)
│  Ephemeral  │   
└─────────────┘
      ↓ No promotion to disk (already durable there!)
┌─────────────┐
│  Disk Cache │ ← SKIP THIS TIER (would duplicate)
│   NOT USED  │   
└─────────────┘
      ↓
┌─────────────┐
│ Durable Disk│ ← Source of truth
│ /mnt/ssd/   │   
└─────────────┘

Rule: Only use memory for acceleration, evict when needed
Rationale: No point duplicating data already on local disk
```

```
SCENARIO 2: Durable Storage on Cloud (S3 Standard)
───────────────────────────────────────────────────
Durable Location: s3://bucket/{collection}/indexes/

Tiering Strategy:
┌─────────────┐     Pressure    ┌─────────────┐     Pressure    ┌─────────────┐
│   Memory    │ ───────────────→│  Local Disk │ ───────────────→│  S3 Express │
│  Ephemeral  │     PROMOTE      │  Ephemeral  │     PROMOTE      │  Ephemeral  │
│ (EVICT: Yes)│                 │ (EVICT: Yes)│                 │(EVICT: Maybe)│
└─────────────┘                 └─────────────┘                 └─────────────┘
      ↑                               ↑                               ↑
      │ All faster than S3 Standard, so all provide acceleration     │
      └───────────────────────────────────────────────────────────────┘
                                      ↓
                            ┌─────────────┐
                            │ S3 Standard │ ← Durable source
                            │   (Slower)  │   
                            └─────────────┘

Rule: Promote through Memory→Disk→S3Express for acceleration
Rationale: All tiers faster than durable storage provide value
```

```
SCENARIO 3: Mixed Durability (Hybrid Cloud)
───────────────────────────────────────────
Some data durable on disk, some on cloud

Collection A (disk-durable):           Collection B (cloud-durable):
┌─────────────┐                        ┌─────────────┐
│   Memory    │                        │   Memory    │
│ Evict: Yes  │                        │ Evict: Yes  │
└─────────────┘                        └─────────────┘
      ↓ NO                                   ↓ YES
┌─────────────┐                        ┌─────────────┐
│ Disk (Skip) │                        │ Disk Cache  │
└─────────────┘                        │ Evict: Yes  │
      ↓                                └─────────────┘
┌─────────────┐                              ↓
│Durable Disk │                        ┌─────────────┐
└─────────────┘                        │Cloud Durable│
                                       └─────────────┘

Rule: Per-collection tiering based on durability location
```

#### Comprehensive Tiering Policy Matrix

```
┌──────────────────┬────────────────┬────────────────┬─────────────────┬──────────────┐
│ Durable Location │ Memory Tier    │ NVMe/SSD Tier  │ HDD Tier        │ Cloud Express│
├──────────────────┼────────────────┼────────────────┼─────────────────┼──────────────┤
│ Memory (tmpfs)   │ USE (no evict) │ PROMOTE        │ PROMOTE         │ PROMOTE      │
│                  │                │ (persistence)  │ (persistence)   │ (durability) │
├──────────────────┼────────────────┼────────────────┼─────────────────┼──────────────┤
│ Local NVMe       │ USE (evict)    │ SKIP           │ PROMOTE         │ PROMOTE      │
│                  │ (buffer only)  │ (redundant)    │ (if slower)     │ (if needed)  │
├──────────────────┼────────────────┼────────────────┼─────────────────┼──────────────┤
│ Local HDD        │ USE (evict)    │ USE (evict)    │ SKIP            │ PROMOTE      │
│                  │ (acceleration) │ (acceleration) │ (redundant)     │ (if faster)  │
├──────────────────┼────────────────┼────────────────┼─────────────────┼──────────────┤
│ S3 Express       │ USE (evict)    │ USE (evict)    │ USE (evict)     │ SKIP/USE*    │
│                  │ (acceleration) │ (acceleration) │ (acceleration)  │ (see note)   │
├──────────────────┼────────────────┼────────────────┼─────────────────┼──────────────┤
│ S3 Standard      │ USE (evict)    │ USE (evict)    │ USE (evict)     │ USE (evict)  │
│                  │ (acceleration) │ (acceleration) │ (acceleration)  │ (acceleration)│
├──────────────────┼────────────────┼────────────────┼─────────────────┼──────────────┤
│ S3 Glacier       │ USE (evict)    │ USE (evict)    │ USE (evict)     │ USE (evict)  │
│                  │ (hot cache)    │ (warm cache)   │ (cold cache)    │ (faster tier)│
└──────────────────┴────────────────┴────────────────┴─────────────────┴──────────────┘

* S3 Express Note: Can be used for long-running servers but data fidelity not guaranteed 
  across restarts (KMS changes, zone failures). Treat as ephemeral acceleration tier.
```

#### Intelligent Promotion Rules

```rust
// Smart tier promotion that avoids redundancy
impl RuleBasedTierPolicy {
    pub fn should_promote(&self, 
        current_tier: StorageTier,
        target_tier: StorageTier,
        durable_location: StorageTier,
    ) -> PromotionDecision {
        
        // Never promote to same tier as durable location (redundant)
        if target_tier == durable_location {
            return PromotionDecision::Skip { 
                reason: "Would duplicate durable storage" 
            };
        }
        
        // Only promote to tiers faster than durable location
        if !target_tier.is_faster_than(&durable_location) {
            return PromotionDecision::Skip { 
                reason: "No acceleration benefit" 
            };
        }
        
        // Check if promotion makes sense based on access pattern
        match (current_tier, target_tier) {
            (Memory, Disk) if durable_location.is_cloud() => {
                // Promote memory to disk for cloud-durable data
                PromotionDecision::Promote { 
                    evict_on_pressure: true 
                }
            },
            (Memory, Disk) if durable_location.is_local() => {
                // Skip disk tier for local-durable data
                PromotionDecision::Skip { 
                    reason: "Data already durable on local disk" 
                }
            },
            (Disk, CloudExpress) if durable_location == CloudStandard => {
                // Use Express tier for acceleration
                PromotionDecision::Promote { 
                    evict_on_pressure: false // Can keep for long-running 
                }
            },
            _ => PromotionDecision::Evaluate { 
                based_on: "access_frequency, cost, latency" 
            }
        }
    }
}
```

#### Restart Recovery Strategies

```
RESTART SCENARIO A: Lost all ephemeral tiers
────────────────────────────────────────────
Action: Lazy load from durable storage on access

┌─────────────┐     Cache Miss    ┌─────────────┐
│   Memory    │ ←────────────────│   Request   │
│   (Empty)   │                  └─────────────┘
└─────────────┘                         ↓
      ↓                          ┌─────────────┐
Load from durable ──────────────→│ Durable Src │
                                 │ (Disk/Cloud)│
                                 └─────────────┘

RESTART SCENARIO B: Partial tier loss
──────────────────────────────────────
Action: Validate remaining tiers, reload missing

┌─────────────┐                  ┌─────────────┐
│   Memory    │                  │  Disk Cache │
│   (Lost)    │                  │ (Preserved) │
└─────────────┘                  └─────────────┘
      ↓                                ↑
   Reload hot data                Validate checksums
      ↓                                ↓
┌─────────────────────────────────────────────┐
│          Durable Storage (Source)           │
└─────────────────────────────────────────────┘

RESTART SCENARIO C: Cloud tier uncertainty
──────────────────────────────────────────
Action: Treat S3 Express as untrusted, reload

┌─────────────┐
│ S3 Express  │ ← May have stale/corrupt data
│ (Untrusted) │   (KMS rotation, zone change)
└─────────────┘
      ↓
  Invalidate & Reload
      ↓
┌─────────────┐
│ S3 Standard │ ← Always trust durable tier
│  (Trusted)  │
└─────────────┘
```

#### Simplified Policy: Everything Can Evict

```rust
// BEFORE (Complex): Different policies for index vs cache
enum EvictionPolicy {
    NeverEvict,        // For indexes (WRONG - not needed!)
    LruEvict,          // For caches
    SizeBasedEvict,    // For caches
}

// AFTER (Simple): Unified eviction for all workloads
enum EvictionPolicy {
    LruEvict { max_entries: usize },        // Works for both!
    SizeBasedEvict { max_memory_mb: usize }, // Works for both!
    TimeBasedEvict { max_age: Duration },    // Works for both!
}

// Both index and cache backends use same eviction logic
impl AdaptiveStore for IndexBackend {
    async fn handle_memory_pressure(&self) {
        // Can evict - data reloadable from AXIS storage
        self.evict_coldest_entries().await;
    }
}

impl AdaptiveStore for CacheBackend {
    async fn handle_memory_pressure(&self) {
        // Can evict - standard cache behavior
        self.evict_coldest_entries().await;
    }
}
```

### Restartability and Recovery

#### Startup Sequence with AXIS Durability

```
Server Restart Sequence:

1. INITIALIZE GlobalTierManager
   ├── Load server config from config.toml
   ├── Detect available tiers (Memory, NVMe, HDD)
   └── Initialize RuleBasedTierPolicy

2. SCAN AXIS Storage
   ├── For each collection in {baseurl}/{collectionid}/indexes/
   │   ├── Load index metadata
   │   ├── Determine hot data from access logs
   │   └── Schedule prefetch tasks
   └── Collections discovered: [user_vectors, product_embeddings, ...]

3. LAZY LOADING Strategy
   ├── Don't load all data immediately (avoid OOM)
   ├── Load on first access (cache miss → load from AXIS)
   └── Background prefetch for historically hot data

4. CACHE WARMING (Optional)
   ├── Load top-K frequently accessed vectors
   ├── Apply rule-based tier placement
   └── Memory tier ← Hot data only

5. READY TO SERVE
   └── All queries can be served (cache hit or AXIS load)
```

#### Recovery from Cache Tier Failures

```rust
// Automatic recovery when cache tier data is lost
impl AdaptiveStore {
    async fn get(&self, key: &K) -> Option<V> {
        // Try cache tiers first
        if let Some(value) = self.memory_cache.get(key) {
            return Some(value);
        }
        
        if let Some(value) = self.nvme_cache.get(key) {
            self.promote_to_memory(key, &value); // Promote hot data
            return Some(value);
        }
        
        // Cache miss - load from AXIS durable storage
        if let Some(value) = self.load_from_axis(key).await {
            // Apply rule-based tier placement
            let tier = self.rule_policy.determine_tier(
                &self.workload_pattern,
                self.get_access_frequency(key),
                self.get_age_days(key)
            );
            
            self.place_in_tier(tier, key, &value);
            return Some(value);
        }
        
        None // Data doesn't exist
    }
    
    async fn load_from_axis(&self, key: &K) -> Option<V> {
        // Load from {baseurl}/{collection_id}/indexes/
        let path = format!("{}/{}/indexes/{}", 
            self.baseurl, self.collection_id, key);
        
        // AXIS storage provides the data
        axis_storage::load(&path).await
    }
}
```

#### Restart Configuration

```toml
# proximadb server config.toml
[restart_strategy]
# Load data from AXIS storage on startup
load_on_startup = true

# Prefetch historically hot data
prefetch_hot_data = true

# Maximum items to load initially per collection
max_initial_load = 10000

# Parallel loading threads
loader_threads = 4

# Cache warming strategy
[restart_strategy.cache_warming]
enabled = true
warm_top_k_per_collection = 1000
warm_collections = ["user_vectors", "product_embeddings"]
```

### Benefits of Rule-Based Approach with Unified Eviction

1. **Scalability**: O(1) policy lookup regardless of number of collections
2. **Simplicity**: Single set of rules to understand and configure
3. **Consistency**: All collections follow same tier management principles  
4. **Maintainability**: Rules can be updated server-wide without per-collection changes
5. **Resource Efficiency**: No per-collection policy storage overhead
6. **Future Extensibility**: Easy to add new rules or expose via API later
7. **Unified Eviction**: Same eviction logic for index and cache workloads
8. **Guaranteed Restartability**: AXIS storage provides durability, caches rebuild automatically

### Configuration Integration

```toml
# proximadb server config.toml
[tier_management]
base_disk_path = "/mnt/proximadb"  # Override default /tmp
base_nvme_path = "/mnt/nvme"       # Optional NVMe path
max_memory_bytes = 8589934592      # 8GB memory limit
enable_cloud_storage = false      # Disabled initially

# Rule thresholds (future enhancement)
[tier_management.rules]
hot_memory_threshold = 100.0      # accesses/day for memory tier
warm_nvme_threshold = 10.0        # accesses/day for NVMe tier  
cold_hdd_threshold = 1.0          # accesses/day for HDD tier
aging_hdd_days = 30               # days before considering HDD demotion
aging_memory_hours = 24           # hours before considering memory demotion
```

This rule-based approach provides a scalable foundation that can be enhanced with more sophisticated rules and eventually exposed through APIs as needed, without the immediate overhead of per-collection policy management.
- [ ] Migrate cache modules to use new infrastructure
- [ ] Performance benchmarking with comprehensive metrics collection
- [ ] Production readiness testing
- [ ] **Metrics Integration**: Prometheus export and alerting setup

### Phase 5: Advanced Features (Week 5)
- [ ] Runtime workload detection and adaptation using metrics analysis
- [ ] Backend migration support with migration performance tracking
- [ ] Advanced metrics and monitoring dashboard integration
- [ ] Documentation and examples
- [ ] **Metrics Integration**: Auto-optimization based on metrics feedback

### Phase 6: Metrics & Observability (Week 6)
- [x] **Comprehensive Metrics Design**: Architecture and integration points defined
- [ ] **Dashboard Integration**: Grafana dashboards for adaptive structures
- [ ] **Alert Configuration**: Production-ready alerting rules
- [ ] **Performance Profiling**: Metrics-driven performance optimization
- [ ] **Cost Analysis**: Memory and CPU cost tracking per operation
- [ ] **Predictive Analytics**: Workload prediction using metrics history

## Performance Expectations

### Benchmark Targets

| Operation Type | Current | Target | Improvement |
|----------------|---------|--------|-------------|
| **Index Bulk Insert** | ~10K ops/sec | ~50K ops/sec | 5x |
| **Cache Read** | ~100K ops/sec | ~500K ops/sec | 5x |
| **Memory Pressure Response** | ~1 second | ~100ms | 10x |
| **Invalidation Cascade** | ~1K ops/sec | ~10K ops/sec | 10x |
| **Mixed Workload** | Varies | Consistent | Stable |

### Memory Efficiency Targets

| Scenario | Current | Target | Improvement |
|----------|---------|--------|-------------|
| **Index Memory Overhead** | ~30% | ~10% | 3x better |
| **Cache Memory Utilization** | ~60% | ~85% | 1.4x better |
| **Memory Pressure Recovery** | Manual | Automatic | Qualitative |

## Risk Analysis and Mitigation

### Technical Risks

1. **Complexity Risk**: Multi-backend architecture increases complexity
   - **Mitigation**: Comprehensive unit tests, clear interfaces, extensive documentation

2. **Performance Risk**: Additional abstraction layer might impact performance
   - **Mitigation**: Zero-cost abstractions, compile-time optimization, benchmarking

3. **Memory Risk**: Multiple data structures might increase memory usage
   - **Mitigation**: Careful memory management, configurable limits, monitoring

### Operational Risks

1. **Migration Risk**: Existing code needs to be migrated
   - **Mitigation**: Phased rollout, backward compatibility, extensive testing

2. **Debugging Risk**: More complex architecture might be harder to debug
   - **Mitigation**: Rich metrics, structured logging, debug modes

## Metrics Integration Framework

### Core Metrics Architecture

The adaptive data structures are deeply integrated with ProximaDB's metrics framework to provide comprehensive observability and performance optimization capabilities.

```rust
// Integration with existing metrics system
use crate::metrics::{
    InternalMetricsUpdater, MetricsUpdate, CacheMetricsSnapshot,
    CompressionMetrics, GlobalMetrics, MetricsAggregationEngine
};

/// Comprehensive metrics collector for adaptive structures
#[derive(Debug)]
pub struct AdaptiveStructureMetrics {
    /// Base metrics integration
    internal_updater: Arc<dyn InternalMetricsUpdater>,
    
    /// Structure-specific metrics
    workload_metrics: WorkloadMetrics,
    performance_metrics: PerformanceMetrics,
    memory_metrics: MemoryUsageMetrics,
    
    /// Adaptive behavior metrics
    adaptation_metrics: AdaptationMetrics,
    pattern_metrics: AccessPatternMetrics,
}

/// Workload characterization metrics
#[derive(Debug, Clone)]
pub struct WorkloadMetrics {
    /// Operation distribution
    pub read_ratio: f64,
    pub write_ratio: f64,
    pub delete_ratio: f64,
    pub batch_ratio: f64,
    
    /// Temporal patterns
    pub peak_hours: Vec<u8>,
    pub load_variance: f64,
    pub burst_frequency: f64,
    
    /// Access patterns
    pub locality_score: f64,
    pub hot_key_percentage: f64,
    pub access_skew: f64,
}
```

### MetricsUpdate Integration

All adaptive structures implement MetricsUpdate for seamless integration with the existing metrics pipeline:

```rust
impl MetricsUpdate for AdaptiveStore<K, V> {
    fn update_metrics(&self, updater: &dyn InternalMetricsUpdater) {
        let snapshot = self.get_comprehensive_metrics();
        
        // Core operation metrics
        updater.update_counter("adaptive_store.operations.total", 
                              snapshot.total_operations as f64);
        updater.update_histogram("adaptive_store.latency.avg", 
                               snapshot.avg_latency_ms);
        updater.update_gauge("adaptive_store.hit_rate", 
                           snapshot.hit_rate);
        
        // Backend-specific metrics
        match &self.backend {
            StorageBackend::Index(backend) => {
                updater.update_gauge("adaptive_store.index.buffer_utilization", 
                                   backend.buffer_utilization());
                updater.update_counter("adaptive_store.index.flush_count", 
                                     backend.flush_count() as f64);
            }
            StorageBackend::Cache(backend) => {
                updater.update_gauge("adaptive_store.cache.memory_utilization", 
                                   backend.memory_utilization());
                updater.update_counter("adaptive_store.cache.eviction_count", 
                                     backend.eviction_count() as f64);
            }
            StorageBackend::Hybrid(backend) => {
                updater.update_counter("adaptive_store.hybrid.promotions", 
                                     backend.promotion_count() as f64);
                updater.update_counter("adaptive_store.hybrid.demotions", 
                                     backend.demotion_count() as f64);
                updater.update_gauge("adaptive_store.hybrid.hot_tier_utilization", 
                                   backend.hot_tier_utilization());
            }
        }
        
        // Adaptive behavior metrics
        updater.update_counter("adaptive_store.adaptations.total", 
                              snapshot.adaptation_count as f64);
        updater.update_histogram("adaptive_store.adaptation.duration_ms", 
                               snapshot.avg_adaptation_time_ms);
    }
}
```

### Metrics Collection Points

#### 1. Operation-Level Metrics
- **Latency tracking**: P50, P95, P99 for all operations
- **Throughput measurement**: Operations per second by type
- **Error rates**: Success/failure ratios with error categorization
- **Queue depths**: Buffer sizes and wait times

#### 2. Workload Pattern Metrics
- **Access frequency distribution**: Hot/warm/cold key identification
- **Temporal patterns**: Peak hours, load variance, burst detection
- **Spatial locality**: Cache line utilization, prefetch effectiveness
- **Operation correlation**: Sequential vs random access patterns

#### 3. Memory Management Metrics
- **Utilization tracking**: Per-tier memory usage and efficiency
- **Pressure events**: Frequency and severity of memory pressure
- **Allocation patterns**: Memory growth, fragmentation, cleanup effectiveness
- **Cost analysis**: Memory cost per operation and per GB-hour

#### 4. Adaptive Behavior Metrics
- **Backend transitions**: Frequency and success rate of adaptations
- **Migration performance**: Data movement speed and downtime
- **Prediction accuracy**: Workload pattern prediction success rates
- **Configuration drift**: How often configurations need adjustment

### Integration with Existing Systems

#### CacheMetricsSnapshot Integration
```rust
impl From<AdaptiveStructureMetricsSnapshot> for CacheMetricsSnapshot {
    fn from(snapshot: AdaptiveStructureMetricsSnapshot) -> Self {
        Self {
            entries: snapshot.total_entries,
            memory_bytes: snapshot.memory_usage_bytes,
            operations: snapshot.total_operations,
            hits: snapshot.cache_hits,
            misses: snapshot.cache_misses,
            hit_rate: snapshot.hit_rate,
            avg_operation_time: Duration::from_nanos(
                (snapshot.avg_latency_ms * 1_000_000.0) as u64
            ),
        }
    }
}
```

#### CompressionMetrics Integration
```rust
impl AdaptiveStore<K, V> {
    pub fn get_compression_metrics(&self) -> CompressionMetrics {
        match &self.backend {
            StorageBackend::Index(backend) => backend.get_compression_metrics(),
            StorageBackend::Cache(backend) => {
                // Cache backends may use compressed serialization
                backend.get_serialization_compression_metrics()
            }
            StorageBackend::Hybrid(backend) => {
                // Combine metrics from both tiers
                let hot_metrics = backend.hot_tier_compression_metrics();
                let cold_metrics = backend.cold_tier_compression_metrics();
                CompressionMetrics::combine(hot_metrics, cold_metrics)
            }
        }
    }
}
```

### Dashboard and Alerting Integration

#### Prometheus Metrics Export
```rust
// Prometheus metric definitions for adaptive structures
pub const ADAPTIVE_STORE_METRICS: &[MetricDefinition] = &[
    MetricDefinition {
        name: "proximadb_adaptive_store_operations_total",
        help: "Total number of operations performed",
        metric_type: MetricType::Counter,
        labels: &["backend_type", "operation_type"],
    },
    MetricDefinition {
        name: "proximadb_adaptive_store_latency_seconds",
        help: "Operation latency distribution",
        metric_type: MetricType::Histogram,
        labels: &["backend_type", "operation_type"],
    },
    MetricDefinition {
        name: "proximadb_adaptive_store_hit_rate",
        help: "Cache hit rate for adaptive structures",
        metric_type: MetricType::Gauge,
        labels: &["backend_type"],
    },
    MetricDefinition {
        name: "proximadb_adaptive_store_memory_utilization",
        help: "Memory utilization percentage",
        metric_type: MetricType::Gauge,
        labels: &["backend_type", "tier"],
    },
    MetricDefinition {
        name: "proximadb_adaptive_store_adaptations_total",
        help: "Number of backend adaptations performed",
        metric_type: MetricType::Counter,
        labels: &["from_backend", "to_backend", "reason"],
    },
];
```

#### Alert Definitions
```yaml
# Adaptive structure performance alerts
- alert: AdaptiveStoreHighLatency
  expr: histogram_quantile(0.95, proximadb_adaptive_store_latency_seconds) > 0.010
  for: 2m
  labels:
    severity: warning
  annotations:
    summary: "Adaptive store showing high latency"
    description: "P95 latency is {{ $value }}s for {{ $labels.backend_type }}"

- alert: AdaptiveStoreMemoryPressure
  expr: proximadb_adaptive_store_memory_utilization > 0.90
  for: 1m
  labels:
    severity: critical
  annotations:
    summary: "Adaptive store memory pressure"
    description: "Memory utilization at {{ $value | humanizePercentage }}"

- alert: AdaptiveStoreFrequentAdaptations
  expr: rate(proximadb_adaptive_store_adaptations_total[5m]) > 0.1
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "Frequent backend adaptations detected"
    description: "{{ $value }} adaptations per second indicates workload instability"
```

### Performance Monitoring Integration

#### Real-time Performance Tracking
```rust
impl AdaptiveStore<K, V> {
    /// Integration point for real-time performance monitoring
    pub fn register_performance_callbacks(&self, monitor: &PerformanceMonitor) {
        // Register latency callback
        monitor.register_latency_callback("adaptive_store", Box::new(|operation, duration| {
            // Called on every operation completion
            self.metrics.record_operation_latency(operation, duration);
        }));
        
        // Register memory pressure callback
        monitor.register_memory_pressure_callback("adaptive_store", Box::new(|| {
            // Called when system memory pressure detected
            self.handle_memory_pressure()
        }));
        
        // Register workload pattern callback
        monitor.register_pattern_change_callback("adaptive_store", Box::new(|pattern| {
            // Called when workload pattern changes detected
            self.consider_adaptation(pattern)
        }));
    }
}
```

### Metrics-Driven Optimization

#### Automatic Performance Tuning
```rust
impl AdaptiveStore<K, V> {
    /// Use metrics to automatically optimize configuration
    pub async fn auto_optimize(&mut self) -> Result<OptimizationReport> {
        let metrics = self.get_comprehensive_metrics();
        
        // Analyze metrics for optimization opportunities
        let optimizations = self.analyze_optimization_opportunities(&metrics);
        
        let mut report = OptimizationReport::new();
        
        for optimization in optimizations {
            match optimization {
                OptimizationOpportunity::BufferSizeIncrease { current, recommended } => {
                    if let StorageBackend::Index(ref mut backend) = &mut self.backend {
                        backend.set_buffer_size(recommended);
                        report.add_change("buffer_size", current, recommended);
                    }
                }
                OptimizationOpportunity::CacheEvictionPolicyChange { from, to } => {
                    if let StorageBackend::Cache(ref mut backend) = &mut self.backend {
                        backend.set_eviction_policy(to);
                        report.add_change("eviction_policy", from, to);
                    }
                }
                OptimizationOpportunity::PromotionThresholdAdjust { threshold } => {
                    if let StorageBackend::Hybrid(ref mut backend) = &mut self.backend {
                        backend.set_promotion_threshold(threshold);
                        report.add_change("promotion_threshold", "auto", threshold);
                    }
                }
            }
        }
        
        Ok(report)
    }
}
```

## Success Metrics

### Performance Metrics
- [ ] **Latency**: P95 latency < 1ms for all operations
- [ ] **Throughput**: > 100K ops/sec for read operations  
- [ ] **Memory**: < 15% memory overhead
- [ ] **Scalability**: Linear scaling with core count
- [ ] **Metrics Overhead**: < 1% performance impact from metrics collection

### Functional Metrics
- [ ] **Reliability**: 99.99% operation success rate
- [ ] **Consistency**: Zero data loss during pressure/invalidation
- [ ] **Adaptability**: < 1 second adaptation to workload changes
- [ ] **Observability**: 100% operation visibility through metrics

### Operational Metrics  
- [ ] **Migration**: Zero downtime migration from existing code
- [ ] **Maintainability**: < 2 hour debugging time for issues
- [ ] **Documentation**: 100% API coverage with examples
- [ ] **Alerting**: < 30 second alert response time for performance issues

## Conclusion

This Adaptive Data Structures Architecture provides a comprehensive solution for ProximaDB's diverse workload requirements. By implementing workload-specific backends with a unified interface, we achieve optimal performance for each use case while maintaining architectural simplicity and future extensibility.

The phased implementation approach ensures minimal risk while delivering incremental value. The extensive metrics and monitoring capabilities provide visibility into system behavior and support data-driven optimization decisions.

This design positions ProximaDB for scalable, high-performance operation across all current and future use cases while minimizing the need for future architectural changes.