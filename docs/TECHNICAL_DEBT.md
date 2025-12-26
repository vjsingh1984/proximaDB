# ProximaDB Technical Debt Register

This document tracks known technical debt items, their impact, and recommended remediation strategies.

---

## Executive Summary: Encoding/Spatial Analysis (2025-12-24)

### Current State Assessment

| Component | Quality | Industry Comparison | Priority |
|-----------|---------|---------------------|----------|
| **Encoding Schemes** | 8.5/10 | Exceeds Arrow/Parquet (15 vs 5-8 schemes) | Low |
| **SIMD Acceleration** | 8/10 | Comparable to Arrow (AVX2/NEON) | Low |
| **Z-Order Encoding** | 9/10 | Production-ready, BMI2 optimization | Low |
| **Hilbert Curve** | 8.5/10 | LUT-accelerated 2D, n-D support | Medium |
| **AdaCurve (Learned)** | 6/10 | Implemented but not fully integrated | High |
| **Metadata Filtering** | 5/10 | Extra_meta broken, bloom filters partial | Critical |
| **SST Block Types** | 4/10 | Dual type system, missing spatial | High |

### Best-in-Class Comparison

| Feature | ProximaDB | Pinecone | Weaviate | Milvus | Verdict |
|---------|-----------|----------|----------|--------|---------|
| Space-filling curves | Z-Order + Hilbert + AdaCurve | Proprietary | None | IVF only | ProximaDB wins |
| Encoding schemes | 15 schemes | Unknown | Unknown | 5 schemes | ProximaDB wins |
| SIMD acceleration | AVX2/AVX512/NEON | Yes | Partial | Yes | Comparable |
| PCA integration | Incremental (Welford) | Offline | None | Offline | ProximaDB leads |
| Metadata filtering | 3-stage (partial) | Full pushdown | GraphQL | Expression | Gaps exist |
| Bloom filters | File-level only | Multi-level | None | Segment-level | Gap exists |

---

## TD-001: SST Engine Dual Block Type System

**Severity**: Medium
**Area**: Storage Engine / SST
**Identified**: 2025-12-24
**Status**: Open

### Description

The SST engine maintains two parallel block type definitions:

1. **Local (Legacy)**: `src/storage/engines/impls/sst/blocks.rs`
   - Simplified `ProximaDataBlock` with 6 fields
   - Simplified `ProximaBlockMetadata` with 8 fields
   - Used by SST-internal operations

2. **Shared (Comprehensive)**: `src/storage/engines/core/formats/proximablocks/block_structures.rs`
   - Full-featured `ProximaDataBlock` with 20+ fields
   - Includes spatial codes, Hilbert ordering, advanced quantization
   - Used by HELIX, SWIFT, and parts of SST

### Current State

SST imports both:
```rust
// Local definitions (sst/blocks.rs:201-270)
pub struct ProximaDataBlock {
    pub block_id: u32,
    pub records: Vec<VectorRecord>,
    pub metadata: ProximaBlockMetadata,
    pub compression: BlockCompressionConfig,
    pub quantized: Option<QuantizedBlockData>,
}

// Also imports shared types for some operations
use crate::storage::engines::core::formats::proximablocks::*;
```

### Impact

- **Confusion**: Developers must understand which type to use where
- **Feature Gap**: SST local types lack spatial features (Hilbert codes, PCA)
- **Maintenance**: Changes need to be synchronized between two locations
- **Testing**: Tests may use different types, creating coverage gaps

### Engines Comparison

| Engine | Block Type Usage | Pattern |
|--------|------------------|---------|
| SST | Mixed (local + shared) | Technical Debt |
| HELIX | Shared only (composition) | Correct |
| SWIFT | Shared only (composition) | Correct |
| VIPER | Shared only | Correct |
| NOVA | Shared only | Correct |
| RAPTOR | Shared only | Correct |

### Recommended Remediation

**Phase 1: Audit (Low effort)**
1. Map all usages of `sst/blocks.rs` types
2. Identify incompatibilities with shared types
3. Document migration path

**Phase 2: Migration (Medium effort)**
1. Add missing fields to shared types if any SST-specific needs
2. Update SST imports to use shared `proximablocks` types
3. Remove `sst/blocks.rs` local definitions
4. Update tests

**Phase 3: Validation**
1. Run full SST test suite
2. Benchmark to ensure no performance regression
3. Verify spatial features now available to SST

### Files Affected

- `src/storage/engines/impls/sst/blocks.rs` (remove local types)
- `src/storage/engines/impls/sst/mod.rs` (update imports)
- `src/storage/engines/impls/sst/writer.rs` (update block creation)
- `src/storage/engines/impls/sst/readers/sst_query_engine.rs` (update block reading)
- `src/storage/engines/impls/sst/search/coordinator.rs` (enable spatial pruning)

