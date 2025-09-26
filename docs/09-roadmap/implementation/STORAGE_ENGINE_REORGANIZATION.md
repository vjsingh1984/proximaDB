# ProximaDB Storage Engine Reorganization Plan

## Executive Summary

This document outlines the comprehensive reorganization plan for ProximaDB's 6 storage engines (SST, VIPER, NOVA, SWIFT, RAPTOR, HELIX) based on architectural analysis. The goal is to improve consistency, maintainability, code reuse, and developer experience while preserving each engine's unique optimizations.

## Current State Analysis

### Critical Issues Identified

1. **Inconsistent Engine Entry Points**
   - SST and HELIX lack dedicated `engine.rs` files (implementations in 56K+ token mod.rs)
   - Makes it difficult to understand main engine interfaces

2. **Missing Standardized Components**
   - SST and RAPTOR lack `unified_strategy_reader.rs`
   - Inconsistent module patterns across engines

3. **Test Coverage Gaps**
   - SWIFT: 0 test files despite 11 modules
   - NOVA: Only 1 test file for 20 modules
   - No standardized test organization

4. **Code Duplication**
   - `progressive_search.rs` duplicated in 3 engines
   - `batch_operations.rs` duplicated in NOVA/SWIFT
   - `zone_maps.rs` in both NOVA and HELIX
   - No shared infrastructure modules

## Implementation Plan

### Phase 1: Critical Standardization (Week 1)

#### Day 1-2: Engine Entry Point Refactoring

```bash
# SST Engine Refactoring
src/storage/engines/impls/sst/
├── mod.rs                    # Reduce from 56K to <1K tokens
├── engine.rs                 # NEW: Extract engine implementation
├── unified_strategy_reader.rs # NEW: Implement missing reader
└── [existing modules]

# HELIX Engine Refactoring
src/storage/engines/impls/helix/
├── mod.rs                    # Reduce to module declarations only
├── engine.rs                 # NEW: Extract engine implementation
└── [existing modules]
```

**Implementation Tasks:**
1. Extract `SstEngine` struct and impl from mod.rs to engine.rs
2. Extract `HelixEngine` struct and impl from mod.rs to engine.rs
3. Update mod.rs files to only contain module declarations
4. Ensure all public APIs remain unchanged

#### Day 3-4: Missing Components Implementation

```rust
// src/storage/engines/impls/sst/unified_strategy_reader.rs
pub struct SstUnifiedStrategyReader {
    // Implement standardized reading strategy
}

// src/storage/engines/impls/raptor/unified_strategy_reader.rs
pub struct RaptorUnifiedStrategyReader {
    // Implement standardized reading strategy
}
```

#### Day 5: Test Infrastructure Setup

```bash
# Standardized test structure for all engines
engine_name/tests/
├── mod.rs
├── unit/
│   ├── engine_tests.rs
│   ├── reader_tests.rs
│   └── writer_tests.rs
├── integration/
│   ├── end_to_end_tests.rs
│   └── cross_engine_tests.rs
└── performance/
    ├── benchmark_tests.rs
    └── stress_tests.rs
```

### Phase 2: Shared Infrastructure (Week 2)

#### Day 6-8: Extract Common Operations

```rust
// src/storage/engines/shared/mod.rs
pub mod progressive_search;
pub mod batch_operations;
pub mod optimized_operations;
pub mod zone_maps;
pub mod clustering_operations;
pub mod test_utils;

// src/storage/engines/shared/progressive_search.rs
pub trait ProgressiveSearch {
    async fn progressive_search(&self, params: SearchParams) -> Result<SearchResults>;
}

// Implement for all engines
impl ProgressiveSearch for SstEngine { ... }
impl ProgressiveSearch for ViperEngine { ... }
// ... etc
```

#### Day 9-10: Shared Test Utilities

```rust
// src/storage/engines/shared/test_utils/mod.rs
pub mod common_engine_tests;
pub mod performance_harness;
pub mod data_generators;

// src/storage/engines/shared/test_utils/common_engine_tests.rs
pub fn test_engine_basic_operations<E: UnifiedStorageEngine>() {
    // Standard tests all engines must pass
}

pub fn test_engine_compaction<E: UnifiedStorageEngine>() {
    // Standard compaction tests
}
```

### Phase 3: Code Consolidation (Week 3)

#### Day 11-12: Reader Infrastructure Consolidation

```rust
// src/storage/engines/shared/readers/mod.rs
pub mod block_filter;
pub mod metadata_cache;
pub mod prefetch_strategy;

// Extract common reader patterns from SST and VIPER
pub trait UnifiedBlockReader {
    async fn read_block(&self, block_id: BlockId) -> Result<Block>;
    async fn filter_blocks(&self, filter: &BlockFilter) -> Result<Vec<BlockId>>;
}
```

#### Day 13-14: Compaction Logic Consolidation

```rust
// src/storage/engines/shared/compaction/mod.rs
pub mod scheduling;
pub mod merge_strategy;
pub mod metadata_update;

pub trait CompactionStrategy {
    async fn should_compact(&self, metrics: &EngineMetrics) -> bool;
    async fn select_files(&self, candidates: Vec<FileInfo>) -> Vec<FileInfo>;
    async fn merge_files(&self, files: Vec<FileInfo>) -> Result<FileInfo>;
}
```

#### Day 15: EventLog Integration Standardization

```rust
// src/storage/engines/shared/eventlog/mod.rs
pub mod flush_integration;
pub mod compaction_tracking;
pub mod recovery_log;

pub trait EventLogIntegration {
    async fn log_flush(&self, event: FlushEvent) -> Result<()>;
    async fn log_compaction(&self, event: CompactionEvent) -> Result<()>;
}
```

### Phase 4: Testing & Documentation (Week 4)

