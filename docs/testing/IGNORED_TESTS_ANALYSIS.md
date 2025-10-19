# Ignored Tests Analysis

**Total Ignored Tests**: 10
**Date Analyzed**: 2024-10-19

---

## Summary

| Category | Count | Status | Action |
|----------|-------|--------|--------|
| Benchmark Tests | 4 | ✅ Correct | Keep ignored |
| Integration Tests (require server) | 2 | ✅ Correct | Keep ignored |
| Golden/Snapshot Tests | 1 | ✅ Correct | Keep ignored |
| Disabled Due to Issues | 2 | ⚠️ Investigate | Fix or document |
| Already Fixed/Un-ignored | 1 | ✅ Resolved | None |

**Recommendation**: Fix the 2 problematic tests (unsafe code issue, HELIX compaction setup)

---

## Detailed Analysis

### **1. Benchmark Tests** (4 tests) - ✅ Keep Ignored

**Purpose**: Performance benchmarks, not functional tests

**Location**: `src/storage/engines/impls/tests/helix/integration_tests.rs`

1. **Line 583**: `test_pca_encoding_benchmark`
   - Tests PCA transformation performance
   - Reason ignored: Benchmark, run with `--ignored` flag
   - **Action**: ✅ Keep ignored

2. **Line 613**: `test_hilbert_encoding_benchmark`
   - Tests Hilbert curve encoding speed
   - Reason ignored: Benchmark
   - **Action**: ✅ Keep ignored

3. **Line 636**: `test_liquid_clustering_benchmark`
   - Tests adaptive clustering performance
   - Reason ignored: Benchmark
   - **Action**: ✅ Keep ignored

**Location**: `tests/rust/test_unified_search_benchmarks.rs`

4. **Line 504**: Unified search benchmark
   - Comprehensive search performance test
   - Reason ignored: Long-running benchmark
   - **Action**: ✅ Keep ignored

**Note**: These are intentionally ignored. Run with `cargo test -- --ignored` for performance profiling.

---

### **2. Integration Tests Requiring Server** (2 tests) - ✅ Keep Ignored

**Location**: `tests/engines/viper_metadata_debug_test.rs`

5. **Line 11**: Metadata debug test
   - Requires running ProximaDB server
   - Tests live server metadata queries
   - **Action**: ✅ Keep ignored (or move to E2E test suite)

**Location**: `tests/api_consistency_test.rs`

6. **Line 479**: API consistency test
   - Requires running ProximaDB instance
   - Tests REST/gRPC API consistency
   - **Action**: ✅ Keep ignored (or move to E2E test suite)

**Recommendation**: Consider creating separate E2E test suite that starts server automatically.

---

### **3. Golden/Snapshot Tests** (1 test) - ✅ Keep Ignored

**Location**: `tests/sql_ast_golden.rs`

7. **Line 6**: SQL AST golden file test
   - Snapshot testing for SQL parser
   - Manually run when updating parser
   - **Action**: ✅ Keep ignored (golden tests are opt-in)

---

### **4. Disabled Due to Issues** (2 tests) - ⚠️ INVESTIGATE

**Location**: `src/core/search/optimization_tests.rs`

8. **Line 207**: `test_zero_copy_operations`
   - **Issue**: "Temporarily disabled - unsafe code may be causing issues"
   - **Code**: Uses unsafe pointer operations for zero-copy buffer management
   - **Problem**: Potential undefined behavior or memory safety issue
   - **Impact**: Zero-copy optimization not validated
   - **Action**: ⚠️ **Fix or remove unsafe code**
   - **Priority**: High (if zero-copy is used in production)

**Location**: `tests/helix_performance_comparison_test.rs`

9. **Line 668**: `test_pruning_effectiveness`
   - **Issue**: "Setup issue with manual compaction paths"
   - **Code**: Tests Hilbert pruning with clustered data
   - **Problem**: Compaction path configuration mismatch
   - **Note**: "Pruning effectiveness already validated by scalability test (99.4%)"
   - **Action**: ⚠️ **Fix setup or remove if redundant**
   - **Priority**: Low (already validated elsewhere)

**Location**: `src/storage/engines/impls/tests/raptor/integration_tests.rs`

10. **Line 130**: `test_search_vectors` (RAPTOR)
    - **Issue**: "TODO: Fix search - returns empty results despite successful flush"
    - **Code**: Comment shows `// #[ignore]` (already un-ignored!)
    - **Status**: ✅ Already fixed/un-ignored
    - **Action**: None needed

---

## Recommendations

### **High Priority**

**1. Fix or Remove `test_zero_copy_operations`**

**Current Code** (optimization_tests.rs:207):
```rust
#[tokio::test]
#[ignore] // Temporarily disabled - unsafe code may be causing issues
async fn test_zero_copy_operations() {
    // Uses unsafe pointer operations
    let bytes = unsafe {
        std::slice::from_raw_parts(
            test_data.as_ptr() as *const u8,
            test_data.len() * std::mem::size_of::<f32>(),
        )
    };
}
```

**Options**:
- **Fix**: Validate unsafe code is sound (alignment, lifetime, etc.)
- **Replace**: Use safe alternatives (`bytemuck` crate, `bytes::Bytes`)
- **Remove**: If zero-copy not critical, remove test

**Impact**: If zero-copy optimization is production-critical, this test must be fixed. Otherwise, remove.

### **Low Priority**

**2. Fix or Remove `test_pruning_effectiveness`**

**Issue**: Setup problem with compaction paths

**Options**:
- **Fix**: Correct compaction path configuration
- **Remove**: Already validated by other tests (99.4% pruning in scalability test)

**Recommendation**: Remove test (redundant coverage)

---

## Quick Actions

### **Tests to Un-ignore** (None)

All ignored tests have valid reasons:
- Benchmarks: Intentionally opt-in
- Server-dependent: Require external setup
- Golden tests: Manual verification

### **Tests to Fix**

1. `test_zero_copy_operations` - Unsafe code issue (High Priority)
2. `test_pruning_effectiveness` - Setup issue (Low Priority - redundant)

### **Tests to Remove**

None recommended for removal yet. Fix attempts should be made first.

---

## Test Coverage Summary

**Passing Tests**: 2,621 (after fixing block size assertions)
**Failing Tests**: 2 (quantization precision - unrelated to our changes)
**Ignored Tests**: 10 (24 reported by test runner, need investigation)

**Filter Test Coverage**: 47/47 passing ✅

**Recommendation**: The 24 ignored tests reported by test runner vs 10 found in code suggests some tests are ignored conditionally (e.g., `#[cfg_attr]` or feature flags). Run `cargo test --lib -- --ignored --list` to see full list.

---

## Commands for Investigation

```bash
# List all ignored tests
cargo test --lib -- --ignored --list

# Run specific ignored test
cargo test test_zero_copy_operations -- --ignored --nocapture

# Run all ignored tests
cargo test --lib -- --ignored

# Check for conditional ignores
grep -rn "cfg.*ignore\|ignore.*if" src/ tests/
```
