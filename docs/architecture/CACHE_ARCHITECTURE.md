# ProximaDB Cache Architecture Analysis

## Executive Summary

ProximaDB implements a sophisticated multi-tier caching system with a global `CrossCacheOrchestrator` for coordinating multiple specialized caches. Recent fixes have addressed critical type mismatch issues where storage engines were incorrectly using QueryCache for individual vectors. A dedicated VectorCache has been implemented and all storage engines have been updated.

## Current Architecture

### 1. Cache Components

#### 1.1 CrossCacheOrchestrator (Global Singleton)
- **Location**: `src/storage/cache/orchestrator.rs`
- **Purpose**: Coordinates all cache types and memory allocation
- **Registration**: Globally registered in `multi_server.rs` and `lib.rs`
- **Memory Budget**: Configured at startup (e.g., 512MB default)

```rust
// Registration in multi_server.rs
let orchestrator = Arc::new(CrossCacheOrchestrator::new(
    (storage_config.cache_size_mb * 1024 * 1024) as usize
));
CrossCacheOrchestrator::register_global(orchestrator.clone());
```

#### 1.2 Cache Types
The system manages 11 distinct cache types:
- **VectorData**: Individual vector storage (FIXED - VectorCache implemented)
- **QueryResult**: Cached search results
- **FilterBitmap**: Bloom filters and bitmaps
- **IndexStructure**: Index nodes (HNSW, IVF, etc.)
- **Metadata**: Collection and vector metadata
- **QueryPlan**: Query execution plans
- **EntityHeader**: Entity metadata
- **EmbeddingCatalog**: Embedding mappings
- **GraphNode**: Graph nodes
- **GraphEdge**: Graph edges
- **DistanceTable**: Pre-computed distances

#### 1.3 Specialized Cache Implementations

##### QueryCache (`src/storage/cache/specialized/query_cache.rs`)
- **Key Type**: `QueryKey` (collection_id, vector_hash, k, filters_hash)
- **Value Type**: `CachedQueryResult` (SearchResult[], cached_at, file_dependencies)
- **Backend**: `BaseCacheImpl` with memory backend
- **Status**: Now properly used only for query results

##### VectorCache (`src/storage/cache/specialized/vector_cache.rs`)
- **Key Type**: `VectorCacheKey` (simple string wrapper)
- **Value Type**: `CachedVector` (VectorRecord, cached_at, access_count)
- **Backend**: `BaseCacheImpl` with memory backend
- **Status**: NEW - Properly handles individual vector caching
- **Key Format**: `"vector:{collection_id}:{vector_id}"`

##### BitmapFilterCache
- Stores bloom filters for fast filtering
- Key: String (filter identifier)
- Value: Compressed bitmap

##### IndexNodeCache
- Caches index tree nodes
- Supports HNSW, IVF, and other index types

##### MetadataStore
- Generic metadata storage
- Key-value store with serialization

### 2. Memory Management

#### 2.1 DynamicMemoryAllocator
- **Total Budget**: Set at initialization
- **Initial Allocation**:
  - VectorData: 40%
  - QueryResult: 30%
  - FilterBitmap: 15%
  - IndexStructure: 10%
  - Metadata: 5%
- **Rebalancing**: Based on hit rates and access frequency
- **Issue**: Rebalancing logic exists but is never triggered

#### 2.2 Storage Backends

##### MemoryBackend (`src/storage/cache/backend/memory_tier.rs`)
- Uses DashMap for concurrent access
- Tracks size with AtomicUsize
- **Critical Issue**: Returns `CapacityExceeded` error when full - NO EVICTION

##### NvmeBackend
- Placeholder for disk-based caching (not implemented)

##### NetworkBackend
- Placeholder for distributed caching (not implemented)

### 3. Eviction System

#### 3.1 CacheEvictor (`src/storage/cache/eviction.rs`)
- **Status**: IMPLEMENTED BUT NOT USED
- **Policies**: LRU, LFU, ARC, TTL, PatternBased
- **Issue**: Never instantiated or started anywhere

#### 3.2 Current Eviction Behavior
```rust
// In base.rs
Err(StorageError::CapacityExceeded) => {
    // Cache is full - eviction handled by global orchestrator
    // For now, just fail the insert
    return Err(anyhow::anyhow!("Cache capacity exceeded - eviction handled by global orchestrator"));
}
```
**Result**: Cache insertions fail when capacity is reached!

### 4. Cache Warming

#### 4.1 CacheWarmer (`src/storage/cache/warming.rs`)
- **Status**: IMPLEMENTED BUT NOT USED
- **Strategies**: PopularityBased, CollectionBased, SimilarityBased, TimeBased
- **Issue**: Never instantiated or started

### 5. Access Pattern Tracking

