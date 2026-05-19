# Hybrid Search Implementation - Technical Details

## Overview

This document tracks the implementation of Hybrid Search (BM25 + Vector Fusion) for ProximaDB using TDD methodology.

## Architecture

### Module Structure

```
src/core/search/hybrid/
├── mod.rs              # Type definitions (FusionStrategy, Result types)
├── fusion.rs           # Fusion algorithm implementations (RRF, Weighted, RBP, CCF)
├── coordinator.rs      # Parallel BM25 + Vector search orchestration
├── bm25_wrapper.rs     # Tantivy BM25 wrapper (stub)
├── reranker.rs         # Result reranking (stub)
└── tests/
    ├── basic_test.rs   # 7 basic TDD tests (currently verifying)
    └── fusion_test.rs  # Comprehensive fusion tests (TODO)
```

### Type System

#### FusionStrategy Enum

```rust
pub enum FusionStrategy {
    ReciprocalRank { k: usize },                    // RRF: score = 1/(k+rank1) + 1/(k+rank2)
    WeightedLinear {
        alpha: f64,                                  // BM25 weight (0-1)
        bm25_normalize: bool,                        // Normalize to [0,1]?
        vector_normalize: bool,                      // Normalize to [0,1]?
    },
    RankBiasedPrecision { persistence: f64 },       // RBP: score = (1-p) * p^(rank-1)
    ConditionalNormalization,                        // CCF: Normalize both, average
}
```

#### Result Types

```rust
pub struct BM25Result {
    pub doc_id: String,
    pub score: f64,
    pub highlights: Option<Vec<TextHighlight>>,
    pub metadata: HashMap<String, serde_json::Value>,
}

pub struct VectorResult {
    pub doc_id: String,
    pub score: f64,
    pub distance: f64,
    pub metadata: HashMap<String, serde_json::Value>,
}

pub struct FusedSearchResult {
    pub doc_id: String,
    pub bm25_score: f64,
    pub vector_score: f64,
    pub fused_score: f64,        // Combined score
    pub bm25_rank: usize,         // Rank in BM25 results
    pub vector_rank: usize,       // Rank in vector results
    pub highlights: Option<Vec<TextHighlight>>,
    pub metadata: HashMap<String, serde_json::Value>,
}
```

### Fusion Algorithms

#### 1. Reciprocal Rank Fusion (RRF)

**Formula**: `score = 1/(k + rank_bm25) + 1/(k + rank_vector)`

**Properties**:
- Robust to score scale differences
- Handles disjoint results (no overlap)
- Handles overlapping results (sums scores)
- Default k=60 (tunable)

**Implementation** (src/core/search/hybrid/fusion.rs:67):
```rust
fn reciprocal_rank_fusion(
    &self,
    bm25_results: Vec<BM25Result>,
    vector_results: Vec<VectorResult>,
    k: usize,
) -> Result<Vec<FusedSearchResult>, FusionError> {
    let mut fused_map: HashMap<String, FusedSearchResult> = HashMap::new();

    // Process BM25 results
    for (rank, bm25) in bm25_results.iter().enumerate() {
        let rrf_score = 1.0 / (k as f64 + rank as f64 + 1.0);
        // Store in map
    }

    // Process vector results and merge
    for (rank, vector) in vector_results.iter().enumerate() {
        let rrf_score = 1.0 / (k as f64 + rank as f64 + 1.0);
        if let Some(existing) = fused_map.get_mut(&vector.doc_id) {
            existing.fused_score += rrf_score;  // Sum for overlapping docs
        }
    }

    // Sort by fused score descending
    Ok(fused_results)
}
```

**Test Case**:
```rust
// doc1: rank 1 in BM25, rank 0 in vector
// Expected: 1/(60+1) + 0 = 0.016393...
assert!((fused[0].fused_score - 0.0164).abs() < 0.0001);
```

#### 2. Weighted Linear Fusion

**Formula**: `score = alpha * bm25_normalized + (1-alpha) * vector_normalized`

**Properties**:
- Requires score normalization (optional)
- Configurable weight (alpha)
- alpha=0.5 gives equal weight

**Implementation** (src/core/search/hybrid/fusion.rs:126):
```rust
fn weighted_linear_fusion(
    &self,
    bm25_results: Vec<BM25Result>,
    vector_results: Vec<VectorResult>,
    alpha: f64,
    bm25_normalize: bool,
    vector_normalize: bool,
) -> Result<Vec<FusedSearchResult>, FusionError> {
    // Find max scores for normalization
    let bm25_max = bm25_results.iter().map(|r| r.score).fold(0.0f64, f64::max);
    let vector_max = vector_results.iter().map(|r| r.score).fold(0.0f64, f64::max);

    // Normalize and combine
    for bm25 in &bm25_results {
        let normalized_score = if bm25_normalize && bm25_max > 0.0 {
            bm25.score / bm25_max
        } else {
            bm25.score
        };
        fused_score = alpha * normalized_score;
        // ... add vector component
    }
}
```

**Test Case**:
```rust
// BM25: 0.8, Vector: 0.6, alpha: 0.5, no normalization
// Expected: 0.5 * 0.8 + 0.5 * 0.6 = 0.7
assert!((fused[0].fused_score - 0.7).abs() < 0.01);
```

#### 3. Rank Biased Precision (RBP)

**Formula**: `score = (1-p) * p^(rank-1)`

**Properties**:
- Emphasizes top ranks
- Higher persistence = more emphasis on early ranks
- Typical values: 0.8 to 0.99

#### 4. Conditional Normalization (CCF)

**Formula**: `score = (bm25_normalized + vector_normalized) / 2`

