# SST Engine Modular Architecture

## Overview

The SST (Sorted String Table) engine has been refactored from a monolithic 5,169-line module into a well-organized modular architecture with clear separation of concerns. This document describes the new structure and guidelines for maintaining and extending the SST engine.

## Module Structure

### Core Module (`core.rs`) - 352 lines
**Purpose**: Central engine struct definition and initialization

**Responsibilities**:
- `SstEngine` struct definition with all components
- Engine construction and configuration
- Component initialization (filesystem, cache, quantization, etc.)
- Core utility methods for accessing engine components

**Key Types**:
- `SstEngine`: Main engine struct
- Configuration management
- Component coordination

### Flush Module (`flush/`) - 1,011 lines total
**Purpose**: Handle all flush operations from memory to persistent storage

**Submodules**:
- `mod.rs`: Main flush implementation and do_flush trait method
- `coordinator.rs`: Flush coordination logic and scheduling
- `optimizer.rs`: Multi-batch sorting with k-way merge optimization
- `operations.rs`: Low-level atomic write operations

**Key Functions**:
- `do_flush()`: Main flush entry point
- `should_flush()`: Determines when to trigger flush
- `calculate_optimal_batch_size()`: Optimizes batch sizes
- `write_vectors_atomic()`: Atomic writes with staging

### Search Module (`search/`) - 1,378 lines total
**Purpose**: Implement three-stage filtering and search operations

**Submodules**:
- `mod.rs`: Unified search implementation
- `coordinator.rs`: Search strategy selection (bloom, quantized, full)
- `optimizer.rs`: Query optimization with bloom filter strategies
- `operations.rs`: File-level search operations

**Key Functions**:
- `search_vectors_unified()`: Main search entry point
- `select_search_strategy()`: Choose optimal search path
- `should_use_bloom_filter()`: Bloom filter decision logic
- `search_file()`: Individual SST file search

### Collections Module (`collections.rs`) - 403 lines
**Purpose**: Collection-level management and operations

**Responsibilities**:
- Collection metadata and statistics
- File management and cleanup
- Vector existence checking with bloom filters
- Collection scanning and enumeration
- Compaction coordination

**Key Functions**:
- `contains_vector()`: Check vector existence
- `cleanup_collection_files()`: Remove orphaned files
- `collection_stats()`: Get collection statistics
- `compact_collection()`: Trigger collection compaction

### Utils Module (`utils.rs`) - 492 lines
**Purpose**: Shared utility functions and helpers

**Responsibilities**:
- Vector sorting for optimal SSTable encoding
- Bloom filter building and management
- Memory estimation and tracking
- Serialization/deserialization helpers
- File naming utilities

**Key Types**:
- `SortingStats`: Track sorting performance
- `MemoryEstimate`: Memory usage calculations
- `SstableFileInfo`: File metadata

### Trait Implementation Module (`trait_impl.rs`) - 251 lines
**Purpose**: Implement UnifiedStorageFormat trait

**Responsibilities**:
- Implement all required trait methods
- Delegate to appropriate modules
- Maintain trait compliance
- Performance optimization traits

**Key Implementations**:
- `UnifiedStorageFormat`: Main storage trait
- `UniversallyOptimized`: Performance optimization trait

### Blocks Module (`blocks.rs`) - 418 lines
**Purpose**: Block management and data structures

**Responsibilities**:
- SST record representation with metadata
- Block compression configuration
- Quantized data management
- Block statistics and metadata

**Key Types**:
- `SstRecord`: Main record type with LSM metadata
- `ProximaDataBlock`: Block structure
- `QuantizedBlockData`: Quantized representations
- `BlockCompressionConfig`: Compression settings

## Module Dependencies

```
mod.rs (main)
├── core.rs
│   ├── filesystem
│   ├── cache
│   ├── quantization
│   └── transaction_coordinator
├── flush/
│   ├── mod.rs
│   ├── coordinator.rs → core
│   ├── optimizer.rs → core
│   └── operations.rs → core
├── search/
│   ├── mod.rs
│   ├── coordinator.rs → core
│   ├── optimizer.rs → core
│   └── operations.rs → core
├── collections.rs → core
├── utils.rs → core
├── trait_impl.rs → all modules
└── blocks.rs (standalone)
```

## Design Principles

### 1. Single Responsibility
Each module has one clear purpose and responsibility. For example:
- Flush module only handles flushing
- Search module only handles searching
- Collections module only handles collection management

### 2. Dependency Injection
All modules receive the `SstEngine` through Arc<SstEngine> for:
- Shared access to engine components
- Thread-safe operation
- Consistent configuration

### 3. Progressive Enhancement
The three-stage filtering in search demonstrates progressive enhancement:
1. Bloom filter (fast elimination)
2. Quantized search (approximate filtering)
3. Full precision (exact results)

### 4. Atomic Operations
Critical operations use atomic patterns:
- Flush uses staging directories
- Compaction uses transaction coordination
- All writes are atomic

## Extension Guidelines

### Adding New Functionality

1. **Identify the appropriate module**:
   - Flush-related → `flush/` module
   - Search-related → `search/` module
   - Collection management → `collections.rs`
   - General utilities → `utils.rs`

2. **Create submodules for complex features**:
   ```rust
   // In flush/new_feature.rs
   pub struct NewFlushFeature {
       engine: Arc<SstEngine>,
   }
   ```

3. **Maintain module boundaries**:
   - Don't directly access other module internals
   - Use public interfaces
   - Coordinate through the core engine

### Testing Strategy

1. **Unit tests**: In each module's #[cfg(test)] block
2. **Integration tests**: In `tests/modular_integration_test.rs`
3. **Cross-module tests**: Test module interactions

### Performance Considerations

1. **Use Arc for shared access**: Avoid cloning large structures
2. **Batch operations**: Process in chunks for cache efficiency
3. **Progressive filtering**: Eliminate early with cheap operations
4. **Async operations**: Use tokio for I/O operations

## Migration from Monolithic Structure

The refactoring preserved all functionality while improving organization:

### Before (Monolithic):
- Single 5,169-line file
- Mixed responsibilities
- Difficult to test individual components
- Hard to maintain and extend

### After (Modular):
- Main file reduced to 1,667 lines (68% reduction)
- Clear module boundaries
- Easy to test individual modules
- Simple to maintain and extend

## Common Patterns

### Module Initialization
```rust
pub struct ModuleComponent {
    engine: Arc<SstEngine>,
}

impl ModuleComponent {
    pub fn new(engine: Arc<SstEngine>) -> Self {
        Self { engine }
    }
}
```

### Error Handling
```rust
use anyhow::{Context, Result};

pub async fn operation(&self) -> Result<Output> {
    self.internal_operation()
        .await
        .context("Failed to perform operation")?
}
```

### Metric Collection
```rust
let start = std::time::Instant::now();
// ... operation ...
let duration = start.elapsed();
metrics.insert("duration_ms", duration.as_millis());
```

## Maintenance Guidelines

1. **Keep modules focused**: Don't let modules grow beyond their purpose
2. **Document public APIs**: All public functions need documentation
3. **Test module boundaries**: Ensure modules interact correctly
4. **Monitor module size**: Consider splitting if >500 lines
5. **Maintain consistency**: Follow existing patterns

## Future Improvements

1. **Further modularization**: Split large submodules if needed
2. **Interface refinement**: Create traits for module interactions
3. **Performance optimization**: Profile and optimize hot paths
4. **Documentation**: Add more inline documentation
5. **Testing**: Increase test coverage for edge cases