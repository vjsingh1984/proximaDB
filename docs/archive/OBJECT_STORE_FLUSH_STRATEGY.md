# Object Store Flush Strategy Design

## Overview

ProximaDB supports multiple strategies for handling compaction and flush operations with object stores (S3, ADLS, GCS). This document outlines the recommended approach based on performance, cost, and reliability considerations.

## Strategy Options

### Option 1: Local Flush + Atomic Move (Recommended)

**Pattern**: Flush locally → Atomic move to object store

```
Local: ./___compaction/segment_001.avro  →  S3: s3://bucket/data/segment_001.avro
Local: ./___flushed/batch_001.avro      →  ADLS: adls://store/data/batch_001.avro
```

**Advantages**:
- Single object store operation (PUT)
- Local filesystem optimizations for large writes
- Same mount point benefits for temp operations
- Predictable upload bandwidth usage
- Easy to implement retry logic

**Disadvantages**:
- Requires local storage space
- Double write (local + object store)

### Option 2: Direct Object Store Flush with Temp Prefix

**Pattern**: Direct flush to object store with temp prefix

```
S3: s3://bucket/___compaction/segment_001.avro  →  s3://bucket/data/segment_001.avro
ADLS: adls://store/___temp/batch_001.avro      →  adls://store/data/batch_001.avro
```

**Advantages**:
- No local storage requirements
- Single write operation
- Natural object store semantics

**Disadvantages**:
- Object store bandwidth during entire operation
- More complex retry/recovery logic
- Potential for orphaned temp objects

## Recommended Implementation

### Configuration

```rust
pub struct ObjectStoreConfig {
    /// Flush strategy selection
    pub flush_strategy: ObjectStoreFlushStrategy,
    
    /// Local temp directory for Option 1
    pub local_temp_dir: Option<String>,
    
    /// Object store temp prefix for Option 2
    pub temp_prefix: String, // Default: "___temp"
}

pub enum ObjectStoreFlushStrategy {
    /// Local flush + atomic move
    LocalFlushMove {
        temp_dir: String,
    },
    
    /// Direct object store flush with temp prefix
    DirectFlushTemp {
        temp_prefix: String,
    },
    
    /// Hybrid: Small files direct, large files local
    Hybrid {
        size_threshold_mb: u64, // Default: 100MB
        temp_dir: String,
        temp_prefix: String,
    },
}
```

### Implementation Details

#### Local Flush + Move Strategy

```rust
// Compaction example
async fn compact_to_object_store(segments: Vec<Segment>) -> Result<()> {
    // 1. Create local temp file in ___compaction directory
    let local_temp = local_fs.create_temp_in_dir("___compaction", "compacted_segment.avro")?;
    
    // 2. Perform compaction locally (fast local I/O)
    let mut writer = AvroWriter::new(local_temp);
    for segment in segments {
        writer.write_segment(segment).await?;
    }
    writer.finalize().await?;
    
    // 3. Atomic upload to object store
    let final_key = "data/compacted_segment_001.avro";
    object_store.upload_file(local_temp, final_key).await?;
    
    // 4. Cleanup local temp
    local_fs.delete(local_temp).await?;
    
    Ok(())
}
```

#### Direct Flush Strategy

```rust
// Direct flush example  
async fn flush_direct_to_object_store(batch: Batch) -> Result<()> {
    // 1. Create temp object with prefix
    let temp_key = "___temp/batch_001.avro";
    let final_key = "data/batch_001.avro";
    
    // 2. Stream directly to object store
    let mut object_writer = object_store.create_writer(temp_key).await?;
    let mut avro_writer = AvroWriter::new(object_writer);
    avro_writer.write_batch(batch).await?;
    avro_writer.finalize().await?;
    
    // 3. Atomic rename in object store
    object_store.copy_object(temp_key, final_key).await?;
    object_store.delete_object(temp_key).await?;
    
    Ok(())
}
```

### WAL Strategy for Object Stores

For WAL operations specifically:

```rust
// WAL configuration for object stores
pub struct ObjectStoreWalConfig {
    /// WAL strategy
    pub strategy: WalStrategy,
    
    /// Batch size for object store WAL
    pub batch_size: usize, // Default: 1000 entries
    
    /// Flush interval
    pub flush_interval_ms: u64, // Default: 5000ms
}

pub enum WalStrategy {
    /// Batch local WAL entries, then flush to object store
    BatchedLocal {
        local_temp_dir: String,
        batch_size: usize,
    },
    
    /// Stream WAL entries directly to object store
    DirectStream {
        buffer_size: usize,
    },
}
```

**Recommended WAL Pattern**:
```
wal/{collection_id}/wal_000001.avro  (batched, 1000 entries)
wal/{collection_id}/wal_000002.avro  (batched, 1000 entries)
```

## Performance Considerations

### Local Flush + Move (Recommended for Most Use Cases)

**Best for**:
- Large compaction operations (>100MB)
- High-frequency flush operations
- Environments with adequate local storage
- When local I/O is much faster than network I/O

**Performance characteristics**:
- Local write: ~500MB/s (SSD)
- Network upload: ~100MB/s (typical)
- Total time: max(local_write_time, upload_time)

### Direct Flush (Recommended for Space-Constrained Environments)

**Best for**:
- Small to medium operations (<100MB)
- Space-constrained environments
- When local storage is slower than network
- Cloud-native deployments

**Performance characteristics**:
- Direct write: ~100MB/s (network limited)
- No local storage requirements
- Simpler recovery semantics

## Configuration Examples

### Production Configuration (Local Flush + Move)

```toml
[object_store]
flush_strategy = "local_flush_move"
local_temp_dir = "/data/proximadb/temp"
temp_prefix = "___temp"

[object_store.performance]
upload_concurrency = 4
chunk_size_mb = 64
```

### Cloud-Native Configuration (Direct Flush)

```toml
[object_store] 
flush_strategy = "direct_flush_temp"
temp_prefix = "___processing"

[object_store.performance]
stream_buffer_mb = 16
upload_timeout_seconds = 300
```

### Hybrid Configuration (Size-Based)

```toml
[object_store]
flush_strategy = "hybrid"
size_threshold_mb = 100
local_temp_dir = "/tmp/proximadb"
temp_prefix = "___temp"
```

## Implementation Roadmap

1. **Phase 1**: Implement local flush + move strategy
2. **Phase 2**: Add direct flush strategy 
3. **Phase 3**: Implement hybrid strategy with size-based selection
4. **Phase 4**: Add performance monitoring and auto-tuning

This design provides flexibility while maintaining optimal performance for different deployment scenarios.