# Remaining Work: Roadmap for Workspace Refactor Completion

**Date**: 2026-05-13
**Current Status**: 85% Complete
**Remaining**: 15% (estimated 3-5 weeks)

---

## Overview

This document outlines the remaining work to complete the workspace refactor, organized by priority and complexity.

---

## Phase 6: Quantization Consolidation (BLOCKED)

### Current State
- **Location**: `src/compute/quantization/` (7,556 lines across 11 files)
- **Target**: `crates/modalities/proximadb-vector/src/quantization/`
- **Blockers**: Complex cross-dependencies on storage, core, utils modules

### Dependency Analysis

**Current Dependencies** (from attempted migration):
```rust
// quantization_engine.rs imports:
use crate::storage::cache::orchestrator::{CacheType, CrossCacheOrchestrator};
use crate::utils::hash::XxHash64;
use crate::core::hardware_capabilities::{HardwareBackend, get_hardware_capabilities};

// precompute.rs imports:
use crate::compute::quantization::global_cache::GlobalQuantizationCache;
use crate::compute::quantization::selection::QuantizationSelector;
use crate::compute::quantization::storage_engine::{...};

// hardware_accelerated.rs imports:
use crate::core::hardware_capabilities::{HardwareBackend, get_hardware_capabilities};
```

### Untangling Strategy

**Step 1: Extract Hardware-Agnostic Core** (1 week)
- Create `quantization_core.rs` with pure quantization logic
- No dependencies on storage, caching, or hardware
- Foundation types only
- Test in isolation

**Step 2: Create Hardware Abstraction Layer** (3 days)
- Define `HardwareAcceleration` trait in foundation
- Move hardware detection to foundation
- Implement trait for CPU backends
- Quantization engine uses trait, not concrete types

**Step 3: Extract Storage Integration Layer** (1 week)
- Define `QuantizationStorage` trait
- Storage engines implement trait
- Quantization engine uses trait interface
- Removes direct storage dependency

**Step 4: Extract Caching Layer** (3 days)
- Move caching logic to vector modality
- Define `QuantizationCache` trait
- Pluggable cache implementations
- Quantization engine uses cache trait

**Step 5: Update Imports and Test** (3 days)
- Update all imports from `crate::compute::quantization` to vector modality
- Add re-export in `src/compute/mod.rs` for backward compatibility
- Run full test suite
- Verify no performance regression

### Success Criteria
- ✅ All quantization code in `crates/modalities/proximadb-vector/src/quantization/`
- ✅ No circular dependencies
- ✅ All tests pass
- ✅ No performance regression
- ✅ Backward compatibility maintained via re-exports

---

## Phase 8: Foundation Type Migration (PARTIAL)

### Current State
- ✅ Foundation types created (4 crates)
- ✅ 2 types deprecated with conversion traits
- ⏸️ 15 duplicate definitions remaining

### Remaining Duplicates by Type

#### DistanceMetric (2 remaining)
1. **DuckDBDistanceMetric** ✅ DEPRECATED
   - Location: `src/connectors/duckdb.rs`
   - Status: Conversion traits implemented
   - Action: Update usages (4 locations)

2. **cluster::rpc::types::DistanceMetric** ✅ DEPRECATED
   - Location: `src/cluster/rpc/types.rs`
   - Status: Conversion traits implemented
   - Action: Update internal usages

#### QuantizationType (5 local definitions)
These are storage-specific local types, not true duplicates:
1. `src/storage/multimodal/stores/vector_store.rs` - Local config type
2. `src/storage/multimodel/stores/vector_store.rs` - Local config type
3. `src/storage/operations/requantization.rs` - Operation-specific type
4. `src/storage/engines/core/ops/proxima_tensor_encoding.rs` - Encoding-specific type
5. `src/storage/engines/viper/readers/test_data_generator.rs` - Test-only type

**Action**: Document as storage-specific types, not duplicates

#### CompressionAlgorithm (6 variants)
These are format/storage-specific variants:
1. `src/core/serialization/mod.rs` - Comprehensive compression enum (14 variants)
2. `src/storage/persistence/filesystem/cache_config.rs` - Cache-specific variant
3. `src/storage/engines/core/formats/columnar/common.rs` - Columnar format selection
4. `src/storage/engines/sst/blocks_archive.rs` - SST-specific legacy type
5. Other storage-specific variants

