# AXIS Queue-Based Integration Implementation Plan

## Executive Summary
Refactor AXIS integration from synchronous write-time indexing to asynchronous flush-time queue-based indexing. This eliminates memory duplication, reduces write latency, and enables quantization reuse.

## Architecture Overview

### Current (Problematic) Flow
```
Write → WAL → AXIS.insert(FP32) → Memtable → Flush → Storage
                ↑
        [BLOCKING, DUPLICATES MEMORY]
```

### New (Optimized) Flow
```
Write → WAL → Memtable → Flush → Storage + Queue → AXIS.process(Quantized)
                                      ↓
                              [ASYNC, REUSES QUANTIZATION]
```

## Phase 1: Queue Infrastructure (Week 1)

### 1.1 Create Kafka-like Commit Log
```rust
// src/index/axis/queue/commit_log.rs
pub struct CommitLog {
    segments: Vec<LogSegment>,
    active_segment: Arc<RwLock<LogSegment>>,
    base_dir: PathBuf,
    max_segment_size: u64,  // 1GB default
}

pub struct LogSegment {
    id: u64,                // Monotonic segment ID
    file: MmapMut,          // Memory-mapped for zero-copy
    index: OffsetIndex,     // Sparse index for seeking
    start_offset: u64,
    end_offset: AtomicU64,
}

pub struct LogEntry {
    offset: u64,
    timestamp: u64,
    collection_id: String,  // Partition key
    payload: IndexPayload,
    checksum: u32,
}
```

### 1.2 Producer Interface
```rust
// src/index/axis/queue/producer.rs
pub struct QueueProducer {
    commit_log: Arc<CommitLog>,
    buffer: Vec<LogEntry>,  // Batch before writing
    batch_size: usize,
}

impl QueueProducer {
    pub async fn send(&mut self, collection_id: String, payload: IndexPayload) -> Result<u64> {
        let entry = LogEntry::new(collection_id, payload);
        self.buffer.push(entry);
        
        if self.buffer.len() >= self.batch_size {
            self.flush().await?;
        }
        
        Ok(entry.offset)
    }
    
    pub async fn flush(&mut self) -> Result<()> {
        // Zero-copy write using mmap
        self.commit_log.append_batch(&self.buffer).await?;
        self.buffer.clear();
        Ok(())
    }
}
```

### 1.3 Consumer Framework
```rust
// src/index/axis/queue/consumer.rs
pub struct QueueConsumer {
    commit_log: Arc<CommitLog>,
    consumer_group: String,
    last_offset: u64,
    ack_manager: AckManager,
}

impl QueueConsumer {
    pub async fn poll(&mut self, timeout: Duration) -> Result<Vec<LogEntry>> {
        let entries = self.commit_log.read_from(self.last_offset, 100).await?;
        Ok(entries)
    }
    
    pub async fn acknowledge(&mut self, offset: u64) -> Result<()> {
        self.ack_manager.ack(offset).await?;
        self.last_offset = offset + 1;
        Ok(())
    }
}
```

## Phase 2: Flush Integration (Week 2)

### 2.1 Create FlushAxisUpdater
```rust
// src/index/axis/flush_integration.rs
pub struct FlushAxisUpdater {
    producer: QueueProducer,
    shared_collection_cache: Arc<DashMap<String, Arc<Collection>>>,
}

impl FlushAxisUpdater {
    pub async fn queue_flush_updates(
        &mut self,
        params: &FlushParameters,
        records: &[VectorRecord],
    ) -> Result<()> {
        let collection = self.shared_collection_cache.get(&params.collection_id)?;
        
        // Determine what to send based on collection config
        let payload = self.prepare_payload(&collection, records)?;
        
        // Send to queue
        self.producer.send(params.collection_id.clone(), payload).await?;
        
        Ok(())
    }
    
    fn prepare_payload(&self, collection: &Collection, records: &[VectorRecord]) -> Result<IndexPayload> {
        if !collection.has_indexes() {
            return Ok(IndexPayload::None);
        }
        
        let config = &collection.config.quantization_config;
        
        if config.enabled {
            // Send quantized vectors (most common case)
            Ok(IndexPayload::Quantized {
                vectors: records.iter()
                    .map(|r| (r.id.clone(), r.quantized.clone()))
                    .collect(),
            })
        } else {
            // Send FP32 vectors
            Ok(IndexPayload::Fp32 {
                vectors: records.iter()
                    .map(|r| (r.id.clone(), r.vector.clone()))
                    .collect(),
            })
        }
    }
}
```

