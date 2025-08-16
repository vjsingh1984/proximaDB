# Flush and Compaction Synergies Analysis

## Executive Summary
Analysis of flush and compaction operations across all 4 engines reveals significant opportunities for consolidation. Both columnar engines (NOVA/VIPER) and row-based engines (SST/SWIFT) share common patterns that can be extracted to their respective common modules.

## Columnar Engines: NOVA & VIPER

### Common Flush Patterns

#### Current Implementation
Both NOVA and VIPER perform similar flush operations:

**VIPER Flush (`viper/flush.rs`)**:
```rust
pub struct FlushManager {
    schema_manager: SchemaManager,
    collection_service: Arc<RwLock<Option<Arc<CollectionService>>>>,
    filesystem_factory: Arc<FilesystemFactory>,
    atomic_coordinator: Arc<TransactionCoordinator>,
    compression_adapter: Arc<UniversalCompressionAdapter>,
    metrics_updater: Option<Arc<dyn InternalMetricsUpdater>>,
}
```

**NOVA Flush (inline in `nova/engine.rs`)**:
```rust
async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
    // Create new Parquet file
    // Map compression to Parquet
    // Configure quantization
    // Write columnar data
}
```

#### Common Elements to Extract
1. **Parquet file creation**
2. **Schema generation from vectors**
3. **Compression mapping to Parquet**
4. **Row group organization**
5. **Metadata statistics collection**
6. **Atomic write coordination**
7. **Metrics tracking**

### Common Compaction Patterns

#### Current Implementation
Both engines use level-based compaction with MVCC resolution:

**VIPER Compaction (`viper/compaction.rs`)**:
```rust
pub struct CompactionManager {
    schema_manager: SchemaManager,
    collection_service: Arc<RwLock<Option<Arc<CollectionService>>>>,
    filesystem_factory: Arc<FilesystemFactory>,
    atomic_coordinator: Arc<TransactionCoordinator>,
    // Level-based compaction logic
}
```

**NOVA Compaction (inline)**:
Similar level-based approach but less formalized.

#### Common Elements to Extract
1. **Level-based file selection**
2. **MVCC resolution for duplicates**
3. **Expired record filtering**
4. **File merging strategies**
5. **Atomic replacement of files**
6. **Compaction statistics**

## Row-Based Engines: SST & SWIFT

### Common Flush Patterns

#### Current Implementation
Both SST and SWIFT flush to SST files with similar structures:

**SST Flush (`sst/mod.rs`)**:
```rust
async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
    // Extract records from WAL
    // Build SST blocks
    // Apply compression
    // Write SSTable file
    // Notify EventLog for AXIS
}
```

**SWIFT Flush (`swift/engine.rs`)**:
```rust
async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
    // Create SstFile
    // Build hierarchical blocks
    // Apply quantization
    // Write with universal adapters
}
```

#### Common Elements to Extract
1. **SST file creation**
2. **Block organization (DataBlock)**
3. **Bloom filter generation**
4. **Index building**
5. **Compression application**
6. **Quantization integration**
7. **EventLog notification**

### Common Compaction Patterns

#### Current Implementation
Both use similar compaction strategies:

**SST Compaction**:
- Level-based merge
- MVCC resolution
- Tombstone handling
- Zero-copy optimization

**SWIFT Compaction**:
- Hierarchical merge (superblocks)
- Progressive refinement
- Similar MVCC handling

#### Common Elements to Extract
1. **K-way merge algorithm**
2. **MVCC resolution logic**
3. **Tombstone processing**
4. **Level management**
5. **File selection strategies**
6. **Compaction statistics**

## Proposed Consolidation

### 1. Columnar Common Module (`storage/engines/columnar/`)

#### New Components
```rust
// columnar/flush_manager.rs
pub struct ColumnarFlushManager {
    schema_manager: Arc<SchemaManager>,
    filesystem: Arc<FilesystemFactory>,
    atomic_coordinator: Arc<TransactionCoordinator>,
    compression_adapter: Arc<UniversalCompressionAdapter>,
    quantization_adapter: Arc<UniversalQuantizationAdapter>,
}

impl ColumnarFlushManager {
    pub async fn flush_to_parquet(
        &self,
        records: &[VectorRecord],
        config: &ColumnarFlushConfig,
    ) -> Result<ParquetFile> {
        // Common Parquet flush logic
    }
    
    pub fn map_compression_to_parquet(&self, config: &UniversalCompressionConfig) -> parquet::basic::Compression {
        // Shared compression mapping
    }
    
    pub async fn generate_schema(&self, records: &[VectorRecord]) -> Arc<Schema> {
        // Dynamic schema generation
    }
}

// columnar/compaction_manager.rs
pub struct ColumnarCompactionManager {
    filesystem: Arc<FilesystemFactory>,
    mvcc_resolver: Arc<MvccResolver>,
    atomic_coordinator: Arc<TransactionCoordinator>,
}

impl ColumnarCompactionManager {
    pub async fn compact_level_based(
        &self,
        input_files: Vec<ParquetFile>,
        config: &CompactionConfig,
    ) -> Result<CompactionResult> {
        // Common level-based compaction
    }
    
    pub async fn merge_parquet_files(&self, files: Vec<ParquetFile>) -> Result<ParquetFile> {
        // K-way merge with MVCC
    }
}
```

#### Benefits
- **Code Reuse**: ~1500 lines saved between NOVA/VIPER
- **Consistent Behavior**: Same flush/compaction logic
- **Easier Testing**: Single test suite for columnar operations
- **Performance**: Optimizations benefit both engines

