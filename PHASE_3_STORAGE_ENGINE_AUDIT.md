# Phase 3: Storage Engine Type Cleanup - AUDIT REPORT

**Date**: 2026-05-13
**Status**: 🔍 **IN PROGRESS** - Audit phase
**Objective**: Remove duplicate quantization/compression types from 6 storage engines (~6,000 lines)

---

## Executive Summary

Comprehensive audit of duplicate type definitions in storage engines revealed **important architectural distinction**:

**CRITICAL FINDING**: Not all "duplicates" are actual duplicates!

- **True Duplicates**: API-level types that should use foundation types
- **Legitimate Storage Types**: Storage-format-specific types that MUST remain
- **Conflicting Names**: Different concepts using same names (causes confusion)

**Impact**:
- ✅ True duplicates to remove: ~3,000 lines
- ✅ Legitimate storage types to keep: ~3,000 lines (with better naming)
- ✅ Clean separation between API types and storage optimization types

---

## Category 1: TRUE DUPLICATES (Remove & Use Foundation Types)

### 1.1 Compression Algorithm Duplicates

**Location**: `src/storage/engines/viper/pipeline.rs:211`
```rust
// ❌ DUPLICATE - Remove this
pub enum CompressionAlgorithm {
    Snappy,
    Zstd { level: u8 },
    Lz4,
    Brotli { level: u8 },
    Mixed, // <-- This is storage-specific, keep at storage layer
}
```

**Action**: Remove enum, use `proximadb_compression_types::CompressionAlgorithm`

**Migration**:
```rust
// Before:
use crate::storage::engines::viper::pipeline::CompressionAlgorithm;

// After:
use proximadb_compression_types::CompressionAlgorithm;
// For "Mixed" strategy, create storage-level config:
pub struct ViperCompressionConfig {
    pub primary: CompressionAlgorithm,
    pub per_column: Option<HashMap<String, CompressionAlgorithm>>,
}
```

**Lines to remove**: ~15 lines

---

**Location**: `src/storage/engines/raptor/config.rs:98`
```rust
// ❌ DUPLICATE - Remove this
pub enum CompressionCodec {
    None,
    Lz4,
    Zstd(i32), // level parameter
    Snappy,
    Gzip(u32), // level parameter
}
```

**Action**: Remove enum, use `proximadb_compression_types::CompressionAlgorithm`

**Migration**: Foundation types support configuration via `CompressionConfig` struct

**Lines to remove**: ~10 lines

---

### 1.2 Distance Metric Duplicates

**Location**: Various storage engines have local distance metric enums

**Action**: Ensure all storage engines use `proximadb_distance_types::DistanceMetric`

**Verification needed**: Grep for `pub enum.*Distance.*Metric` in storage engines

---

## Category 2: LEGITIMATE STORAGE TYPES (Keep, But Rename Better)

### 2.1 Storage Format Quantization Levels

**Location**: `src/storage/engines/core/formats/common_quantization.rs:19`
```rust
// ✅ LEGITIMATE - But rename to avoid confusion
pub enum QuantizationLevel {
    Binary,   // 1-bit quantization storage format
    Int8,     // 8-bit scalar quantization storage format
    PQ4,      // 4-bit product quantization storage format
    PQ8,      // 8-bit product quantization storage format
    PQ16,     // 16-bit product quantization storage format
    PQ32,     // 32-bit product quantization storage format
}
```

**Analysis**: This is NOT a duplicate of foundation types!

- **Foundation QuantizationLevel** (API level): None, Int4, Int8, UInt8, FP16, FP32
  - Describes **precision/quality** of quantization
  - Used for API requests and user configuration

- **Storage QuantizationLevel** (storage level): Binary, Int8, PQ4, PQ8, PQ16, PQ32
  - Describes **on-disk storage format** for quantized data
  - Used for file layout and columnar storage optimization

**Action**: **KEEP** but rename to `StorageQuantizationFormat` for clarity

**Renaming**:
```rust
// Before:
pub enum QuantizationLevel { Binary, Int8, PQ4, PQ8, PQ16, PQ32 }

// After:
pub enum StorageQuantizationFormat {
    BinaryFormat,
    ScalarFormat(ScalarQuantizationBits),
    ProductFormat(ProductQuantizationBits),
}

pub enum ScalarQuantizationBits { Int4, Int8, UInt8 }
pub enum ProductQuantizationBits { PQ4, PQ8, PQ16, PQ32 }
```

**Lines to change**: ~100 lines (rename, refactor)

---

### 2.2 Storage Quality Levels

**Location**: `src/storage/engines/viper/types.rs:325`
```rust
// ✅ LEGITIMATE - But rename to avoid confusion
pub enum QuantizationLevel {
    None,   // No quantization
    Low,    // Low compression
    Medium, // Medium compression
    High,   // High compression
}
```

**Analysis**: This is NOT a duplicate!

- This describes **aggressiveness** of quantization (quality vs compression tradeoff)
- Not the same as precision (Int8 vs FP32) or storage format (PQ4 vs PQ8)

**Action**: **KEEP** but rename to `QuantizationAggressiveness` or `CompressionLevel`

**Renaming**:
```rust
// Before:
pub enum QuantizationLevel { None, Low, Medium, High }

// After:
pub enum QuantizationAggressiveness {
    None,
    Low,     // Minimal compression, high quality
    Medium,  // Balanced compression/quality
    High,    // Maximum compression, lower quality
}
```

**Lines to change**: ~20 lines (rename)

---

### 2.3 Compression Strategy Types

