# ProximaDB Filesystem Consolidation Migration Specification

## Overview
This specification guides the migration from the current fragmented filesystem architecture (IntelligentFilesystem, ZeroCopyFilesystem, ZeroCopyIOSystem) to a unified, caching-aware filesystem that seamlessly integrates with cloud storage providers.

## Migration Goals
1. **Consolidate** IntelligentFilesystem and ZeroCopyFilesystem into UnifiedCachingFilesystem
2. **Integrate** ZeroCopyIOSystem as an internal component
3. **Unify** all metadata caches into a single shared cache
4. **Standardize** configuration with workload presets
5. **Maintain** backward compatibility during migration
6. **Ensure** zero downtime and data integrity

## Architecture Target State

```
┌─────────────────────────────────────────┐
│        UnifiedCachingFilesystem         │
│  (Single entry point for all caching)   │
├─────────────────────────────────────────┤
│ Components:                             │
│ • Metadata Cache (shared)               │
│ • Disk Cache Manager                    │
│ • Range Optimizer (from ZeroCopyIO)     │
│ • Access Pattern Tracker                │
│ • Prefetch Engine                       │
├─────────────────────────────────────────┤
│ Integrations:                           │
│ • CrossCacheOrchestrator                │
│ • FilesystemFactory                     │
│ • Cloud Storage Backends                │
└─────────────────────────────────────────┘
```

## Migration Phases and Tasks

### Phase 1: Analysis & Preparation (Week 1)

| Task ID | Task Description | File/Component | Status | Owner | Notes |
|---------|-----------------|----------------|--------|-------|-------|
| P1.1 | Document all current filesystem usages | All engines | 🟢 Complete | Claude | Created filesystem_usage_audit.md |
| P1.2 | Map metadata cache dependencies | cache/orchestrator.rs | 🟢 Complete | Claude | Created metadata_cache_dependencies.md |
| P1.3 | Create compatibility test suite | tests/filesystem_compat.rs | 🟢 Complete | Claude | Created comprehensive test suite |
| P1.4 | Document current configuration schema | All configs | 🟢 Complete | Claude | Created configuration_schema_analysis.md |
| P1.5 | Identify performance critical paths | Engine reads/writes | 🟢 Complete | Claude | Created performance_critical_paths.md |

### Phase 2: Core Implementation (Week 2-3)

| Task ID | Task Description | File/Component | Status | Owner | Notes |
|---------|-----------------|----------------|--------|-------|-------|
| P2.1 | Create UnifiedCachingFilesystem structure | persistence/filesystem/unified.rs | 🔴 Not Started | | New core implementation |
| P2.2 | Implement unified metadata cache | persistence/filesystem/unified_cache.rs | 🔴 Not Started | | Single cache instance |
| P2.3 | Integrate ZeroCopyIOSystem components | unified.rs | 🔴 Not Started | | Embed as internal component |
| P2.4 | Create unified configuration | persistence/filesystem/unified_config.rs | 🔴 Not Started | | Single config structure |
| P2.5 | Implement FileSystem trait | unified.rs | 🔴 Not Started | | Core filesystem operations |
| P2.6 | Add workload presets | unified_config.rs | 🔴 Not Started | | HighPerformance, Balanced, CostOptimized |
| P2.7 | Create builder pattern | unified.rs | 🔴 Not Started | | Fluent API for configuration |

### Phase 3: Component Migration (Week 3-4)

| Task ID | Task Description | File/Component | Status | Owner | Notes |
|---------|-----------------|----------------|--------|-------|-------|
| P3.1 | Migrate metadata caching logic | From intelligent_filesystem.rs | 🔴 Not Started | | Port cache operations |
| P3.2 | Migrate zero-copy range optimization | From zero_copy_filesystem.rs | 🔴 Not Started | | Port range download logic |
| P3.3 | Migrate access pattern tracking | From both systems | 🔴 Not Started | | Unify tracking logic |
| P3.4 | Migrate prefetch engine | From intelligent_filesystem.rs | 🔴 Not Started | | Port prefetching |
| P3.5 | Migrate disk cache management | From intelligent_filesystem.rs | 🔴 Not Started | | Port local caching |
| P3.6 | Integrate with CrossCacheOrchestrator | cache/orchestrator.rs | 🔴 Not Started | | Connect to cache system |