**Properties**:
- Normalizes both to [0,1]
- Simple average
- No tunable parameters

## TDD Implementation Status

### ✅ Phase 0: Test Infrastructure (Complete)

**Files Created**:
- `tests/tdd/test_utils/mod.rs` - TestContext for auto cleanup
- `tests/tdd/test_utils/approx.rs` - Floating-point comparisons
- `tests/tdd/test_utils/mock_data.rs` - Test data generators
- `tests/tdd/test_utils/perf.rs` - Performance assertions
- `.github/workflows/tdd.yml` - CI/CD pipeline
- `Makefile` - TDD targets
- `docs/TDD_GUIDE.md` - Developer guide

### 🔄 Phase 1: Hybrid Search (In Progress)

#### ✅ Completed

1. **Type Definitions** (src/core/search/hybrid/mod.rs)
   - FusionStrategy enum with 4 strategies
   - BM25Result, VectorResult, FusedSearchResult
   - TextHighlight struct
   - Display implementation for FusionStrategy

2. **Fusion Algorithms** (src/core/search/hybrid/fusion.rs)
   - Reciprocal Rank Fusion (RRF) - FULLY IMPLEMENTED
   - Weighted Linear Fusion - FULLY IMPLEMENTED
   - Rank Biased Precision (RBP) - FULLY IMPLEMENTED
   - Conditional Normalization (CCF) - FULLY IMPLEMENTED
   - HybridFusionEngine with fuse() method
   - FusionError for error handling

3. **Basic Tests** (src/core/search/hybrid/tests/basic_test.rs)
   - test_fusion_strategy_types - Type creation
   - test_result_types - Result types
   - test_fusion_engine_creation - Engine creation
   - test_empty_fusion - Empty results
   - test_fusion_basic_disjoint - Disjoint BM25+Vector
   - test_rrf_score_calculation - RRF math verification
   - test_weighted_linear_fusion - Weighted average

4. **Coordinator** (src/core/search/hybrid/coordinator.rs)
   - Parallel search orchestration (stub)
   - Type annotations fixed

#### 🟡 Current Status: Fixing Module Structure

**Issues Fixed**:
1. ✅ Removed duplicate `hybrid.rs` file (conflicted with `hybrid/` directory)
2. ✅ Fixed circular import in `mod.rs` (was trying to re-export from `fusion.rs`)
3. ✅ Fixed type annotations in `coordinator.rs` (explicit `Ok::<Type, Error>()`)

**Current**: Compiling and running tests to verify GREEN phase

#### ⏳ TODO (This Week)

1. **Verify Tests Pass** - Waiting for compilation to complete
2. **Comprehensive Tests** - Add fusion_test.rs with:
   - RRF with overlapping results
   - Different k values
   - Edge cases (empty, single result, large results)
   - All 4 fusion strategies
   - Score normalization verification

3. **BM25 Integration** (Week 2-3)
   - Implement actual Tantivy integration in bm25_wrapper.rs
   - Document indexing
   - Query parsing
   - Highlight generation

4. **REST API** (Week 3)
   - `POST /api/v1/search/hybrid`
   - Request/response types
   - Query validation

5. **Python SDK** (Week 4)
   - `client.hybrid_search()` method
   - Fusion strategy configuration
   - Result parsing

## Test Coverage

### Current Tests (7 tests, ~5 assertions each)

```rust
test_fusion_strategy_types       // Type creation ✓
test_result_types                // Result types ✓
test_fusion_engine_creation      // Engine creation ✓
test_empty_fusion                // Empty handling ✓
test_fusion_basic_disjoint        // Disjoint fusion ✓
test_rrf_score_calculation       // RRF math ✓
test_weighted_linear_fusion      // Weighted fusion ✓
```

### Planned Tests (fusion_test.rs)

- RRF with overlapping results
- RRF with different k values (10, 60, 100)
- Weighted linear with normalization
- Weighted linear with different alpha values
- RBP with different persistence values
- CCF edge cases
- Large result sets (1000+)
- Score sorting verification
- Metadata preservation

## Integration Points

### Within ProximaDB

- **Tantivy** (via bm25_wrapper.rs): Full-text BM25 search
- **Vector Engines**: SST, HELIX, VIPER, NOVA, RAPTOR, SWIFT
- **REST API**: `/api/v1/search/hybrid` (pending)
- **SQL**: `SELECT * FROM HYBRID_SEARCH(...)` (pending)

### External Systems

- **Python SDK**: `client.hybrid_search()` (pending)
- **REST API**: JSON request/response (pending)

## Performance Characteristics

### Fusion Algorithm Complexity

| Strategy | Time Complexity | Space Complexity |
|----------|----------------|------------------|
| RRF | O(n + m) | O(n + m) |
| Weighted Linear | O(n + m) | O(n + m) |
| RBP | O(n + m) | O(n + m) |
| CCF | O(n + m) | O(n + m) |

Where:
- n = number of BM25 results
- m = number of vector results

### Sorting Complexity

All strategies require sorting by fused_score: O((n+m) log (n+m))

## Next Steps

1. **Immediate** (Today)
   - ✅ Fix module structure
   - ⏳ Verify basic tests pass (GREEN phase)
   - ⏳ Add comprehensive tests (fusion_test.rs)

2. **Short Term** (Week 2-3)
   - BM25 wrapper with Tantivy
   - REST API endpoint
   - Integration tests

3. **Medium Term** (Week 4+)
   - Python SDK
   - Performance benchmarks
   - Documentation

## References

- [TDD Guide](../TDD_GUIDE.md)
- [Progress Tracker](../HYBRID_SEARCH_PROGRESS.md)
- Test files: `src/core/search/hybrid/tests/`