#### 5.1 AccessPatternTracker
- Tracks access history (limited to 10,000 entries)
- Builds correlation matrix for predictive prefetching
- Uses async event processing to avoid blocking
- **Works**: This component is functional

### 6. Storage Engine Integration

All 7 storage engines now properly use VectorCache:

```rust
// Fixed example from SST engine
let cache_key = format!("vector:{}:{}", collection_id, vector_id);
if let Some(orchestrator) = CrossCacheOrchestrator::global() {
    if let Some(vector_cache) = orchestrator.get_vector_cache() {
        // FIXED: Using VectorCache for individual vectors
        if let Some(cached_vector) = vector_cache.get(&cache_key).await {
            return Ok(Some(cached_vector));
        }
    }
}
```

**Fixed Issues**:
1. ✅ All engines now use VectorCache with string keys
2. ✅ Dedicated VectorCache implementation created
3. ✅ Type safety ensured with proper cache types

## Critical Issues Status

### Fixed Issues (Phase 1 Complete)
1. **✅ Type Mismatch in Vector Caching**
   - Created dedicated VectorCache class
   - Updated CrossCacheOrchestrator to include vector_cache field
   - Fixed all 7 storage engines (SST, VIPER, NOVA, SWIFT, RAPTOR, PRISM, HELIX)
   - Cache key format standardized: `"vector:{collection_id}:{vector_id}"`

### Fixed Issues (Phase 2 Complete)
2. **✅ Cache Eviction Now Working**
   - Added `cache_evictor` and `cache_warmer` fields to CrossCacheOrchestrator
   - Implemented `start_eviction_service()` and `start_warming_service()` methods
   - Modified base.rs to trigger eviction when capacity exceeded
   - Services now automatically started in lib.rs and multi_server.rs
   - Added `trigger_immediate_eviction()` method for on-demand eviction

3. **✅ Cache Services Now Started**
   - CacheEvictor automatically initialized with default LRU and TTL policies
   - CacheWarmer available but disabled by default (can be enabled via config)
   - Background tasks spawned in tokio runtime
   - Eviction triggered when memory usage exceeds 90%

### All Critical Issues Resolved (Phases 3-4 Complete)

### Issue #1: Memory Rebalancing ✅ RESOLVED
- **Solution**: Added `start_rebalancing_service()` to CrossCacheOrchestrator
- **Implementation**: Periodic task runs every 5 minutes
- **Status**: Automatically started in lib.rs and multi_server.rs

### Issue #2: Cache Monitoring ✅ RESOLVED
- **Solution**: Added complete monitoring API to all cache layers
- **Implementation**:
  - BaseCacheImpl: size(), remove(), memory_usage(), metrics()
  - MemoryBackend: Full monitoring support
  - QueryCache: Complete statistics API with CacheStatistics struct
- **Status**: All monitoring methods operational

### Issue #3: Metrics Duplication ✅ RESOLVED
- **Solution**: Replaced CacheMetrics with UnifiedMetricsCollector throughout
- **Implementation**:
  - All cache layers now use UnifiedMetricsCollector
  - Extended MetricsSnapshot with cache_hits and cache_misses
  - Added record_operation() for consistent metrics recording
- **Status**: Single unified metrics system across entire cache subsystem

### Issue #4: Configuration Hardcoding ✅ RESOLVED (Phase 5)
- **Solution**: Exposed comprehensive cache configuration in config.toml
- **Implementation**:
  - Added [cache] section with total memory budget
  - Per-cache-type allocations (vector, query, metadata, index, filter)
  - Configurable eviction policies (LRU, TTL)
  - Rebalancing intervals and thresholds
  - Cache warming settings (disabled by default)
- **Status**: Full configurability without code changes

## Cache Warming Service Status

**Decision**: DISABLED by default

**Rationale**:
1. **Resource Efficiency**: Warming wastes memory on speculative loading
2. **Workload Dependent**: Only beneficial for predictable access patterns
3. **Performance Impact**: Can slow startup and consume CPU unnecessarily
4. **Configuration Available**: Can be enabled via `cache.enable_warming = true`

**When to Enable**:
- Stable, predictable workloads
- Known hot data sets
- After analyzing access patterns
- When startup latency is acceptable

**Configuration Example**:
```toml
[cache]
enable_warming = true  # Enable only for predictable workloads

[cache.warming]
strategies = ["popularity", "time_based"]
warm_on_startup = true
popularity_threshold = 10  # Items accessed > 10 times
```

## Suggested Improvements

### 1. Implement VectorCache
```rust
pub struct VectorCache {
    base: BaseCacheImpl<String, VectorRecord>,
}

impl VectorCache {
    pub async fn get(&self, key: &str) -> Option<VectorRecord> {
        self.base.get(key).await
    }

    pub async fn put(&self, key: String, value: VectorRecord) {
        self.base.put(key, value).await
    }
}
```

