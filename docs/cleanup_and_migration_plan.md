# Cleanup and Migration Plan for Storage Engines

## Phase 1: Identify Duplicate Code to Remove

### SST Engine Duplicates
```
Location: /src/storage/engines/sst/
Duplicates to remove:
- quantization.rs → Use row_based/quantization_adapter.rs
- compression logic → Use common/compression_adapter.rs
- block structures → Use row_based/block_structures.rs
- Search logic that duplicates common/search_common.rs
```

### SWIFT Engine Duplicates
```
Location: /src/storage/engines/swift/
Duplicates to remove:
- SstFile struct → Use row_based structures
- Quantization logic → Use row_based/quantization_adapter.rs
- Hierarchical block logic that can use row_based/block_structures.rs
```

### VIPER Engine Duplicates
```
Location: /src/storage/engines/viper/
Duplicates to remove:
- types.rs: CollectionMetadata, ClusterMetadata → Use columnar types
- readers/unified_parquet_reader.rs: FileMetadata, MetadataFilter → Use columnar
- compaction.rs: FileMetadata → Use columnar
- quantization.rs → Use columnar/quantization_adapter.rs
```

### NOVA Engine Duplicates
```
Location: /src/storage/engines/nova/
Duplicates to remove:
- Already cleaned NovaMetadata ✅
- quantized_columns.rs → Use columnar structures
- columnar_search.rs → Use columnar/search.rs
```

## Phase 2: Dead Code to Remove

### Deprecated Modules
- [x] /viper/dual_mode/ - Already deleted ✅
- [ ] /viper/pipeline_tests.rs - Check if still needed
- [ ] /nova/progressive_refinement.rs - Duplicate of common functionality
- [ ] Any test modules referencing deleted code

### Unused Type Definitions
- [ ] VIPER: VectorStorageFormat, VectorQualityMetrics if unused
- [ ] NOVA: NovaQuantizationConfig if redundant
- [ ] SST: SstRecord if fully replaced by VectorRecord
- [ ] SWIFT: SwiftMetadata if replaced

## Phase 3: Migration Implementation

### Step 1: Update SST to Use Row-Based Common
```rust
// Before (sst/mod.rs)
use crate::storage::engines::sst::quantization::SstQuantization;
use crate::storage::engines::sst::block::DataBlock;

// After
use crate::storage::engines::row_based::{
    quantization_adapter::RowBasedQuantizationAdapter,
    block_structures::{DataBlock, BlockMetadata},
};
```

### Step 2: Update SWIFT to Use Row-Based Common
```rust
// Before (swift/mod.rs)
pub struct SstFile {
    // Duplicate fields
}

// After
use crate::storage::engines::row_based::block_structures::RowBasedFile;
pub type SstFile = RowBasedFile; // Or compose it
```

### Step 3: Update VIPER to Use Columnar Common
```rust
// Before (viper/types.rs)
pub struct CollectionMetadata { /* duplicate */ }

// After
use crate::storage::engines::columnar::ColumnarFileMetadata;
// Remove CollectionMetadata entirely
```

### Step 4: Update NOVA to Use Columnar Common
```rust
// Before
use crate::storage::engines::nova::columnar_search;

// After
use crate::storage::engines::columnar::search::ColumnarSearcher;
```

## Phase 4: Code Cleanup Actions

### 1. Remove Duplicate Imports
Find and replace all imports of deleted types with common module imports.

### 2. Update Function Signatures
Change functions that used engine-specific types to use common types.

### 3. Delete Empty/Deprecated Files
Remove files that only contained type aliases or deprecated code.

### 4. Fix Compilation Errors
Address any compilation errors from the cleanup.

## Phase 5: Verification

### Testing Strategy
1. Run existing tests to ensure no regression
2. Add integration tests for common modules
3. Benchmark performance before/after

### Code Quality Checks
1. No duplicate type definitions
2. All engines use appropriate common modules
3. No dead code warnings
4. Clean compilation

## Implementation Order

1. **Start with SST** (simplest row-based engine)
   - Migrate to row_based common
   - Remove duplicates
   - Test thoroughly

2. **Then SWIFT** (builds on SST work)
   - Migrate hierarchical structures
   - Reuse SST migration patterns

3. **Then VIPER** (simplest columnar engine)
   - Clean up types.rs
   - Migrate to columnar common

4. **Finally NOVA** (most complex)
   - Complete columnar migration
   - Remove all duplicates

## Files to Delete/Clean

### Definitely Delete
- `/viper/types.rs` - Most of it is duplicate
- `/nova/columnar_search.rs` - Use columnar/search.rs
- `/swift/quantization.rs` - Use row_based
- `/sst/compression.rs` - If duplicate of common

### Refactor/Simplify
- `/viper/readers/unified_parquet_reader.rs` - Keep only VIPER-specific
- `/sst/mod.rs` - Simplify to use common modules
- `/nova/mod.rs` - Already partially done
- `/swift/engine.rs` - Update to use common

### Keep (Engine-Specific)
- Engine-specific optimizations
- Unique algorithms (e.g., SWIFT's 3-tier hierarchy)
- Custom metrics/monitoring
- Specialized test utilities

## Success Metrics

- **Lines of Code**: Reduce by ~8000 lines (45%)
- **Duplicate Types**: Zero duplicate type definitions
- **Common Module Usage**: 100% of engines using appropriate common modules
- **Compilation**: Clean build with no warnings
- **Tests**: All existing tests pass
- **Performance**: No regression in benchmarks

## Risks and Mitigations

### Risk: Breaking Changes
**Mitigation**: Incremental migration, one engine at a time

### Risk: Performance Regression
**Mitigation**: Benchmark before/after each change

### Risk: Lost Functionality
**Mitigation**: Comprehensive test coverage before changes