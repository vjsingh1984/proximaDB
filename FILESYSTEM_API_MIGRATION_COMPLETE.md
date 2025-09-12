# Complete Filesystem API Migration Plan

## Critical Issue
Direct file I/O usage (`std::fs::File`) breaks cloud storage compatibility. All file operations MUST use the filesystem API to work with S3, Azure Blob, and GCS.

## Production Code Requiring Immediate Migration

### 1. PRISM Engine Memory Optimizer
**File:** `src/storage/engines/prism/memory_optimizer.rs`
```rust
// CURRENT (line ~582)
let file = std::fs::File::open(file_path)?;
let mmap = unsafe { MmapOptions::new().map(&file)? };

// SHOULD BE:
let filesystem = self.filesystem_factory.get_filesystem(file_path).await?;
let mmap = filesystem.get_mmap(file_path).await?
    .ok_or_else(|| anyhow!("Memory mapping not supported for {}", file_path))?;
```

### 2. RAPTOR Engine
**File:** `src/storage/engines/raptor/engine.rs`
```rust
// Uses std::fs::File for memory mapping
// Should use filesystem.get_mmap()
```

### 3. RAPTOR Consolidated Reader
**File:** `src/storage/engines/raptor/consolidated_reader.rs`
```rust
// Uses std::fs::File for memory mapping
// Should use filesystem.get_mmap()
```

### 4. Row-Based Shared SST Reader
**File:** `src/storage/engines/row_based/shared_sst_reader.rs`
```rust
// CURRENT (line ~179)
let file = std::fs::File::open(&cached_path)?;

// SHOULD BE:
let filesystem = self.filesystem_factory.get_filesystem(&cached_path).await?;
let data = filesystem.read(&cached_path).await?;
```

### 5. Common Performance Optimization
**File:** `src/storage/engines/common/performance_optimization.rs`
```rust
// Uses std::fs::File for memory mapping
// Should use filesystem API
```

### 6. Columnar Optimization (Multiple Locations)
**File:** `src/storage/engines/columnar/optimization.rs`
```rust
// Lines 134, 200, 553
// CURRENT:
let file = std::fs::File::open(file_path)?;
let reader_builder = ParquetRecordBatchReaderBuilder::try_new(file)?;

// SHOULD BE:
let filesystem = self.filesystem_factory.get_filesystem(file_path).await?;
let data = filesystem.read(file_path).await?;
let bytes = bytes::Bytes::from(data);
let reader_builder = ParquetRecordBatchReaderBuilder::try_new(bytes)?;
```

### 7. Event Log Persistence
**File:** `src/services/event_log_persistence.rs`
```rust
// CURRENT:
let mut file = fs::File::open(path).await?;

// SHOULD BE:
let filesystem = self.filesystem_factory.get_filesystem(path).await?;
let data = filesystem.read(path).await?;
```

### 8. MMap File Module
**File:** `src/storage/mmap_file.rs`
```rust
// Entire module needs refactoring to use filesystem API
// Should be a wrapper around filesystem.get_mmap()
```

## Test/Benchmark Code (Lower Priority)

1. `src/bin/benchmark_engines.rs` - Benchmark results writer
2. `src/storage/engines/viper/readers/test_data_generator.rs` - Test data generation
3. Various test files in `tests/` directory

## Migration Strategy

### Phase 1: Add Filesystem Factory Access
- Add `filesystem_factory: Arc<FilesystemFactory>` to all affected structs
- Pass through constructors

### Phase 2: Replace File Operations
- Replace `File::open()` with `filesystem.read()`
- Replace `File::create()` with `filesystem.write()`
- Replace memory mapping with `filesystem.get_mmap()`

### Phase 3: Handle Bytes/Streams
- Parquet readers can use `bytes::Bytes` directly
- For streaming, use `filesystem.read_stream()`

## Benefits After Migration

1. **Cloud Storage Support**: Seamlessly work with S3, Azure, GCS
2. **Caching**: Automatic caching for remote files
3. **Retry Logic**: Built-in retry with exponential backoff
4. **Authentication**: Unified auth for all providers
5. **Zero-Copy I/O**: When supported by backend
6. **Atomic Writes**: Built-in atomic write support

## Code Pattern Examples

### Reading a File
```rust
// Before
let file = std::fs::File::open(path)?;

// After
let filesystem = self.filesystem_factory.get_filesystem(path).await?;
let data = filesystem.read(path).await?;
```

### Memory Mapping
```rust
// Before
let file = std::fs::File::open(path)?;
let mmap = unsafe { MmapOptions::new().map(&file)? };

// After
let filesystem = self.filesystem_factory.get_filesystem(path).await?;
let mmap = filesystem.get_mmap(path).await?
    .ok_or_else(|| anyhow!("Memory mapping not supported"))?;
```

### Parquet Reading
```rust
// Before
let file = std::fs::File::open(path)?;
let reader = ParquetRecordBatchReaderBuilder::try_new(file)?;

// After
let filesystem = self.filesystem_factory.get_filesystem(path).await?;
let data = filesystem.read(path).await?;
let bytes = bytes::Bytes::from(data);
let reader = ParquetRecordBatchReaderBuilder::try_new(bytes)?;
```

## Testing the Migration

1. Unit tests should mock FilesystemFactory
2. Integration tests should test with both local and S3 backends
3. Verify no performance regression for local files
4. Ensure cloud storage works correctly

## Priority Order

1. **CRITICAL**: Columnar optimization (breaks cloud storage for NOVA/VIPER)
2. **HIGH**: PRISM memory optimizer
3. **HIGH**: Row-based SST reader
4. **MEDIUM**: RAPTOR engine components
5. **LOW**: Test and benchmark code

## Checklist

- [ ] Add filesystem_factory to all affected structs
- [ ] Migrate columnar optimization
- [ ] Migrate PRISM memory optimizer
- [ ] Migrate row-based SST reader
- [ ] Migrate RAPTOR engine
- [ ] Migrate RAPTOR consolidated reader
- [ ] Migrate common performance optimization
- [ ] Migrate event log persistence
- [ ] Refactor mmap_file module
- [ ] Update tests
- [ ] Performance testing
- [ ] Cloud storage testing (S3, Azure, GCS)