### 2. Fix CrossCacheOrchestrator Initialization
```rust
impl CrossCacheOrchestrator {
    pub fn new(total_memory_budget: usize) -> Self {
        let mut orchestrator = Self {
            // ... existing initialization
            vector_cache: Some(Arc::new(VectorCache::new(...))),
            // ...
        };

        // Start background tasks
        orchestrator.start_eviction_task();
        orchestrator.start_warming_task();
        orchestrator.start_rebalancing_task();

        orchestrator
    }

    fn start_eviction_task(&self) {
        let evictor = Arc::new(CacheEvictor::new(self.clone(), metrics));
        tokio::spawn(async move {
            evictor.start_eviction().await
        });
    }
}
```

### 3. Implement Proper Eviction in BaseCacheImpl
```rust
async fn put_l1(&self, key: K, value: V) -> Result<()> {
    loop {
        match self.l1_backend.put(key.clone(), entry.clone()).await {
            Ok(_) => return Ok(()),
            Err(StorageError::CapacityExceeded) => {
                // Trigger eviction through global orchestrator
                if let Some(orchestrator) = CrossCacheOrchestrator::global() {
                    orchestrator.trigger_eviction(CacheType::from(&key)).await?;
                } else {
                    return Err(anyhow!("Cache full and no orchestrator"));
                }
            }
        }
    }
}
```

### 4. Add Monitoring and Metrics
```rust
impl CrossCacheOrchestrator {
    pub async fn get_cache_stats(&self) -> CacheStats {
        CacheStats {
            hit_rate: self.calculate_hit_rate(),
            memory_usage: self.get_memory_usage(),
            eviction_count: self.eviction_count.load(Ordering::Relaxed),
            items_count: self.get_total_items(),
        }
    }
}
```

### 5. Implement Cache Partitioning
```rust
pub struct CachePartition {
    tenant_id: String,
    memory_limit: usize,
    cache: Arc<VectorCache>,
}

// Multi-tenant cache isolation
impl CrossCacheOrchestrator {
    pub fn get_tenant_cache(&self, tenant_id: &str) -> Arc<CachePartition> {
        self.tenant_caches.get(tenant_id)
            .or_else(|| self.create_tenant_partition(tenant_id))
    }
}
```

### 6. Add Cache Coherency
```rust
impl CacheInvalidator {
    pub async fn invalidate_collection(&self, collection_id: &str) {
        // Invalidate all caches related to this collection
        self.invalidate_query_cache(collection_id).await;
        self.invalidate_vector_cache(collection_id).await;
        self.invalidate_index_cache(collection_id).await;
    }
}
```

## Performance Optimizations

### 1. Reduce Lock Contention
- Use sharded DashMaps for better concurrent access
- Implement lock-free algorithms where possible

### 2. Optimize Memory Estimation
```rust
trait CacheValue {
    fn actual_size(&self) -> usize;
}

impl CacheValue for VectorRecord {
    fn actual_size(&self) -> usize {
        size_of::<Self>()
        + self.id.len()
        + self.values.len() * size_of::<f32>()
        + self.metadata.iter().map(|(k,v)| k.len() + v.len()).sum()
    }
}
```

### 3. Implement Cache Compression
```rust
impl VectorCache {
    pub async fn put_compressed(&self, key: String, value: VectorRecord) {
        let compressed = compress_vector(&value);
        self.base.put(key, compressed).await;
    }
}
```

### 4. Add Prefetching
```rust
impl PrefetchEngine {
    pub async fn prefetch_correlated(&self, key: &str) {
        let correlated = self.pattern_tracker.get_correlated(key);
        for (related_key, _) in correlated {
            self.prefetch_queue.push(related_key).await;
        }
    }
}
```

## Implementation Priority

1. **Critical (Immediate)**:
   - Fix VectorCache implementation
   - Enable CacheEvictor
   - Fix capacity exceeded handling

2. **High Priority**:
   - Start background tasks for eviction/warming
   - Implement memory rebalancing
   - Add monitoring/metrics

3. **Medium Priority**:
   - Optimize memory estimation
   - Add cache compression
   - Implement prefetching

4. **Low Priority**:
   - Multi-tenant partitioning
   - Distributed caching
   - Advanced eviction policies

## Testing Requirements

1. **Unit Tests**:
   - Test eviction under memory pressure
   - Test cache warming strategies
   - Test memory rebalancing

2. **Integration Tests**:
   - Test with all storage engines
   - Test concurrent access patterns
   - Test crash recovery

3. **Performance Tests**:
   - Benchmark cache hit rates
   - Measure eviction overhead
   - Test memory efficiency

## Conclusion

The ProximaDB cache architecture is well-designed but has critical implementation gaps:
- No actual eviction occurs
- Type mismatches prevent vector caching
- Background tasks are not running
- Memory management is incomplete

These issues must be addressed to achieve production-ready caching performance.