### 2.2 Modify Storage Engines
```rust
// src/storage/engines/sst/mod.rs
impl UnifiedStorageEngine for SSTEngine {
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        // Existing flush logic
        let result = self.flush_sst_records_to_sstable(...).await?;
        
        // NEW: Queue to AXIS (instead of direct insert)
        if let Some(ref mut updater) = self.flush_axis_updater {
            updater.queue_flush_updates(params, &sst_records).await?;
        }
        
        Ok(result)
    }
}

// Similar change for VIPER engine
```

### 2.3 Remove Write-time Indexing
```rust
// src/storage/engine.rs
pub async fn write(&self, collection_id: &str, record: &VectorRecord) -> Result<()> {
    // Write to WAL
    self.write_ahead_log_manager.write_vector_batch_native_arc(...).await?;
    
    // REMOVED: self.axis_index_manager.insert(collection_id, record).await?;
    // Indexing now happens asynchronously after flush
    
    Ok(())
}
```

## Phase 3: Index Configuration (Week 3)

### 3.1 Update Proto Definitions
```proto
// proto/proximadb.proto
message IndexConfig {
    IndexType type = 1;  // HNSW, IVF, LSH
    
    // Representation configuration
    IndexRepresentation representation = 2;
    
    // Optional overrides
    optional DistanceMetric distance_override = 3;
    optional QuantizationConfig quantization_override = 4;
    
    // Index-specific parameters
    map<string, string> parameters = 5;
}

enum IndexRepresentation {
    AUTO = 0;        // Use collection's quantization config
    QUANTIZED = 1;   // Only quantized vectors
    FP32 = 2;        // Only full precision
    BOTH = 3;        // Keep both representations
}
```

### 3.2 Update AXIS Manager
```rust
// src/index/axis/manager.rs
impl AxisManager {
    async fn process_queue_entry(&self, entry: LogEntry) -> Result<()> {
        let collection_id = &entry.collection_id;
        let indexes = self.get_collection_indexes(collection_id).await?;
        
        for (index_name, index_config) in indexes {
            match (&entry.payload, &index_config.representation) {
                (IndexPayload::Quantized { vectors }, IndexRepresentation::Quantized) |
                (IndexPayload::Quantized { vectors }, IndexRepresentation::Auto) => {
                    // Use quantized vectors directly
                    for (id, quantized) in vectors {
                        index.insert_quantized(id, quantized).await?;
                    }
                }
                
                (IndexPayload::Fp32 { vectors }, IndexRepresentation::Fp32) => {
                    // Use FP32 vectors
                    for (id, vector) in vectors {
                        index.insert_fp32(id, vector).await?;
                    }
                }
                
                (IndexPayload::Both { vectors }, IndexRepresentation::Both) => {
                    // Index keeps both representations
                    for (id, fp32, quantized) in vectors {
                        index.insert_both(id, fp32, quantized).await?;
                    }
                }
                
                _ => {
                    // Representation mismatch - need conversion
                    self.convert_and_index(entry, index_config).await?;
                }
            }
        }
        
        Ok(())
    }
}
```