### 2. Row-Based Common Module (`storage/engines/row_based/`)

#### New Components
```rust
// row_based/flush_manager.rs
pub struct RowBasedFlushManager {
    filesystem: Arc<FilesystemFactory>,
    compression_adapter: Arc<UniversalCompressionAdapter>,
    quantization_adapter: Arc<UniversalQuantizationAdapter>,
    bloom_factory: Arc<BloomFilterFactory>,
}

impl RowBasedFlushManager {
    pub async fn flush_to_sst(
        &self,
        records: &[VectorRecord],
        config: &SstFlushConfig,
    ) -> Result<SstFile> {
        // Common SST flush logic
    }
    
    pub fn build_data_blocks(&self, records: &[VectorRecord]) -> Vec<DataBlock> {
        // Shared block building
    }
    
    pub async fn notify_eventlog(&self, files: Vec<String>) -> Result<()> {
        // EventLog notification for AXIS
    }
}

// row_based/compaction_manager.rs
pub struct RowBasedCompactionManager {
    filesystem: Arc<FilesystemFactory>,
    mvcc_resolver: Arc<MvccResolver>,
    compactor: Arc<SstCompactor>,
}

impl RowBasedCompactionManager {
    pub async fn compact_level_based(
        &self,
        input_files: Vec<SstFile>,
        config: &CompactionConfig,
    ) -> Result<CompactionResult> {
        // Common SST compaction
    }
    
    pub fn k_way_merge(&self, files: Vec<SstFile>) -> impl Stream<Item = VectorRecord> {
        // Streaming k-way merge
    }
}
```

#### Benefits
- **Code Reuse**: ~1200 lines saved between SST/SWIFT
- **Shared Optimizations**: Zero-copy, cloud I/O optimization
- **Unified Testing**: Common test infrastructure
- **Consistency**: Same compaction behavior

## Implementation Strategy

### Phase 1: Extract Common Interfaces
1. Define common traits for flush and compaction
2. Create configuration structures
3. Establish result types

### Phase 2: Implement Columnar Common
1. Extract VIPER flush/compaction logic
2. Adapt NOVA to use common module
3. Add tests for columnar operations

### Phase 3: Implement Row-Based Common
1. Extract SST flush/compaction logic
2. Adapt SWIFT to use common module
3. Add hierarchical extensions for SWIFT

### Phase 4: Integration & Testing
1. Update engines to use common modules
2. Performance benchmarking
3. Migration guide

## Migration Example

### Before (VIPER Flush)
```rust
impl FlushManager {
    pub async fn flush_vectors(&self, records: &[VectorRecord]) -> Result<FlushResult> {
        // 300+ lines of flush logic
    }
}
```

### After (Using Columnar Common)
```rust
impl ViperEngine {
    pub async fn flush(&self, records: &[VectorRecord]) -> Result<FlushResult> {
        let config = ViperFlushConfig {
            compression: self.compression_adapter.get_default_config(),
            quantization: self.quantization_adapter.get_default_config(),
            // VIPER-specific settings
        };
        
        let result = self.columnar_flush_manager
            .flush_to_parquet(records, &config)
            .await?;
            
        // VIPER-specific post-processing
        Ok(result)
    }
}
```

## Specific Synergies

### Columnar Synergies (NOVA/VIPER)
1. **Parquet Integration**: Both use Arrow/Parquet
2. **Schema Evolution**: Dynamic schema generation
3. **Row Group Management**: Similar chunking strategies
4. **Column Statistics**: Shared metadata collection
5. **Compression Mapping**: Identical Parquet compression mapping

### Row-Based Synergies (SST/SWIFT)
1. **SST Format**: Both use SSTable structure
2. **Block Organization**: DataBlock with quantization
3. **Bloom Filters**: Already sharing implementation
4. **MVCC Resolution**: Same version handling
5. **EventLog Integration**: Common AXIS notification

## Metrics & Monitoring

### Shared Metrics
```rust
pub struct FlushMetrics {
    pub records_flushed: u64,
    pub bytes_written: u64,
    pub compression_ratio: f32,
    pub flush_duration_ms: u64,
}

pub struct CompactionMetrics {
    pub input_files: usize,
    pub output_files: usize,
    pub records_compacted: u64,
    pub records_deleted: u64,
    pub space_saved_bytes: i64,
}
```

## Estimated Impact

### Code Reduction
- **Columnar**: ~1500 lines saved
- **Row-Based**: ~1200 lines saved
- **Total**: ~2700 lines (45% reduction in flush/compaction code)

### Performance Benefits
- **Unified Optimizations**: All engines benefit from improvements
- **Better Caching**: Shared cache strategies
- **Reduced Memory**: Single implementation in memory

### Maintenance Benefits
- **Single Source of Truth**: One implementation per pattern
- **Easier Debugging**: Centralized logic
- **Faster Development**: New features available to all engines

## Risks & Mitigations

### Risk: Over-Generalization
**Mitigation**: Keep engine-specific hooks and configuration options

### Risk: Performance Regression
**Mitigation**: Comprehensive benchmarking before/after

### Risk: Breaking Changes
**Mitigation**: Incremental migration with fallback options

## Conclusion

The flush and compaction operations across all engines show significant commonalities that can be consolidated:

1. **Columnar engines** (NOVA/VIPER) share Parquet-based operations
2. **Row-based engines** (SST/SWIFT) share SST-based operations
3. **~2700 lines** of code can be eliminated through consolidation
4. **Performance and maintenance** benefits justify the refactoring

The proposed common modules will provide a solid foundation for future enhancements while maintaining engine-specific optimizations where needed.