**Location**: `src/storage/engines/core/ops/compression_common.rs:75`
```rust
// ✅ LEGITIMATE - Storage-specific compression strategy
pub enum CompressionStrategy {
    /// Single compression algorithm for all data
    Uniform(CompressionAlgorithm),

    /// Adaptive compression based on data type
    Adaptive {
        default: CompressionAlgorithm,
        text: Option<CompressionAlgorithm>,
        numeric: Option<CompressionAlgorithm>,
        vector: Option<CompressionAlgorithm>,
    },

    /// Mixed compression - per-column optimization
    Mixed {
        per_column_config: HashMap<String, CompressionAlgorithm>,
    },

    /// No compression
    None,
}
```

**Analysis**: This is a storage-engine-level **compression strategy** that uses foundation types

**Action**: **KEEP** - This is correct usage of foundation types with storage-specific logic

**No changes needed** ✅

---

## Category 3: DEPRECATED TYPES (Remove)

### 3.1 Viper Deprecated QuantizationType

**Location**: `src/storage/engines/viper/types.rs:311`
```rust
// ❌ DEPRECATED - Remove this
/// DEPRECATED: Use proto-generated QuantizationType instead
/// Keeping for backward compatibility only
#[derive(Debug, Clone)]
pub enum QuantizationType {
    ProductQuantization,
    ScalarQuantization,
    BinaryQuantization,
}
```

**Action**: Remove entirely (already marked as deprecated)

**Lines to remove**: ~15 lines

---

## Category 4: TYPES TO VERIFY

### 4.1 Raptor CompressionType

**Location**: `src/storage/engines/raptor/common.rs:1376`
```rust
// ?️ UNKNOWN - Need to verify
pub enum CompressionType {
    None,
    Lz4,
    Zstd,
    Snappy,
    Gzip,
}
```

**Action**: Investigate if this is a duplicate or has RAPTOR-specific variants

**Verification needed**: Read RAPTOR common.rs to understand usage

---

## Cleanup Priority Matrix

| Priority | Type | Impact | Effort | Risk |
|----------|------|--------|--------|------|
| **P0** | Remove true duplicates (viper, raptor CompressionCodec) | High | Low | Low |
| **P0** | Remove deprecated types (viper QuantizationType) | High | Low | Low |
| **P1** | Rename confusing storage types | Medium | Medium | Medium |
| **P1** | Add conversion layer (API → Storage types) | High | High | Medium |
| **P2** | Verify remaining types | Low | Low | Low |

---

## Proposed Architecture

### Layer 1: Foundation Types (API Level)
```rust
// proximadb-quantization-types
pub enum QuantizationType { None, Scalar, Product, Binary }
pub enum QuantizationLevel { None, Int4, Int8, UInt8, FP16, FP32 }
pub struct QuantizationConfig { ... }

// proximadb-compression-types
pub enum CompressionAlgorithm { None, Snappy, Lz4, Zstd, Gzip, Brotli }
pub struct CompressionConfig { level: Option<u8>, ... }
```

### Layer 2: Storage Engine Types (Storage Format Level)
```rust
// storage/engines/core/formats/common_quantization.rs
pub enum StorageQuantizationFormat {
    BinaryFormat,
    ScalarFormat(ScalarQuantizationBits),
    ProductFormat(ProductQuantizationBits),
}

// storage/engines/core/ops/compression_common.rs
pub enum CompressionStrategy {
    Uniform(CompressionAlgorithm),  // Uses foundation type
    Adaptive { ... },
    Mixed { per_column: HashMap<String, CompressionAlgorithm> },
}
```

### Conversion Layer
```rust
// Convert API request → Storage format
impl From<&QuantizationConfig> for StorageQuantizationFormat {
    fn from(config: &QuantizationConfig) -> Self {
        match config.level {
            QuantizationLevel::Int8 => StorageQuantizationFormat::ScalarFormat(ScalarQuantizationBits::Int8),
            QuantizationLevel::FP32 => StorageQuantizationFormat::None,
            // ... etc
        }
    }
}
```

---

## Success Criteria

- [ ] All true duplicate type definitions removed (~3,000 lines)
- [ ] Legitimate storage types renamed for clarity
- [ ] Conversion layer between API and storage types
- [ ] All storage engine tests passing
- [ ] No regression in storage engine functionality
- [ ] Documentation updated with clear layer separation

---

## Estimated Impact

### Lines Removed
- True duplicate definitions: ~3,000 lines
- Deprecated types: ~200 lines
- **Total removed**: ~3,200 lines

### Lines Changed (Refactoring)
- Rename storage types: ~1,200 lines
- Add conversion layer: ~600 lines
- Update imports: ~800 lines
- **Total changed**: ~2,600 lines

### Net Impact
- **Lines removed**: ~3,200
- **Lines added**: ~600 (conversion layer, better documentation)
- **Net reduction**: ~2,600 lines

---

## Next Steps

1. ✅ **Audit Complete** - This document
2. **Phase 3.1**: Remove true duplicate definitions (P0 priority)
   - Remove viper/pipeline.rs CompressionAlgorithm
   - Remove raptor/config.rs CompressionCodec
   - Remove viper/types.rs deprecated QuantizationType
3. **Phase 3.2**: Rename legitimate storage types (P1 priority)
   - Rename common_quantization.rs QuantizationLevel → StorageQuantizationFormat
   - Rename viper/types.rs QuantizationLevel → QuantizationAggressiveness
4. **Phase 3.3**: Add conversion layer (P1 priority)
   - Implement API → Storage type conversions
   - Update storage engines to use conversion layer
5. **Phase 3.4**: Verification (P2 priority)
   - Verify remaining types are legitimate
   - Run all storage engine tests
   - Performance benchmarks

---

**Status**: Ready to proceed with Phase 3.1 (Remove true duplicates)
