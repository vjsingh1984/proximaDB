# ProximaDB Storage Engines

## Overview

ProximaDB offers multiple storage engines optimized for different workloads. The system automatically selects the best engine based on your access patterns.

## Storage Engine Comparison

| Engine | Format | Best For | Compression | Write Speed | Read Speed |
|--------|--------|----------|-------------|-------------|------------|
| **SST** | Row-based | Real-time OLTP | Medium | Fast | Fast |
| **VIPER** | Columnar Parquet | Analytics | High | Medium | Very Fast |
| **NOVA** | Hybrid Columnar | Mixed workloads | High | Fast | Fast |
| **SWIFT** | Hierarchical | High throughput | Low | Very Fast | Fast |
| **PRISM** | LSM-tree | Memory-constrained | Medium | Fast | Medium |
| **RAPTOR** | Adaptive Matrix | Hardware-optimized | Variable | Adaptive | Adaptive |

## Engine Details

### SST (Sorted String Table)
**Use Case**: Real-time queries with frequent updates

- Three-stage filtering (bloom → zone map → binary search)
- Optimized for point lookups and small range scans
- Memory-mapped I/O for fast access
- Supports MVCC for concurrent access

**Configuration**:
```toml
[storage.sst]
block_size = 4096
bloom_filter_bits = 10
compression = "lz4"
```

### VIPER (Vector-Optimized Parquet)
**Use Case**: Analytical queries, batch operations

- Columnar storage with Apache Parquet
- Advanced quantization (INT8, PQ8, PQ4)
- Optimized for sequential scans
- Superior compression ratios

**Configuration**:
```toml
[storage.viper]
row_group_size = 65536
compression = "zstd"
quantization = "pq8"
```

### NOVA (Next-gen Optimized Vector Architecture)
**Use Case**: Hybrid OLTP/OLAP workloads

- Combines row and columnar benefits
- Progressive search with early termination
- Hardware-aware optimization
- Dynamic format switching

### SWIFT (Streaming Write-optimized Index)
**Use Case**: High-velocity data ingestion

- Hierarchical block structure
- Lock-free concurrent writes
- Minimal write amplification
- Optimized for SSD/NVMe

### PRISM (Partitioned Range-Index Storage)
**Use Case**: Memory-constrained environments

- Efficient LSM-tree implementation
- Tiered compaction strategy
- Low memory footprint
- Good for edge deployments

### RAPTOR (Rapid Adaptive Performance-Tuned Organization)
**Use Case**: Self-optimizing storage

- Learns access patterns
- Adapts storage layout dynamically
- Hardware-aware (cache sizes, SIMD)
- Matrix-based organization

## Selection Strategy

ProximaDB automatically selects engines based on:

1. **Workload Pattern**
   - OLTP → SST
   - OLAP → VIPER
   - Mixed → NOVA

2. **Data Characteristics**
   - High cardinality → VIPER
   - Frequent updates → SST
   - Streaming → SWIFT

3. **Hardware Resources**
   - Limited memory → PRISM
   - GPU available → RAPTOR
   - NVMe SSD → SWIFT

## Performance Tuning

### Write Optimization
```toml
# For high write throughput
[storage]
default_engine = "swift"
flush_threshold_mb = 256
compaction_threads = 4
```

### Read Optimization
```toml
# For fast queries
[storage]
default_engine = "viper"
cache_size_mb = 8192
prefetch_enabled = true
```

### Balanced Workload
```toml
# For mixed operations
[storage]
default_engine = "nova"
adaptive_selection = true
```

## Compression Options

All engines support multiple compression algorithms:

- **LZ4**: Fastest, moderate compression
- **Snappy**: Fast, good for real-time
- **ZSTD**: Best compression ratio
- **None**: No compression, maximum speed

## Migration Between Engines

```sql
-- Change engine for existing collection
ALTER COLLECTION products SET ENGINE = 'viper';

-- Compact and optimize
OPTIMIZE COLLECTION products;
```

## Monitoring

Track engine performance:

```sql
-- View engine statistics
SELECT * FROM system.storage_engines;

-- Check compression ratios
SELECT engine, compression_ratio, bytes_written 
FROM system.engine_stats
WHERE collection = 'products';
```