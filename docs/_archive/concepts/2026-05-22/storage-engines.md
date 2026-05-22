# Storage Engines

**6 specialized engines for different workloads**

```mermaid
%%{init: {"theme": "neutral"}}%%
flowchart TB
  subgraph Write["Write-Optimized"]
    SST[SST<br/>~5ms]
    SWIFT[SWIFT<br/>~95ms]
  end

  subgraph Read["Read-Optimized"]
    HELIX[HELIX<br/>~13ms]
    RAPTOR[RAPTOR<br/>~9ms]
  end

  subgraph Analytics["Analytics"]
    VIPER[VIPER<br/>~89ms]
    NOVA[NOVA<br/>~101ms]
  end

  SST -->|Real-time| WAL[Unified WAL]
  SWIFT -->|Ultra-fast| WAL
  HELIX -->|Locality| WAL
  RAPTOR -->|Adaptive| WAL
  VIPER -->|Columnar| WAL
  NOVA -->|Hybrid| WAL

  style SST fill:#27ae60,color:#fff
  style SWIFT fill:#27ae60,color:#fff
  style HELIX fill:#e67e22,color:#fff
  style RAPTOR fill:#e67e22,color:#fff
  style VIPER fill:#9b59b6,color:#fff
  style NOVA fill:#9b59b6,color:#fff
  style WAL fill:#3498db,color:#fff
```

---

## Overview

ProximaDB offers 6 storage engines, each optimized for specific workloads:

| Engine | Best For | Write Speed | Query Speed | Memory |
|--------|----------|-------------|-------------|--------|
| **SST** | Real-time, high-velocity | 🟢 Fastest | 🟡 Medium | Low |
| **SWIFT** | Ultra-low latency (<5K vectors) | 🟢 Fastest | 🟢 Fastest | Very Low |
| **HELIX** | Locality-optimized queries | 🟡 Medium | 🟢 Fast | Medium |
| **RAPTOR** | Adaptive, dynamic workloads | 🟢 Fast | 🟢 Fast | Adaptive |
| **VIPER** | Columnar analytics | 🟡 Medium | 🟡 Medium | High |
| **NOVA** | Mixed workloads | 🟡 Medium | 🟡 Medium | Medium |

---

## SST Engine

**Write-optimized LSM-tree with real-time compaction**

```mermaid
%%{init: {"theme": "neutral"}}%%
flowchart LR
  A[Write] --> B[Memtable]
  B --> C[Immutable Memtable]
  C --> D[SSTable L0]
  D --> E[SSTable L1]
  E --> F[SSTable L2]

  style B fill:#e74c3c,color:#fff
  style C fill:#f39c12,color:#fff
```

**Characteristics:**
- Log-Structured Merge (LSM) tree design
- LZ4 compression for SSTables
- Tiered compaction strategy
- Bloom filters for fast lookups

**Best for:**
- Real-time data ingestion
- Time-series data
- High write throughput scenarios

**Performance:**
- Writes: ~5ms (P99)
- Queries: ~10-20ms (depends on level)

**Configuration:**
```toml
[storage.engines.sst]
memtable_size_mb = 256
sstable_size_mb = 64
compression = "lz4"
bloom_filter_bits_per_key = 10
```

---

## SWIFT Engine

**Ultra-low latency for small datasets**

```mermaid
%%{init: {"theme": "neutral"}}%%
flowchart TB
  A[In-Memory Hash Index] --> B[Vector Array]
  B --> C[Metadata]

  style A fill:#e74c3c,color:#fff
```

**Characteristics:**
- Everything in memory
- No disk I/O for queries
- Simple hash index
- Lock-free reads (Arc-based)

**Best for:**
- Small datasets (<5K vectors)
- Caching layer
- Prototyping and testing

**Performance:**
- Writes: ~1ms (memory only)
- Queries: <1ms (no disk)

**Limitations:**
- Max ~5K vectors
- Data lost on crash (no WAL)
- Not for production use

---

## HELIX Engine

**Locality-optimized using Hilbert curve space-filling**

```mermaid
%%{init: {"theme": "neutral"}}%%
flowchart LR
  A[Vector] --> B[Hilbert Curve Index]
  B --> C[Sorted Storage]

  style B fill:#9b59b6,color:#fff
```

**Characteristics:**
- Hilbert curve Z-ordering
- Spatial locality preserved
- Range queries optimized
- LSM-tree base with spatial index

**Best for:**
- Spatial queries (find neighbors in region)
- Geographic data
- Time-series with spatial correlation

**Performance:**
- Writes: ~20ms (index computation overhead)
- Queries: ~13ms (excellent for range queries)

**Configuration:**
```toml
[storage.engines.helix]
hilbert_order = 16  # Precision
enable_spatial_index = true
```

---

## RAPTOR Engine

**Adaptive engine with auto-tuning**

```mermaid
%%{init: {"theme": "neutral"}}%%
flowchart TB
  subgraph Monitor["Workload Monitor"]
    W[Write Pattern]
    R[Read Pattern]
  end

  subgraph Adapt["Auto-Tuner"]
    T[Switch Strategy]
  end

  subgraph Modes["Modes"]
    M1[Write Mode]
    M2[Read Mode]
    M3[Hybrid Mode]
  end

  Monitor --> Adapt
  Adapt --> Modes

  style T fill:#e74c3c,color:#fff
```

**Characteristics:**
- Monitors access patterns
- Switches between write/read modes
- Automatic cache sizing
- Predictive pre-fetching

**Best for:**
- Unknown or varying workloads
- Multi-tenant environments
- "Set and forget" scenarios

**Performance:**
- Writes: ~5-20ms (adaptive)
- Queries: ~9ms (adaptive)
- Auto-tuning overhead: <5%

