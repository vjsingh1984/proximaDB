# Filesystem Usage Audit

## Summary
- **IntelligentFilesystem**: 92 occurrences across 33 files
- **ZeroCopyFilesystem**: 63 occurrences across 14 files
- **ZeroCopyIOSystem**: 86 occurrences across 23 files

## IntelligentFilesystem Usage

### Storage Engines
1. **SST Engine** (`storage/engines/impls/sst/mod.rs`)
   - Line 1551: Type definition in SstStorage
   - Line 1606: Comment about per-collection instances
   - Line 4564: Getting IntelligentFilesystem for collection

2. **VIPER Engine** (`storage/engines/impls/viper/engine.rs`)
   - Line 1872: Critical for cloud storage performance
   - Line 1884: TODO to update UnifiedParquetReader

3. **NOVA Engine** (`storage/engines/impls/nova/engine.rs`)
   - Line 124: Benefits from caching hierarchical stats
   - Line 169: Caches hierarchical stats and Parquet metadata

4. **RAPTOR Engine** (`storage/engines/impls/raptor/engine.rs`)
   - Uses both IntelligentFilesystem AND ZeroCopyFilesystem (double wrapping)

### Core Components
- **ParquetQueryEngine** (`storage/engines/core/formats/columnar/parquet_query_engine.rs`)
  - Lines 307, 390, 458: Comments about wrapping with IntelligentFilesystem

- **FilesystemFactory** (`storage/persistence/filesystem/mod.rs`)
  - Creates and wraps filesystems with IntelligentFilesystem

## ZeroCopyFilesystem Usage

### Primary Users
1. **TransactionCoordinator** (`storage/transaction_coordinator.rs`)
   - Lines 130, 425-454: Zero-copy managed atomic operations
   - Lines 557-564: Checking if operations are ZeroCopyFilesystem managed
   - Lines 737-742: Abort operation handling

2. **Zero-Copy Reader Integration** (`storage/engines/core/ops/zero_copy_reader_integration.rs`)
   - Lines 193, 198: SstZeroCopyReader
   - Lines 238, 243: ViperZeroCopyReader
   - Lines 283, 288: NovaZeroCopyReader

3. **RAPTOR Engine** (`storage/engines/impls/raptor/engine.rs`)
   - Creates ZeroCopyFilesystem wrapping IntelligentFilesystem

## ZeroCopyIOSystem Usage

### Main Integration Points
1. **SST Engine** (`storage/engines/impls/sst/`)
   - Multiple files use ZeroCopyIOSystem for I/O optimization
   - streaming_compaction.rs, indexed_reader.rs, sst_query_engine.rs

2. **SWIFT Engine** (`storage/engines/impls/swift/unified_reader.rs`)
   - Uses ZeroCopyIOSystem for read optimization

3. **IVF Storage** (`index/axis/storage/ivf_posting_list_storage.rs`)
   - Integrates ZeroCopyIOSystem for index operations

## Double-Wrapping Issues

### RAPTOR Engine Pattern (PROBLEMATIC)
```rust
// Current problematic pattern in RAPTOR
let fs = factory.get_filesystem(url)?;
let intelligent_fs = IntelligentFilesystem::new(fs, collection_id, "raptor");
let io_system = ZeroCopyIOSystem::new(...);
let zero_copy_fs = ZeroCopyFilesystem::new(intelligent_fs, io_system);
```

This creates:
- Double metadata caching
- Double access pattern tracking
- Redundant prefetching logic
- Memory overhead

## Configuration Overlaps

### Multiple Cache Configurations
1. **CacheConfig** (IntelligentFilesystem)
   - max_memory_mb, max_disk_gb, metadata_ttl_secs
   - enable_prefetch, enable_learning

2. **ZeroCopyIOConfig** (ZeroCopyIOSystem)
   - metadata_cache, download_optimizer
   - access_prediction, background_tasks

3. **CrossCacheOrchestrator Config**
   - Separate cache configuration for orchestration

## Critical Observations

### Performance Impact Areas
1. **Metadata Operations**: Triple caching (Intelligent + ZeroCopy + CrossCache)
2. **Read Path**: Multiple layers of indirection
3. **Configuration**: Difficult to tune with overlapping configs
4. **Memory Usage**: Redundant caches consuming memory

### Migration Priority
1. **High Priority**: RAPTOR engine (double wrapping)
2. **Medium Priority**: SST, VIPER, NOVA (single wrapping)
3. **Low Priority**: SWIFT, PRISM, HELIX (minimal usage)

## Recommendations for Migration

### Quick Wins
1. Fix RAPTOR double-wrapping immediately
2. Consolidate metadata caches
3. Unify configuration structures

### Phase 1 Completion
- ✅ P1.1: Filesystem usage documented
- Next: P1.2 - Map metadata cache dependencies