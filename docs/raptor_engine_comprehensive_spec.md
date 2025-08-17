# RAPTOR Storage Engine - Comprehensive Design Specification
## Row-Aligned Predicated Tensor Optimized Repository

### Version 2.0 - Updated with ProximaDB Engine Ecosystem

## Executive Summary

RAPTOR is a cloud-native storage engine designed for real-time vector search with embedded graph indexing. It combines Google Artus filesystem concepts with advanced vector database requirements, emphasizing cloud I/O efficiency, SIMD optimization, and intelligent clustering for sub-10ms latency at scale.

## Architecture Overview

### Core Design Principles

1. **Cloud-First Architecture**: Automatic tier detection and optimization for S3, GCS, Azure
2. **Row-Aligned Organization**: RowGroups for data locality with embedded HNSW segments
3. **Zero-Copy Operations**: Arrow IPC format throughout the pipeline
4. **Progressive Refinement**: Multi-phase search with clustering-based pruning
5. **Deep Integration**: Leverages ProximaDB's unified components (compression, quantization, distance)

### Storage Format

```
┌─────────────────────────────────────────────────────────┐
│                    RAPTOR File Layout                    │
├─────────────────────────────────────────────────────────┤
│  Header (8KB)                                           │
│  ├── Magic Number: "RAPT0001"                          │
│  ├── Schema (Arrow Schema)                             │
│  ├── Collection Metadata                               │
│  └── RowGroup Index                                    │
├─────────────────────────────────────────────────────────┤
│  RowGroup 0 (Default: 10K vectors)                     │
│  ├── Arrow IPC RecordBatch                             │
│  ├── Vector Column (SIMD-aligned)                      │
│  ├── Metadata Columns (complex types)                  │
│  ├── Local HNSW Graph Segment                          │
│  ├── Bloom Filter (optional)                           │
│  └── Cluster Assignments                               │
├─────────────────────────────────────────────────────────┤
│  RowGroup 1                                            │
│  └── ... (same structure)                              │
├─────────────────────────────────────────────────────────┤
│  ...                                                    │
├─────────────────────────────────────────────────────────┤
│  Global HNSW Index                                     │
│  ├── Entry Points                                      │
│  ├── Layer Information                                 │
│  └── Cross-RowGroup Links                              │
├─────────────────────────────────────────────────────────┤
│  Cluster Metadata                                      │
│  ├── Centroids                                         │
│  ├── Cluster Statistics                                │
│  └── RowGroup Mapping                                  │
└─────────────────────────────────────────────────────────┘
```

## RowGroup Architecture

### Structure
- **Size**: 10,000 vectors (configurable)
- **Format**: Arrow IPC for zero-copy operations
- **Compression**: Per-RowGroup compression with codec selection
- **Organization**: Self-contained units for parallel processing

### Components
1. **Vector Data**: SIMD-aligned float arrays
2. **Metadata**: Nested structures, maps, lists (full Arrow type system)
3. **Local HNSW**: Graph segment for this RowGroup
4. **Statistics**: Min/max, centroids, quantization error
5. **Bloom Filter**: Optional for ID lookups

## Cloud I/O Optimization

### Storage Tier Detection
```rust
StorageTier::S3Express     // Single-digit ms latency
StorageTier::S3Standard    // 10-100ms latency
StorageTier::GcsSSD        // Google Cloud SSD
StorageTier::AzurePremium  // Azure Premium SSD
StorageTier::NVMe          // Local NVMe
StorageTier::SSD           // Local SSD
StorageTier::HDD           // Local HDD
```

### Adaptive I/O Patterns
- **Range Reads**: Efficient partial file access for cloud storage
- **Optimal Block Sizes**: Per-tier I/O size optimization
- **Bandwidth Management**: Throttling and batching for cloud providers
- **Prefetching**: Predictive loading based on access patterns

## AXIS Clustering Integration

### Cluster Management
- **Algorithm**: Adaptive K-means with automatic K selection
- **Integration**: Deep integration with AXIS clustering module
- **Updates**: Incremental clustering on new data
- **Pruning**: RowGroup selection based on cluster similarity

