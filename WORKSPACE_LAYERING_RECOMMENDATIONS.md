# Workspace Layering Recommendations

## Current State vs Target State

### Current Workspace Structure (Problems Highlighted)

```
crates/
├── foundation/        ✅ WELL-ORGANIZED
│   ├── proximadb-kernel             (2,249 lines)
│   ├── proximadb-proto              (120 lines)
│   ├── proximadb-data-model         (410 lines)
│   ├── proximadb-config             (790 lines)
│   ├── proximadb-filter-expression  (105 lines)
│   ├── proximadb-pipeline-operator  (72 lines)
│   └── proximadb-records            (1,621 lines)
│
├── contracts/         ❌ MISSING LAYER
│   └── (Should contain: vector-query, graph-query, document-query, observability-query)
│
├── query/            ⚠️ MIX OF CONTRACTS AND IMPLEMENTATIONS
│   ├── proximadb-vector-query        ✅ Contract (Phase 2.1)
│   ├── proximadb-graph-query         ✅ Contract (Phase 2.2)
│   ├── proximadb-document-query      ✅ Contract (Phase 2.2)
│   ├── proximadb-observability-query ✅ Contract (Phase 2.2)
│   ├── proximadb-query-filter        ✅ Contract
│   ├── proximadb-query-clauses       ⚠️ Utility (should be in foundation or contract crate)
│   ├── proximadb-multimodel-plan     ⚠️ Implementation (3,000+ lines, should be in runtime crate)
│   ├── proximadb-uql                 ⚠️ Implementation (should be in runtime crate)
│   ├── proximadb-multimodel-query    ⚠️ Implementation (should be in runtime crate)
│   ├── proximadb-query               ❌ LARGE IMPLEMENTATION (3,000+ lines, should be split)
│   └── proximadb-query-fusion        ⚠️ Implementation (should be in runtime crate)
│
├── modalities/       ⚠️ INCOMPLETE EXTRACTION
│   ├── proximadb-vector             ✅ Phase 3 complete (2,137 lines)
│   ├── proximadb-graph              ⚠️ Incomplete (5 files)
│   └── proximadb-document           ⚠️ Incomplete (2 files)
│   └── proximadb-compression        ❌ MISSING (should exist)
│   └── proximadb-encoding           ❌ MISSING (should exist)
│
├── storage/          ⚠️ TOO COARSE-GRAINED
│   └── proximadb-storage-common     (Should be split: storage-contracts, storage-runtime)
│
├── platform/         ✅ APPROPRIATE
│   ├── proximadb-api
│   └── proximadb-runtime
│
├── horizontal/       ✅ APPROPRIATE
│   ├── proximadb-runtime-common
│   ├── proximadb-security
│   └── proximadb-telemetry
│
└── control/          ✅ APPROPRIATE
    └── proximadb-catalog

src/                 ❌ CONTAINS IMPLEMENTATIONS THAT SHOULD BE IN CRATES
├── compute/          ❌ SHOULD BE IN MODALITY CRATES
│   ├── quantization/    ❌ 12,817 lines (duplicates proximadb-vector)
│   └── distance_computation/ ❌ 2,500+ lines (duplicates proximadb-vector)
│
├── core/             ⚠️ CONTAINS IMPLEMENTATIONS + FOUNDATION TYPES
│   ├── compression/     ❌ Should be in proximadb-compression modality
│   ├── search/          ❌ Should be split across modalities
│   ├── storage/         ❌ Should be in storage-runtime crate
│   └── foundation/      ✅ Thin re-export shims (correct for transition)
│
├── storage/          ⚠️ CONTAINS DUPLICATE IMPLEMENTATIONS
│   └── engines/*/quantization*.rs  ❌ Duplicates (6 engines × ~500 lines each)
│
└── query/            ⚠️ CONTAINS LARGE IMPLEMENTATIONS
    └── unified.rs     ❌ 3,000+ lines (should be in runtime crate)
```

### Target Workspace Structure

