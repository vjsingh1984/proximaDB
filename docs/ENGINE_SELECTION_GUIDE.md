# ProximaDB Storage Engine Selection Guide

## Overview

ProximaDB offers four storage engines, each optimized for different workloads. This guide helps you choose the right engine for your use case.

## Available Engines

### 1. VIPER (Vector-optimized Intelligent Parquet with Efficient Retrieval)
**Status**: Production Ready  
**Type**: Columnar Storage  
**Best For**: Analytics workloads, read-heavy operations

#### Characteristics:
- ✅ Excellent compression (Parquet format)
- ✅ Column projection for efficient queries
- ✅ Great for aggregations and scans
- ⚠️ Higher write latency
- ⚠️ Complex compaction process

#### When to Use:
- Analytics and reporting workloads
- Large-scale batch processing
- Historical data analysis
- When storage efficiency is critical

### 2. SST (Sorted String Table)
**Status**: Production Ready  
**Type**: Row-based Storage  
**Best For**: Write-heavy workloads, streaming data

#### Characteristics:
- ✅ Fast writes with LSM-tree structure
- ✅ Simple design and operations
- ✅ Good for streaming ingestion
- ⚠️ Less compression than columnar
- ⚠️ More I/O for analytical queries

#### When to Use:
- Real-time data ingestion
- Write-heavy applications
- Time-series data
- Simple key-value lookups

### 3. DSST (Dual-mode SST)
**Status**: R&D / Beta  
**Type**: Row-based with Zero-overhead Extensions  
**Best For**: AXIS integration, ID-based lookups

#### Characteristics:
- ✅ Zero-overhead vector storage (metadata in AXIS only)
- ✅ B+ tree ID indexing for O(log n) lookups
- ✅ Progressive search (Binary → INT8 → PQ → Full)
- ✅ Dual-mode: Index-driven and index-free
- ⚠️ Newer, less battle-tested
- ⚠️ Complex architecture

#### When to Use:
- AXIS-heavy deployments
- Memory-constrained environments
- When you need fast ID lookups after index returns
- Experimental/R&D workloads

### 4. DVIPER (Dual-mode VIPER)
**Status**: R&D / Alpha  
**Type**: Columnar with Advanced Features  
**Best For**: Advanced analytics, large-scale experiments

#### Characteristics:
- ✅ All VIPER benefits plus dual-mode operation
- ✅ Predicate pushdown to storage layer
- ✅ Column projection for minimal I/O
- ✅ Best compression ratios
- ✅ Progressive columnar search
- ⚠️ Newest engine, least tested
- ⚠️ Most complex configuration

#### When to Use:
- Advanced analytics with complex filters
- Large-scale R&D experiments
- When you need maximum query optimization
- Testing cutting-edge features

## Selection Decision Tree

```
Start → What's your primary workload?
         │
         ├─> Analytics/Read-heavy
         │    │
         │    └─> Need advanced features? 
         │         ├─> Yes → DVIPER
         │         └─> No → VIPER
         │
         ├─> Transactional/Write-heavy
         │    │
         │    └─> Using AXIS heavily?
         │         ├─> Yes → DSST
         │         └─> No → SST
         │
         └─> Mixed/Balanced
              │
              └─> Prioritize stability?
                   ├─> Yes → VIPER (default)
                   └─> No → DVIPER (experimental)
```

## Performance Comparison

| Metric | VIPER | SST | DSST | DVIPER |
|--------|-------|-----|------|--------|
| Write Latency | Medium | Low | Low | Medium |
| Read Latency | Low | Medium | Very Low* | Very Low |
| Compression | Excellent | Good | Good | Best |
| Memory Usage | Medium | Low | Very Low | Medium |
| Query Flexibility | High | Low | Medium | Highest |
| Maturity | Stable | Stable | Beta | Alpha |

*When using ID lookups after AXIS returns

## Configuration Examples

### Using the Factory

