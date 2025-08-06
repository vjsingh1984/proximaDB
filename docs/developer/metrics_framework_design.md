# ProximaDB Metrics Framework Design

## Overview

A comprehensive metrics system providing operational insights and query optimization hints while maintaining system stability through non-critical path design.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     External REST API                        │
│                   GET /metrics (read-only)                   │
│              GET /metrics/{collection_id}                    │
│           GET /metrics/query-hints/{collection_id}           │
└────────────────────────┬────────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────┐
│                  MetricsQueryService                        │
│              (Read-only interface)                          │
│         - Serves aggregated metrics                         │
│         - Caches hot data in memory (LRU)                   │
│         - Provides query optimization hints                 │
└────────────────────────┬────────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────┐
│              PersistentMetricsStore                         │
│         (Partitioned by collection_id)                      │
│    - Uses FilesystemFactory for cross-cloud                 │
│    - Periodic snapshots (configurable)                      │
│    - Efficient binary format (Bincode + zstd)               │
│    - Automatic retention management                         │
└────────────────────────▲────────────────────────────────────┘
                         │
┌────────────────────────┴────────────────────────────────────┐
│           InternalMetricsUpdater                            │
│         (Internal-only interface)                           │
│    - Called by FlushManager                                 │
│    - Called by CompactionManager                            │
│    - Called by DirectVectorService                          │
│    - Non-blocking, async updates                            │
│    - Failure-tolerant (never blocks operations)             │
└──────────────────────────────────────────────────────────────┘
```

## Key Design Decisions

### 1. Configuration Parameters

| Parameter | Default | Min | Max | Purpose |
|-----------|---------|-----|-----|---------|
| `snapshot_interval_seconds` | 1800 (30min) | 300 (5min) | - | Balance between data freshness and I/O |
| `retention_days` | 7 | 1 | 30 | Historical data for trend analysis |
| `max_memory_mb` | 100 | 10 | 1024 | Memory budget for metrics cache |
| `parallel_scan_threshold` | 10 files | - | - | Trigger parallel query execution |
| `sparsity_threshold` | 30% | - | - | Identify sparse vectors for optimization |
| `quantization_size_threshold` | 100MB | - | - | Suggest quantization for large collections |

### 2. Metrics Schema

```rust
pub struct CollectionMetrics {
    // Basic Statistics
    pub vector_count: i64,
    pub dimension: i32,
    pub index_size_bytes: i64,
    pub data_size_bytes: i64,
    
    // Operation Counts
    pub total_inserts: i64,
    pub total_updates: i64,
    pub total_deletes: i64,
    pub total_searches: i64,
    
    // Performance Metrics
    pub avg_insert_latency_us: f64,
    pub avg_search_latency_us: f64,
    pub p50_search_latency_us: f64,
    pub p95_search_latency_us: f64,
    pub p99_search_latency_us: f64,
    
    // Storage Metrics
    pub parquet_file_count: i32,      // VIPER engine
    pub sstable_file_count: i32,      // SST engine
    pub wal_size_bytes: i64,
    pub memtable_size_bytes: i64,
    pub total_flush_count: i64,
    pub total_compaction_count: i64,
    pub last_flush_timestamp: i64,
    pub last_compaction_timestamp: i64,
    
    // Data Characteristics (for optimization)
    pub sparsity_ratio: f32,          // % of zero/null dimensions
    pub avg_vector_magnitude: f32,    // For normalization decisions
    pub distinct_metadata_keys: i32,   // Number of unique metadata fields
    pub filterable_column_cardinality: HashMap<String, i64>, // Selectivity per column
    
    // Index Characteristics
    pub index_types: Vec<String>,     // Available indexes
    pub primary_index: String,
    pub index_build_progress: f32,    // % complete for building indexes
    pub bloom_filter_size_bytes: i64, // SST bloom filter size
    
    // Query Optimization Hints
    pub estimated_scan_cost: f64,
    pub index_selectivity: f64,
    pub cache_hit_ratio: f64,
    pub suggested_batch_size: i32,
    pub quantization_benefit_score: f32, // 0-1, higher = more benefit
}
```

### 3. Query Optimization Hints

The metrics system provides actionable hints for query optimization:

#### Parallel Scan Optimization
```rust
if metrics.parquet_file_count > config.parallel_scan_threshold {
    hint: "Enable parallel scan for {collection} with {n} workers"
}
```

#### Sparsity-Based Optimization
```rust
if metrics.sparsity_ratio > config.sparsity_threshold {
    hint: "Collection {collection} is {sparsity}% sparse - consider:
           1. Sparse vector format for storage
           2. Skip zero-value dimensions in distance calculations
           3. Reduced compression may improve performance"
}
```

#### Quantization Recommendations
```rust
if metrics.data_size_bytes > config.quantization_size_threshold 
   && metrics.quantization_benefit_score > 0.7 {
    hint: "Collection {collection} would benefit from quantization:
           - Current size: {size}GB
           - Estimated reduction: {reduction}%
           - Suggested method: {method} (PQ/SQ/BQ)"
}
```

#### Filterable Column Optimization
```rust
for (column, cardinality) in metrics.filterable_column_cardinality {
    selectivity = cardinality / metrics.vector_count
    if selectivity < 0.1 {  // High selectivity
        hint: "Column {column} has high selectivity ({selectivity}) - 
               ideal for predicate pushdown in VIPER engine"
    }
}
```

#### Index Selection
```rust
if query.filter_count > 0 && metrics.bloom_filter_size_bytes > 0 {
    hint: "Use SST engine for filtered queries - bloom filters available"
} else if query.requires_exact && metrics.index_types.contains("FLAT") {
    hint: "Use FLAT index for exact results"
} else if query.k > 100 && metrics.index_types.contains("IVF") {
    hint: "Use IVF index for large k={k} queries"
}
```

### 4. Storage Layout

```
/metrics/
├── snapshots/
│   ├── global/
│   │   ├── snapshot_2025_01_15_1200.bincode  # Hourly aggregates
│   │   └── snapshot_2025_01_15.bincode       # Daily aggregates
│   └── collections/
│       ├── {collection_id}/
│       │   ├── snapshot_latest.bincode       # Latest snapshot
│       │   ├── snapshot_2025_01_15_1200.bincode
│       │   └── history/
│       │       └── snapshot_2025_01_14_1200.bincode
│       └── _index.json                    # Collection ID mapping
└── incremental/
    └── {collection_id}/
        └── updates_{timestamp}.bincode       # Pending updates
