# Unified Collection Cache Architecture

## Overview
ProximaDB uses a single, shared collection metadata cache per VectorOperationsService node to avoid memory duplication and support efficient distributed scaling.

## Architecture Design

```
┌─────────────────────────────────────────────────────────┐
│             VectorOperationsService (per node)           │
│                                                           │
│  ┌─────────────────────────────────────────────────┐    │
│  │         Collection Cache (Primary Owner)         │    │
│  │   Arc<DashMap<String, Arc<Collection>>>         │    │
│  │                                                  │    │
│  │   • Single source of truth for this node        │    │
│  │   • Fetches from CollectionService on miss      │    │
│  │   • Shared read-only with other components      │    │
│  └──────────────┬──────────────┬───────────────────┘    │
│                 │              │                         │
│         Read-only│      Read-only│                       │
│                 ▼              ▼                         │
│  ┌──────────────────┐  ┌──────────────────┐            │
│  │UnifiedSearchOptimizer│  │  AxisManager    │          │
│  │                      │  │                 │          │
│  │ • No cache duplication│  │ • No cache duplication│    │
│  │ • References parent  │  │ • References parent  │      │
│  │   cache via handle   │  │   cache via handle   │      │
│  └──────────────────────┘  └──────────────────────┘     │
└───────────────────────────────────────────────────────────┘
```

## Memory Efficiency

### Before (Problem)
- CollectionService: Maintains its own cache
- VectorOperationsService: Duplicates collection data
- UnifiedSearchOptimizer: Creates another cache copy
- AxisManager: Yet another copy for indexing
- **Result**: 4x memory usage for same data with 1000+ collections

### After (Solution)
- CollectionService: Still has its cache (source of truth)
- VectorOperationsService: Single cache per node
- UnifiedSearchOptimizer: Read-only reference to parent cache
- AxisManager: Read-only reference to parent cache
- **Result**: 1x memory usage + minimal reference overhead

## Implementation Details

### VectorOperationsService (Cache Owner)
```rust
pub struct VectorOperationsService {
    // Single collection cache for this node
    collection_cache: Arc<DashMap<String, Arc<Collection>>>,
    // ...
}

impl VectorOperationsService {
    // Primary cache access method
    pub async fn get_cached_collection(&self, id: &str) -> Result<Arc<Collection>> {
        // Check cache first
        if let Some(cached) = self.collection_cache.get(id) {
            return Ok(cached.clone());
        }
        // Fetch from CollectionService on miss
        // Cache and return
    }
    
    // Provide read-only handle to other components
    pub fn get_collection_cache_handle(&self) -> Arc<DashMap<...>> {
        self.collection_cache.clone()
    }
}
```

### UnifiedSearchOptimizer (Cache Consumer)
```rust
pub struct UnifiedSearchOptimizer {
    // NO collection cache - uses parent's cache
    // Only maintains optimizer-specific data
    file_metadata_cache: Arc<DashMap<...>>,
    quantization_engines: Arc<DashMap<...>>,
}
```

### AxisManager (Cache Consumer)
```rust
pub struct AxisManager {
    // Reference to shared cache (read-only)
    shared_collection_cache: Option<Arc<DashMap<...>>>,
}

impl AxisManager {
    pub fn set_shared_collection_cache(&mut self, cache: Arc<DashMap<...>>) {
        self.shared_collection_cache = Some(cache);
    }
}
```

## Benefits for Distributed Architecture

1. **Memory Efficiency**: Single cache per node regardless of components
2. **Cache Coherency**: One update point per node
3. **Scalability**: Each node maintains its own cache independently
4. **Performance**: Arc<DashMap> provides lock-free concurrent reads
5. **Flexibility**: Components can work with or without shared cache

## Cache Invalidation Strategy

- **Update**: `invalidate_collection_cache(collection_id)` when collection is modified
- **TTL**: Optional time-based eviction (future enhancement)
- **Memory Pressure**: LRU eviction when cache size exceeds limit (future)

## Future Enhancements

1. **Distributed Cache Sync**: Redis/Hazelcast for cross-node consistency
2. **Smart Prefetching**: Preload frequently accessed collections
3. **Compression**: Store compressed proto in cache, decompress on access
4. **Metrics**: Track cache hit rates, memory usage per collection