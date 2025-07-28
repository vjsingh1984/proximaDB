# WAL Persistence Recommendations for ProximaDB

## Current Issues

1. **No Durability Guarantees**: WAL writes are not synced to disk, leaving data vulnerable to power failures
2. **Misleading Architecture**: `immediate_sync` parameter is ignored, `force_sync()` methods are no-ops
3. **Data Loss Risk**: Between write and memtable flush, data only exists in OS page cache

## Recommended Solution

### 1. Add Sync Support to Filesystem Layer

```rust
// In filesystem/mod.rs
#[async_trait]
pub trait FileSystem: Send + Sync {
    // ... existing methods ...
    
    /// Sync data to disk (fsync/fdatasync)
    /// Returns Ok(()) if sync is not needed/supported
    async fn sync(&self, url: &str) -> FsResult<()> {
        // Default implementation - no sync
        Ok(())
    }
}

// In filesystem/local.rs
impl FileSystem for LocalFileSystem {
    async fn sync(&self, url: &str) -> FsResult<()> {
        let path = self.url_to_path(url)?;
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .await?;
        file.sync_all().await?; // This calls fsync()
        Ok(())
    }
}
```

### 2. Update WalDiskManager to Sync After Write

```rust
// In wal/disk_manager.rs
pub async fn write_batch(
    &self,
    collection_id: &str,
    batch_id: &BatchId,
    data: &[u8],
    format: SerializationFormat,
    sync_to_disk: bool, // New parameter
) -> Result<WalFileInfo> {
    // ... existing write logic ...
    
    filesystem.write(&file_url, data, None).await?;
    
    // NEW: Sync to disk if requested
    if sync_to_disk {
        filesystem.sync(&file_url).await
            .context("Failed to sync WAL batch to disk")?;
    }
    
    // ... rest of method ...
}
```

### 3. Configure Sync Behavior Based on Requirements

```rust
// In WalConfig
pub enum DurabilityLevel {
    /// No sync - fastest, but risk of data loss (development only)
    NoSync,
    
    /// Sync metadata only (fdatasync) - good balance
    SyncData,
    
    /// Full sync (fsync) - safest but slowest
    SyncFull,
    
    /// Batch sync - sync every N writes or T seconds
    BatchSync { batch_size: usize, interval_secs: u64 },
}
```

### 4. Performance Considerations

1. **Group Commit**: Batch multiple writes before syncing
   ```rust
   // Accumulate writes for 10ms or 100 vectors, then sync once
   let batch_writer = BatchWriter::new(
       Duration::from_millis(10),
       100, // vectors
   );
   ```

2. **Async I/O**: Use io_uring on Linux for better performance
   ```rust
   // Future enhancement: use tokio-uring for async fsync
   ```

3. **Write-Ahead Buffer**: Keep recent writes in memory for fast reads
   ```rust
   // Even after sync, keep in memory for performance
   memtable.insert(vector);
   wal_buffer.insert(vector); // Fast recent access
   ```

## Trade-offs

### Option 1: Sync Every Write (Safest)
- **Pros**: No data loss on power failure
- **Cons**: ~50% write performance impact
- **Use**: Financial, healthcare, critical data

### Option 2: Batch Sync (Balanced)
- **Pros**: Good durability, better performance
- **Cons**: Can lose last batch on failure
- **Use**: Most production workloads

### Option 3: No Sync (Fastest)
- **Pros**: Maximum performance
- **Cons**: Can lose data on power failure
- **Use**: Development, non-critical data

## Recommended Default

For production use, recommend **Batch Sync** with:
- Sync every 100 vectors OR every 100ms (whichever comes first)
- This provides good durability with minimal performance impact
- Can be tuned based on workload requirements

## Implementation Priority

1. **Phase 1**: Add basic fsync support (1 day)
2. **Phase 2**: Add batch sync optimization (2 days)
3. **Phase 3**: Add io_uring support for Linux (1 week)
4. **Phase 4**: Add group commit optimization (1 week)

## Testing

1. **Durability Test**: Kill process after write, verify data persists
2. **Performance Test**: Measure impact of different sync modes
3. **Crash Test**: Simulate power failure scenarios
4. **Benchmark**: Compare with other databases (RocksDB, PostgreSQL)