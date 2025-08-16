# Cleanup Duplicate Types in Storage Engines

## Overview
This document tracks the cleanup of duplicate type definitions across storage engines. All engines should use common types from `columnar` or `common` modules directly.

## Types to Replace

### From Columnar Module (`storage/engines/columnar/`)
- `ColumnarFileMetadata` - Replace all engine-specific metadata structs
- `QuantizationConfig` - Replace all engine-specific quantization configs
- `ColumnStatistics` - Replace duplicate column stats
- `MetadataFilter` - Replace duplicate filter types
- `FilterCondition` - Replace duplicate filter conditions
- `RowGroupStats` - Replace duplicate row group statistics
- `QuantizationLevel` - Replace duplicate quantization levels

### From Common Module (`storage/engines/common/`)
- `UniversalCompressionAdapter` - Already migrated ✅
- `UniversalQuantizationAdapter` - Already migrated ✅
- `UniversalSearchPipeline` - Created ✅

## Changes Required

### VIPER Engine
1. **Remove from `types.rs`**:
   - `CollectionMetadata` → Use `ColumnarFileMetadata`
   - `ClusterMetadata` → Create in columnar if needed
   - `CompressionStats` → Use common statistics
   - `CompressionConfig` → Use `UniversalCompressionConfig`
   - `CompressionAlgorithm` → Use from `core::compression`

2. **Remove from `compaction.rs`**:
   - `FileMetadata` → Use `ColumnarFileMetadata`

3. **Remove from `readers/unified_parquet_reader.rs`**:
   - `FileMetadata` → Use `ColumnarFileMetadata`
   - `MetadataFilter` → Use from columnar

4. **Remove from `index_based_reader.rs`**:
   - `VIPERParquetMetadata` → Use `ColumnarFileMetadata`
   - `VIPERParquetMetadataSource` → Use columnar equivalent

### NOVA Engine
1. **Already cleaned in `mod.rs`**:
   - ~~`NovaMetadata`~~ → Using `ColumnarFileMetadata` ✅
   - ~~`NovaFile.metadata`~~ → Changed to `ColumnarFileMetadata` ✅

2. **Remove type aliases**:
   - Remove `pub type NovaMetadata = ColumnarFileMetadata;`
   - Remove `pub use ... as NovaFileMetadata;`
   - Just use `ColumnarFileMetadata` directly

### SST Engine
1. **Check for duplicates**:
   - Any metadata structures that duplicate row-based common

### SWIFT Engine  
1. **Check for duplicates**:
   - Any metadata structures that duplicate row-based common

## Direct Usage Pattern

Instead of:
```rust
pub type ViperMetadata = ColumnarFileMetadata;
pub struct ViperFile {
    metadata: ViperMetadata,
}
```

Use directly:
```rust
use crate::storage::engines::columnar::ColumnarFileMetadata;
pub struct ViperFile {
    metadata: ColumnarFileMetadata,
}
```

## Benefits
1. **No Confusion**: Clear where types come from
2. **No Duplication**: Single source of truth
3. **Easy Maintenance**: Changes in one place affect all engines
4. **Type Safety**: Compiler ensures compatibility

## Implementation Steps
1. ✅ Delete `dual_mode` directories
2. ✅ Update NOVA to use `ColumnarFileMetadata` directly
3. ⏳ Update VIPER to use columnar types directly
4. ⏳ Remove all type aliases (NovaMetadata, ViperMetadata, etc.)
5. ⏳ Update all references to use columnar types directly
6. ⏳ Check SST/SWIFT for similar duplicates with row-based common

## Code Locations
- Columnar common: `/src/storage/engines/columnar/`
- Universal common: `/src/storage/engines/common/`
- VIPER types: `/src/storage/engines/viper/types.rs`
- NOVA types: `/src/storage/engines/nova/mod.rs`
- SST types: `/src/storage/engines/sst/`
- SWIFT types: `/src/storage/engines/swift/`