### Search Optimization
1. Query vector → Nearest clusters identification
2. Cluster → RowGroup mapping
3. Selective RowGroup loading
4. SIMD-accelerated distance computation
5. Progressive refinement

## HNSW Graph Management

### Embedded Architecture
- **Local Graphs**: Per-RowGroup HNSW segments
- **Global Index**: Cross-RowGroup navigation
- **Compaction-Aware**: Graph preservation during compaction
- **Memory Efficient**: Lazy loading of graph segments

### Graph Operations
- **Insert**: Add to current RowGroup's graph
- **Search**: Hierarchical navigation (global → local)
- **Update**: Version-based graph updates
- **Delete**: Tombstone markers in graph

## Query Pipeline

### Phase 1: Clustering-Based Pruning
```
Query Vector → Cluster Manager → Nearest Clusters → RowGroup Selection
```

### Phase 2: Graph Navigation (if HNSW enabled)
```
Selected RowGroups → Global HNSW → Local Graph Segments → Candidates
```

### Phase 3: Progressive Refinement
```
Candidates → SIMD Distance → Filtering → Reranking → Results
```

## Comparison with Other ProximaDB Engines

### Engine Characteristics Matrix

| Feature | RAPTOR | VIPER | SST | NOVA | SWIFT | PRISM |
|---------|--------|-------|-----|------|-------|-------|
| **Storage Model** | Hybrid Row-Columnar | Pure Columnar | Row-Based | Advanced Columnar | Dual-Mode SST | Hierarchical Memory |
| **Primary Format** | Arrow IPC | Parquet | SSTable | Parquet+ | SSTable+ | Custom Tree |
| **Cloud Optimization** | Excellent | Good | Fair | Good | Fair | Excellent |
| **SIMD Support** | Native | Limited | Basic | Advanced | Good | Moderate |
| **Graph Integration** | Embedded HNSW | External | External | External | ID Index | Tree-Based |
| **Compression** | Good (2-3x) | Excellent (3-5x) | Good (2-4x) | Excellent (4-6x) | Good (2-4x) | Best (5-10x) |
| **Write Speed** | Very High | High | High | Moderate | High | Moderate |
| **Read Latency** | 5-10ms | 10-20ms | 15-25ms | 8-15ms | 10-20ms | 1-5ms |
| **Memory Usage** | High | Moderate | Low | Moderate | Low | Very High |
| **Complex Metadata** | Excellent | Limited | Basic | Good | Basic | Moderate |

### Use Case Optimization

#### RAPTOR Excels At:
- **Real-time vector search** with <10ms latency requirements
- **Cloud-native deployments** with S3/GCS/Azure backends
- **Complex metadata queries** with nested structures
- **High QPS workloads** (>1000 QPS)
- **Multi-modal search** combining vectors and metadata

#### VIPER Excels At:
- **Analytics workloads** with large aggregations
- **Maximum compression** for storage efficiency
- **Batch processing** and ETL pipelines
- **Column-wise operations** and projections
- **Historical data analysis**

#### SST Excels At:
- **Write-heavy workloads** with high ingestion rates
- **Simple key-value** access patterns
- **Low memory footprint** requirements
- **Predictable performance** with LSM tree
- **Transaction support** with WAL

#### NOVA Excels At:
- **Advanced analytics** with hierarchical statistics
- **Streaming operations** for large datasets
- **Progressive search** with zone maps
- **Quantized storage** with dual columns
- **Cost optimization** with advanced compression

#### SWIFT Excels At:
- **ID-based lookups** with O(log n) performance
- **AXIS integration** with zero-overhead vectors
- **Dual-mode operations** (ID + similarity)
- **Memory efficiency** for metadata-heavy workloads
- **Fast point queries** with B+ tree index

#### PRISM Excels At:
- **Ultra-low latency** (<1.5ms for 95% queries)
- **Memory-first** architecture
- **Hierarchical caching** (L1/L2/L3)
- **Read-heavy workloads** with hot data
- **Cost efficiency** (97% savings vs competitors)

## Performance Characteristics

### RAPTOR Performance Profile

