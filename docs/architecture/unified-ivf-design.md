# Unified IVF Index with Dual-Store Architecture

## Overview

The IVF index should be a single module that internally manages two distinct stores with different elasticity characteristics:

1. **Centroid Store**: Inelastic, always in memory
2. **Posting List Store**: Elastic, tierable across memory/NVMe/disk

## Proposed Architecture

```rust
pub struct UnifiedIvfIndex {
    // Inelastic: Always in memory, never evicted
    centroid_store: CentroidStore,
    
    // Elastic: Tierable posting lists
    posting_list_store: Arc<AdaptiveStore<(CollectionId, ClusterId), PostingList>>,
    
    // Collection-specific configuration
    collection_id: String,
    config: IvfConfig,
}

struct CentroidStore {
    // Pinned in memory with no eviction
    centroids: Arc<Vec<Vec<f32>>>,
    dimension: usize,
    
    // Small metadata always in memory
    centroid_stats: Vec<CentroidStats>,
}

impl CentroidStore {
    fn new(config: CentroidConfig) -> Self {
        // Configure as inelastic
        Self {
            centroids: Arc::new(Vec::new()),
            dimension: config.dimension,
            centroid_stats: Vec::new(),
        }
    }
    
    // No eviction methods - these are always in memory
}
```

## Collection Partitioning in Shared Infrastructure

### Current Problem
The global shared infrastructure doesn't properly partition by collection_id, leading to:
- Cross-collection scanning overhead
- Poor cache locality
- Difficulty in per-collection resource management

### Proposed Solution: Hierarchical Key Design

```rust
// Instead of simple keys, use compound keys everywhere
pub struct PartitionedKey<K> {
    collection_id: String,
    partition_id: Option<u32>,  // For sub-partitioning large collections
    key: K,
}

// Example for IVF posting lists
type IvfPostingListKey = PartitionedKey<ClusterId>;

// Example for vector store
type VectorKey = PartitionedKey<VectorId>;
```

### Implementation in AdaptiveStore

```rust
pub struct PartitionedAdaptiveStore<K, V> {
    // First level: Collection partitioning
    collections: DashMap<String, CollectionStore<K, V>>,
    
    // Global tier manager (shared across collections)
    tier_manager: Arc<GlobalTierManager>,
    
    // Collection-specific policies
    collection_policies: DashMap<String, TierPolicy>,
}

struct CollectionStore<K, V> {
    // Second level: Data within collection
    data: DashMap<K, TieredValue<V>>,
    
    // Collection-specific metrics
    metrics: CollectionMetrics,
    
    // Collection-specific working set
    working_set: WorkingSet<K>,
}
```

## Comprehensive Design Recommendations

### 1. Unified IVF Index Structure

```rust
// Single module that handles both stores
pub struct IvfIndex {
    collection_id: String,
    
    // INELASTIC: Always in memory (small, critical)
    centroids: InMemoryCentroids {
        data: Vec<Vec<f32>>,          // ~1.5MB for 1024 centroids
        tier_policy: TierPolicy {
            evictable: false,          // NEVER evict
            priority: Priority::Critical,
            min_memory_guarantee: true,
        }
    },
    
    // ELASTIC: Can be tiered (large, access-dependent)  
    posting_lists: TieredPostingLists {
        store: AdaptiveStore<ClusterId, PostingList>,
        tier_policy: TierPolicy {
            evictable: true,           // CAN evict
            promotion_threshold: 100,   // accesses/hour
            demotion_threshold: 3600,   // seconds idle
            max_memory_mb: 1500,        // 1.5GB limit
        }
    },
    
    // Shared distance computation
    distance_compute: UnifiedDistanceCompute,
}
```

### 2. Collection Partitioning Strategy

#### Option A: Hierarchical Namespace (Recommended)

```rust
// All keys are namespaced by collection
Key Format: {collection_id}:{index_type}:{data_key}

Examples:
- "users:ivf_centroids:0"       // Centroid 0 for users collection
- "users:ivf_posting:42"         // Posting list 42 for users
- "products:hnsw_layer:3:100"   // HNSW layer 3, node 100
- "docs:vectors:doc_123"         // Vector for document 123
```

**Pros:**
- Clear separation between collections
- Easy to implement collection-level operations (delete all, export)
- Natural sharding boundary for distributed systems
- Efficient range scans within collection

**Cons:**
- Slightly larger key size
- Need to parse keys for routing

#### Option B: Separate Stores per Collection

```rust
pub struct GlobalIndexManager {
    // Each collection gets its own store instances
    collection_stores: DashMap<String, CollectionStores>,
}

struct CollectionStores {
    ivf_index: Option<IvfIndex>,
    hnsw_index: Option<HnswIndex>,
    vector_store: VectorStore,
    metadata_store: MetadataStore,
}
```

**Pros:**
- Complete isolation between collections
- Can use different configurations per collection
- Easier resource limits per collection

**Cons:**
- More memory overhead for management structures
- Complex cross-collection operations
- Harder to implement global resource management

### 3. Trade-offs Analysis

