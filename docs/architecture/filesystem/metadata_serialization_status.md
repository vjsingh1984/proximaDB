# Metadata Serialization Implementation Status

## Overview
Each storage engine must provide its own metadata serialization implementation to work with UnifiedCachingFilesystem. This follows the **Engine-Owned Serialization** pattern where each engine is the expert on its own metadata format.

## Architecture Decision
- **Pattern**: Strategy Pattern with Engine-Owned Serialization
- **Interface**: `EngineMetadataSerializer` trait
- **Location**: `src/storage/persistence/filesystem/metadata_traits.rs`
- **Principle**: Each engine owns its metadata format and provides serializer to filesystem

## Implementation Status by Engine

| Engine | Status | Metadata Types | Serializer Location | Notes |
|--------|--------|----------------|---------------------|-------|
| **VIPER** | ⚠️ Partial | `ClusterMetadata`, `FilterableColumn` | Not implemented | Has metadata types in `types.rs`, needs serializer |
| **NOVA** | ⚠️ Partial | `QuantizedColumnMetadata` | Not implemented | Has metadata in `quantized_columns.rs` |
| **SST** | ⚠️ Partial | SSTable metadata | Not implemented | Has bloom filter support |
| **SWIFT** | ⚠️ Partial | `SuperBlockMetadata`, `CachedDataBlockMetadata` | Not implemented | Has cache structures in `superblock_cache.rs` |
| **RAPTOR** | ✅ Started | `RaptorCachedMetadata`, `RaptorFileMetadata` | `metadata_serializer.rs` | Has dedicated serializer module |
| **PRISM** | ⚠️ Partial | `PrismResolutionMetadata`, `MetadataItem` | Not implemented | Has metadata in `fastlanes_serializer.rs` |
| **HELIX** | ⚠️ Partial | `HelixBlockMetadata`, `SStableMetadata` | Not implemented | Has metadata in `fastlane.rs` |

## Required Implementation per Engine

### What Each Engine Must Provide

```rust
// In each engine's module (e.g., viper/metadata_serializer.rs)
pub struct ViperMetadataSerializer;

impl EngineMetadataSerializer for ViperMetadataSerializer {
    fn serialize(&self, metadata: &dyn Any) -> Result<Bytes>;
    fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>>;
    fn engine_type(&self) -> &str;

    // Optional: Extract cacheable components (e.g., Parquet footer)
    fn extract_cacheable_component(&self, data: &[u8], file_path: &str) -> Option<Bytes>;

    // Optional: Control what gets cached
    fn should_cache_metadata(&self, file_path: &str) -> bool;
}
```

### Integration with UnifiedCachingFilesystem

```rust
// In engine initialization
let serializer = Arc::new(ViperMetadataSerializer::new());
let cached_fs = UnifiedCachingFilesystem::with_serializer(
    underlying_fs,
    collection_id,
    "viper".to_string(),
    serializer,
);
```

## Existing Code to Leverage

### IntelligentFilesystem (Legacy)
- Location: `src/storage/persistence/filesystem/intelligent_filesystem.rs`
- Has basic metadata caching with HashMap
- TODO markers for Parquet metadata extraction
- Can be migrated to use engine serializers

### ZeroCopyFilesystem (Legacy)
- Location: `src/storage/persistence/filesystem/zero_copy_filesystem.rs`
- Has cache invalidation logic
- Integrated with ZeroCopyIOSystem
- Needs metadata serialization integration

### RAPTOR Engine (Best Example)
- Location: `src/storage/engines/impls/raptor/metadata_serializer.rs`
- Already has `RaptorMetadataSerializer` struct
- Implements file metadata caching
- Good template for other engines

## Migration Plan

### Phase 1: Create Serializers (Current)
- [x] Define `EngineMetadataSerializer` trait
- [ ] VIPER: Create `viper/metadata_serializer.rs`
- [ ] NOVA: Create `nova/metadata_serializer.rs`
- [ ] SST: Create `sst/metadata_serializer.rs`
- [ ] SWIFT: Create `swift/metadata_serializer.rs`
- [x] RAPTOR: Enhance existing serializer
- [ ] PRISM: Create `prism/metadata_serializer.rs`
- [ ] HELIX: Create `helix/metadata_serializer.rs`

### Phase 2: Engine Integration
- [ ] Update each engine's constructor to create serializer
- [ ] Pass serializer when creating UnifiedCachingFilesystem
- [ ] Test metadata caching for each engine

### Phase 3: Optimization
- [ ] VIPER: Implement Parquet footer extraction
- [ ] NOVA: Implement quantization level caching
- [ ] SST: Implement bloom filter caching
- [ ] SWIFT: Implement superblock metadata caching

## Benefits of Engine-Owned Serialization

1. **Loose Coupling**: Filesystem doesn't know engine internals
2. **Flexibility**: Each engine can evolve independently
3. **Expertise**: Engine teams own their metadata format
4. **No Circular Dependencies**: Clean architecture
5. **Open/Closed Principle**: Add new engines without modifying filesystem

## Testing Requirements

Each engine must provide tests for:
1. Metadata serialization/deserialization roundtrip
2. Cacheable component extraction
3. Cache invalidation on writes
4. Performance benchmarks for metadata operations

## Performance Targets

- Metadata serialization: < 1ms for typical metadata
- Metadata deserialization: < 0.5ms
- Cache hit rate: > 90% for hot files
- Memory overhead: < 100 bytes per cached entry