#### Write Performance
- **Throughput**: 70K vectors/sec (streaming mode)
- **Batch Size**: Optimal at 10K vectors
- **Compression**: 2-3x reduction
- **Index Update**: Asynchronous via EventLog

#### Read Performance
- **Latency P50**: 5ms
- **Latency P95**: 8ms
- **Latency P99**: 12ms
- **QPS**: 1,200+ with caching

#### Resource Usage
- **Memory**: 4GB working set (1M vectors)
- **CPU**: SIMD-optimized, 60% reduction
- **Network**: Range reads reduce bandwidth 70%
- **Storage**: 45% compression ratio

## Implementation Details

### Core Components

#### RaptorEngine
- Implements `UnifiedStorageEngine` trait
- Manages RowGroups and compaction
- Coordinates with AXIS clustering
- Handles cloud I/O optimization

#### RowGroupManager
- Tracks RowGroup metadata
- Manages bloom filters
- Handles predicate pushdown
- Coordinates clustering assignments

#### ClusterManager
- K-means clustering implementation
- Incremental updates
- Centroid management
- RowGroup mapping

#### HnswManager
- Local graph segments
- Global index coordination
- Search orchestration
- Compaction preservation

### Integration Points

#### Filesystem API
```rust
FilesystemFactory::create(&base_path)
filesystem.read_range(path, offset, length)
```

#### AXIS Clustering
```rust
ClusterManager::cluster_vectors(&vectors)
ClusterManager::find_nearest_clusters(query, k)
```

#### Unified Components
- `UnifiedDistanceCompute`: SIMD distance calculations
- `CompressionConfig`: Per-collection compression
- `StorageQuantizationEngine`: Vector quantization

## Configuration

### RaptorConfig
```rust
pub struct RaptorConfig {
    pub rowgroup_size: usize,           // Default: 10,000
    pub compression: CompressionCodec,   // Snappy, LZ4, Zstd
    pub enable_statistics: bool,         // Row group stats
    pub enable_bloom_filters: bool,      // ID lookup optimization
    pub bloom_fpp: f64,                  // False positive probability
    pub enable_hnsw: bool,               // Graph indexing
    pub enable_simd: bool,               // SIMD acceleration
    pub cache_size_mb: usize,            // LRU cache size
    pub enable_prefetching: bool,        // Predictive loading
    pub enable_range_reads: bool,        // Cloud optimization
    pub compaction_threshold_files: usize,
}
```

## Migration Paths

### From VIPER to RAPTOR
1. Export Parquet files
2. Convert to Arrow IPC format
3. Build HNSW indices
4. Generate cluster assignments
5. Optimize for row-aligned access

### From SST to RAPTOR
1. Read SSTable blocks
2. Convert to Arrow batches
3. Organize into RowGroups
4. Build embedded indices
5. Enable cloud optimizations

### From RAPTOR to Others
- **To VIPER**: Export as Parquet with column statistics
- **To SST**: Flatten to key-value pairs
- **To NOVA**: Convert to advanced columnar format
- **To SWIFT**: Extract ID index and hierarchical blocks
- **To PRISM**: Build tree structure from vectors

## Future Enhancements

### Phase 1: Performance (Q1 2025)
- GPU acceleration for distance computation
- Adaptive compression per RowGroup
- Smart caching with ML predictions
- Distributed RowGroup processing

### Phase 2: Features (Q2 2025)
- Incremental HNSW updates
- Multi-version concurrency control
- Cross-collection clustering
- Federated search support

### Phase 3: Scale (Q3 2025)
- Sharding across nodes
- Replication for availability
- Global index coordination
- Elastic scaling

## Conclusion

RAPTOR represents a modern approach to vector storage, optimized for cloud-native deployments and real-time search. Its unique combination of row-aligned organization, embedded graph indexing, and cloud I/O optimization makes it ideal for:

1. **SaaS Applications**: Multi-tenant vector search
2. **E-commerce**: Real-time product recommendations
3. **Content Platforms**: Semantic search with rich metadata
4. **AI Applications**: RAG systems with low latency
5. **Monitoring Systems**: Time-series vector analytics

The engine's deep integration with ProximaDB's ecosystem ensures optimal resource utilization while maintaining compatibility with existing components and workflows.