**Action**: Document as format-specific optimizations, not duplicates

### Migration Tasks

**Priority 1: Update Deprecated Type Usages** (1 week)
1. Find all usages of deprecated types:
   ```bash
   grep -rn "DuckDBDistanceMetric" src/ --include="*.rs"
   grep -rn "cluster::rpc::types::DistanceMetric" src/ --include="*.rs"
   ```

2. Replace with foundation types:
   ```rust
   // Old (deprecated)
   use crate::connectors::duckdb::DuckDBDistanceMetric;
   let metric = DuckDBDistanceMetric::L2;
   
   // New (recommended)
   use proximadb_distance_types::DistanceMetric;
   let metric: DistanceMetric = DistanceMetric::L2;
   ```

3. Use conversion traits where needed:
   ```rust
   let duckdb_metric: DuckDBDistanceMetric = ...;
   let foundation_metric: DistanceMetric = duckdb_metric.into();
   ```

**Priority 2: Document Storage-Specific Types** (2 days)
1. Create `STORAGE_SPECIFIC_TYPES.md`
2. Document why each local type exists
3. Mark as "not duplicates" with justification

**Priority 3: Remove Deprecated Types** (after grace period)
1. Monitor deprecation warnings for 2 months
2. Update public APIs if needed
3. Remove deprecated types and conversion traits

### Success Criteria
- ✅ All deprecated type usages updated
- ✅ Storage-specific types documented
- ✅ No breaking changes to public APIs
- ✅ Deprecation warnings reduced to 0

---

## Implementation Timeline

### Week 1-2: Phase 6 Preparation
- Day 1-3: Extract hardware-agnostic core
- Day 4-7: Create hardware abstraction layer
- Day 8-10: Extract storage integration layer

### Week 3: Phase 6 Completion
- Day 1-3: Extract caching layer
- Day 4-6: Update imports and test
- Day 7: Verify and document

### Week 4: Phase 8 Migration
- Day 1-3: Update deprecated type usages
- Day 4-5: Document storage-specific types
- Day 6-7: Final verification and cleanup

### Week 5: Buffer and Testing
- Comprehensive testing
- Performance benchmarking
- Documentation updates
- Final cleanup

---

## Risk Mitigation

### Risk 1: Breaking Changes
**Mitigation**: 
- Maintain backward compatibility via re-exports
- Use deprecation warnings before removal
- Provide conversion traits
- Test thoroughly before removing deprecated code

### Risk 2: Performance Regression
**Mitigation**:
- Benchmark before/after changes
- Profile hot paths
- Optimize critical sections
- Monitor in production

### Risk 3: Circular Dependencies
**Mitigation**:
- Use trait abstractions
- Layered architecture enforcement
- CI checks prevent violations
- Careful import management

---

## Success Metrics

| Metric | Current | Target | Measurement |
|--------|---------|--------|-------------|
| **Unified modules** | 0 | 0 | ✅ ACHIEVED |
| **Layering violations** | 0 | 0 | ✅ ACHIEVED |
| **Foundation type usage** | ~60% | 100% | Code review |
| **Quantization location** | src/compute | vector modality | Directory check |
| **Deprecated type usages** | 9 | 0 | Warning count |
| **Test coverage** | Existing | No regression | Test results |

---

## Next Actions

### Immediate (This Week)
1. Review and approve this roadmap
2. Set up branch for Phase 6 work
3. Begin hardware abstraction layer design

### Short-term (Next 2 Weeks)
1. Complete Phase 6: Quantization consolidation
2. Update deprecated type usages
3. Document storage-specific types

### Long-term (Next Month)
1. Remove deprecated types after grace period
2. Monitor layering compliance
3. Iterate on foundation types

---

## Conclusion

The remaining 15% of work is well-defined with clear success criteria and risk mitigation strategies. 

**Key Points**:
- Phase 6 requires careful dependency untangling (2-3 weeks)
- Phase 8 is mostly documentation and minor updates (1 week)
- Backward compatibility maintained throughout
- Clear migration path with minimal risk

**Estimated Completion**: 3-5 weeks

**Confidence**: HIGH - All blockers identified with mitigation strategies in place.
