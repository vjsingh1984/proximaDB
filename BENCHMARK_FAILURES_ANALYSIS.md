# Benchmark Failures Analysis - bench_04_storage_unified.rs

**Date**: 2025-09-29
**Log File**: `bench_output.log.20250929`
**Status**: Partially Fixed (SST ✅, VIPER/NOVA 🔄)

---

## Executive Summary

Analysis of benchmark failures revealed critical path construction bugs in multiple storage engines:

1. **SST Engine**: Path parsing bug causing search to look in wrong directory - **FIXED** ✅
2. **VIPER Engine**: Path duplication bug causing directory creation failures - **IN PROGRESS** 🔄
3. **NOVA Engine**: Similar directory creation issue - **IN PROGRESS** 🔄

---

## 1. SST Engine Search Failure

### Symptoms
```
✓ Flushed 1000 vectors, 8770856 bytes written in 17ms
✓ Created 1 files/directories:
  data (DIR)
❌ Pure search failed for sst with none: IO error: No such file or directory (os error 2)
```

### Root Cause

**File**: `src/storage/engines/impls/sst/search/mod.rs`
**Method**: `discover_sstable_files()` (lines 228-250)

The method incorrectly parsed and reconstructed storage paths:

```rust
// BEFORE (BROKEN):
async fn discover_sstable_files(&self, storage_url: &str) -> Result<Vec<String>> {
    let (base_url, collection_id) = self.parse_storage_url(storage_url)?;
    let data_url = StoragePath::collection_data_path(&base_url, &collection_id);
    // ...
}

// Input: "/tmp/bench/sst-none/data"
// Parsed base: "/tmp/bench/sst-none"
// Parsed collection: "data" ❌ WRONG!
// Reconstructed: "/tmp/bench/sst-none/data/data" ❌ WRONG PATH!
```

### Fix Applied

**Commit**: `e65282a9`

```rust
// AFTER (FIXED):
async fn discover_sstable_files(&self, storage_url: &str) -> Result<Vec<String>> {
    // storage_url is already the correct data directory path
    let data_url = storage_url;
    debug!("🔍 SST discover_sstable_files: Looking for .sst files in {}", data_url);
    // ...
}
```

**Result**: SST search now looks in correct directory matching flush output

---

## 2. VIPER Engine Path Duplication

### Symptoms
```
🟩 HYBRID_WRITER: Records: 250, Final URL: /tmp/proximadb-bench/viper-none-250/viper-none-250/data/L0_20250929T205706_5085f85f.parquet
🟩 HYBRID_WRITER: ❌ write_with_cache failed: No such file or directory (os error 2)
⚠️  Flush failed for viper with none: Failed to write Parquet via HybridParquetWriter
```

### Root Cause (SUSPECTED)

**File**: `src/storage/engines/impls/viper/flush.rs`
**Lines**: 311-314

Path construction creates duplicate collection_id:

```rust
let data_url = format!(
    "{}/{}/data",
    storage_assignment.base_location, collection_id
);
// Expected: /tmp/proximadb-bench/viper-none-250/data
// Actual:   /tmp/proximadb-bench/viper-none-250/viper-none-250/data ❌
```

### Possible Causes

1. **`base_location` already contains collection_id**
   - Benchmark sets `base_location: base_path.clone()` where `base_path = /tmp/proximadb-bench`
   - But somewhere `collection_id` is being appended to it

2. **Filesystem `get_filesystem()` modifies paths**
   - The filesystem factory might be interpreting the path and adding collection_id

3. **`collection_storage_path()` vs `storage_url()` confusion**
   - VIPER might be receiving the wrong path type

### Investigation Needed

```rust
// Add debug logging to identify where duplication occurs:
debug!("VIPER FLUSH PATH DEBUG:");
debug!("  storage_assignment.base_location: {}", storage_assignment.base_location);
debug!("  collection_id: {}", collection_id);
debug!("  data_url constructed: {}", data_url);
debug!("  final_url: {}", final_url);
```

### Recommended Fix Options

**Option 1**: Check if `base_location` already has collection_id and avoid duplication
```rust
let data_url = if storage_assignment.base_location.ends_with(&collection_id) {
    // Already includes collection_id
    format!("{}/data", storage_assignment.base_location)
} else {
    format!("{}/{}/data", storage_assignment.base_location, collection_id)
};
```