```

### 5. Integration Points

#### FlushManager Integration
```rust
// After successful flush
metrics_updater.update(CollectionMetricsUpdate {
    collection_id,
    vectors_flushed: result.entries_flushed,
    bytes_written: result.bytes_written,
    flush_duration_ms: result.duration_ms,
    parquet_files_created: result.files_created,
    timestamp: Utc::now(),
}).await; // Non-blocking, fire-and-forget
```

#### CompactionManager Integration
```rust
// After successful compaction
metrics_updater.update(CollectionMetricsUpdate {
    collection_id,
    files_before: input_files.len(),
    files_after: output_files.len(),
    bytes_saved: bytes_before - bytes_after,
    compaction_duration_ms: duration.as_millis(),
}).await;
```

#### DirectVectorService Integration
```rust
// Track operations with minimal overhead
let start = Instant::now();
let result = insert_vector(...).await?;
metrics_updater.record_operation(
    collection_id,
    OperationType::Insert,
    start.elapsed(),
    result.is_ok()
).await;
```

### 6. REST API Endpoints

#### Global Metrics
```
GET /metrics
Response: {
    "global": {
        "total_collections": 42,
        "total_vectors": 1000000,
        "total_storage_bytes": 5368709120,
        "uptime_seconds": 86400,
        "operations_per_second": 1500
    },
    "collections": [...]  // Summary only
}
```

#### Collection-Specific Metrics
```
GET /metrics/{collection_id}
Response: {
    "collection_id": "products",
    "metrics": { ... },  // Full CollectionMetrics
    "query_hints": { ... },
    "last_updated": "2025-01-15T12:00:00Z"
}
```

#### Query Optimization Hints
```
GET /metrics/query-hints/{collection_id}?query_type=search&k=100
Response: {
    "collection_id": "products",
    "hints": [
        {
            "type": "index_selection",
            "recommendation": "Use IVF index for k=100",
            "estimated_latency_ms": 15,
            "confidence": 0.9
        },
        {
            "type": "parallelization",
            "recommendation": "Enable 4-way parallel scan",
            "reason": "24 parquet files exceed threshold",
            "estimated_speedup": 3.2
        }
    ]
}
```

### 7. Failure Handling

The metrics system is designed to never impact core operations:

```rust
// All metrics operations are wrapped
async fn safe_update(&self, update: MetricsUpdate) {
    if let Err(e) = self.try_update(update).await {
        // Log but don't propagate
        tracing::debug!("Metrics update failed (non-critical): {}", e);
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }
}
```

### 8. Performance Considerations

- **Memory Usage**: ~1KB per collection for hot metrics, bounded by LRU cache
- **Write Overhead**: <100μs per operation (async, non-blocking)
- **Query Latency**: <1ms for cached metrics, <10ms for cold reads
- **Storage Overhead**: ~10KB per collection per snapshot
- **Network Traffic**: Snapshots use compression, ~80% reduction

### 9. Future Enhancements

1. **Machine Learning Integration**: Use metrics to train index selection models
2. **Anomaly Detection**: Alert on unusual patterns (spike in latency, data drift)
3. **Cost Estimation**: Predict query costs based on historical metrics
4. **Auto-Tuning**: Automatically adjust parameters based on workload patterns
5. **Multi-Tenant Isolation**: Per-tenant metrics quotas and isolation

## Implementation Phases

### Phase 1: Core Infrastructure (Week 1)
- [x] Design document
- [ ] PersistentMetricsStore with FilesystemFactory
- [ ] InternalMetricsUpdater trait and implementation
- [ ] Basic collection-partitioned storage

### Phase 2: Integration Points (Week 1-2)
- [ ] FlushManager integration
- [ ] CompactionManager integration
- [ ] DirectVectorService hooks
- [ ] Non-blocking update pipeline

### Phase 3: REST API (Week 2)
- [ ] /metrics endpoint enhancement
- [ ] /metrics/{collection_id} endpoint
- [ ] /metrics/query-hints endpoint
- [ ] Response caching layer

### Phase 4: Query Optimization (Week 2-3)
- [ ] Sparsity detection
- [ ] Parallel scan recommendations
- [ ] Quantization scoring
- [ ] Index selection hints
- [ ] Filterable column analysis

### Phase 5: Aggregations & Analytics (Week 3)
- [ ] Hourly/daily aggregations
- [ ] Trend analysis
- [ ] Anomaly detection (basic)
- [ ] Performance regression alerts