### Phase 4: Factory Integration (Week 4)

| Task ID | Task Description | File/Component | Status | Owner | Notes |
|---------|-----------------|----------------|--------|-------|-------|
| P4.1 | Update FilesystemFactory | persistence/filesystem/mod.rs | 🔴 Not Started | | Add unified filesystem creation |
| P4.2 | Create get_cached_filesystem method | persistence/filesystem/mod.rs | 🔴 Not Started | | New factory method |
| P4.3 | Add backward compatibility shims | persistence/filesystem/compat.rs | 🔴 Not Started | | Support old APIs temporarily |
| P4.4 | Update factory tests | tests/filesystem_factory_test.rs | 🔴 Not Started | | Test new factory methods |

### Phase 5: Engine Migration - SST (Week 5)

| Task ID | Task Description | File/Component | Status | Owner | Notes |
|---------|-----------------|----------------|--------|-------|-------|
| P5.1 | Migrate SST engine filesystem usage | engines/impls/sst/mod.rs | 🔴 Not Started | | First engine migration |
| P5.2 | Update SST reader | engines/impls/sst/indexed_reader.rs | 🔴 Not Started | | Use unified filesystem |
| P5.3 | Update SST writer | engines/impls/sst/writer.rs | 🔴 Not Started | | Use unified filesystem |
| P5.4 | Update SST compaction | engines/impls/sst/compaction.rs | 🔴 Not Started | | Use unified filesystem |
| P5.5 | Run SST integration tests | tests/sst_integration_test.rs | 🔴 Not Started | | Verify migration |
| P5.6 | Performance benchmark SST | benches/sst_bench.rs | 🔴 Not Started | | Compare before/after |

### Phase 6: Engine Migration - VIPER (Week 5-6)

| Task ID | Task Description | File/Component | Status | Owner | Notes |
|---------|-----------------|----------------|--------|-------|-------|
| P6.1 | Migrate VIPER engine filesystem usage | engines/impls/viper/engine.rs | 🔴 Not Started | | Parquet-specific optimizations |
| P6.2 | Update VIPER reader | engines/impls/viper/reader.rs | 🔴 Not Started | | Use unified filesystem |
| P6.3 | Update VIPER writer | engines/impls/viper/writer.rs | 🔴 Not Started | | Use unified filesystem |
| P6.4 | Run VIPER integration tests | tests/viper_integration_test.rs | 🔴 Not Started | | Verify migration |
| P6.5 | Performance benchmark VIPER | benches/viper_bench.rs | 🔴 Not Started | | Compare before/after |

### Phase 7: Engine Migration - Others (Week 6-7)

| Task ID | Task Description | File/Component | Status | Owner | Notes |
|---------|-----------------|----------------|--------|-------|-------|
| P7.1 | Migrate NOVA engine | engines/impls/nova/engine.rs | 🔴 Not Started | | Progressive storage |
| P7.2 | Migrate SWIFT engine | engines/impls/swift/engine.rs | 🔴 Not Started | | FastLanes encoding |
| P7.3 | Migrate RAPTOR engine | engines/impls/raptor/engine.rs | 🔴 Not Started | | Most complex - uses both layers |
| P7.4 | Migrate PRISM engine | engines/impls/prism/engine.rs | 🔴 Not Started | | Memory-optimized |
| P7.5 | Migrate HELIX engine | engines/impls/helix/engine.rs | 🔴 Not Started | | Time-series patterns |
| P7.6 | Run all engine tests | tests/integration/ | 🔴 Not Started | | Full test suite |

### Phase 8: Cleanup & Optimization (Week 7-8)