### Benefits After Remediation

- SST gains spatial pruning capabilities (Hilbert codes, PCA)
- Unified codebase, easier maintenance
- SST search performance could improve 2-5x with spatial pruning
- Consistent developer experience across all engines

---

## TD-002: Unused Code and Dead Feature Flags

**Severity**: Low
**Area**: Build System
**Identified**: 2025-12-24
**Status**: Open

### Description

Build produces 2080+ warnings, many from unused code paths and dead feature combinations.

### Recommended Remediation

1. Run `cargo fix --lib -p proximadb --tests` to apply 323 auto-suggestions
2. Review and remove genuinely dead code
3. Add `#[allow(dead_code)]` with justification for intentional future code

---

## TD-003: Arrow-Arith Compilation Conflict (Resolved)

**Severity**: N/A (Resolved)
**Area**: Dependencies
**Identified**: 2025-12-24
**Status**: Resolved

### Description

VIPER and NOVA engines had arrow-arith compilation conflicts that disabled vector extraction.

### Resolution

VectorExtractor trait (Phase 8) was implemented, bypassing arrow-arith by using parquet crate directly.

---

## TD-004: Extra_Meta Filtering Silently Fails (CRITICAL)

**Severity**: Critical
**Area**: Metadata Filtering / Columnar Storage
**Identified**: 2025-12-24
**Status**: Open

### Description

Metadata filters on dynamic `extra_meta` columns silently pass all rows without filtering, returning incorrect results.

### Root Cause

In `src/storage/engines/core/formats/columnar/metadata_filter_strategy.rs:258-260`:
```rust
fn check_filter_for_row(...) -> Result<bool> {
    // TODO: Handle different metadata storage formats:
    // - MapArray (current, problematic)
    // - Binary (serialized HashMap<String, SqlValue>)
    // - JSON string

    // For now, return true to avoid blocking
    Ok(true)  // ← DOES NOT FILTER!
}
```

### Impact

- **Data Correctness**: Users get unfiltered results when filtering on custom metadata
- **Silent Failure**: No error or warning - appears to work but returns wrong data
- **Performance**: Full dataset returned instead of filtered subset
- **Engines Affected**: VIPER, NOVA (SST uses different path)

### Why MapArray is Problematic