```
crates/
├── foundation/        ✅ SINGLE SOURCE OF TRUTH FOR TYPES
│   ├── proximadb-kernel             Error types, traits, utilities
│   ├── proximadb-proto              Protocol buffer types
│   ├── proximadb-data-model         ProximaType, ProximaValue
│   ├── proximadb-config             Configuration
│   ├── proximadb-filter-expression  Filter expressions
│   ├── proximadb-pipeline-operator  Pipeline operators
│   ├── proximadb-records            ProximaRecord envelope
│   ├── proximadb-distance-types    ✅ NEW: DistanceMetric enum
│   ├── proximadb-quantization-types ✅ NEW: QuantizationType, Level, Config
│   ├── proximadb-compression-types  ✅ NEW: CompressionAlgorithm, Config
│   └── proximadb-encoding-types     ✅ NEW: Encoding algorithms
│
├── contracts/        ✅ STABLE SERVICE CONTRACTS
│   ├── vector/
│   │   ├── proximadb-vector-query        VectorQueryService trait
│   │   └── proximadb-vector-index        VectorIndex trait
│   ├── graph/
│   │   └── proximadb-graph-query         GraphQueryService trait
│   ├── document/
│   │   └── proximadb-document-query      DocumentQueryService trait
│   ├── observability/
│   │   └── proximadb-observability-query ObservabilityQueryService trait
│   └── storage/
│       ├── proximadb-storage-contracts  UnifiedStorageEngine trait
│       └── proximadb-compression        CompressionProvider trait
│
├── modalities/       ✅ MODALITY RUNTIMES (IMPLEMENTATIONS)
│   ├── proximadb-vector             ✅ Complete vector operations runtime
│   │   ├── distance/       (from src/compute/distance_computation/)
│   │   ├── quantization/   (from src/compute/quantization/)
│   │   ├── index/          (HNSW, IVF, PQ, Annoy, LSH)
│   │   ├── search/         (Vector search algorithms)
│   │   └── service/        (VectorQueryService implementation)
│   │
│   ├── proximadb-graph              ✅ Complete graph operations runtime
│   │   ├── traversal/      (Graph traversal algorithms)
│   │   ├── storage/        (Graph storage formats)
│   │   ├── query/          (Graph query execution)
│   │   └── service/        (GraphQueryService implementation)
│   │
│   ├── proximadb-document           ✅ Complete document operations runtime
│   │   ├── query/          (Document query execution)
│   │   ├── storage/        (Document storage formats)
│   │   └── service/        (DocumentQueryService implementation)
│   │
│   ├── proximadb-compression        ✅ NEW: Compression runtime
│   │   ├── providers/      (Snappy, Lz4, Zstd, Gzip, Brotli)
│   │   ├── streaming/      (Streaming compression)
│   │   └── adapter/        (CompressionProvider implementations)
│   │
│   └── proximadb-encoding           ✅ NEW: Encoding runtime
│       ├── binary/         (Binary encoding)
│       ├── json/           (JSON encoding)
│       ├── protobuf/       (Protobuf encoding)
│       └── streaming/      (Streaming encoding)
│
├── query/            ✅ CROSS-MODEL QUERY RUNTIME
│   ├── contracts/
│   │   ├── proximadb-query-filter
│   │   └── proximadb-query-clauses
│   ├── runtime/
│   │   ├── proximadb-multimodel-query     (from src/query/unified.rs)
│   │   ├── proximadb-multimodel-plan
│   │   ├── proximadb-uql
│   │   └── proximadb-query-fusion
│   └── adapters/
│       ├── proximadb-query               (Main query facade)
│       └── proximadb-query-fusion
│
├── storage/          ✅ STORAGE RUNTIME
│   ├── contracts/
│   │   └── proximadb-storage-contracts
│   ├── runtime/
│   │   ├── proximadb-storage-core        (Storage engine management)
│   │   ├── proximadb-persistence         (WAL, filesystem, cache)
│   │   └── proximadb-engines             (Engine implementations)
│   └── engines/
│       ├── sst/              (Uses proximadb-vector for vector ops)
│       ├── helix/            (Uses proximadb-vector for vector ops)
│       ├── viper/            (Uses proximadb-vector for vector ops)
│       ├── nova/             (Uses proximadb-vector for vector ops)
│       ├── swift/            (Uses proximadb-vector for vector ops)
│       └── raptor/           (Uses proximadb-vector for vector ops)
│
├── platform/         ✅ APPLICATION LAYER
│   ├── proximadb-api                (REST, gRPC, PostgreSQL wire)
│   └── proximadb-runtime            (Server composition, hardware detection)
│
├── horizontal/       ✅ CROSS-CUTTING CONCERNS
│   ├── proximadb-runtime-common
│   ├── proximadb-security
│   └── proximadb-telemetry
│
└── control/          ✅ CONTROL PLANE
    └── proximadb-catalog

src/                 ✅ MINIMAL (MOSTLY RE-EXPORTS)
├── lib.rs           (Binary entry point, re-exports from crates)
├── database.rs      (Database struct using runtime crates)
├── main.rs          (Main function)
└── legacy/          (Temporary compatibility shims during migration)
```

