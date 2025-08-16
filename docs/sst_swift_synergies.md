# SST and SWIFT Engine Synergies

## Overview
Both SST and SWIFT engines use SST (Sorted String Table) file formats but with different optimizations. This document outlines their synergies and recommendations for code reuse.

## Common Elements

### 1. Core Data Structures
Both engines use similar block-based structures:

#### SST Engine
```rust
pub struct DataBlock {
    pub block_id: u32,
    pub records: Vec<VectorRecord>,
    pub uncompressed_size: u32,
    pub compression_algorithm: CompressionAlgorithm,
    pub metadata_stats: DataBlockMetadata,
    pub block_bloom_filter: Option<Vec<u8>>,
    pub has_deletes: bool,
    pub quantized_section: QuantizedSection  // From quantization module
}
```

#### SWIFT Engine
```rust
pub struct DataBlock {
    pub id: u32,
    pub records: Vec<VectorRecord>,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub quantized_block: QuantizedBlock,  // Custom implementation
    pub metadata_stats: HashMap<String, ColumnStats>,
    // ... other fields
}
```

### 2. Common Features
- **VectorRecord Usage**: Both directly use `VectorRecord` (no intermediate conversions)
- **Bloom Filters**: Both implement bloom filters for efficient filtering
- **Quantization**: Both have quantization sections (different implementations)
- **Metadata Statistics**: Both track metadata for query optimization
- **Compression**: Both support block-level compression

## Key Differences

### 1. Hierarchy
- **SST**: Flat structure with blocks
- **SWIFT**: 3-tier hierarchy (SuperBlock → DataBlock → Records)
  - SuperBlocks: 1GB of data
  - DataBlocks: 16MB of vectors
  - ~2000 vectors per block

### 2. Quantization
- **SST**: Uses `QuantizedSection` from the shared quantization module
- **SWIFT**: Custom `QuantizedBlock` with progressive search optimization
  - Binary sketches
  - INT8 vectors
  - PQ codes
  - Distance tables

### 3. Indexing
- **SST**: Simple block-level indexing
- **SWIFT**: Advanced hierarchical indexing
  - ID index (B+ tree)
  - Quantized index
  - Metadata index

## Opportunities for Code Reuse

### 1. Shared DataBlock Base
Create a common `BaseDataBlock` in `row_based` module:
```rust
pub struct BaseDataBlock {
    pub id: u32,
    pub records: Vec<VectorRecord>,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub metadata_stats: HashMap<String, MetadataStats>,
    pub bloom_filter: Option<Vec<u8>>,
}
```

### 2. Unified Quantization
Consolidate quantization implementations:
- Move SWIFT's `QuantizedBlock` features into SST's `QuantizedSection`
- Use universal quantization adapter for both
- Share distance table computation
- Reuse PQ codebook training

### 3. Shared Bloom Filter
Both engines can use the same bloom filter implementation:
- Already exists in `crate::storage::engines::sst::bloom_filter`
- SWIFT can import and use SST's bloom filter

### 4. Common Compression
Both should use the universal compression adapter:
- SST: Already migrated ✅
- SWIFT: Migration in progress

### 5. Shared Utilities
Extract to `row_based::common`:
- Block serialization/deserialization
- Metadata statistics computation
- ID range tracking
- Timestamp management

## Implementation Strategy

### Phase 1: Extract Common Structures
1. Create `BaseDataBlock` in `row_based::common`
2. Extract shared metadata types
3. Create common block interfaces

### Phase 2: Unify Quantization
1. Merge SWIFT's progressive search into `QuantizedSection`
2. Use universal quantization adapter
3. Share codebook training logic

### Phase 3: Consolidate Utilities
1. Move bloom filter to row_based common
2. Share compression configuration
3. Unify block serialization

### Phase 4: Optimize Integration
1. SWIFT extends `BaseDataBlock` with SuperBlock hierarchy
2. SST uses simpler flat structure
3. Both share core functionality

## Benefits

1. **Code Deduplication**: ~2000 lines of duplicate code eliminated
2. **Consistent Behavior**: Same quantization and compression across engines
3. **Easier Maintenance**: Single implementation to maintain
4. **Performance**: Shared optimizations benefit both engines
5. **Testing**: Single test suite for shared components

## Migration Path

### Current Status
- ✅ SST: Migrated to universal adapters
- 🔄 SWIFT: Migration in progress
- ⏳ Shared structures: Not yet extracted

### Next Steps
1. Complete SWIFT universal adapter migration
2. Extract `BaseDataBlock` to row_based common
3. Unify quantization implementations
4. Share bloom filter implementation
5. Create comprehensive tests

## Code Examples

### Using Shared BaseDataBlock
```rust
// In SST engine
impl From<BaseDataBlock> for SstDataBlock {
    fn from(base: BaseDataBlock) -> Self {
        SstDataBlock {
            base,
            quantized_section: QuantizedSection::new(),
        }
    }
}

// In SWIFT engine
impl From<BaseDataBlock> for SwiftDataBlock {
    fn from(base: BaseDataBlock) -> Self {
        SwiftDataBlock {
            base,
            superblock_id: 0,
            hierarchical_metadata: HierarchicalMetadata::new(),
        }
    }
}
```

### Shared Bloom Filter Usage
```rust
// Both engines use the same bloom filter
use crate::storage::engines::row_based::common::bloom::UnifiedBloomFilter;

let bloom = UnifiedBloomFilter::new(expected_items, false_positive_rate);
bloom.add(&record.id);
if bloom.contains(&query_id) {
    // Process block
}
```

## Conclusion

The SST and SWIFT engines share significant commonalities that should be leveraged through the row_based common module. This will reduce code duplication, improve maintainability, and ensure consistent behavior across both engines.

The hierarchical structure of SWIFT provides advanced capabilities while SST offers simplicity. By sharing core components, both engines benefit from improvements to the shared codebase while maintaining their unique optimizations.