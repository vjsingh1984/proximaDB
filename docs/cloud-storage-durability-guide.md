# Cloud Storage Durability Guide for ProximaDB

## Overview

Cloud storage services (S3, Azure Blob, GCS) handle durability very differently from local filesystems. Understanding these differences is crucial for implementing proper data persistence in ProximaDB.

## Key Differences: Local vs Cloud Storage

### Local Filesystem
- **Write**: Data goes to OS page cache
- **fsync()**: Forces data from page cache to disk
- **Risk**: Power loss before fsync = data loss
- **Control**: Direct control over when data hits disk

### Cloud Storage
- **Write**: Data goes to cloud provider's infrastructure
- **Durability**: Automatic once write succeeds
- **Risk**: Network failure during write = no data written
- **Control**: No explicit sync needed - provider handles it

## How Cloud Storage Durability Works

### Understanding ProximaDB's Atomic Write Pattern

ProximaDB uses the UnifiedAtomicCoordinator for all writes:
```
1. Write to staging location (may be local temp storage)
2. Atomic move from staging to final destination
3. Delete staging file only after successful move
```

This pattern ensures:
- No partial writes are visible
- Failed operations don't corrupt existing data
- Atomic visibility of complete data

### 1. Amazon S3
```
Client → S3 API → Multiple Data Centers → Acknowledgment
```
- **Durability**: 99.999999999% (11 9's) annually
- **Replication**: Automatically replicated across multiple AZs
- **Consistency**: Strong read-after-write consistency
- **No fsync needed**: Write success = data is durable
- **Atomic Operations**: S3 PutObject is atomic - either fully succeeds or fails

### 2. Azure Blob Storage
```
Client → Azure API → Storage Stamps → Acknowledgment
```
- **Durability**: 99.999999999% (11 9's) for LRS
- **Replication**: LRS, ZRS, GRS, RA-GRS options
- **Consistency**: Strong consistency
- **No fsync needed**: Write completion = durability guaranteed
- **Atomic Operations**: Block blob uploads are atomic

### 3. Google Cloud Storage
```
Client → GCS API → Regional Storage → Acknowledgment
```
- **Durability**: 99.999999999% (11 9's) annually
- **Replication**: Automatic within region
- **Consistency**: Strong global consistency
- **No fsync needed**: Successful write = durable
- **Atomic Operations**: Object uploads are atomic and consistent

## ProximaDB Implementation

### Current sync_file Implementation

```rust
// In LocalFileSystem
async fn sync_file(&self, path: &str) -> FsResult<()> {
    let file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&resolved_path)
        .await?;
    file.sync_all().await?; // This calls fsync()
    Ok(())
}

// For cloud storage - sync_file should be a no-op
async fn sync_file(&self, path: &str) -> FsResult<()> {
    // Cloud storage doesn't need explicit sync
    // Data is durable once write succeeds
    Ok(())
}
```

### Recommended Implementation for Cloud Storage

```rust
// In S3FileSystem
async fn sync_file(&self, path: &str) -> FsResult<()> {
    // S3 doesn't support or need fsync
    // Data is already durable after successful PutObject
    tracing::debug!("sync_file called on S3 - no-op as S3 handles durability");
    Ok(())
}

async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
    let put_request = self.client
        .put_object()
        .bucket(&bucket)
        .key(&key)
        .body(data.to_vec().into());
    
    // Once this succeeds, data is durable across multiple data centers
    let result = put_request.send().await?;
    
    // Verify write with ETag if critical
    if let Some(etag) = result.e_tag() {
        tracing::debug!("S3 write confirmed with ETag: {}", etag);
    }
    
    Ok(())
}
```

## Best Practices for Cloud Storage Durability

### 1. **Atomic Writes**
Cloud storage provides atomic writes by default:
```rust
// BAD: Partial writes possible with streaming
let mut file = s3.open_for_write(path).await?;
file.write_chunk(data1).await?; // Could fail here
file.write_chunk(data2).await?;
file.close().await?;

// GOOD: Atomic write
s3.write(path, &complete_data).await?; // All or nothing
```

### 2. **Write Verification**
Use ETags or checksums for critical data:
```rust
// Write with verification
let checksum = calculate_md5(&data);
let result = s3.write_with_checksum(path, &data, checksum).await?;
assert_eq!(result.etag, expected_etag);
```

### 3. **Multipart Upload for Large Files**
For files > 100MB, use multipart upload:
```rust
// Automatic multipart for large files
if data.len() > MULTIPART_THRESHOLD {
    s3.multipart_upload(path, &data).await?;
} else {
    s3.put_object(path, &data).await?;
}
```

### 4. **Handling Network Failures**
Implement retry logic for transient failures:
```rust
// Exponential backoff retry
let mut retries = 0;
loop {
    match s3.write(path, &data).await {
        Ok(_) => break,
        Err(e) if retries < MAX_RETRIES => {
            let delay = Duration::from_millis(100 * 2u64.pow(retries));
            tokio::time::sleep(delay).await;
            retries += 1;
        }
        Err(e) => return Err(e),
    }
}
```

## ProximaDB WAL Strategy for Cloud Storage

### 1. **Write-Through Pattern**
```rust
// For cloud storage, write directly without local caching
async fn write_wal_batch(&self, batch: &WalBatch) -> Result<()> {
    let path = format!("wal/{}/batch_{}.wal", 
        collection_id, 
        batch.id.to_base62()
    );
    
    // Serialize batch
    let data = serialize_batch(batch)?;
    
    // Write to cloud - this is already durable
    self.cloud_storage.write(&path, &data).await?;
    
    // No sync needed - cloud handles durability
    Ok(())
}
```

### 2. **Batch Aggregation for Efficiency**
```rust
// Aggregate small writes to reduce API calls
struct CloudWalWriter {
    pending_writes: Vec<WalEntry>,
    last_flush: Instant,
}

impl CloudWalWriter {
    async fn add_entry(&mut self, entry: WalEntry) -> Result<()> {
        self.pending_writes.push(entry);
        
        // Flush if batch is large enough or timeout reached
        if self.pending_writes.len() >= BATCH_SIZE ||
           self.last_flush.elapsed() > BATCH_TIMEOUT {
            self.flush().await?;
        }
        Ok(())
    }
    
    async fn flush(&mut self) -> Result<()> {
        if self.pending_writes.is_empty() {
            return Ok(());
        }
        
        // Combine entries into single write
        let batch_data = serialize_entries(&self.pending_writes)?;
        let path = format!("wal/batch_{}.wal", Uuid::new_v4());
        
        // Single atomic write to cloud
        self.cloud_storage.write(&path, &batch_data).await?;
        
        self.pending_writes.clear();
        self.last_flush = Instant::now();
        Ok(())
    }
}
```

### 3. **Recovery Optimization**
```rust
// List and read WAL files efficiently
async fn recover_wal(&self) -> Result<Vec<WalEntry>> {
    // List all WAL files
    let wal_files = self.cloud_storage
        .list("wal/")
        .await?;
    
    // Read in parallel for faster recovery
    let mut handles = vec![];
    for file in wal_files {
        let storage = self.cloud_storage.clone();
        handles.push(tokio::spawn(async move {
            storage.read(&file.path).await
        }));
    }
    
    // Collect results
    let mut all_entries = vec![];
    for handle in handles {
        let data = handle.await??;
        let entries = deserialize_entries(&data)?;
        all_entries.extend(entries);
    }
    
    Ok(all_entries)
}
```

## Configuration Recommendations

### Local Storage (Development)
```toml
[wal]
durability_level = "SyncFull"  # Full fsync for safety
sync_mode = "PerBatch"         # Sync after each batch

[filesystem.local]
sync_enabled = true            # Enable fsync
```

### Cloud Storage (Production)
```toml
[wal]
durability_level = "NoSync"    # Cloud handles durability
sync_mode = "Never"            # No explicit sync needed

[filesystem.s3]
multipart_threshold = 104857600  # 100MB
multipart_chunk_size = 10485760  # 10MB chunks
max_retries = 3                  # Retry transient failures
```

### Hybrid Setup (Local Cache + Cloud)
```toml
[wal]
# Write to local cache first, then async upload to cloud
local_cache_dir = "/var/cache/proximadb/wal"
cloud_backup_url = "s3://my-bucket/proximadb/wal"
sync_to_cloud_interval = 60  # seconds

[durability]
local_sync = true            # Sync local cache
cloud_sync_on_flush = true   # Upload to cloud on flush
```

## Implementation Checklist

- [ ] Implement no-op sync_file for cloud filesystems
- [ ] Add write verification using ETags/checksums
- [ ] Implement multipart upload for large files
- [ ] Add retry logic with exponential backoff
- [ ] Optimize batch sizes for cloud API limits
- [ ] Implement parallel reads for recovery
- [ ] Add cloud-specific configuration options
- [ ] Document durability guarantees per storage type

## Summary

1. **Cloud storage is inherently durable** - no fsync needed
2. **Focus on atomic writes** - use single PutObject when possible
3. **Batch small writes** - reduce API calls and costs
4. **Verify critical writes** - use ETags/checksums
5. **Handle network failures** - implement proper retry logic
6. **Optimize for cloud** - different strategies than local storage