| Task ID | Task Description | File/Component | Status | Owner | Notes |
|---------|-----------------|----------------|--------|-------|-------|
| P8.1 | Remove IntelligentFilesystem | intelligent_filesystem.rs | 🔴 Not Started | | After all migrations |
| P8.2 | Remove ZeroCopyFilesystem | zero_copy_filesystem.rs | 🔴 Not Started | | After all migrations |
| P8.3 | Clean up ZeroCopyIOSystem exports | io/zero_copy/mod.rs | 🔴 Not Started | | Make internal only |
| P8.4 | Remove compatibility shims | compat.rs | 🔴 Not Started | | Clean up temp code |
| P8.5 | Update all documentation | docs/ | 🔴 Not Started | | New architecture docs |
| P8.6 | Final performance testing | benches/ | 🔴 Not Started | | Complete benchmark suite |
| P8.7 | Update CLAUDE.md | CLAUDE.md | 🔴 Not Started | | Development guidelines |

## Implementation Details

### 1. UnifiedCachingFilesystem Structure

```rust
// File: src/storage/persistence/filesystem/unified.rs

pub struct UnifiedCachingFilesystem {
    // Core filesystem
    underlying_fs: Arc<dyn FileSystem>,

    // Unified cache components
    metadata_cache: Arc<UnifiedMetadataCache>,
    disk_cache: Arc<DiskCacheManager>,

    // I/O optimization (from ZeroCopyIOSystem)
    range_optimizer: Arc<RangeOptimizer>,
    bandwidth_optimizer: Arc<BandwidthOptimizer>,

    // Pattern tracking & learning
    access_tracker: Arc<AccessPatternTracker>,
    prefetch_engine: Arc<PrefetchEngine>,

    // Configuration
    config: UnifiedCacheConfig,

    // Metrics
    metrics: Arc<CacheMetrics>,
}

impl UnifiedCachingFilesystem {
    pub fn builder() -> UnifiedFilesystemBuilder {
        UnifiedFilesystemBuilder::new()
    }

    pub fn for_workload(
        underlying_fs: Arc<dyn FileSystem>,
        workload: WorkloadType,
        collection_id: String,
        engine_type: String,
    ) -> Self {
        // Preset configurations
    }
}
```

### 2. Unified Configuration

```rust
// File: src/storage/persistence/filesystem/unified_config.rs

pub struct UnifiedCacheConfig {
    // Memory settings
    pub memory_cache_mb: usize,
    pub metadata_cache_mb: usize,

    // Disk cache settings
    pub disk_cache_enabled: bool,
    pub disk_cache_path: PathBuf,
    pub disk_cache_gb: usize,

    // I/O optimization
    pub enable_range_optimization: bool,
    pub enable_prefetching: bool,
    pub enable_pattern_learning: bool,

    // Performance tuning
    pub cache_ttl_secs: u64,
    pub max_concurrent_downloads: usize,
    pub range_merge_threshold: usize,

    // Workload preset
    pub workload_type: WorkloadType,
}

pub enum WorkloadType {
    HighPerformance,  // Max caching, aggressive prefetch
    Balanced,         // Default settings
    CostOptimized,    // Minimal bandwidth, selective caching
    WriteHeavy,       // Optimized for writes
    ReadHeavy,        // Optimized for reads
}
```

### 3. Factory Integration

```rust
// File: src/storage/persistence/filesystem/mod.rs

impl FilesystemFactory {
    /// Create filesystem with unified caching
    pub fn get_cached_filesystem(
        &self,
        url: &str,
        cache_strategy: CacheStrategy,
        collection_id: String,
        engine_type: String,
    ) -> Result<Arc<dyn FileSystem>> {
        // Create base filesystem
        let base_fs = self.get_filesystem(url)?;

        // Wrap with unified caching
        let cached_fs = UnifiedCachingFilesystem::builder()
            .with_base_filesystem(base_fs)
            .with_cache_strategy(cache_strategy)
            .with_collection(collection_id)
            .with_engine(engine_type)
            .build()?;

        Ok(Arc::new(cached_fs))
    }
}
```

### 4. Migration Pattern for Engines

```rust
// Before (current code)
let fs = filesystem_factory.get_filesystem(path)?;
let intelligent_fs = IntelligentFilesystem::new(fs, collection_id, engine_type);
let io_system = ZeroCopyIOSystem::new(...);
let zero_copy_fs = ZeroCopyFilesystem::new(intelligent_fs, io_system);

// After (migrated code)
let fs = filesystem_factory.get_cached_filesystem(
    path,
    CacheStrategy::ForWorkload(WorkloadType::HighPerformance),
    collection_id,
    engine_type,
)?;
```

