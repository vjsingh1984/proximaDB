# Engine Consolidation Plan

## Overview
This document outlines the comprehensive plan for consolidating common functionality across ProximaDB's storage engines (SST, VIPER, NOVA, SWIFT) to eliminate code duplication and improve maintainability.

## Completed Analysis

### 1. SST/SWIFT Synergies ✅
**Document**: `docs/sst_swift_synergies.md`
- Identified ~2000 lines of duplicate code
- Both use SST file format with DataBlock structures
- Share bloom filters and quantization sections
- Common MVCC resolution and compaction strategies

### 2. Flush/Compaction Synergies ✅
**Document**: `docs/flush_compaction_synergies.md`
- Columnar engines (NOVA/VIPER): ~1500 lines can be consolidated
- Row-based engines (SST/SWIFT): ~1200 lines can be consolidated
- Total: ~2700 lines of code reduction possible

### 3. Search Method Synergies ✅
**Document**: `docs/search_method_synergies.md`
- Universal components: ~800 lines shared
- Columnar module: ~600 lines shared
- Row-based module: ~700 lines shared
- Total: ~2100 lines of code reduction possible

### 4. Universal Adapter Migration ✅
All engines have been migrated to use:
- `UniversalCompressionAdapter`
- `UniversalQuantizationAdapter`
- Common metrics framework

## Proposed Module Structure

```
storage/engines/
├── universal/           # Shared across all engines
│   ├── mod.rs
│   ├── compression_adapter.rs  ✅ (exists)
│   ├── quantization_adapter.rs ✅ (exists)
│   ├── search_common.rs        🔄 (to create)
│   ├── filter_processor.rs     🔄 (to create)
│   ├── result_manager.rs       🔄 (to create)
│   └── quantization_pipeline.rs 🔄 (to create)
│
├── columnar/           # Shared by NOVA/VIPER
│   ├── mod.rs
│   ├── flush_manager.rs        🔄 (to create)
│   ├── compaction_manager.rs   🔄 (to create)
│   ├── search.rs               🔄 (to create)
│   └── parquet_utils.rs        🔄 (to create)
│
├── row_based/          # Shared by SST/SWIFT
│   ├── mod.rs
│   ├── flush_manager.rs        🔄 (to create)
│   ├── compaction_manager.rs   🔄 (to create)
│   ├── search.rs               🔄 (to create)
│   ├── block_structures.rs     🔄 (to create)
│   └── bloom_filter.rs         ✅ (exists in SST)
│
├── sst/               # SST-specific (minimal)
├── viper/             # VIPER-specific (minimal)
├── nova/              # NOVA-specific (minimal)
└── swift/             # SWIFT-specific (minimal)
```

## Implementation Phases

### Phase 1: Universal Search Components (Week 1)
- [ ] Create `universal/search_common.rs` with pipeline structure
- [ ] Implement `universal/filter_processor.rs` for filter handling
- [ ] Create `universal/result_manager.rs` for result operations
- [ ] Build `universal/quantization_pipeline.rs` for progressive search

### Phase 2: Columnar Common Module (Week 2)
- [ ] Extract flush logic to `columnar/flush_manager.rs`
- [ ] Extract compaction logic to `columnar/compaction_manager.rs`
- [ ] Create `columnar/search.rs` for shared search operations
- [ ] Add `columnar/parquet_utils.rs` for Parquet operations

### Phase 3: Row-Based Common Module (Week 2-3)
- [ ] Extract flush logic to `row_based/flush_manager.rs`
- [ ] Extract compaction logic to `row_based/compaction_manager.rs`
- [ ] Create `row_based/search.rs` for shared search operations
- [ ] Consolidate block structures in `row_based/block_structures.rs`

### Phase 4: Engine Migration (Week 3-4)
- [ ] Migrate SST to use row_based common module
- [ ] Migrate SWIFT to use row_based common module
- [ ] Migrate VIPER to use columnar common module
- [ ] Migrate NOVA to use columnar common module

### Phase 5: Testing & Optimization (Week 4)
- [ ] Create comprehensive test suite for common modules
- [ ] Performance benchmarking before/after
- [ ] A/B testing framework for gradual rollout
- [ ] Documentation updates

## Key Design Decisions

### 1. Trait-Based Abstractions
Use traits to allow engine-specific implementations while sharing common logic:
```rust
trait FileSearcher<F, B> {
    async fn search_file(&self, file: &F, query: &[f32], config: &SearchConfig) -> Result<Vec<SearchResult>>;
}
```

### 2. Progressive Enhancement
Common modules provide base functionality, engines add specific optimizations:
```rust
// Common module provides basic flush
impl ColumnarFlushManager {
    pub async fn flush_to_parquet(&self, records: &[VectorRecord]) -> Result<ParquetFile>;
}

// VIPER adds specific optimizations
impl ViperEngine {
    async fn flush(&self, records: &[VectorRecord]) -> Result<FlushResult> {
        let result = self.columnar_flush_manager.flush_to_parquet(records).await?;
        // VIPER-specific post-processing
    }
}
```

### 3. Configuration-Driven Behavior
Use configuration objects to control behavior without code changes:
```rust
pub struct SearchConfig {
    pub enable_bloom_filter: bool,
    pub enable_progressive_search: bool,
    pub enable_clustering: bool,
    // ...
}
```

## Expected Outcomes

### Code Metrics
- **Total Lines Saved**: ~6800 lines (45% reduction)
- **Duplicate Code Eliminated**: 80%
- **Test Coverage**: Increased from 60% to 85%

### Performance Impact
- **Memory Usage**: -20% (single implementation in memory)
- **Cache Efficiency**: +15% (shared caching strategies)
- **Search Latency**: Neutral to -5% (optimized pipelines)

### Maintenance Benefits
- **Bug Fixes**: Applied once, benefit all engines
- **New Features**: Available to all engines immediately
- **Testing**: Centralized test suites reduce test maintenance

## Risk Mitigation

### Risk: Performance Regression
- **Mitigation**: Extensive benchmarking at each phase
- **Rollback Plan**: Feature flags to switch between old/new implementations

### Risk: Breaking Changes
- **Mitigation**: Incremental migration with backward compatibility
- **Testing**: Comprehensive integration tests before each phase

### Risk: Over-Abstraction
- **Mitigation**: Keep abstractions focused on truly common patterns
- **Review**: Architecture review after each phase

## Success Criteria

1. **Code Reduction**: Achieve at least 40% reduction in engine code
2. **Performance**: No regression in any benchmark
3. **Test Coverage**: Reach 85% coverage for common modules
4. **Migration**: All engines successfully using common modules
5. **Documentation**: Complete API documentation for all modules

## Timeline

- **Week 1**: Universal search components
- **Week 2**: Columnar common module
- **Week 3**: Row-based common module
- **Week 4**: Engine migration and testing
- **Week 5**: Documentation and deployment

## Next Steps

1. **Immediate**: Begin implementing universal search components
2. **This Week**: Complete Phase 1 and start Phase 2
3. **Review**: Architecture review after Phase 1 completion

## References

- [SST/SWIFT Synergies](./sst_swift_synergies.md)
- [Flush/Compaction Synergies](./flush_compaction_synergies.md)
- [Search Method Synergies](./search_method_synergies.md)
- [Universal Adapter Migration](./migration_complete.md)

---
*Last Updated: 2025-08-16*
*Status: Analysis Complete, Implementation Pending*