---

## Dependency Direction Rules

### Golden Rule: Downward Only

```
Application Layer (platform/)
    ↓ depends on
Cross-Model Query Runtime (query/runtime/)
    ↓ depends on
Modality Runtimes (modalities/)
    ↓ depends on
Service Contracts (contracts/)
    ↓ depends on
Foundation Types (foundation/)
    ↓ depends on
EXTERNAL DEPENDENCIES (arrow, tokio, etc.)
```

### Forbidden Patterns

❌ **Modality runtime depends on application layer**
```rust
// BAD: src/compute/quantization/unified.rs depends on src/storage/
use crate::storage::engines::viper::ViperEngine;
```

❌ **Foundation types depend on implementations**
```rust
// BAD: proximadb-kernel depends on proximadb-vector
// This creates circular dependencies
```

❌ **Storage engine implements modality operations**
```rust
// BAD: src/storage/engines/viper/quantization.rs
// Should use: crates/modalities/proximadb-vector/quantization
```

✅ **Correct patterns**
```rust
// GOOD: Storage engine uses modality runtime
use proximadb_vector::{QuantizationType, DistanceMetric};

// GOOD: Modality runtime uses foundation types
use proximadb_quantization_types::{QuantizationType, QuantizationLevel};

// GOOD: Application uses modality runtime
use proximadb_vector::VectorServiceImpl;
```

---

## Module Naming Guidelines

### Avoid "Unified" Prefix

**Problem**: "Unified" is overused (70+ modules) and often meaningless

**Replace with semantic names**:

| Current Name | Semantic Name | Rationale |
|--------------|--------------|-----------|
| `unified_scan_strategy` | `scan_strategy` | Only one scan strategy in storage layer |
| `unified_reader` | `columnar_reader` | Describes the format (columnar), not "unified" |
| `unified_metadata_serializer` | `metadata_serializer` | Only one per engine, name is descriptive |
| `unified_cache` | `cache_coordinator` | Describes its role (coordination) |
| `unified_handler` | `multi_protocol_handler` | Describes what it does (handles multiple protocols) |
| `unified_query` | `multimodal_query` | Describes what it queries (multiple modalities) |
| `unified_auth` | `auth_service` | Describes what it provides (auth) |
| `unified_rbac` | `rbac_service` | Describes what it provides (RBAC) |

### Guidelines

1. **Use descriptive names**: What does it do? What format? What protocol?
2. **Avoid implementation details**: `vector_handler` not `vector_v1_handler_v2_impl`
3. **Prefer domain terms**: `scan_strategy` not `query_execution_optimization`
4. **Single responsibility**: One module, one clear purpose

---

## Migration Checklist

### Phase 1: Foundation Types (Week 1-2)

- [ ] Create `proximadb-distance-types` crate
- [ ] Create `proximadb-quantization-types` crate
- [ ] Create `proximadb-compression-types` crate
- [ ] Create `proximadb-encoding-types` crate
- [ ] Move type definitions from `src/core/`, `src/storage/`, `src/network/`
- [ ] Update all imports across codebase
- [ ] Run `cargo clippy` to find missing imports
- [ ] Run `cargo test` to verify correctness

### Phase 2: Modality Runtimes (Week 3-5)