## Testing Strategy

### Unit Tests
- Each component of UnifiedCachingFilesystem
- Configuration validation
- Cache operations
- Range optimization logic

### Integration Tests
- End-to-end filesystem operations
- Cache coherency across operations
- Cloud storage integration
- Performance under load

### Migration Tests
- Backward compatibility during migration
- Data integrity verification
- Performance regression tests
- Memory usage comparison

### Test Files to Create/Update

| Test File | Purpose | Priority |
|-----------|---------|----------|
| tests/unified_filesystem_test.rs | Core functionality | High |
| tests/unified_cache_test.rs | Cache operations | High |
| tests/migration_compat_test.rs | Backward compatibility | High |
| tests/unified_performance_test.rs | Performance validation | Medium |
| tests/unified_cloud_integration_test.rs | Cloud storage tests | Medium |
| benches/unified_filesystem_bench.rs | Performance benchmarks | Low |

## Success Metrics

### Performance Targets
- **Cache hit rate**: > 80% for repeated access patterns
- **Latency reduction**: 30% for cached metadata operations
- **Memory usage**: 50% reduction in metadata cache memory
- **I/O reduction**: 40% fewer cloud storage API calls

### Code Quality Metrics
- **Test coverage**: > 80% for new code
- **Documentation**: 100% of public APIs documented
- **Compilation**: Zero warnings with `cargo clippy`
- **Benchmarks**: All benchmarks show no regression

## Risk Mitigation

### Identified Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Performance regression | High | Comprehensive benchmarking before/after |
| Data corruption during migration | Critical | Read-only migration with verification |
| Backward compatibility breaks | High | Compatibility shims and gradual migration |
| Cache coherency issues | Medium | Single cache instance with proper locking |
| Memory leaks | Medium | Stress testing and memory profiling |

## Rollback Plan

### Phase-wise Rollback Strategy

1. **Feature flags**: Use feature flags to toggle between old and new implementation
2. **Compatibility layer**: Maintain compatibility shims for 2 releases
3. **Gradual rollout**: Migrate one engine at a time with ability to rollback
4. **Data preservation**: All migrations are non-destructive to existing data

## Commands for Progress Tracking

```bash
# Check migration status
grep -r "🔴 Not Started\|🟡 In Progress\|🟢 Complete" FILESYSTEM_MIGRATION_SPEC.md | wc -l

# Run migration tests
cargo test --package proximadb --test unified_filesystem_test

# Check for old filesystem usage
grep -r "IntelligentFilesystem\|ZeroCopyFilesystem" src/

# Run benchmarks
cargo bench --bench unified_filesystem_bench

# Check test coverage
cargo tarpaulin --out Html --output-dir target/coverage
```

## Session Handoff Instructions

### For Next Claude Code Session

1. **Check current status**: Review task tables above for 🔴/🟡/🟢 status
2. **Pick next task**: Start with earliest incomplete task in current phase
3. **Update status**: Change 🔴 → 🟡 when starting, 🟡 → 🟢 when complete
4. **Run tests**: After each task, run relevant tests
5. **Update this doc**: Mark tasks complete and add notes

### Status Legend
- 🔴 Not Started
- 🟡 In Progress
- 🟢 Complete
- ⚠️ Blocked
- ❌ Cancelled

## Appendix: File Mappings

### Files to Create
- src/storage/persistence/filesystem/unified.rs
- src/storage/persistence/filesystem/unified_config.rs
- src/storage/persistence/filesystem/unified_cache.rs
- tests/unified_filesystem_test.rs
- tests/migration_compat_test.rs

### Files to Modify
- src/storage/persistence/filesystem/mod.rs (add factory method)
- src/storage/engines/impls/*/engine.rs (all engines)
- src/storage/cache/orchestrator.rs (integration)

### Files to Delete (Phase 8)
- src/storage/persistence/filesystem/intelligent_filesystem.rs
- src/storage/persistence/filesystem/zero_copy_filesystem.rs

---

**Last Updated**: 2024-01-17
**Next Review**: Start of Phase 2
**Migration Lead**: TBD