| Aspect | Single Module | Dual Module | Recommendation |
|--------|--------------|-------------|----------------|
| **Code Simplicity** | ✅ Better | ❌ More complex | Single |
| **Configurability** | ✅ Can configure internally | ✅ Explicit separation | Single |
| **Memory Management** | ✅ Unified | ❌ Duplicate structures | Single |
| **Tier Policy Clarity** | ⚠️ Need clear internal separation | ✅ Explicit | Single with clear policies |
| **Testing** | ✅ Test one module | ❌ Test coordination | Single |

### 4. Implementation Path

#### Phase 1: Refactor IVF to Single Module
```rust
// Merge ivf_index.rs and ivf_tier_aware.rs into single module
// /src/index/axis/ivf_index.rs

impl IvfIndex {
    pub fn new(config: IvfConfig, collection_id: String) -> Self {
        // Create centroid store (inelastic)
        let centroid_store = Self::create_centroid_store(&config);
        
        // Create posting list store (elastic)
        let posting_store_config = AdaptiveStoreConfig {
            key_prefix: format!("{}:ivf_posting", collection_id),
            tier_policy: Self::posting_list_policy(&config),
            // ... elastic configuration
        };
        
        Self {
            collection_id,
            centroids: centroid_store,
            posting_lists: AdaptiveStore::new(posting_store_config),
            // ...
        }
    }
}
```

#### Phase 2: Add Collection Partitioning to Shared Infra
```rust
// Update adaptive_structures.rs to support partitioned keys

impl<K, V> AdaptiveStore<K, V> {
    pub fn new_partitioned(
        collection_id: String,
        config: AdaptiveStoreConfig,
    ) -> Self {
        let key_prefix = format!("{}:{}", collection_id, config.store_type);
        // All operations automatically scoped to collection
    }
    
    pub fn get_collection_scoped(&self, key: &K) -> Option<V> {
        let full_key = self.make_collection_key(key);
        self.inner_get(&full_key)
    }
}
```

#### Phase 3: Update All Index Types
```rust
// Make all indexes collection-aware
trait CollectionAwareIndex {
    fn collection_id(&self) -> &str;
    fn collection_stats(&self) -> CollectionIndexStats;
    fn clear_collection(&self) -> Result<()>;
}

impl CollectionAwareIndex for IvfIndex { ... }
impl CollectionAwareIndex for HnswIndex { ... }
impl CollectionAwareIndex for AnnoyIndex { ... }
```

### 5. Global Resource Management

```rust
pub struct GlobalResourceManager {
    // Total memory budget across all collections
    total_memory_budget: usize,
    
    // Per-collection allocations
    collection_budgets: DashMap<String, CollectionBudget>,
    
    // Priority-based allocation
    collection_priorities: DashMap<String, Priority>,
}

impl GlobalResourceManager {
    fn allocate_memory(&self, collection_id: &str, requested: usize) -> usize {
        // Consider:
        // 1. Collection priority
        // 2. Current usage
        // 3. Global pressure
        // 4. Minimum guarantees (centroids)
        
        let priority = self.get_priority(collection_id);
        let available = self.get_available_memory();
        
        match priority {
            Priority::Critical => {
                // Always allocate for critical (e.g., centroids)
                min(requested, available)
            }
            Priority::High => {
                // Allocate if > 20% available
                if available > self.total_memory_budget / 5 {
                    min(requested, available * 0.5)
                } else {
                    0
                }
            }
            Priority::Normal => {
                // Only allocate if no pressure
                if available > self.total_memory_budget / 2 {
                    min(requested, available * 0.2)
                } else {
                    0
                }
            }
        }
    }
}
```

## Performance Implications

### Without Collection Partitioning
```
Query: "Find vector in collection 'users'"
1. Scan all keys in global store (millions)
2. Filter by collection prefix
3. Increased CPU and memory bandwidth usage
4. Poor cache locality
```

### With Collection Partitioning
```
Query: "Find vector in collection 'users'"  
1. Direct lookup in collection's partition
2. Only scan relevant keys (thousands)
3. Better cache locality
4. Can parallelize per collection
```

### Benchmark Expectations

| Operation | Without Partitioning | With Partitioning | Improvement |
|-----------|---------------------|-------------------|-------------|
| Single vector lookup | 50μs | 5μs | 10x |
| Range scan (1K vectors) | 500ms | 50ms | 10x |
| Collection deletion | 30s | 100ms | 300x |
| Memory overhead | 100MB | 120MB | -20% |

## Final Recommendations

1. **Consolidate to single IVF module** with internal dual-store management
2. **Implement hierarchical namespace partitioning** for collection isolation
3. **Add collection-aware interfaces** to all index types
4. **Use compound keys** throughout the system
5. **Implement global resource manager** with priority-based allocation
6. **Add collection-level metrics** and monitoring
7. **Support collection-level operations** (backup, migration, deletion)

## Migration Plan

```bash
Week 1: Refactor IVF to single module with dual stores
Week 2: Add collection partitioning to AdaptiveStore
Week 3: Update all index types to be collection-aware
Week 4: Implement global resource management
Week 5: Add monitoring and testing
Week 6: Performance validation and tuning
```

This approach provides:
- **Clean separation** between collections
- **Efficient resource usage** with shared infrastructure
- **Flexibility** for per-collection policies
- **Scalability** to thousands of collections
- **Performance** through proper partitioning