- [ ] Complete `proximadb-vector` extraction
  - [ ] Move `src/compute/distance_computation/` → vector modality
  - [ ] Move `src/compute/quantization/` → vector modality
  - [ ] Implement `VectorIndex` trait
  - [ ] Add comprehensive tests
- [ ] Complete `proximadb-graph` extraction
  - [ ] Move graph-specific code from `src/graph/`
  - [ ] Implement graph traversal algorithms
  - [ ] Add comprehensive tests
- [ ] Complete `proximadb-document` extraction
  - [ ] Move document-specific code from `src/storage/document/`
  - [ ] Implement document query execution
  - [ ] Add comprehensive tests
- [ ] Create `proximadb-compression` modality
  - [ ] Consolidate compression implementations
  - [ ] Implement `CompressionProvider` trait
  - [ ] Add comprehensive tests
- [ ] Create `proximadb-encoding` modality
  - [ ] Consolidate encoding implementations
  - [ ] Add comprehensive tests

### Phase 3: Storage Engine Simplification (Week 6-7)

- [ ] Remove quantization from all storage engines
- [ ] Remove distance computation from all storage engines
- [ ] Remove compression implementations from storage engines
- [ ] Update storage engines to use modality runtimes
- [ ] Add tests verifying storage engine uses modality runtimes
- [ ] Run benchmarks to ensure no performance regression

### Phase 4: "Unified" Module Cleanup (Week 8)

- [ ] Audit all 70+ "unified" modules
- [ ] Categorize: consolidation, wrapper, duplicate
- [ ] Rename to semantic names
- [ ] Remove unnecessary wrappers
- [ ] Consolidate duplicates
- [ ] Update all imports
- [ ] Update documentation

### Phase 5: Layering Enforcement (Week 9-10)

- [ ] Update `CLAUDE.md` with layering rules
- [ ] Add dependency direction checks to CI
- [ ] Create ADRs for layering decisions
- [ ] Add linter rules for layering violations
- [ ] Document crate placement in `WORKSPACE_REFACTOR_PLAN`
- [ ] Train team on layering guidelines

---

## Success Metrics

### Code Quality Metrics

| Metric | Current | Target | Measurement |
|--------|---------|--------|-------------|
| Duplicate type definitions | 90+ | < 5 | `grep -r "pub enum.*Metric" src/` |
| "Unified" modules | 70+ | < 20 | `find . -name "*unified*.rs"` |
| Lines in src/compute/ | ~15,000 | 0 | `wc -l src/compute/**/*.rs` |
| Storage engine duplicate code | ~3,000 lines | 0 | `wc -l src/storage/engines/*/quantization*.rs` |
| Foundation crates | 7 | 11 | `ls crates/foundation/` |
| Modality crates | 3 | 5 | `ls crates/modalities/` |

### Architecture Metrics

| Metric | Current | Target | Verification |
|--------|---------|--------|--------------|
| Upward dependencies | Unknown | 0 | `cargo tree --invert` |
| Circular dependencies | 0 | 0 | `cargo tree --duplicates` |
| Foundation type usage | ~20% | 100% | Code review |
| Modality runtime usage | ~10% | 100% | Code review |
| Storage engine complexity | High | Low | Lines per engine |

### Process Metrics

| Metric | Current | Target | Measurement |
|--------|---------|--------|-------------|
| Layering violations per sprint | Unknown | 0 | Code review checklist |
| New type definitions in src/ | Unknown | 0 | Code review checklist |
| CI time | ~30 min | < 35 min | CI duration |
| Compilation time | ~5 min | < 5 min | `cargo build --timings` |

---

## Conclusion

The workspace layering recommendations provide a clear path from the current state (with significant code proliferation) to a well-organized, layered workspace that follows the DATA_AI_PLATFORM_ARCHITECTURE_ANCHOR vision.

**Key Takeaways**:
1. Foundation types should be the single source of truth
2. Modality runtimes should contain all modality-specific implementations
3. Storage engines should use modality runtimes, not parallel implementations
4. "Unified" prefix should be replaced with semantic names
5. Dependency direction should always be downward

**Next Steps**: Execute migration checklist in phases, starting with Phase 1 (Foundation Types).