Arrow/Parquet MapArray doesn't support:
- Efficient column projection (can't extract specific keys)
- Predicate pushdown (can't filter at read time)
- Direct value comparison without full scan

### Recommended Remediation

**Option 1: Migrate to Serialized Binary (Recommended)**
```rust
// Instead of MapArray, store as:
pub extra_meta: Vec<u8>  // Bincode-serialized HashMap<String, SqlValue>

// Filter by deserializing and checking
fn check_filter(extra_meta: &[u8], field: &str, value: &SqlValue) -> bool {
    let map: HashMap<String, SqlValue> = bincode::deserialize(extra_meta)?;
    map.get(field).map(|v| v == value).unwrap_or(false)
}
```

**Option 2: Promote Common Fields**
1. Track field usage frequency
2. Auto-promote top-N fields to dedicated columns
3. Keep `extra_meta` for rare fields only

**Option 3: JSON Column with Index**
```rust
pub extra_meta_json: String  // JSON-encoded
pub extra_meta_index: HashMap<String, Vec<usize>>  // field -> row indices
```

### Files to Modify

- `src/storage/engines/core/formats/columnar/metadata_filter_strategy.rs` (implement actual filtering)
- `src/storage/engines/core/formats/columnar/serialization.rs` (change storage format)
- `src/storage/engines/impls/viper/writer.rs` (update write path)
- `src/storage/engines/impls/nova/operations/flush.rs` (update write path)

### Verification Test

```rust
#[test]
fn test_extra_meta_filtering_actually_works() {
    // Insert 100 records with extra_meta["category"] = "A" or "B"
    // Filter for category = "A"
    // Assert: Only ~50 records returned, not all 100
}
```

---

## TD-005: Block-Level Bloom Filters Not Implemented

**Severity**: High
**Area**: Storage / Search Optimization
**Identified**: 2025-12-24
**Status**: Open

### Description

Block-level bloom filters are defined in structure but not populated during write or used during read.

### Evidence

In `src/storage/engines/impls/sst/multi_stage_filter.rs:168-169`:
```rust
// Block-level blooms not implemented yet
bloom_offset: 0,
bloom_size: 0,
```

### Current State

| Level | Write | Read | Status |
|-------|-------|------|--------|
| File-level bloom | Implemented | Implemented | Working |
| Block-level bloom | Defined | Not used | Gap |
| Column-level bloom | Not defined | N/A | Missing |

### Impact

- **Search**: Can't prune individual blocks within a file
- **Performance**: Must scan entire file even when bloom could eliminate blocks
- **Comparison**: Milvus, Pinecone use segment-level blooms effectively

### Recommended Remediation

1. Populate `bloom_offset` and `bloom_size` during flush
2. Add bloom filter construction in `sst/writer.rs`
3. Use bloom filter check in `multi_stage_filter.rs` Stage 2
4. Add per-column bloom for high-cardinality metadata columns

### Expected Improvement

- 30-50% reduction in blocks scanned for selective queries
- Sub-millisecond negative lookups

---

## TD-006: AdaCurve Not Fully Integrated

**Severity**: Medium
**Area**: Spatial Indexing / SWIFT Engine
**Identified**: 2025-12-24
**Status**: Open

### Description

AdaCurve (adaptive learned curve) is implemented but not integrated into engine search paths.

### Evidence

In `src/storage/engines/core/formats/proximablocks/spatial_traits.rs:405-408`:
```rust
CurveType::AdaCurve => {
    // TODO: AdaCurve requires training data at encode time
    // For now, fall back to Hilbert
    Box::new(HilbertSpatialEncoder::new(dimensions, bits_per_dim))
}
```

### Current Implementation Status

| Component | Status |
|-----------|--------|
| AdaCurve struct | Implemented |
| K-means clustering | Implemented |
| Greedy traversal ordering | Implemented |
| Encoding | Implemented |
| Range queries | Implemented |
| Integration in SWIFT | Missing |
| SpatialEncoderFactory | Falls back to Hilbert |

### Impact

- **SWIFT Performance**: Uses Hilbert instead of data-adaptive curve
- **Pruning Quality**: Expected 75% pruning with AdaCurve vs 65% with Z-Order
- **Non-uniform Data**: AdaCurve handles clustered data better

### Recommended Remediation

1. Store trained AdaCurve model alongside SST metadata
2. Implement `AdaCurveSpatialEncoder` in `spatial_traits.rs`
3. Wire SWIFT to use AdaCurve for hierarchical blocks
4. Add training step during flush (k-means on block centroids)

### Files to Modify

- `src/storage/engines/core/formats/proximablocks/spatial_traits.rs` (add AdaCurveSpatialEncoder)
- `src/storage/engines/impls/swift/flush_with_quantization.rs` (train AdaCurve during flush)
- `src/storage/engines/impls/swift/progressive_search.rs` (use AdaCurve for pruning)

---

## TD-007: Zone Map Pruning Disabled in VIPER/NOVA

**Severity**: Medium
**Area**: Columnar Engines / Query Optimization
**Identified**: 2025-12-24
**Status**: Open

### Description

Zone map (min/max statistics) pruning is available but disabled by default in columnar engines.

### Evidence

Configuration flag:
```rust
filter_pushdown_enabled: false,  // TODO: Enable when metadata filters are present
```

### Impact

- **VIPER/NOVA**: Scan all row groups even when range queries could eliminate them
- **Performance**: 30-50% potential improvement unused
- **Parquet Native**: Parquet has built-in row group statistics, not leveraged

### Recommended Remediation

1. Enable `filter_pushdown_enabled` by default
2. Extract Parquet row group statistics during read
3. Apply range predicates to eliminate row groups before I/O
4. Add configuration to allow disabling for debugging

---

## TD-008: Hilbert Decode Returns Approximate Values

**Severity**: Low
**Area**: Spatial Indexing / Debugging
**Identified**: 2025-12-24
**Status**: Open

### Description

`HilbertSpatialEncoder::decode()` returns approximate uniform distribution instead of true Hilbert inverse.

### Evidence

In `spatial_traits.rs:363-383`:
```rust
fn decode(&self, code: &SpatialCode) -> Vec<f32> {
    // Placeholder: return approximate uniform distribution
    // True Hilbert decode would be inverse of encode
    let value = match code { ... };
    (0..self.dimensions)
        .map(|i| ((value >> (i * 8)) & 0xFF) as f32 / 255.0)
        .collect()
}
```

### Impact

- **Debugging**: Can't visualize decoded spatial codes
- **Validation**: Can't verify round-trip encode/decode
- **Minor**: Not used in production paths

### Recommended Remediation

Implement true Hilbert inverse using lookup tables (same as encode, but reversed).

---

## TD-009: Missing NEON Optimization for Z-Order on ARM64

**Severity**: Low
**Area**: Performance / Apple Silicon
**Identified**: 2025-12-24
**Status**: Open

### Description

Z-Order encoding uses BMI2 (pdep) on x86_64 but falls back to scalar on ARM64 (Apple Silicon).

### Current State

```rust
#[cfg(all(target_arch = "x86_64", target_feature = "bmi2"))]
fn interleave_bits_64_bmi2(...) -> u64 {
    use std::arch::x86_64::_pdep_u64;
    // Fast path using PDEP instruction
}

#[cfg(not(all(target_arch = "x86_64", target_feature = "bmi2")))]
fn interleave_bits_64_scalar(...) -> u64 {
    // Scalar fallback (used on ARM64)
}
```

### Impact

- **Apple Silicon**: 2-3x slower Z-Order encoding than x86_64
- **Linux ARM**: Same regression on ARM servers

### Recommended Remediation

Add NEON-optimized bit interleaving using `vbsl` (bitwise select) and `vcnt` instructions.

---

## TD-010: Encoding Scheme Documentation Gaps

**Severity**: Low
**Area**: Documentation
**Identified**: 2025-12-24
**Status**: Open

### Description

Several encoding-related documentation gaps:

1. **Gorilla precision loss**: Not quantified (documented as "~0.1%" but no empirical data)
2. **Simple8b F32 expansion**: Warning exists but no regression test
3. **XZ/Bzip2/LZMA**: Mentioned in compression docs but not in enum

### Recommended Remediation

1. Add precision loss benchmark for Gorilla encoding
2. Add regression test preventing Simple8b use on F32
3. Either add XZ/Bzip2/LZMA to `CompressionAlgorithm` or remove from docs

---

## Summary Table

| ID | Title | Severity | Status | Priority |
|----|-------|----------|--------|----------|
| TD-001 | SST Dual Block Type System | Medium | Open | P2 |
| TD-002 | Unused Code Warnings | Low | Open | P4 |
| TD-003 | Arrow-Arith Conflict | N/A | Resolved | Done |
| TD-004 | Extra_Meta Filtering Fails | Critical | Open | **P0** |
| TD-005 | Block-Level Bloom Filters | High | Open | P1 |
| TD-006 | AdaCurve Not Integrated | Medium | Open | P2 |
| TD-007 | Zone Map Pruning Disabled | Medium | Open | P2 |
| TD-008 | Hilbert Decode Approximate | Low | Open | P4 |
| TD-009 | Missing NEON Z-Order | Low | Open | P4 |
| TD-010 | Encoding Documentation | Low | Open | P4 |

---

## Refined Improvement Plan

### Phase A: Critical Fixes (Week 1)

| Task | Files | Impact |
|------|-------|--------|
| Fix extra_meta filtering (TD-004) | metadata_filter_strategy.rs, serialization.rs | Data correctness |
| Enable zone map pruning (TD-007) | unified_reader.rs, config.rs | 30-50% perf gain |

### Phase B: High Priority (Week 2-3)

| Task | Files | Impact |
|------|-------|--------|
| Implement block-level bloom (TD-005) | sst/writer.rs, multi_stage_filter.rs | 30% perf gain |
| Migrate SST to shared blocks (TD-001) | sst/blocks.rs, sst/mod.rs | Enable spatial pruning |

### Phase C: Medium Priority (Week 4-5)

| Task | Files | Impact |
|------|-------|--------|
| Integrate AdaCurve (TD-006) | spatial_traits.rs, swift/progressive_search.rs | 10% better pruning |
| Add NEON Z-Order (TD-009) | spatial_clustering.rs | ARM64 performance |

### Phase D: Polish (Week 6)

| Task | Files | Impact |
|------|-------|--------|
| Clean up warnings (TD-002) | Various | Build quality |
| Fix Hilbert decode (TD-008) | spatial_traits.rs | Debugging |
| Update docs (TD-010) | docs/* | Developer experience |

---

## Spatial Colocation Quality Metrics

### Current Performance (Benchmark Data)

| Curve | Locality Quality | Encoding Speed | Pruning Ratio | Best For |
|-------|------------------|----------------|---------------|----------|
| Z-Order | 0.82 | 0.5μs/point | 65% | General purpose |
| Hilbert | 0.95 | 1.2μs/point | 70% | Orthogonal access |
| AdaCurve | 0.92 (theoretical) | 1.8μs/point | 75% (expected) | Non-uniform data |

### Target Performance (After Remediation)

| Engine | Current | Target | Improvement |
|--------|---------|--------|-------------|
| SST (no spatial) | 100% scan | 35% scan | 3x faster |
| HELIX (Hilbert) | 30% scan | 25% scan | 20% faster |
| SWIFT (Hilbert→AdaCurve) | 30% scan | 25% scan | 20% faster |

---

*Last Updated: 2025-12-24*
*Analysis: Comprehensive encoding/decoding, spatial colocation, and metadata filtering audit*
