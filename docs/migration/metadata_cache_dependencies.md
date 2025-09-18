# Metadata Cache Dependencies Analysis

## Metadata Cache Implementations Found

### 1. IntelligentFilesystem Metadata Cache
**Location**: `storage/persistence/filesystem/intelligent_filesystem.rs`
- **Type**: `Arc<RwLock<HashMap<String, CachedMetadata>>>`
- **Stores**: FileMetadata, Parquet footers, Bloom filters
- **Users**: All engines via IntelligentFilesystem wrapper

### 2. ZeroCopyIOSystem Metadata Cache
**Location**: `storage/engines/core/io/zero_copy/metadata_cache.rs`
- **Type**: `ZeroCopyMetadataCache` with `MmappedMetadata`
- **Stores**: Memory-mapped metadata for zero-copy access
- **Users**: SST indexed_reader, SWIFT unified_reader, IVF storage

### 3. CrossCacheOrchestrator MetadataStore
**Location**: `storage/cache/orchestrator.rs`
- **Type**: `Arc<MetadataStore>`
- **Stores**: High-level collection metadata
- **Users**: Query optimizer, search optimization

### 4. FilesystemMetadataStore
**Location**: `storage/cache/specialized/filesystem_metadata_store.rs`
- **Type**: Specialized filesystem metadata caching
- **Users**: ZeroCopyIOSystem orchestrator

### 5. Storage Metadata Store
**Location**: `storage/metadata/store.rs`
- **Type**: `MetadataStore` for collection/vector metadata
- **Stores**: Collection configs, vector metadata
- **Users**: VectorOperationsService, CollectionService

## Dependency Graph

```
┌─────────────────────────────────────────┐
│         Storage Engines                  │
│  (SST, VIPER, NOVA, RAPTOR, etc.)       │
└────────────┬─────────────────────────────┘
             │
             ├──────────────┬──────────────┬─────────────┐
             ▼              ▼              ▼             ▼
    IntelligentFilesystem  ZeroCopyFS   ZeroCopyIO   Direct FS
    └─metadata_cache       └─uses→      └─metadata_cache
                                           │
                           ┌───────────────┴──────────────┐
                           ▼                              ▼
                    FilesystemMetadataStore      MmappedMetadata
                           │
                           └──────────┬────────────────────┘
                                      ▼
                            CrossCacheOrchestrator
                            └─metadata_cache (MetadataStore)
```

## Cache Duplication Issues

### Problem 1: Triple Caching in RAPTOR
```
RAPTOR Engine → IntelligentFilesystem (cache 1)
              → ZeroCopyFilesystem
              → ZeroCopyIOSystem (cache 2)
              → CrossCacheOrchestrator (cache 3)
```

### Problem 2: Inconsistent Cache Keys
- IntelligentFilesystem: `"{path}:{collection_id}:{engine_type}"`
- ZeroCopyIOSystem: `"{filename}:{collection_id}:{engine}"`
- CrossCacheOrchestrator: Uses different key format

### Problem 3: No Cache Coherency Protocol
- No invalidation propagation between caches
- No shared eviction policy
- No coordinated TTL management

## Memory Impact Analysis

### Current Memory Usage (Worst Case)
Assuming 10,000 files with metadata:
- IntelligentFilesystem: ~200MB (20KB per file metadata)
- ZeroCopyMetadataCache: ~300MB (30KB per mmap'd metadata)
- CrossCacheOrchestrator: ~100MB (10KB per high-level metadata)
- **Total**: ~600MB for metadata alone

### After Consolidation
- UnifiedMetadataCache: ~250MB (25KB per file, single instance)
- **Savings**: 350MB (58% reduction)

## Critical Dependencies to Address

### High Priority
1. **SST Engine** (`storage/engines/impls/sst/`)
   - Uses both IntelligentFilesystem and ZeroCopyIOSystem
   - indexed_reader.rs heavily depends on metadata cache
   - row_filter.rs uses metadata for filtering

2. **VIPER Engine** (`storage/engines/impls/viper/`)
   - Critical dependency on Parquet metadata caching
   - Performance heavily impacted by metadata cache misses

3. **Query Optimization** (`query/unified_query_optimizer.rs`)
   - Depends on metadata for query planning
   - Uses CrossCacheOrchestrator metadata

### Medium Priority
1. **Transaction Coordinator** (`storage/transaction_coordinator.rs`)
   - Uses metadata for atomic operations
   - Needs consistent metadata view

2. **Search Optimization** (`core/search/integrated_search_optimization.rs`)
   - Uses metadata for search pruning
   - Depends on accurate statistics

### Low Priority
1. **Monitoring** (`monitoring/enterprise_dashboard.rs`)
   - Read-only metadata access
   - Can adapt to new cache structure

## Migration Requirements

### Functional Requirements
1. Single metadata cache instance shared across all components
2. Unified cache key format
3. Cache coherency protocol
4. Backward compatibility during migration

### Non-Functional Requirements
1. Zero-copy access where possible (preserve ZeroCopyIOSystem benefits)
2. Lock-free reads for hot paths
3. Configurable TTL and eviction policies
4. Metrics and monitoring integration

## Recommended Approach

### Step 1: Create Unified Cache Interface
```rust
pub trait UnifiedMetadataCache: Send + Sync {
    async fn get(&self, key: &str) -> Option<Arc<CachedMetadata>>;
    async fn put(&self, key: String, metadata: CachedMetadata);
    async fn invalidate(&self, key: &str);
    async fn invalidate_pattern(&self, pattern: &str);
}
```

### Step 2: Implement Adapter Pattern
Create adapters for existing caches to implement the unified interface during migration.

### Step 3: Gradual Migration
Migrate one engine at a time to use the unified cache interface.

## Phase 1 Progress
- ✅ P1.1: Filesystem usage documented
- ✅ P1.2: Metadata cache dependencies mapped
- Next: P1.3 - Create compatibility test suite