```rust
use proximadb::storage::engines::{
    StorageEngineFactory, 
    WorkloadType,
    EngineRequirements,
};

// Automatic selection based on workload
let engine = StorageEngineFactory::create_for_workload(
    WorkloadType::Analytics
)?; // Returns DVIPER

// Manual selection
let engine = StorageEngineFactory::create_from_proto(
    ProtoStorageEngine::Dsst
)?;

// Recommendation based on requirements
let requirements = EngineRequirements {
    needs_columnar: true,
    needs_compression: true,
    needs_predicate_pushdown: true,
    needs_zero_overhead: true,
    ..Default::default()
};

let recommended = StorageEngineFactory::recommend_engine(&requirements);
// Returns DVIPER
```

### Collection Configuration

```toml
# VIPER Configuration (config.toml)
[collections.analytics_data]
engine = "VIPER"
compression = "zstd"
block_size = 16777216  # 16MB

# SST Configuration
[collections.streaming_data]
engine = "SST"
flush_threshold = 1000000  # 1M records
compaction_strategy = "leveled"

# DSST Configuration
[collections.indexed_data]
engine = "DSST"
enable_progressive_search = true
quantization_levels = ["binary", "int8", "pq"]

# DVIPER Configuration
[collections.experimental_data]
engine = "DVIPER"
enable_projection = true
enable_pushdown = true
columnar_batch_size = 100000
```

## Migration Path

### From SST to DSST
1. Both use similar row-based format
2. DSST adds ID indexing and quantization
3. Migration is straightforward via flush/compact

### From VIPER to DVIPER
1. Both use Parquet columnar format
2. DVIPER adds dual-mode capabilities
3. Existing Parquet files are compatible

### Cross-Engine Migration
1. Use collection export/import
2. Or dual-write during transition
3. Verify performance before switching

## Monitoring and Metrics

Each engine exposes specific metrics:

### Common Metrics
- `flush_count`: Number of flush operations
- `compaction_count`: Number of compactions
- `bytes_written`: Total bytes written
- `bytes_read`: Total bytes read

### Engine-Specific Metrics

**VIPER/DVIPER**:
- `row_groups_scanned`: Parquet row groups accessed
- `columns_projected`: Columns selected
- `predicates_pushed`: Filters pushed to storage

**SST/DSST**:
- `bloom_filter_hits`: Bloom filter effectiveness
- `id_index_lookups`: ID-based queries
- `progressive_search_stages`: Search refinement steps

## Best Practices

### For Production
1. Use VIPER for analytics workloads
2. Use SST for write-heavy workloads
3. Monitor metrics and adjust configuration
4. Regular compaction schedules

### For R&D
1. Test DSST for AXIS-heavy workloads
2. Experiment with DVIPER for advanced features
3. Run A/B tests between engines
4. Collect detailed performance metrics

### General Guidelines
1. Start with the default (VIPER)
2. Measure your actual workload
3. Test alternatives with real data
4. Make data-driven decisions

## Troubleshooting

### High Write Latency
- Consider SST or DSST
- Tune flush thresholds
- Check compaction backlog

### High Read Latency
- Consider VIPER or DVIPER
- Enable caching layers
- Check index configuration

### High Memory Usage
- Consider DSST (zero-overhead)
- Tune cache sizes
- Monitor memory pools

### Poor Compression
- Consider DVIPER
- Tune compression algorithms
- Check data characteristics

## Future Roadmap

### Q1 2025
- DSST production readiness
- DVIPER beta release
- Cross-engine replication

### Q2 2025
- Hybrid engine (VIPER + SST)
- Automatic engine selection
- Live engine migration

### Q3 2025
- Multi-engine collections
- Engine-specific optimizations
- Advanced monitoring

## Conclusion

Choose your engine based on:
1. **Workload characteristics** (read vs write)
2. **Feature requirements** (compression, projection, etc.)
3. **Stability needs** (production vs experimental)
4. **Performance goals** (latency vs throughput)

Start with VIPER (default), measure performance, and migrate if needed. The dual-mode engines (DSST, DVIPER) offer exciting capabilities for specific use cases but should be thoroughly tested before production use.