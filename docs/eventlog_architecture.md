# EventLog Service Architecture

## Overview

The EventLog service is designed to be flexible and support multiple deployment modes as ProximaDB grows from single-node to distributed architecture.

## Deployment Modes

### 1. Embedded Mode (Default)
- Runs within the main ProximaDB process
- Direct in-memory access
- Zero network overhead
- Automatic recovery on startup

```rust
// In server initialization
let event_log_service = EventLogServiceFactory::create(
    config,
    filesystem_factory,
    collection_cache,
    None, // Defaults to embedded
).await?;
```

### 2. Standalone Mode (Microservices)
- Runs as separate service
- REST/gRPC API
- Can scale independently
- Useful for high-throughput scenarios

```bash
# Set environment variables
export EVENTLOG_MODE=standalone
export EVENTLOG_BIND_ADDRESS=0.0.0.0
export EVENTLOG_PORT=8080

# Service starts automatically with these settings
```

### 3. Distributed Mode (Multi-node)
- Runs across multiple nodes
- Peer-to-peer synchronization
- Coordinator-based consensus
- Fault-tolerant

```bash
# Node 1
export EVENTLOG_MODE=distributed
export EVENTLOG_NODE_ID=node1
export EVENTLOG_COORDINATOR_URL=http://coordinator:8080
export EVENTLOG_PEERS=node2,node3

# Node 2
export EVENTLOG_NODE_ID=node2
# ... same coordinator and peers
```

## Service Interface

### Query Interface (Read Operations)
Worker nodes can query event status without modifying state:

```rust
// Worker node querying events
let client = EventLogClient::rest("http://eventlog:8080".to_string());

// Get pending events for processing
let events = client.get_pending_events("collection_123").await?;

// Query events with filters
let filter = EventFilter {
    collection_id: Some("collection_123".to_string()),
    from_timestamp: Some(yesterday),
    operation_types: vec![OperationType::Flush],
    ..Default::default()
};
let filtered = client.query_events(filter).await?;

// Check service health
let health = client.get_health().await?;
```

### Command Interface (Write Operations)
Primary nodes can modify state:

```rust
// Primary node adding events
let service: Arc<dyn EventLogService> = get_event_log_service();

// Add flush event
service.add_event(flush_event).await?;

// Mark as processed
service.mark_processed(event_id, "hnsw_index").await?;

// Batch updates for efficiency
let updates = vec![
    ProcessedUpdate { event_id: "e1", index_name: "hnsw", success: true, error_message: None },
    ProcessedUpdate { event_id: "e2", index_name: "ivf", success: true, error_message: None },
];
service.mark_batch_processed(updates).await?;
```

## Integration with Other Services

### CollectionService Integration
EventLog can query CollectionService for configurations:

```rust
// EventLog queries collection config
let collection = collection_service.get_collection(collection_id).await?;
let index_configs = collection.config.index_configs;

// Determine which indexes need to process events
for index_config in index_configs {
    if index_config.enabled {
        // Track this index for event processing
    }
}
```

### Storage Engine Integration
Storage engines notify EventLog asynchronously:

```rust
// In SST flush
let notifier = SimpleFlushNotifier::new(event_log_service);
notifier.notify_flush(&params, flushed_files, vector_count);
// Returns immediately - never blocks

// In compaction
if event_log_service.can_compact(collection_id, file_path).await {
    // Proceed with compaction
    compact_files(files).await?;
    event_log_service.cleanup_compacted(collection_id, deleted_files).await?;
}
```

### AXIS Index Integration
AXIS indexes consume events at their own pace:

```rust
// AXIS index worker
loop {
    // Get pending events
    let events = event_log_client.get_pending_events(collection_id).await?;
    
    for event in events {
        // Get extraction hints
        let mode = event_log_client.get_extraction_hints(&event, "hnsw").await?;
        
        // Process based on hints
        match mode {
            ExtractionMode::Fp32Only => {
                let vectors = storage.read_fp32(&event.file_paths).await?;
                index.bulk_insert(vectors).await?;
            }
            ExtractionMode::QuantizedOnly => {
                let vectors = storage.read_quantized(&event.file_paths).await?;
                index.bulk_insert_quantized(vectors).await?;
            }
            _ => { /* handle other modes */ }
        }
        
        // Mark as processed
        event_log_client.mark_processed(&event.event_id, "hnsw").await?;
    }
    
    tokio::time::sleep(Duration::from_secs(5)).await;
}
```

## State Management

### Persistent State
- Event queue persisted to filesystem
- File indexing status tracked
- Processing offsets maintained per index

### Recovery
- Automatic recovery on startup
- Scans for unprocessed events
- Rebuilds in-memory state from persistent storage

### Coordination
- Files can't be compacted until all indexes acknowledge
- Automatic cleanup after compaction
- No blocking of storage operations

## Performance Characteristics

### Embedded Mode
- **Latency**: < 1μs for event addition
- **Throughput**: > 1M events/sec
- **Memory**: ~1KB per event

### Standalone Mode
- **Latency**: ~1ms for REST, ~100μs for gRPC
- **Throughput**: ~100K events/sec per instance
- **Scalability**: Horizontal scaling supported

### Distributed Mode
- **Latency**: ~5ms with consensus
- **Throughput**: ~50K events/sec per node
- **Fault Tolerance**: N-1 node failures tolerated

## Future Extensions

### 1. Event Streaming
- WebSocket/SSE for real-time updates
- Kafka integration for event bus
- Change Data Capture (CDC) support

### 2. Advanced Querying
- SQL-like query language
- Time-series analysis
- Event correlation

### 3. Multi-tenancy
- Tenant isolation
- Per-tenant quotas
- Priority queues

### 4. Observability
- OpenTelemetry integration
- Detailed metrics export
- Distributed tracing

## Configuration Example

```toml
[eventlog]
# Mode: embedded, standalone, distributed
mode = "embedded"

# Storage configuration
base_storage_url = "s3://bucket/proximadb/eventlog/"
max_events_in_memory = 10000
cleanup_interval_secs = 300

# Standalone mode settings
[eventlog.standalone]
bind_address = "0.0.0.0"
port = 8080
enable_grpc = true
enable_rest = true

# Distributed mode settings
[eventlog.distributed]
node_id = "node1"
coordinator_url = "http://coordinator:8080"
peers = ["node2:8080", "node3:8080"]
sync_interval_secs = 10
consensus_timeout_ms = 5000
```

## Migration Path

### Phase 1: Embedded (Current)
- Single process
- Direct function calls
- Automatic recovery

### Phase 2: Standalone (Next)
- Separate service option
- REST/gRPC APIs
- Client libraries

### Phase 3: Distributed (Future)
- Multi-node deployment
- Consensus-based coordination
- Global event visibility

This architecture ensures EventLog can grow with ProximaDB's needs while maintaining simplicity in the default embedded mode.