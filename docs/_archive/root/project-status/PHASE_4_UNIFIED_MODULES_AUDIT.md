# Phase 4: "Unified" Modules Audit

**Date**: 2026-05-13
**Total Modules Found**: 56
**Status**: Audit complete, ready for cleanup

---

## Audit Summary

| Category | Count | Lines | Action |
|----------|-------|-------|--------|
| **Genuine Consolidations** | 20 | ~18,000 | Rename to semantic names |
| **Unnecessary Re-exports** | 6 | ~12 | Remove (inline imports) |
| **Engine-Specific Duplicates** | 10 | ~2,500 | Consolidate or rename |
| **Test Files** | 14 | ~6,000 | Rename to match subject |
| **Documentation/Examples** | 6 | ~2,000 | Keep as-is or rename |

---

## Category 1: Genuine Consolidations (Keep, Rename to Semantic Names)

**Count**: 20 modules (~18,000 lines)
**Action**: Rename to semantic names, update all imports

### Network Layer (3 modules)

| Module | Lines | New Name | Purpose |
|--------|-------|----------|---------|
| `network/unified_handler.rs` | 1,714 | `multi_protocol_handler.rs` | Consolidates REST, gRPC, PostgreSQL wire protocol handlers |
| `network/multiplex/unified_server.rs` | 355 | `protocol_multiplexer.rs` | Multi-protocol server multiplexing |
| `network/rest/v1/unified_query.rs` | 2,164 | `multimodal_query.rs` | Multi-model query endpoint handler |

**Rationale**: These modules consolidate multiple protocols into a unified interface. New names clearly indicate multi-protocol/multi-model nature.

### Query Layer (4 modules)

| Module | Lines | New Name | Purpose |
|--------|-------|----------|---------|
| `query/unified_routing.rs` | 494 | `query_router.rs` | Routes SQL/facade/UQL through MultiModelPlan |
| `query/unified_query_optimizer.rs` | 2,566 | `query_optimizer.rs` | Cost-based query optimization |
| `query/unified_explain.rs` | 416 | `explain_schema.rs` | Query explain plan schema |
| `graph/query/unified_parser.rs` | 30 | `graph_parser.rs` | Graph query parser (Cypher) |

**Rationale**: These are core query infrastructure with clear purposes. Names should reflect function, not "unified".

### Storage Layer (6 modules)