#### Day 16-17: Comprehensive Test Coverage

```yaml
# Test Coverage Goals
SST:    80% → 95%
VIPER:  75% → 95%
NOVA:   40% → 95%
SWIFT:  0%  → 95%
RAPTOR: 60% → 95%
HELIX:  70% → 95%
```

**Test Implementation Priority:**
1. SWIFT: Create complete test suite (highest priority - currently 0%)
2. NOVA: Expand from 1 to full test suite
3. Other engines: Fill coverage gaps

#### Day 18-19: Cross-Engine Testing

```rust
// tests/engines/cross_engine_tests.rs
#[tokio::test]
async fn test_all_engines_consistency() {
    for engine_type in [SST, VIPER, NOVA, SWIFT, RAPTOR, HELIX] {
        let engine = create_engine(engine_type);

        // Test identical operations produce consistent results
        test_insert_consistency(&engine).await;
        test_search_consistency(&engine).await;
        test_compaction_consistency(&engine).await;
    }
}
```

#### Day 20: Documentation & Templates

```markdown
# docs/development/storage-engine-template.md
## Creating a New Storage Engine

### Required Files
- engine.rs
- unified_metadata_serializer.rs
- unified_strategy_reader.rs
- tests/mod.rs

### Required Trait Implementations
- UnifiedStorageEngine
- ProgressiveSearch
- CompactionStrategy
- EventLogIntegration
```

## Success Metrics

### Quantitative Metrics
- **Code Duplication**: Reduce by 30% through shared modules
- **Test Coverage**: All engines at 95%+ coverage
- **Module Size**: No single module > 5K tokens
- **Build Time**: Reduce by 20% through better modularization

### Qualitative Metrics
- **Consistency Score**: All engines follow identical organizational patterns
- **Developer Onboarding**: New developers can understand any engine in < 1 hour
- **Maintenance Effort**: 50% reduction in cross-engine bug fixes
- **Code Review Time**: 30% faster reviews due to standardized patterns

## Migration Strategy

### Step 1: Non-Breaking Refactoring
- All changes maintain backward compatibility
- Public APIs remain unchanged
- Gradual migration using feature flags

### Step 2: Deprecation Notices
```rust
#[deprecated(since = "0.2.0", note = "Use engine.rs instead")]
pub use mod::SstEngine;
```

### Step 3: Version Migration
- v0.1.x: Current structure (maintain support)
- v0.2.x: New structure with deprecation warnings
- v0.3.x: Remove deprecated code paths

## Risk Mitigation

### Identified Risks
1. **Breaking Changes**: Mitigated by maintaining public API compatibility
2. **Performance Regression**: Mitigated by comprehensive benchmarking
3. **Test Coverage Gaps**: Mitigated by phased test implementation
4. **Integration Issues**: Mitigated by cross-engine testing

### Rollback Plan
- Git tags at each phase completion
- Feature flags for new code paths
- Parallel testing of old vs new implementations
- 1-week stabilization period after each phase

## Implementation Checklist

### Week 1 Deliverables
- [ ] Extract SST engine from mod.rs
- [ ] Extract HELIX engine from mod.rs
- [ ] Implement SST unified_strategy_reader
- [ ] Implement RAPTOR unified_strategy_reader
- [ ] Standardize test directory structure

### Week 2 Deliverables
- [ ] Create shared operations module
- [ ] Extract progressive_search to shared
- [ ] Extract batch_operations to shared
- [ ] Create shared test utilities
- [ ] Implement common_engine_tests

### Week 3 Deliverables
- [ ] Consolidate reader infrastructure
- [ ] Consolidate compaction logic
- [ ] Standardize EventLog integration
- [ ] Remove duplicate code
- [ ] Update all engine imports

### Week 4 Deliverables
- [ ] SWIFT test suite (0% → 95%)
- [ ] NOVA test suite (5% → 95%)
- [ ] Cross-engine consistency tests
- [ ] Documentation templates
- [ ] Migration guide

## Long-Term Vision

### Future Enhancements (Q2 2025)
1. **Plugin Architecture**: Allow external storage engines
2. **Engine Hot-Swapping**: Change engines without restart
3. **Adaptive Engine Selection**: Auto-select based on workload
4. **Engine Composition**: Combine multiple engines for hybrid workloads

### Architecture Evolution
```
Current: 6 Independent Engines
    ↓
Phase 1: Standardized Engines with Shared Core
    ↓
Phase 2: Plugin-Based Architecture
    ↓
Future: Adaptive Multi-Engine System
```

## Conclusion

This reorganization will transform ProximaDB's storage layer from a collection of independent engines into a cohesive, maintainable system while preserving the unique optimizations that make each engine valuable. The phased approach ensures minimal disruption while delivering immediate benefits in consistency, testability, and developer experience.

## Appendix: File Count Analysis

### Current State
| Engine | Files | Tests | Coverage |
|--------|-------|-------|----------|
| SST    | 40    | 19    | ~80%     |
| VIPER  | 33    | 14    | ~75%     |
| NOVA   | 20    | 1     | ~40%     |
| SWIFT  | 11    | 0     | 0%       |
| RAPTOR | 18    | 4     | ~60%     |
| HELIX  | 19    | 2     | ~70%     |

### Target State
| Engine | Files | Tests | Coverage |
|--------|-------|-------|----------|
| SST    | 25    | 25    | 95%      |
| VIPER  | 25    | 25    | 95%      |
| NOVA   | 25    | 25    | 95%      |
| SWIFT  | 25    | 25    | 95%      |
| RAPTOR | 25    | 25    | 95%      |
| HELIX  | 25    | 25    | 95%      |
| Shared | 15    | 10    | 100%     |

Total File Reduction: 141 → 165 (but 30% less duplication)