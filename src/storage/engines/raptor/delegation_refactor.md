# RAPTOR Engine - Delegation Refactor

## Issues Found (2025-08-21)

RAPTOR is duplicating functionality that should be delegated to unified modules:

### 1. SIMD Implementation Duplication ❌
**Current**: RAPTOR has its own `simd_eq_avx512`, `simd_eq_avx2`, `simd_eq_sse4`, `simd_eq_neon` methods
**Should Be**: Delegate to `UnifiedDistanceCompute` which handles SIMD automatically

### 2. Quantization Duplication ❌
**Current**: RAPTOR has custom quantization logic in `rowgroup_manager.rs`
**Should Be**: Delegate to `UnifiedQuantizationEngine` from `compute::quantization::unified`

### 3. Metadata Store Duplication ❌
**Current**: RAPTOR has its own `MetadataColumns` struct and metadata filtering
**Should Be**: Use `storage::cache::specialized::MetadataStore`

### 4. Bitmap Index Duplication ❌
**Current**: RAPTOR directly uses `RoaringBitmap` for filtering
**Should Be**: Use `storage::cache::specialized::BitmapFilterCache`

## Required Changes

### Step 1: Remove SIMD Methods
```rust
// REMOVE all these methods from reader.rs:
- fn simd_eq_neon()
- fn simd_eq_avx512()
- fn simd_eq_avx2() 
- fn simd_eq_sse4()

// REPLACE with:
// Use self.distance_calculator for vector operations
// Use BitmapFilterCache for metadata filtering
```

### Step 2: Use Unified Quantization
```rust
// In rowgroup_manager.rs
// REMOVE: Custom quantization logic
// ADD:
use crate::compute::quantization::unified::UnifiedQuantizationEngine;

struct RowGroupManager {
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    // ... other fields
}
```

### Step 3: Use Unified Metadata Store
```rust
// REMOVE: MetadataColumns struct
// ADD:
use crate::storage::cache::specialized::metadata_store::MetadataStore;

struct RaptorReader {
    metadata_store: Arc<MetadataStore>,
    // ... other fields
}
```

### Step 4: Use Bitmap Filter Cache
```rust
// REMOVE: Direct RoaringBitmap usage
// ADD:
use crate::storage::cache::specialized::bitmap_filter_cache::{
    BitmapFilterCache, CachedFilterResult
};

struct RaptorReader {
    filter_cache: Arc<BitmapFilterCache>,
    // ... other fields
}
```

## Benefits of Proper Delegation

1. **No Code Duplication**: Single source of truth for each functionality
2. **Automatic SIMD**: Hardware detection handled by unified modules
3. **Shared Caching**: All engines benefit from same cache optimizations
4. **Consistent Behavior**: All engines behave the same way
5. **Easier Maintenance**: Fix bugs in one place, not multiple

## Implementation Priority

1. ✅ Remove SIMD methods (DONE - commented out)
2. 🔄 Fix async/await issues (IN PROGRESS)
3. ⏳ Replace metadata storage with unified cache
4. ⏳ Replace quantization with unified engine
5. ⏳ Replace bitmap operations with filter cache