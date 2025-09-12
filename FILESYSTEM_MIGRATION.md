# Filesystem API Migration Tracking

This document tracks all instances of direct file I/O that need to be migrated to use the filesystem API for seamless integration with cloud storage (S3, Azure, GCS).

## Files Requiring Migration

### High Priority (Production Code)

1. **src/storage/engines/columnar/optimization.rs - Multiple locations**
   - Current: `std::fs::File::open(file_path)` 
   - Lines: 134, 200, 553
   - Should use: `filesystem.read()` to get bytes, then `ParquetRecordBatchReaderBuilder::try_new(bytes)`
   - Impact: Breaks cloud storage compatibility in columnar optimization

2. **src/storage/engines/viper/readers/test_data_generator.rs:585**
   - Current: `File::create(file_path)`
   - Should use: `filesystem.write()` or `filesystem.create_file()`
   - Impact: Test data generation for VIPER engine

3. **src/storage/engines/nova/optimized_operations.rs:300**
   - Current: `MmapParquetReader::open(parquet_path)`
   - Should use: `filesystem.get_mmap()` for memory-mapped access
   - Impact: Optimized Parquet reading in NOVA engine

4. **src/storage/engines/columnar/search.rs:516**
   - Current: Direct file opening for Parquet
   - Should use: filesystem API
   - Impact: Columnar search operations

5. **src/storage/engines/sst/readers/unified_sstable_reader.rs:1032**
   - Current: `ModularBlockReader::open(filesystem_factory.clone(), file_path)`
   - Already using filesystem factory ✓
   - Impact: SSTable reading

### Low Priority (Test Code)

1. **src/storage/engines/sst/readers/tests/test_sstable_format_fix.rs:109**
   - Test file creation
   - Can remain as-is for unit tests

## Migration Guidelines

### Before (Direct I/O):
```rust
use std::fs::File;
let file = File::create(path)?;
```

### After (Filesystem API):
```rust
let filesystem = self.filesystem_factory.get_filesystem(path).await?;
let data = filesystem.write(path, content).await?;
```

## Benefits of Migration

1. **Cloud Storage Support**: Seamlessly work with S3, Azure Blob, GCS
2. **Atomic Operations**: Built-in atomic write support via temp files
3. **Caching**: Integrated caching layer for remote files
4. **Zero-Copy I/O**: Memory-mapped access when available
5. **Retry Logic**: Automatic retry with exponential backoff
6. **Authentication**: Unified auth for all cloud providers

## Status

- [ ] VIPER test_data_generator migration
- [ ] NOVA optimized_operations migration
- [x] SST unified_sstable_reader (already using filesystem factory)

## Notes

The filesystem API is defined in `src/storage/persistence/filesystem/mod.rs` and provides:
- `FileSystem` trait for all backends
- `FilesystemFactory` for URL-based routing
- `ZeroCopyFilesystem` for optimized access
- Support for file://, s3://, adls://, gcs:// URLs