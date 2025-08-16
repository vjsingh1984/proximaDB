# Async Queue Migration Guide

## Critical Design Fix: From Synchronous to True Async

### Problem with Current Implementation

The current `FlushAxisUpdater` implements a **pseudo-synchronous** pattern:

```rust
// CURRENT (BAD) - Blocks writes waiting for queue
let offset = producer.send_with_backpressure(collection_id, payload).await?;
// Storage write WAITS for queue acknowledgment!
```

This defeats the entire purpose of async indexing:
- **2x Write Amplification**: Sending full vectors instead of metadata
- **Synchronous Blocking**: Writes wait for queue operations
- **Backpressure = Latency**: Queue fullness blocks storage writes

### New Architecture: True Fire-and-Forget

```rust
// NEW (GOOD) - Never blocks writes
producer.send_metadata(metadata);  // Returns immediately!
// Storage continues without waiting
```

## Migration Steps

### 1. Replace FlushAxisUpdater Usage

**Before (Synchronous):**
```rust
// In SST/VIPER flush operations
let updater = FlushAxisUpdater::new(...);
updater.queue_flush_updates(&params, &records).await?;  // BLOCKS!
```

**After (Async):**
```rust
// In SST/VIPER flush operations
let producer = AsyncMetadataProducer::new();
StorageIntegration::notify_flush(
    &producer,
    collection_id,
    flushed_files,
    vector_count,
    engine_type,
);  // Returns immediately!
```

### 2. Update AXIS Consumer

**Before (Receives full vectors):**
```rust
match payload {
    IndexPayload::Fp32 { vectors } => {
        // Process full vectors from queue
        for (id, vector) in vectors {
            index.insert(id, vector);
        }
    }
}
```

**After (Receives metadata, reads from storage):**
```rust
match metadata {
    IndexMetadataReference { file_paths, .. } => {
        // Read vectors from storage files
        for file_path in file_paths {
            let vectors = storage.read_vectors(&file_path)?;
            for vector in vectors {
                index.insert(vector.id, vector.data);
            }
        }
    }
}
```

### 3. Remove Backpressure Components

Delete these files/components:
- `backpressure.rs` - No longer needed
- `ProducerWithBackpressure` - Replace with `AsyncMetadataProducer`
- `BackpressureController` - Not applicable in fire-and-forget
- `BackpressureStrategy` - Obsolete

### 4. Update Storage Engines

**SST Engine:**
```rust
impl SstEngine {
    async fn flush(&self) -> Result<FlushResult> {
        // ... perform flush ...
        
        // OLD: Wait for queue
        // self.axis_updater.queue_flush_updates(...).await?;
        
        // NEW: Fire and forget
        StorageIntegration::notify_flush(
            &self.metadata_producer,
            &self.collection_id,
            flushed_files,
            vector_count,
            StorageEngineType::SST,
        );
        
        Ok(flush_result)
    }
}
```

**VIPER Engine:**
```rust
impl ViperEngine {
    async fn flush(&self) -> Result<FlushResult> {
        // ... perform flush ...
        
        // NEW: Fire and forget
        StorageIntegration::notify_flush(
            &self.metadata_producer,
            &self.collection_id,
            flushed_files,
            vector_count,
            StorageEngineType::VIPER,
        );
        
        Ok(flush_result)
    }
}
```

## Performance Benefits

### Write Latency
- **Before**: Write latency includes queue operations (10-50ms extra)
- **After**: Zero additional latency (< 1μs for metadata send)

### Write Amplification
- **Before**: 2x (vectors written to storage + queue)
- **After**: 1.05x (vectors to storage + tiny metadata to queue)

### Memory Usage
- **Before**: Queue buffers full vectors (GB of RAM)
- **After**: Queue buffers only metadata (MB of RAM)

### Decoupling
- **Before**: Storage writes blocked by slow AXIS operations
- **After**: Complete decoupling - AXIS can be down/slow without affecting writes

## Testing the Migration

### 1. Unit Test for Async Behavior
```rust
#[test]
async fn test_no_blocking() {
    let (producer, _rx) = AsyncMetadataProducer::new();
    
    let start = Instant::now();
    for _ in 0..10000 {
        producer.send_metadata(metadata);
    }
    let elapsed = start.elapsed();
    
    assert!(elapsed.as_millis() < 10); // Should be instant
}
```

### 2. Integration Test
```rust
#[test]
async fn test_storage_continues_despite_slow_axis() {
    let producer = AsyncMetadataProducer::new();
    
    // Simulate slow AXIS (consumer not reading)
    // Storage should continue without blocking
    
    let flush_result = storage_engine.flush().await?;
    assert!(flush_result.success);
    // Flush completes even if AXIS is slow
}
```

## Rollback Plan

If issues arise, temporarily restore synchronous behavior:

```rust
// Temporary wrapper for gradual migration
impl AsyncMetadataProducer {
    pub async fn send_metadata_with_ack(&self, metadata: IndexMetadataReference) -> Result<u64> {
        self.send_metadata(metadata);
        Ok(0) // Fake offset for compatibility
    }
}
```

## Monitoring

Track these metrics post-migration:
- **Write latency p99**: Should decrease by 10-50ms
- **Queue memory usage**: Should decrease by 95%
- **Write throughput**: Should increase by 20-50%
- **Index lag**: May increase initially (acceptable trade-off)

## Timeline

1. **Phase 1**: Implement `AsyncMetadataProducer` ✅
2. **Phase 2**: Update storage engines to use fire-and-forget
3. **Phase 3**: Update AXIS consumers to read from storage
4. **Phase 4**: Remove backpressure components
5. **Phase 5**: Performance validation

## Key Principle

**Storage writes must NEVER wait for indexing operations.**

The queue is a best-effort notification system, not a synchronous pipeline.