**Configuration:**
```toml
[storage.engines.raptor]
auto_tune = true
adaptation_interval_sec = 60
prefetch_enabled = true
```

---

## VIPER Engine

**Columnar Parquet storage for analytics**

```mermaid
%%{init: {"theme": "neutral"}}%%
flowchart LR
  A[Write] --> B[Row Buffer]
  B --> C[Column Convert]
  C --> D[Parquet File]

  style C fill:#9b59b6,color:#fff
  style D fill:#f39c12
```

**Characteristics:**
- Apache Parquet format
- Column-level compression (Snappy/ZSTD)
- Row-group sized for queries
- Statistics for push-down filters

**Best for:**
- Analytics workloads
- Aggregation queries
- Large scans (filter + aggregate)

**Performance:**
- Writes: ~50ms (column conversion overhead)
- Queries: ~89ms (but excellent for aggregations)
- Scans: ~100MB/sec

**Configuration:**
```toml
[storage.engines.viper]
row_group_size = 10000
compression = "zstd"
statistics_enabled = true
```

---

## NOVA Engine

**Progressive columnar with hybrid design**

```mermaid
%%{init: {"theme": "neutral"}}%%
flowchart TB
  subgraph L0["L0: Row-based"]
    R1[Recent Writes]
  end

  subgraph L1["L1: Progressive"]
    P1[Hybrid Layout]
  end

  subgraph L2["L2: Columnar"]
    C1[Analytics Ready]
  end

  L0 --> L1
  L1 --> L2

  style R1 fill:#e74c3c,color:#fff
  style P1 fill:#f39c12
  style C1 fill:#9b59b6,color:#fff
```

**Characteristics:**
- Progressive columnarization
- L0: Row-based (fast writes)
- L1: Hybrid (balanced)
- L2: Full columnar (analytics)

**Best for:**
- Mixed workloads
- Recent data + historical analytics
- Evolving access patterns

**Performance:**
- Writes: ~20ms (L0 row-based)
- Recent queries: ~15ms (L0/L1)
- Historical queries: ~100ms (L2 columnar)

**Configuration:**
```toml
[storage.engines.nova]
l0_max_size_mb = 256
l1_threshold_hours = 1
promote_to_columnar_after = "24h"
```

---

## Engine Selection Guide

### Decision Tree

```mermaid
%%{init: {"theme": "neutral"}}%%
flowchart TB
  A[Data Size] --> B{< 5K vectors?}
  B -->|Yes| SWIFT[SWIFT]
  B -->|No| C{Write frequency?}

  C -->|High writes| D{Real-time needed?}
  D -->|Yes| SST[SST]
  D -->|No| NOVA[NOVA]

  C -->|Mixed/Unknown| RAPTOR[RAPTOR]
  C -->|Read-heavy| E{Query type?}

  E -->|Point lookup| HELIX[HELIX]
  E -->|Aggregations| VIPER[VIPER]

  style SWIFT fill:#27ae60,color:#fff
  style SST fill:#27ae60,color:#fff
  style RAPTOR fill:#e74c3c,color:#fff
  style HELIX fill:#e67e22,color:#fff
  style VIPER fill:#9b59b6,color:#fff
  style NOVA fill:#f39c12
```

### Quick Reference

| If you need... | Use... |
|---------------|--------|
| Fastest writes | SST |
| Lowest latency queries | SWIFT |
| Spatial queries | HELIX |
| Auto-tuning | RAPTOR |
| Analytics | VIPER |
| Mixed workloads | NOVA |

---

## Engine Comparison

### Write Performance

```
SWIFT (1ms) < SST (5ms) < RAPTOR (5-20ms) < HELIX (20ms) < NOVA (20ms) < VIPER (50ms)
```

### Query Performance

```
SWIFT (<1ms) < RAPTOR (9ms) < HELIX (13ms) < SST (15ms) < NOVA (15-100ms) < VIPER (89ms)
```

### Memory Usage

```
SWIFT (100%) < RAPTOR (adaptive) < HELIX (medium) < NOVA (medium) < SST (low) < VIPER (high)
```

---

## Configuration

### Select Engine at Collection Creation

```python
# Python SDK
collection = client.create_collection(
    name="products",
    dimension=384,
    engine="sst"  # sst, helix, viper, swift, nova, raptor
)
```

```bash
# REST API
curl -X POST http://localhost:5678/api/v1/collections \
  -d '{
    "name": "products",
    "engine": "sst"
  }'
```

### Change Engine (Migration Required)

```python
# Export from old engine
old_collection = client.get_collection("products", engine="sst")
vectors = old_collection.export()

# Import to new engine
new_collection = client.create_collection("products_v2", engine="viper")
new_collection.import(vectors)
```

---

## Internals

### Shared Components

All engines share:
- **Unified WAL**: Single write-ahead log
- **Block Cache**: Unified caching layer
- **Metrics Collection**: Consistent observability
- **Arc-based Memory**: Zero-copy operations

### Engine Plugin System

Engines implement `UnifiedStorageEngine` trait:

```rust
pub trait UnifiedStorageEngine: Send + Sync {
    async fn insert(&self, vectors: Vec<VectorRecord>) -> Result<()>;
    async fn search(&self, query: SearchRequest) -> Result<SearchResult>;
    async fn delete(&self, ids: Vec<ID>) -> Result<()>;
    fn engine_type(&self) -> EngineType;
}
```

---

## Next Steps

- [Graph Engines](./graph-engines.md) - Graph storage internals
- [Unified WAL](./unified-wal.md) - Durability layer
- [Query Planner](./query-planner.md) - Query optimization
- [Performance Tuning](../02-guides/performance-tuning.md) - Production tuning

---

*Need help?* [GitHub Issues](https://github.com/vjsingh1984/proximadb/issues)
