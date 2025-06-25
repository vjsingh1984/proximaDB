# Storage Package Reorganization Plan

## Current Issues:
1. **Duplicate directories**: `viper/` and `vector/engines/` both contain VIPER code
2. **Inconsistent naming**: `memtable.rs` vs `memtable/` directory structure
3. **Mixed concerns**: metadata scattered across multiple locations
4. **Legacy files**: obsolete search_index.rs, search/ directory

## New Consistent Structure:

```
src/storage/
├── mod.rs                     # Main exports and module declarations
├── traits.rs                  # UnifiedStorageEngine trait (NEW)
├── builder.rs                 # StorageSystemBuilder
├── validation.rs              # Configuration validation
│
├── engines/                   # STORAGE ENGINES (organized by type)
│   ├── mod.rs
│   ├── lsm/                   # LSM Tree Engine
│   │   ├── mod.rs             # LSM main implementation
│   │   ├── compaction.rs      # LSM compaction logic
│   │   └── memtable.rs        # LSM memtable (moved from root)
│   ├── viper/                 # VIPER Engine (consolidated)
│   │   ├── mod.rs             # VIPER main implementation
│   │   ├── core.rs            # Core VIPER operations
│   │   ├── pipeline.rs        # Data processing pipeline
│   │   ├── factory.rs         # VIPER factory pattern
│   │   └── utilities.rs       # Background services & monitoring
│   └── hybrid/                # Future: Hybrid engine
│       └── mod.rs
│
├── persistence/               # DATA PERSISTENCE LAYER
│   ├── mod.rs
│   ├── wal/                   # Write-Ahead Log
│   │   ├── mod.rs
│   │   ├── config.rs
│   │   ├── avro.rs            # Avro WAL strategy
│   │   ├── bincode.rs         # Bincode WAL strategy
│   │   ├── disk.rs            # Disk management
│   │   ├── factory.rs         # WAL factory
│   │   ├── background_manager.rs
│   │   └── memtable/          # WAL memtable implementations
│   │       ├── mod.rs
│   │       ├── art.rs
│   │       ├── btree.rs
│   │       ├── hashmap.rs
│   │       └── skiplist.rs
│   ├── filesystem/            # Multi-backend filesystem
│   │   ├── mod.rs
│   │   ├── local.rs
│   │   ├── s3.rs
│   │   ├── azure.rs
│   │   ├── gcs.rs
│   │   └── hdfs.rs
│   └── disk_manager/          # Local disk management
│       └── mod.rs
│
├── indexing/                  # INDEXING SYSTEM (unified)
│   ├── mod.rs
│   ├── manager.rs             # Unified index manager
│   ├── algorithms/            # Index algorithms
│   │   ├── mod.rs
│   │   ├── hnsw.rs
│   │   ├── ivf.rs
│   │   └── flat.rs
│   └── operations/            # Index operations
│       ├── mod.rs
│       ├── build.rs
│       ├── search.rs
│       └── maintenance.rs
│
├── query/                     # SEARCH & QUERY ENGINE
│   ├── mod.rs
│   ├── engine.rs              # Unified search engine
│   ├── operations/
│   │   ├── mod.rs
│   │   ├── vector_search.rs
│   │   ├── filter.rs
│   │   └── aggregation.rs
│   └── coordinator.rs         # Query coordination
│
├── encoding/                  # DATA ENCODING & COMPRESSION
│   ├── mod.rs
│   ├── parquet_encoder.rs
│   ├── compression.rs
│   └── column_family.rs
│
├── strategy/                  # STORAGE STRATEGIES
│   ├── mod.rs
│   ├── viper.rs              # VIPER strategy
│   ├── standard.rs           # LSM strategy  
│   ├── custom.rs             # Custom strategies
│   └── config.rs             # Strategy configuration
│
├── coordination/             # STORAGE COORDINATION
│   ├── mod.rs
│   ├── atomicity.rs          # Atomic operations
│   ├── tiered.rs             # Tiered storage
│   └── mmap/                 # Memory mapping
│       └── mod.rs
│
└── legacy/                   # DEPRECATED FILES (to be removed)
    ├── search_index.rs       # Use indexing/ instead
    ├── search/               # Use query/ instead
    └── viper/                # Use engines/viper/ instead
```

## Reorganization Steps:

### Phase 1: Create new structure
1. Create `engines/` directory with `lsm/` and `viper/` subdirectories
2. Create `persistence/` directory for WAL, filesystem, disk management
3. Create `indexing/` directory for unified indexing
4. Create `query/` directory for search operations
5. Create `coordination/` directory for cross-cutting concerns

### Phase 2: Move and consolidate files
1. Move `lsm/` → `engines/lsm/`
2. Move `memtable.rs` → `engines/lsm/memtable.rs`
3. Consolidate `viper/` and `vector/engines/` → `engines/viper/`
4. Move `wal/` → `persistence/wal/`
5. Move `filesystem/` → `persistence/filesystem/`
6. Move `disk_manager/` → `persistence/disk_manager/`
7. Move vector indexing → `indexing/`
8. Move vector search → `query/`

### Phase 3: Update imports and module declarations
1. Update all `mod.rs` files with new structure
2. Update imports throughout codebase
3. Update `pub use` statements for clean API

### Phase 4: Mark legacy files as deprecated
1. Add deprecation warnings to old locations
2. Provide migration paths in documentation
3. Plan removal timeline

## Benefits:
1. **Consistent naming**: All packages follow verb/noun pattern
2. **Clear separation**: Each directory has single responsibility
3. **Logical grouping**: Related functionality is co-located
4. **Scalable structure**: Easy to add new engines/strategies
5. **Clean API**: Top-level exports hide internal organization