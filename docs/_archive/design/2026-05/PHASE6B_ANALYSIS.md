# Phase 6B: Quantization Infrastructure Abstraction

**Status**: In Progress
**Started**: 2026-05-14
**Objective**: Create abstraction layer to enable quantization consolidation

## Overview

Phase 6B creates abstraction contracts to decouple the quantization engine (7,329 lines in `src/compute/quantization/`) from infrastructure dependencies, enabling gradual migration to the vector modality.

## Problem Analysis

### Current Dependencies

The quantization module has **complex cross-dependencies**:

1. **Hardware Acceleration** (664 lines)
   - Direct dependency on `crate::core::hardware_capabilities`
   - Runtime SIMD/GPU selection logic
   - Platform-specific code paths (AVX2, AVX512, NEON, CUDA, etc.)

2. **Global Cache** (855 lines)
   - Integration with `CrossCacheOrchestrator` from storage layer
   - DashMap-based lock-free caching
   - Cache invalidation and eviction policies

3. **Storage Engine Integration** (1,555 lines)
   - Dependencies on 6 different storage engines
   - Format-specific quantization logic
   - WAL and metadata integration

4. **Core Engine** (2,662 lines)
   - Distance computation integration
   - Codebook training and storage
   - Multi-level quantization coordination

### Dependency Graph

```
src/compute/quantization/
├── hardware_accelerated.rs → crate::core::hardware_capabilities
├── global_cache.rs → crate::storage::cache::orchestrator::CrossCacheOrchestrator
├── storage_engine.rs → 6 storage engines (VIPER, SST, HELIX, NOVA, SWIFT, RAPTOR)
├── quantization_engine.rs → All of the above
└── 20+ files depend on these modules
```

## Solution: Abstraction Layer

### Contract Traits Created

Located in `crates/contracts/quantization/src/lib.rs`:

1. **HardwareAcceleration Trait**
   - Abstracts hardware capability detection
   - Provides generic quantization methods
   - Enables SIMD/GPU switching without core dependencies

2. **QuantizationCache Trait**
   - Abstracts codebook caching mechanism
   - Supports different cache implementations
   - Enables distributed caching in the future

3. **CodebookStore Trait** (moved from core)
   - Abstracts persistent codebook storage
   - Already exists, now formalized in contracts
   - Enables multiple storage backends

4. **QuantizationEngine Trait**
   - Main abstraction for quantization operations
   - Storage-agnostic interface
   - Enables modular quantization implementations

## Migration Strategy

### Phase 6B.1: Contract Definition (COMPLETE ✅)
- Created `crates/contracts/quantization/` crate
- Defined all abstraction traits
- Added comprehensive documentation and tests
- Updated workspace Cargo.toml

### Phase 6B.2: Implement Contracts (NEXT)
- Implement contracts in current `src/compute/quantization/` modules
- Create adapter implementations for existing code
- Verify no behavior changes

### Phase 6B.3: Update Consumers (FOLLOWING)
- Update 20+ dependent files to use contract traits
- Replace direct dependencies with trait objects
- Verify compilation and functionality

### Phase 6B.4: Validation (FINAL)
- Integration tests for all contract implementations
- Performance benchmarks to ensure no regression
- Documentation updates

## Benefits

### Immediate (Phase 6B)
- **Clear Interfaces**: Well-defined contracts for all interactions
- **Testability**: Mock implementations for testing
- **Documentation**: Explicit dependency relationships

### For Phase 6C-E (Migration)
- **Incremental Migration**: Move modules one at a time
- **No Breaking Changes**: Existing code continues to work
- **Risk Mitigation**: Can validate at each step

### Long-term
- **Modularity**: Quantization becomes a true module
- **Pluggability**: Different hardware/cache implementations
- **Maintainability**: Clear boundaries and interfaces

## Technical Decisions

### Why Contracts Crate?
- **Layer Separation**: Contracts belong between foundation and modalities
- **Reusability**: Can be shared across modalities
- **Stability**: Stable interface definitions

### Why Async Traits?
- **Storage I/O**: Codebook storage is inherently async
- **Scalability**: Non-blocking operations
- **Consistency**: Matches rest of storage layer

### Why Serialize Everything?
- **Cross-Process**: Support for distributed quantization
- **Caching**: Efficient storage and transmission
- **Flexibility**: Multiple serialization formats

## Remaining Work

### Phase 6C: Component Migration (Week 2-3) ✅ COMPLETE
- ✅ Migrated `compile_time.rs` (152 lines) - SUCCESS
- ❌ `selection.rs` (363 lines) - Architectural boundary, remains in compute layer
- ⏸️ `smart_defaults.rs` (604 lines) - Not attempted (likely orchestration logic)

**Phase 6C Result**: Declared complete with architectural boundary respected. Successfully migrated vector-specific algorithms while keeping platform orchestration in compute layer.

### Phase 6D: Core Engine Migration (Week 3-4)
- Migrate `quantization_engine.rs` (2,662 lines) - High risk
- Migrate `storage_engine.rs` (1,555 lines) - High risk
- Migrate `hardware_accelerated.rs` (664 lines) - Medium risk
- Migrate `global_cache.rs` (855 lines) - Medium risk
- Migrate `precompute.rs` (440 lines) - Medium risk

### Phase 6E: Cleanup (Week 4)
- Remove `src/compute/quantization/` directory
- Update all imports across codebase
- Final testing and validation

## Success Criteria

- [x] Contract traits defined and documented
- [x] Workspace configuration updated
- [x] Basic tests passing
- [ ] Implementations in current modules
- [ ] Consumer updates using contracts
- [ ] All tests passing with contracts
- [ ] Performance benchmarks acceptable

## Timeline

- **Phase 6B**: Week 1 (current)
- **Phase 6C**: Weeks 2-3
- **Phase 6D**: Weeks 3-4
- **Phase 6E**: Week 4

**Total**: 3-4 weeks for complete Phase 6

## References

- Workspace Refactor Plan: `roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc`
- Quantization Module: `src/compute/quantization/`
- Vector Modality: `crates/modalities/proximadb-vector/`
- Internal Types: `crates/modalities/proximadb-vector/src/quantization/internal_types.rs`