| Module | Lines | New Name | Purpose |
|--------|-------|----------|---------|
| `storage/unified_scan_strategy.rs` | 349 | `scan_strategy.rs` | Unified scan strategy for all storage engines |
| `storage/cache/unified_cache.rs` | 548 | `cache_coordinator.rs` | Coordinates all cache types (Vector, Metadata, Query) |
| `storage/cache/unified_eviction.rs` | 486 | `eviction_policy.rs` | Unified eviction policy across caches |
| `storage/metadata/unified_index.rs` | 548 | `metadata_index.rs` | Unified metadata index structure |
| `storage/persistence/write_ahead_log/unified_operations.rs` | 736 | `wal_operations.rs` | Write-ahead log operations |
| `storage/persistence/filesystem/unified.rs` | 649 | `unified_filesystem.rs` | Unified filesystem abstraction (keep name - it's accurate) |

**Rationale**: These are genuine consolidations of storage infrastructure. Names should reflect specific purpose.

### Core/Search Layer (3 modules)

| Module | Lines | New Name | Purpose |
|--------|-------|----------|---------|
| `core/search/unified_interface.rs` | 403 | `search_interface.rs` | Unified search interface for vector/graph/document |
| `core/search/unified_progressive_pipeline.rs` | 935 | `progressive_search_pipeline.rs` | Progressive multi-stage search pipeline |
| `compute/quantization/unified.rs` | 2,662 | `quantization_engine.rs` | Unified quantization engine (keep location, rename file) |

**Rationale**: Core search infrastructure with clear purposes.

### Security Layer (2 modules)

| Module | Lines | New Name | Purpose |
|--------|-------|----------|---------|
| `security/unified_auth.rs` | 1,285 | `auth_service.rs` | Unified authentication service |
| `security/unified_rbac.rs` | 1,253 | `rbac_service.rs` | Unified role-based access control |

**Rationale**: Security services with clear purposes.

### API Layer (2 modules)

| Module | Lines | New Name | Purpose |
|--------|-------|----------|---------|
| `api_handlers/unified_handlers.rs` | 3,432 | `request_handlers.rs` | Multi-protocol request handlers |
| `storage/engines/core/unified_scan_impl.rs` | 498 | `scan_implementation.rs` | Scan implementation for storage engines |

**Rationale**: Core request handling infrastructure.

---

## Category 2: Unnecessary Re-exports (Remove, Inline Imports)

**Count**: 6 modules (~12 lines)
**Action**: Remove files, update imports to use `core::*` directly

### Engine-Specific Re-exports (6 modules)

| Module | Lines | Action | Replacement |
|--------|-------|--------|-------------|
| `storage/engines/viper/unified_metadata_serializer.rs` | 2 | Remove | Use `crate::storage::engines::core::viper_unified_metadata_serializer::*` |
| `storage/engines/raptor/unified_metadata_serializer.rs` | 2 | Remove | Use `crate::storage::engines::core::raptor_unified_metadata_serializer::*` |
| `storage/engines/sst/unified_metadata_serializer.rs` | 2 | Remove | Use `crate::storage::engines::core::sst_unified_metadata_serializer::*` |
| `storage/engines/helix/unified_metadata_serializer.rs` | 2 | Remove | Use `crate::storage::engines::core::helix_unified_metadata_serializer::*` |
| `storage/engines/nova/unified_metadata_serializer.rs` | 2 | Remove | Use `crate::storage::engines::core::nova_unified_metadata_serializer::*` |
| `storage/engines/swift/unified_metadata_serializer.rs` | 2 | Remove | Use `crate::storage::engines::core::swift_unified_metadata_serializer::*` |

**Rationale**: These are 2-line re-export files. Unnecessary indirection - import directly from `core::*`.

**Migration Pattern**:
```rust
// Old code
use crate::storage::engines::viper::unified_metadata_serializer::*;

// New code
use crate::storage::engines::core::viper_unified_metadata_serializer::*;
```

---

## Category 3: Engine-Specific Duplicates (Consolidate or Rename)

**Count**: 10 modules (~2,500 lines)
**Action**: Rename to format-specific names or consolidate

### Core Metadata Serializers (5 modules)

| Module | Lines | New Name | Purpose |
|--------|-------|----------|---------|
| `storage/engines/core/viper_unified_metadata_serializer.rs` | 264 | `viper_metadata_serializer.rs` | VIPER metadata serialization |
| `storage/engines/core/raptor_unified_metadata_serializer.rs` | 216 | `raptor_metadata_serializer.rs` | RAPTOR metadata serialization |
| `storage/engines/core/sst_unified_metadata_serializer.rs` | 241 | `sst_metadata_serializer.rs` | SST metadata serialization |
| `storage/engines/core/helix_unified_metadata_serializer.rs` | 355 | `helix_metadata_serializer.rs` | HELIX metadata serialization |
| `storage/engines/core/swift_unified_metadata_serializer.rs` | 301 | `swift_metadata_serializer.rs` | SWIFT metadata serialization |

**Rationale**: These are in `core/` directory already, so "unified" prefix is redundant. Rename to `{engine}_metadata_serializer.rs`.

### Engine Strategy Readers (4 modules)

| Module | Lines | New Name | Purpose |
|--------|-------|----------|---------|
| `storage/engines/viper/unified_strategy_reader.rs` | 161 | `strategy_reader.rs` | VIPER Parquet strategy reader |
| `storage/engines/helix/unified_strategy_reader.rs` | 447 | `strategy_reader.rs` | HELIX strategy reader |
| `storage/engines/nova/unified_strategy_reader.rs` | 521 | `progressive_reader.rs` | NOVA progressive strategy reader |
| `storage/engines/swift/unified_strategy_reader.rs` | 503 | `hierarchical_reader.rs` | SWIFT hierarchical strategy reader |

**Rationale**: Engine-specific implementations. Rename to reflect actual strategy (progressive, hierarchical, etc.).

### Engine Readers (1 module)

| Module | Lines | New Name | Purpose |
|--------|-------|----------|---------|
| `storage/engines/swift/unified_reader.rs` | 984 | `hierarchical_reader.rs` | SWIFT hierarchical storage reader |

**Rationale**: SWIFT-specific hierarchical reader implementation.

---

## Category 4: Test Files (Rename to Match Subject)

**Count**: 14 test files (~6,000 lines)
**Action**: Rename to test the actual module names after Category 1-3 renames

### Engine Tests (8 files)

| Module | Lines | New Name | Tests |
|--------|-------|----------|-------|
| `storage/engines/viper/tests/unified_storage_tests.rs` | 287 | `storage_integration_tests.rs` | VIPER storage integration |
| `storage/engines/viper/tests/unified_parquet_reader_tests.rs` | 776 | `parquet_reader_tests.rs` | VIPER Parquet reader |
| `storage/engines/viper/tests/unified_parquet_reader_edge_tests.rs` | 551 | `parquet_reader_edge_tests.rs` | VIPER Parquet edge cases |
| `storage/engines/sst/tests/unified_sstable_reader_tests.rs` | 273 | `sstable_reader_tests.rs` | SSTable reader |
| `storage/engines/sst/tests/unified_sstable_reader_edge_tests.rs` | 1,062 | `sstable_reader_edge_tests.rs` | SSTable edge cases |
| `api_handlers/unified_handlers_tests.rs` | 611 | `request_handlers_tests.rs` | Request handler tests |
| `core/search/unified_interface_tests.rs` | 644 | `search_interface_tests.rs` | Search interface tests |
| `metrics/collectors/tests/unified_integration_test.rs` | 440 | `metrics_integration_test.rs` | Metrics integration |

**Rationale**: Test files should match the module they test, not the old "unified" naming.

### Additional Test Files (6 files)

| Module | Lines | New Name | Tests |
|--------|-------|----------|-------|
| `storage/engines/nova/unified_columnar_integration.rs` | 1,301 | Keep or rename to `columnar_integration.rs` | NOVA columnar integration (may be production code, not test) |
| `storage/engines/core/formats/columnar/unified_columnar_io.rs` | 872 | `columnar_io.rs` | Columnar I/O operations |
| `storage/engines/core/formats/columnar/unified_compaction.rs` | 894 | `compaction_manager.rs` | Compaction management |
| `storage/engines/core/formats/columnar/columnar_query_engine/unified_reader.rs` | 2,985 | `columnar_reader.rs` | Columnar query engine reader |

---

## Category 5: Documentation/Examples (Keep as-is or Minor Rename)

**Count**: 6 modules (~2,000 lines)
**Action**: Keep documentation, optionally rename for clarity

| Module | Lines | Action | Notes |
|--------|-------|--------|-------|
| `metrics/examples/unified_metrics_usage.rs` | 314 | Keep or rename to `metrics_usage_examples.rs` | Example code - keep |
| `storage/persistence/filesystem/unified_config.rs` | 486 | `filesystem_config.rs` | Filesystem configuration |
| `storage/persistence/filesystem/unified_cache.rs` | 457 | `filesystem_cache.rs` | Filesystem cache |
| `index/axis/indexes/ivf_unified.rs` | 1,880 | `ivf_index.rs` | IVF index implementation |
| `storage/engines/sst/unified_reader.rs` | 302 | `sstable_reader.rs` | SSTable reader |
| `storage/engines/nova/unified_metadata_serializer.rs` | 2 | Remove (re-export, see Category 2) | |

---

## Detailed File Inventory

### All 56 "Unified" Modules Sorted by Size

| Lines | Module | Category | New Name |
|-------|--------|----------|----------|
| 3,432 | api_handlers/unified_handlers.rs | 1 | request_handlers.rs |
| 2,985 | storage/engines/core/formats/columnar/columnar_query_engine/unified_reader.rs | 4 | columnar_reader.rs |
| 2,662 | compute/quantization/unified.rs | 1 | quantization_engine.rs |
| 2,566 | query/unified_query_optimizer.rs | 1 | query_optimizer.rs |
| 2,164 | network/rest/v1/unified_query.rs | 1 | multimodal_query.rs |
| 1,880 | index/axis/indexes/ivf_unified.rs | 5 | ivf_index.rs |
| 1,714 | network/unified_handler.rs | 1 | multi_protocol_handler.rs |
| 1,285 | security/unified_auth.rs | 1 | auth_service.rs |
| 1,253 | security/unified_rbac.rs | 1 | rbac_service.rs |
| 1,301 | storage/engines/nova/unified_columnar_integration.rs | 4 | columnar_integration.rs |
| 1,062 | storage/engines/sst/tests/unified_sstable_reader_edge_tests.rs | 4 | sstable_reader_edge_tests.rs |
| 984 | storage/engines/swift/unified_reader.rs | 3 | hierarchical_reader.rs |
| 935 | core/search/unified_progressive_pipeline.rs | 1 | progressive_search_pipeline.rs |
| 894 | storage/engines/core/formats/columnar/unified_compaction.rs | 4 | compaction_manager.rs |
| 872 | storage/engines/core/formats/columnar/unified_columnar_io.rs | 4 | columnar_io.rs |
| 776 | storage/engines/viper/tests/unified_parquet_reader_tests.rs | 4 | parquet_reader_tests.rs |
| 736 | storage/persistence/write_ahead_log/unified_operations.rs | 1 | wal_operations.rs |
| 649 | storage/persistence/filesystem/unified.rs | 1 | unified_filesystem.rs (keep) |
| 644 | core/search/unified_interface_tests.rs | 4 | search_interface_tests.rs |
| 611 | api_handlers/unified_handlers_tests.rs | 4 | request_handlers_tests.rs |
| 551 | storage/engines/viper/tests/unified_parquet_reader_edge_tests.rs | 4 | parquet_reader_edge_tests.rs |
| 548 | storage/cache/unified_cache.rs | 1 | cache_coordinator.rs |
| 548 | storage/metadata/unified_index.rs | 1 | metadata_index.rs |
| 521 | storage/engines/nova/unified_strategy_reader.rs | 3 | progressive_reader.rs |
| 503 | storage/engines/swift/unified_strategy_reader.rs | 3 | hierarchical_reader.rs |
| 498 | storage/engines/core/unified_scan_impl.rs | 1 | scan_implementation.rs |
| 494 | query/unified_routing.rs | 1 | query_router.rs |
| 486 | storage/cache/unified_eviction.rs | 1 | eviction_policy.rs |
| 486 | storage/persistence/filesystem/unified_config.rs | 5 | filesystem_config.rs |
| 457 | storage/persistence/filesystem/unified_cache.rs | 5 | filesystem_cache.rs |
| 447 | storage/engines/helix/unified_strategy_reader.rs | 3 | strategy_reader.rs |
| 440 | metrics/collectors/tests/unified_integration_test.rs | 4 | metrics_integration_test.rs |
| 416 | query/unified_explain.rs | 1 | explain_schema.rs |
| 403 | core/search/unified_interface.rs | 1 | search_interface.rs |
| 355 | storage/engines/core/helix_unified_metadata_serializer.rs | 3 | helix_metadata_serializer.rs |
| 355 | network/multiplex/unified_server.rs | 1 | protocol_multiplexer.rs |
| 349 | storage/unified_scan_strategy.rs | 1 | scan_strategy.rs |
| 314 | metrics/examples/unified_metrics_usage.rs | 5 | metrics_usage_examples.rs |
| 302 | storage/engines/sst/unified_reader.rs | 5 | sstable_reader.rs |
| 301 | storage/engines/core/swift_unified_metadata_serializer.rs | 3 | swift_metadata_serializer.rs |
| 290 | storage/engines/core/nova_unified_metadata_serializer.rs | 3 | nova_metadata_serializer.rs |
| 287 | storage/engines/sst/tests/unified_sstable_reader_tests.rs | 4 | sstable_reader_tests.rs |
| 264 | storage/engines/core/viper_unified_metadata_serializer.rs | 3 | viper_metadata_serializer.rs |
| 241 | storage/engines/core/sst_unified_metadata_serializer.rs | 3 | sst_metadata_serializer.rs |
| 216 | storage/engines/core/raptor_unified_metadata_serializer.rs | 3 | raptor_metadata_serializer.rs |
| 161 | storage/engines/viper/unified_strategy_reader.rs | 3 | strategy_reader.rs |
| 30 | graph/query/unified_parser.rs | 1 | graph_parser.rs |
| 2 | storage/engines/viper/unified_metadata_serializer.rs | 2 | REMOVE (re-export) |
| 2 | storage/engines/raptor/unified_metadata_serializer.rs | 2 | REMOVE (re-export) |
| 2 | storage/engines/sst/unified_metadata_serializer.rs | 2 | REMOVE (re-export) |
| 2 | storage/engines/helix/unified_metadata_serializer.rs | 2 | REMOVE (re-export) |
| 2 | storage/engines/nova/unified_metadata_serializer.rs | 2 | REMOVE (re-export) |
| 2 | storage/engines/swift/unified_metadata_serializer.rs | 2 | REMOVE (re-export) |

---

## Implementation Plan

### Phase 4.1: Remove Unnecessary Re-exports (Easiest)
- **Count**: 6 files (~12 lines)
- **Risk**: Low
- **Time**: 1 hour
- **Action**: Delete 2-line re-export files, update imports

### Phase 4.2: Rename Genuine Consolidations (Highest Impact)
- **Count**: 20 files (~18,000 lines)
- **Risk**: Medium
- **Time**: 4-6 hours
- **Action**: Rename files and update all imports

### Phase 4.3: Rename Engine-Specific Duplicates (Medium Impact)
- **Count**: 10 files (~2,500 lines)
- **Risk**: Medium
- **Time**: 2-3 hours
- **Action**: Rename to engine-specific or format-specific names

### Phase 4.4: Rename Test Files (Low Priority)
- **Count**: 14 files (~6,000 lines)
- **Risk**: Low
- **Time**: 2-3 hours
- **Action**: Rename test files to match module names

---

## Estimated Impact

| Phase | Files | Lines Changed | Time | Risk |
|-------|-------|---------------|------|------|
| 4.1 Remove Re-exports | 6 | ~50 imports | 1 hour | Low |
| 4.2 Rename Consolidations | 20 | ~5,000 imports/docs | 4-6 hours | Medium |
| 4.3 Rename Duplicates | 10 | ~500 imports/docs | 2-3 hours | Medium |
| 4.4 Rename Tests | 14 | ~100 imports/docs | 2-3 hours | Low |
| **Total** | **50** | **~5,650 changes** | **9-13 hours** | **Medium** |

**Net Impact**:
- Lines removed: ~12 (re-export files)
- Lines changed: ~5,650 (imports, docs, module declarations)
- Net reduction: ~12 lines (minimal, but cleaner architecture)

**Key Benefit**: Semantic, self-documenting module names instead of generic "unified" prefix.

---

## Success Criteria

- [ ] All "unified" modules renamed to semantic names
- [ ] All imports updated to new module names
- [ ] All documentation updated
- [ ] All tests passing after renaming
- [ ] No breaking changes to public APIs
- [ ] Clear module names that indicate purpose

---

## Next Steps

1. **Start with Phase 4.1** (Remove re-exports) - lowest risk, quick win
2. **Proceed to Phase 4.2** (Rename consolidations) - highest impact
3. **Complete Phase 4.3** (Rename duplicates) - medium impact
4. **Finish with Phase 4.4** (Rename tests) - polish

**Order**: 4.1 → 4.2 → 4.3 → 4.4 (easiest to hardest, least to most risky)