**Option 2**: Use `collection_storage_path()` consistently like SST does
```rust
// Get context to access collection_storage_path()
let ctx = self.context().await?;
let data_url = ctx.collection_storage_path();  // Already correct path
```

**Option 3**: Ensure parent directory exists before writing
```rust
// Before writing, ensure parent directory exists
let parent_dir = std::path::Path::new(final_url).parent()
    .ok_or_else(|| anyhow::anyhow!("Invalid final_url: {}", final_url))?;
fs.create_dir_all(parent_dir.to_str().unwrap()).await?;
fs.write(final_url, &data, None).await?;
```

---

## 3. NOVA Engine Issues

### Symptoms
```
⚠️  Flush failed for nova with none: IO error: No such file or directory (os error 2)
```

**Status**: Similar to VIPER, likely same root cause

---

## 4. Testing Recommendations

### Unit Test for Path Construction
```rust
#[tokio::test]
async fn test_viper_path_construction() {
    let base_location = "/tmp/test";
    let collection_id = "test-collection";

    let storage_assignment = StorageAssignment {
        base_location: base_location.to_string(),
        ..Default::default()
    };

    let data_url = format!("{}/{}/data", storage_assignment.base_location, collection_id);

    assert_eq!(data_url, "/tmp/test/test-collection/data");
    assert!(!data_url.contains("test-collection/test-collection"),
           "Path should not contain duplicate collection_id");
}
```

### Integration Test
```bash
# Run with debug logging to trace path construction
RUST_LOG=proximadb::storage::engines::impls::viper=debug \
  cargo test --lib storage::engines::impls::viper::tests::test_flush \
  -- --exact --nocapture
```

### Benchmark Test
```bash
# Run specific engine/compression combination
RUST_LOG=debug cargo bench --bench bench_04_storage_unified 2>&1 | \
  grep -A10 "viper-none"
```

---

## 5. Immediate Action Items

### Priority 1: VIPER Path Fix
- [ ] Add debug logging to VIPER flush.rs to trace path construction
- [ ] Identify where collection_id duplication occurs
- [ ] Implement fix (likely Option 2 - use collection_storage_path())
- [ ] Test with bench_04_storage_unified

### Priority 2: NOVA Path Fix
- [ ] Check if NOVA has same issue
- [ ] Apply similar fix

### Priority 3: Prevent Regression
- [ ] Add path construction unit tests for all engines
- [ ] Add integration test that verifies flush → search roundtrip
- [ ] Document path construction patterns in CLAUDE.md

---

## 6. Root Cause Pattern

All three failures share a common pattern:

**Inconsistent Path Semantics**:
- `storage_url()` returns base path (e.g., `/tmp/bench`)
- `collection_storage_path()` returns data path (e.g., `/tmp/bench/collection/data`)
- Engines mixing these conventions cause path mismatches

**Solution**: Standardize on `collection_storage_path()` for all data operations

---

## 7. Debug Trace Commands

Enable comprehensive debug logging:

```bash
# SST Engine
RUST_LOG=proximadb::storage::engines::impls::sst=debug \
  cargo bench --bench bench_04_storage_unified

# VIPER Engine
RUST_LOG=proximadb::storage::engines::impls::viper=debug,\
proximadb::storage::engines::core::formats::columnar::hybrid_writer=debug \
  cargo bench --bench bench_04_storage_unified

# All storage engines
RUST_LOG=proximadb::storage::engines=debug \
  cargo bench --bench bench_04_storage_unified
```

---

## Status Summary

| Engine | Issue | Status | Commit |
|--------|-------|--------|--------|
| SST    | Path parsing bug | ✅ FIXED | e65282a9 |
| VIPER  | Path duplication | 🔄 IN PROGRESS | - |
| NOVA   | Directory creation | 🔄 IN PROGRESS | - |
| RAPTOR | No issues found | ✅ OK | - |
| SWIFT  | No issues found | ✅ OK | - |
| HELIX  | No issues found | ✅ OK | - |

---

## References

- **Log File**: `bench_output.log.20250929`
- **Benchmark**: `benches/bench_04_storage_unified.rs`
- **SST Fix Commit**: `e65282a9`
- **Analysis Document**: This file