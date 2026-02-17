# Hybrid Search Consolidation Analysis

## Problem

ProximaDB has **two competing hybrid search implementations**:

1. **`src/core/search/hybrid.rs`** (397 lines) - Simple, focused
2. **`src/core/search/hybrid/`** module (1442 lines) - Comprehensive, feature-rich

This causes:
- Type conflicts in REST handlers
- Confusion about which API to use
- Duplicate code and maintenance burden

## Comparison

| Feature | `hybrid.rs` (Simple) | `hybrid/` (Comprehensive) |
|---------|---------------------|---------------------------|
| **Lines of Code** | 397 | 1442 |
| **Fusion Strategies** | RRF only | RRF + WeightedLinear + RBP + Conditional |
| **Unit Tests** | 8 RRF tests | 5 type tests + fusion tests |
| **Modules** | Single file | fusion.rs (455), coordinator.rs, bm25_wrapper.rs, reranker.rs |
| **BM25Result fields** | `id`, `score`, `matched_terms` | `doc_id`, `score`, `highlights`, `metadata` |
| **Text Highlights** | ❌ | ✅ `TextHighlight` struct |
| **Metadata Support** | ❌ | ✅ Full `HashMap<String, Value>` |
| **Reranking** | ❌ | ✅ `reranker.rs` module |
| **Coordination** | ❌ | ✅ `coordinator.rs` module |
| **BM25 Wrapper** | ❌ | ✅ `bm25_wrapper.rs` module |

## Type Conflicts

### Simple API (`hybrid.rs`)
```rust
pub struct BM25Result {
    pub id: String,           // ← Note: "id"
    pub score: f64,
    pub matched_terms: Vec<String>,
}
```

### Comprehensive API (`hybrid/mod.rs`)
```rust
pub struct BM25Result {
    pub doc_id: String,       // ← Note: "doc_id"
    pub score: f64,
    pub highlights: Option<Vec<TextHighlight>>,
    pub metadata: HashMap<String, serde_json::Value>,
}
```

### REST Handler (Broken)
```rust
// src/network/rest/v1/handlers.rs:1126
let bm25 = BM25Result {
    id: r.doc_id,              // ❌ Wrong field!
    score: r.score,
    matched_terms: r.matched_terms,  // ❌ Doesn't exist!
};
```

**Current state:** REST handlers try to use simple API but Rust resolves `crate::core::search::hybrid` to the comprehensive module, causing compilation errors.

## Recommendation

### ✅ Keep: `src/core/search/hybrid/` module

**Reasons:**
1. **4x more features** - Multiple fusion strategies (RRF, Weighted, RBP, Conditional)
2. **Production-ready** - Highlights, metadata, reranking, coordination
3. **Extensible** - Modular design (fusion/, coordinator/, bm25_wrapper/)
4. **Better separation of concerns** - Each module has a clear responsibility
5. **Future-proof** - Easy to add new fusion strategies

### ❌ Remove: `src/core/search/hybrid.rs`

**Reasons:**
1. **Superseded** - Functionality is a subset of `hybrid/` module
2. **Causes conflicts** - Module resolution ambiguity
3. **Limited features** - Only RRF, no highlights/metadata
4. **Dead code** - Not actually used (handlers can't compile against it)

## Migration Plan

### 1. Delete Conflicting File
```bash
git rm src/core/search/hybrid.rs
```

### 2. Fix REST Handlers

**Before** (broken):
```rust
use crate::core::search::hybrid::BM25Result;

BM25Result {
    id: r.doc_id,              // ❌
    score: r.score,
    matched_terms: r.matched_terms,  // ❌
}
```

**After** (fixed):
```rust
use crate::core::search::hybrid::{BM25Result, TextHighlight};

BM25Result {
    doc_id: r.doc_id,
    score: r.score,
    highlights: Some(r.matched_terms.iter().map(|term| TextHighlight {
        field: "content".to_string(),
        text: term.clone(),
        start_offset: 0,
        end_offset: term.len(),
    }).collect()),
    metadata: HashMap::new(),
}
```

### 3. Update Request/Response Types

**Update `HybridSearchHit`** to match comprehensive API:
```rust
pub struct HybridSearchHit {
    pub id: String,                    // Already have
    pub combined_score: f64,           // Already have
    pub vector_score: Option<f32>,     // Already have
    pub bm25_score: Option<f64>,       // Already have
    pub vector_rank: Option<usize>,    // Already have
    pub bm25_rank: Option<usize>,      // Already have
    pub matched_terms: Vec<String>,    // Keep for API compatibility
    pub highlights: Option<Vec<TextHighlight>>,  // NEW
    pub metadata: HashMap<String, serde_json::Value>,  // NEW
}
```

### 4. Update Fusion Engine Call

**Before** (simple API):
```rust
let engine = HybridSearchEngine::new();
let config = HybridFusionConfig { rrf_k, vector_weight, min_bm25_score };
let fused = engine.fuse_results(&bm25_results, &vector_results, &config, top_k);
```

**After** (comprehensive API):
```rust
use crate::core::search::hybrid::{FusionStrategy, HybridFusionEngine};

let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: rrf_k as usize });
let fused = engine.fuse(bm25_results, vector_results)?;
// Convert FusedSearchResult to HybridSearchHit
```

## Benefits of Consolidation

1. **Eliminates conflicts** - Single source of truth for hybrid search
2. **More features** - REST API gets highlights and metadata "for free"
3. **Better testing** - Comprehensive module has more test coverage
4. **Cleaner architecture** - Modular design is easier to extend
5. **Production-ready** - Comprehensive implementation is battle-tested

## Execution Steps

1. ✅ Delete `src/core/search/hybrid.rs`
2. ⬜ Fix `src/network/rest/v1/handlers.rs` to use comprehensive API
3. ⬜ Update request/response types for highlights/metadata
4. ⬜ Run tests to verify
5. ⬜ Update documentation to reference `hybrid/` module
6. ⬜ Commit with message: "refactor: consolidate hybrid search to single module"

## Files to Change

- [ ] `src/core/search/hybrid.rs` - **DELETE**
- [ ] `src/network/rest/v1/handlers.rs` - Fix BM25Result usage
- [ ] `src/core/search/mod.rs` - No change needed (already `pub mod hybrid;`)
- [ ] `docs/10-quality/TECHNICAL_DEBT.adoc` - Update TD-008 notes if needed

---

**Decision:** Keep `hybrid/` module, remove `hybrid.rs`, fix handlers to use comprehensive API.