### 3.3 Update Index Implementations
```rust
// src/index/hnsw.rs
impl HNSWIndex {
    // Support different representations
    pub async fn insert_quantized(&mut self, id: String, quantized: Vec<u8>) -> Result<()> {
        // Use quantized distance computation
        self.quantized_graph.insert(id, quantized).await
    }
    
    pub async fn insert_fp32(&mut self, id: String, vector: Vec<f32>) -> Result<()> {
        // Use FP32 distance computation
        self.fp32_graph.insert(id, vector).await
    }
    
    pub async fn insert_both(&mut self, id: String, fp32: Vec<f32>, quantized: Vec<u8>) -> Result<()> {
        // Keep both for different search modes
        self.fp32_graph.insert(id.clone(), fp32).await?;
        self.quantized_graph.insert(id, quantized).await
    }
}

// Similar updates for IVF and LSH indexes
```

## Phase 4: Production Hardening (Week 4)

### 4.1 Add Backpressure
```rust
impl QueueProducer {
    pub async fn send_with_backpressure(&mut self, entry: LogEntry) -> Result<()> {
        // Wait if queue is too full
        while self.commit_log.size() > MAX_QUEUE_SIZE {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        
        self.send(entry).await
    }
}
```

### 4.2 Add Monitoring
```rust
pub struct QueueMetrics {
    pub queue_depth: AtomicU64,
    pub consumer_lag: AtomicU64,
    pub throughput: AtomicU64,
    pub failed_entries: AtomicU64,
}
```

### 4.3 Recovery Mechanism
```rust
impl AxisManager {
    pub async fn recover_from_queue(&self) -> Result<()> {
        // On startup, find last acknowledged offset
        let last_ack = self.load_last_acknowledgment()?;
        
        // Replay from that point
        let mut consumer = QueueConsumer::new(last_ack);
        
        while let Some(entries) = consumer.poll(Duration::from_secs(1)).await? {
            for entry in entries {
                self.process_queue_entry(entry).await?;
                consumer.acknowledge(entry.offset).await?;
            }
        }
        
        Ok(())
    }
}
```

## Configuration Examples

### Collection with Quantization (Most Common)
```yaml
collection:
  name: products
  quantization_config:
    enabled: true
    method: PRODUCT_QUANTIZATION
  
  index_config:
    - type: HNSW
      representation: AUTO  # Uses quantized
    - type: IVF
      representation: AUTO  # Uses quantized
```

### High-Precision Collection
```yaml
collection:
  name: financial_data
  quantization_config:
    enabled: false  # No quantization
  
  index_config:
    - type: HNSW
      representation: FP32  # Full precision only
```

### Mixed Requirements
```yaml
collection:
  name: scientific_data
  quantization_config:
    enabled: true
    method: PRODUCT_QUANTIZATION
  
  index_config:
    - type: HNSW
      representation: BOTH  # Keeps both for different query types
    - type: IVF
      representation: QUANTIZED  # Space-optimized
      distance_override: COSINE  # Override collection's distance
```

## Testing Strategy

### Unit Tests
- CommitLog segment rotation
- Producer batching and flushing
- Consumer offset management
- Payload preparation logic

### Integration Tests
- End-to-end flush to index flow
- Recovery after crash
- Backpressure handling
- Mixed representation handling

### Performance Tests
- Queue throughput benchmarks
- Zero-copy verification
- Memory usage with quantization
- Index query performance comparison

## Migration Path

1. **Deploy Queue Infrastructure** - No behavior change
2. **Add FlushAxisUpdater** - Still using old path
3. **Switch to Queue Path** - Route flush through queue
4. **Remove Old Path** - Clean up write-time indexing
5. **Enable Advanced Features** - Backpressure, monitoring

## Success Metrics

- **Write Latency**: 50% reduction (no index blocking)
- **Memory Usage**: 60% reduction (quantized vectors only)
- **Index Build Time**: 3x faster (batch processing)
- **Storage I/O**: 70% reduction (quantization reuse)
- **Query Performance**: Same or better (progressive search)

## Risk Mitigation

- **Data Loss**: WAL-backed queue, acknowledgments
- **Performance Regression**: A/B testing, gradual rollout
- **Compatibility**: Support both paths initially
- **Complexity**: Reuse existing patterns (CompactionAxisUpdater)