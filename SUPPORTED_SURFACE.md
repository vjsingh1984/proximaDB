# ProximaDB Supported Surface

**Version**: 0.2.0  
**Last Updated**: May 1, 2026 (Wave 2 & Wave 3 updates)  
**Status**: **Production Ready** (Specific Use Cases)

## Recent Updates

### Wave 2: Graph Query Completeness (Completed May 1, 2026)
- ✅ **TD-035**: Graph Query Executor enhanced with BFS/DFS/A* algorithms
- ✅ **TD-035**: Arrow integration for vectorized processing
- ✅ **TD-035**: Cost-based query optimization with statistics
- ⏸️ **TD-046**: gRPC parity blocked by proto regeneration workflow

### Wave 3: Cache Consolidation (Completed May 1, 2026)
- ✅ **TD-042**: Unified cache interface for cross-cache operations
- ✅ **TD-042**: Coordinated eviction policies with priority system
- ✅ **TD-042**: Automated memory pressure handling

### Wave 1: Foundation Infrastructure (Completed May 1, 2026)
- ✅ **#55**: Fixed test binary cdylib issue (feature-gated approach)
- ✅ **#30**: Created feature toggle and production readiness documentation
- ✅ 48+ gap implementation tests now visible and executable

This document describes the officially supported features, APIs, and capabilities of ProximaDB v0.2.0. Only features marked as ✅ **Supported** are covered by compatibility guarantees.

## Table of Contents

- [Storage Engines](#storage-engines)
- [Query Capabilities](#query-capabilities)
- [Vector Search](#vector-search)
- [Graph Query](#graph-query)
- [Document Query](#document-query)
- [Observability](#observability)
- [API Protocols](#api-protocols)
- [Feature Matrix](#feature-matrix)

---

## Storage Engines

### ✅ SST (Sorted String Table) - **RECOMMENDED**

**Purpose**: General-purpose storage with write optimization  
**Strengths**: Vector similarity search, filtering, real-time ingestion  
**Maturity**: Most mature (253+ tests)  
**Status**: ✅ **Production Ready**

**Supported Capabilities**:
- ✅ Vector similarity search (HNSW, IVF, Annoy)
- ✅ Filtered ANN (hybrid vector + metadata search)
- ✅ Scalar and product quantization
- ✅ Predicate pushdown
- ✅ WAL recovery
- ✅ Compression (LZ4, ZSTD, Snappy)
- ✅ All standard operations (scan, filter, project, aggregate, sort)

**Performance**: 5.32ms for 10K vectors, 1.8M ops/sec write throughput  
**Best For**: Write-heavy workloads, real-time ingestion, general vector search

---

### ✅ VIPER (Parquet-based Vector Engine)

**Purpose**: Columnar analytics with Parquet integration  
**Strengths**: Batch analytics, read-optimized, Parquet ecosystem  
**Maturity**: Production (120+ tests)  
**Status**: ✅ **Production Ready**

**Supported Capabilities**:
- ✅ Vector similarity search
- ✅ Parquet format support
- ✅ Columnar storage optimization
- ✅ Predicate pushdown
- ✅ WAL recovery
- ✅ Compression
- ✅ All standard operations

**Performance**: 89.5ms for 10K vectors, optimized for analytics  
**Best For**: Analytical workloads, batch processing, Parquet ecosystem integration

---

### ✅ NOVA (Enhanced Columnar Analytics)

**Purpose**: Advanced columnar analytics with zone maps  
**Strengths**: Predicate pushdown, zone maps, advanced filtering  
**Maturity**: Production (66+ tests)  
**Status**: ✅ **Production Ready**

**Supported Capabilities**:
- ✅ Zone map pruning
- ✅ Advanced predicate pushdown
- ✅ Columnar optimizations
- ✅ WAL recovery
- ✅ Compression
- ✅ All standard operations

**Performance**: 101.6ms for 10K vectors, best for complex queries  
**Best For**: Advanced analytics, complex filtering, data warehousing

---

### ✅ HELIX (High-Dimensional Data Engine)

**Purpose**: High-dimensional vector storage with PCA reduction  
**Strengths**: Dimensionality reduction, locality optimization  
**Maturity**: Production (38+ tests)  
**Status**: ✅ **Production Ready**

**Supported Capabilities**:
- ✅ PCA dimension reduction
- ✅ Hilbert curve locality
- ✅ High-dimensional optimization (dimensions > 512)
- ✅ WAL recovery
- ✅ Compression
- ✅ All standard operations

**Performance**: 13.2ms for 10K vectors with locality optimization  
**Best For**: High-dimensional data (>512 dimensions), spatial locality

---

### ⚠️ CEDAR (Document Engine) - **NEW**

**Purpose**: MongoDB-like JSON document store with MVCC versioning  
**Strengths**: Schema-free JSON documents, concurrent access, pagination  
**Maturity**: Experimental (14 tests)  
**Status**: ⚠️ **Experimental** (Phase 1 — in-memory only)

**Supported Capabilities**:
- ✅ Document CRUD (insert, get, update, delete)
- ✅ Collection scan with limit
- ✅ Query with pagination (offset/limit)
- ✅ Document count per collection
- ✅ MVCC versioning
- ✅ Lock-free concurrent access (DashMap)
- ❌ Secondary indexes (Phase 2)
- ❌ Aggregation pipeline (Phase 2)
- ❌ Disk persistence (Phase 2)

**API**: REST `POST /api/v1/document/query`, SQL `DOCUMENT_QUERY()`, PG wire  
**Best For**: Semi-structured JSON data, flexible schemas

---

### ⚠️ CHRONO (Observability Engine) - **NEW**

**Purpose**: Native observability store for metrics, logs, and traces  
**Strengths**: Time-range queries, label matching, severity filtering  
**Maturity**: Experimental (13 tests)  
**Status**: ⚠️ **Experimental** (Phase 1 — in-memory only)

**Supported Capabilities**:
- ✅ Metric ingestion and range queries with label filters
- ✅ Log ingestion with severity and text filtering
- ✅ Trace span ingestion and trace_id lookups
- ✅ Series key tracking (distinct metric+label combos)
- ❌ Gorilla-encoded disk persistence (Phase 5)
- ❌ Time-window compaction and downsampling (Phase 5)

**API**: REST `/api/v1/logs`, REST `/api/v1/metrics`, SQL `LOGS()`, SQL `METRICS()`, PG wire  
**Best For**: Application logs, infrastructure metrics, distributed traces

---

### ⚠️ SEQUOIA (Relational Engine) - **NEW**

**Purpose**: Standard relational row-store with typed columns  
**Strengths**: Schema enforcement, SQL compatibility, complex filtering  
**Maturity**: Experimental (10+ tests)  
**Status**: ⚠️ **Experimental** (Phase 1 — in-memory only)

**Supported Capabilities**:
- ✅ DDL: CREATE TABLE, DROP TABLE with typed columns
- ✅ DML: INSERT, SELECT, UPDATE, DELETE
- ✅ Recursive boolean filters (Eq/Ne/Gt/Lt/Gte/Lte/And/Or/IsNull/IsNotNull)
- ✅ Projection (column selection)
- ✅ Multi-column ORDER BY
- ✅ LIMIT/OFFSET pagination
- ✅ Type coercion (int32 ↔ int64)
- ❌ Disk persistence (Phase 2)
- ❌ Compaction (Phase 2)

**API**: SQL via PG wire (port 5433), REST `POST /api/v1/sql`  
**Best For**: Structured data with schema enforcement, traditional SQL workloads

---

### ⚠️ TST (Time-Series Engine) - **NEW**

**Purpose**: Financial time-series with OHLC aggregation and temporal joins  
**Strengths**: Sub-millisecond OHLC queries, partition pruning, ASOF joins  
**Maturity**: Production-ready architecture (3+ unit tests, integration tests)  
**Status**: ⚠️ **Experimental** (core features complete, optimization ongoing)

**Supported Capabilities**:
- ✅ Time-partitioned columnar storage (Hour/Day/Week/Month)
- ✅ OHLC bar aggregation
- ✅ Automatic downsampling (Hour, Day, Week)
- ✅ ASOF temporal joins for trading systems
- ✅ Gorilla-like compression (>10:1 ratio)
- ✅ Time-range queries with partition pruning
- ✅ WAL support for recovery

**Performance**: <1ms OHLC queries, >100K bars/sec ingestion  
**Best For**: Financial tick data, IoT sensor time-series, OHLC analytics

---

### ⚠️ EventLog (Event Sourcing Engine) - **NEW**

**Purpose**: Append-only immutable event store with temporal queries  
**Strengths**: Event sourcing, audit trails, regulatory compliance  
**Maturity**: Experimental (4+ unit tests, integration tests)  
**Status**: ⚠️ **Experimental** (Phase 1)

**Supported Capabilities**:
- ✅ Append-only immutable events with monotonic sequence numbers
- ✅ Event indexing by entity_id, event_type, timestamp
- ✅ Snapshot management for state reconstruction
- ✅ Temporal queries (as-of, replay, point-in-time)
- ✅ Regulatory compliance mode (MiFID II)
- ✅ Causation tracking (correlation/user/request IDs)
- ✅ Configurable retention policies (7-year default)

**Best For**: Audit logs, event sourcing, regulatory compliance, financial records

---

### ⚠️ SWIFT (Hierarchical Storage) - **DEPRECATED**

**Purpose**: Three-tier hierarchical storage  
**Status**: ⚠️ **DEPRECATED** - Incomplete (30+ TODOs)  
**Feature Flag**: `experimental-engines` required  
**Maturity**: 40% complete  
**Not Recommended**: Use SST with application-level hierarchy instead

**Limitations**:
- ❌ Incomplete batch operations
- ❌ Missing SuperBlock cache optimization
- ❌ Limited hierarchical search
- ❌ No tenant isolation enforcement

---

### ⚠️ RAPTOR (Matrix Trinity Architecture) - **DEPRECATED**

**Purpose**: Adaptive workload optimization  
**Status**: ⚠️ **DEPRECATED** - Incomplete (35+ TODOs)  
**Feature Flag**: `experimental-engines` required  
**Maturity**: 35% complete  
**Not Recommended**: Use VIPER or NOVA instead

**Limitations**:
- ❌ Incomplete adaptive learning
- ❌ Missing workload pattern detection
- ❌ No centroid optimization
- ❌ Limited matrix compression

---

## Multi-Model Routing

ProximaDB uses a unified `StoreType` enum to route queries to the correct engine. Store type is detected automatically from SQL syntax:

| StoreType | Detection Signals | Engine |
|-----------|-------------------|--------|
| **Vector** | `VECTOR()` column type, `<->` / `<=>` / `<#>` operators, `USING VECTOR` | SST/HELIX/VIPER/NOVA |
| **Document** | `JSONB` column type, `$.` JSON path syntax, `doc_` prefix, `USING DOCUMENT` | CEDAR |
| **Graph** | `graph_` / `node_` / `edge_` prefix, `USING GRAPH` | ORION/PULSAR |
| **Observability** | `log_` / `metric_` / `trace_` prefix, `USING OBSERVABILITY` | CHRONO |
| **Relational** | Default (standard SQL), no special markers | SEQUOIA |
| **TimeSeries** | `USING TIMESERIES` | TST |
| **Event** | Routed via catalog lookup | EventLog |

All protocols (REST, gRPC, PostgreSQL wire, Arrow Flight) use the same detection functions, ensuring consistent routing regardless of entry point.

---

## Query Capabilities

### Vector Similarity Search

**Supported Engines**: SST, VIPER, NOVA, HELIX (all production engines)

**Features**:
- ✅ HNSW (Hierarchical Navigable Small World) - 95-99% recall
- ✅ IVF (Inverted File) - 90-95% recall
- ✅ Filtered ANN (hybrid search with metadata filters)
- ✅ Distance metrics: L2, Cosine, Inner Product, Hamming, Jaccard
- ✅ Scalar and product quantization
- ✅ Sparse vector support

**API**: REST `/api/v1/vector/search`, gRPC `VectorSearch()`, SQL `VECTOR_SEARCH()`, PostgreSQL wire `<->` operator

---

### Graph Query

**Supported Engines**: ORION (production-ready graph engine)

**Features**:
- ✅ Full Cypher query language support
- ✅ Advanced features: UNWIND, REDUCE, comprehensions
- ✅ Pattern matching
- ✅ BFS/DFS/A* traversal algorithms (TD-035: Enhanced executor)
- ✅ Edge filtering with property and weight constraints (TD-035)
- ✅ Path queries
- ✅ Property filtering
- ✅ WAL persistence
- ✅ CSR format for efficiency
- ✅ **NEW**: Arrow-native result format (TD-035 Phase 2)
- ✅ **NEW**: Cost-based query optimization (TD-035 Phase 3)
- ✅ **NEW**: Streaming support for large traversals (TD-035)

**API**: REST `/api/v1/graph/query`, gRPC `GraphQuery()`, SQL `GRAPH_QUERY()`

**Performance**: 150+ tests, production-ready for in-memory graphs

**Recent Improvements (Wave 2 - TD-035)**:
- Implemented actual traversal algorithms (BFS, DFS, A*) instead of stub implementations
- Added Arrow integration for vectorized processing and federated queries
- Implemented cost-based query optimizer with statistics and hints
- Added cross-cache invalidation and coordinated eviction (TD-042)

---

### ⚠️ Distributed Graph Query

**Supported Engines**: PULSAR (experimental)

**Status**: ⚠️ **EXPERIMENTAL** - 75% complete  
**Not Production Ready**: No distributed transactions, manual failover

**Limitations**:
- ❌ No distributed ACID guarantees
- ❌ Eventual consistency only
- ❌ No automatic failover
- ❌ High cross-shard latency (50-500ms)

**Recommendation**: Use ORION with application-level sharding for production

---

### Observability Queries

**Supported Engines**: NOVA (optimized for logs/metrics)

**Features**:
- ✅ Log aggregation and search
- ✅ PromQL-compatible metrics queries
- ✅ Time-range filtering
- ✅ Aggregation functions
- ✅ Zone map pruning

**API**: REST `/api/v1/logs`, REST `/api/v1/metrics`, SQL `LOGS()`, SQL `METRICS()`

---

## API Protocols

### REST API

**Port**: 5678 (default unified port)  
**Documentation**: `/api/docs` (OpenAPI/Swagger)

**Supported Endpoints**:
- `POST /api/v1/vector/search` - Vector similarity search
- `POST /api/v1/graph/query` - Graph queries
- `POST /api/v1/document/query` - Document queries
- `GET /api/v1/logs` - Log search
- `GET /api/v1/metrics` - Metrics query
- `POST /api/v1/sql` - SQL queries (including extensions)

---

### gRPC API

**Port**: 5678 (default unified port)  
**Protos**: `proto/proximadb/v1/proximadb.proto`

**Supported Services**:
- `VectorSearch` - Vector similarity search
- `GraphQuery` - Graph queries
- `DocumentQuery` - Document queries
- `QueryService` - SQL and federated queries

---

### PostgreSQL Wire Protocol

**Port**: 5433 (default)  
**Compatibility**: pgvector-compatible

**Features**:
- Standard SQL commands
- Vector operations: `<->` (distance), `<=>` (cosine)
- Vector search: `ORDER BY <->`
- SQL extensions: `VECTOR_SEARCH()`, `GRAPH_QUERY()`, `DOCUMENT_QUERY()`

---

### Arrow Flight

**Port**: 5680 (default)

**Features**:
- High-performance data transfer
- Columnar data format
- Integration with Arrow ecosystem
- Export via `DoGet`/file tickets
- Rich-record batch writes via `DoPut` (`insert`/`upsert`/`delete`)
- Progress-aware batch writes via `DoExchange` (`bulk_insert`/`bulk_upsert`/`bulk_delete`)

**Current limitations**:
- Arrow Flight `insert` rejects duplicate IDs in the request, but existing-record detection is not yet an atomic compare-and-insert storage primitive.
- Arrow Flight `write_mode=direct` is accepted for forward compatibility, but currently falls back to WAL-backed writes.
- Flight write handlers validate API-key/JWT metadata when the shared security coordinator is enabled, but Flight mTLS client-certificate extraction is not yet wired.

---

## Feature Matrix

### Vector & Graph Engines

| Feature | SST | VIPER | NOVA | HELIX | ORION | PULSAR |
|---------|-----|-------|-------|-------|-------|---------|
| **Data Models** |
| Vector Search | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| Graph Query | ❌ | ❌ | ❌ | ❌ | ✅ | ⚠️ |
| Observability | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ |
| **Operations** |
| Scan | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ |
| Filter | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ |
| Project | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ |
| Aggregate | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ |
| Sort | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ |
| Limit | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ |
| **Indexes** |
| HNSW | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| IVF | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| Filtered ANN | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| Sparse Vectors | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| **Advanced** |
| Predicate Pushdown | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ |
| WAL Recovery | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ |
| Quantization | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| PCA Reduction | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ |

### Multi-Model Engines (NEW in v0.2.0)

| Feature | CEDAR | CHRONO | SEQUOIA | TST | EventLog |
|---------|-------|--------|---------|-----|----------|
| **Data Model** | Document | Observability | Relational | Time-Series | Event Sourcing |
| **Core Operations** |
| Insert | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Query | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Update | ⚠️ | ❌ | ⚠️ | ❌ | ❌ (immutable) |
| Delete | ⚠️ | ❌ | ⚠️ | ❌ | ❌ (immutable) |
| **Specialized** |
| MVCC | ⚠️ | ❌ | ❌ | ❌ | ❌ |
| Schema DDL | ❌ | ❌ | ⚠️ | ❌ | ❌ |
| OHLC Aggregation | ❌ | ❌ | ❌ | ⚠️ | ❌ |
| ASOF Join | ❌ | ❌ | ❌ | ⚠️ | ❌ |
| Temporal Query | ❌ | ❌ | ❌ | ❌ | ⚠️ |
| Metrics Query | ❌ | ⚠️ | ❌ | ❌ | ❌ |
| Log Query | ❌ | ⚠️ | ❌ | ❌ | ❌ |
| Trace Query | ❌ | ⚠️ | ❌ | ❌ | ❌ |
| **Advanced** |
| Persistence | ❌ | ❌ | ❌ | ⚠️ | ❌ |
| WAL Recovery | ❌ | ❌ | ❌ | ⚠️ | ❌ |
| Compression | ❌ | ❌ | ❌ | ⚠️ | ❌ |
| Secondary Indexes | ❌ | ❌ | ❌ | ❌ | ⚠️ |

Legend: ✅ Production Ready, ⚠️ Experimental, ❌ Not Supported

---

## SQL Extensions

ProximaDB extends standard SQL with multi-model query functions:

### VECTOR_SEARCH(collection, query_vector, top_k)

Perform vector similarity search.

```sql
SELECT * FROM VECTOR_SEARCH(
    'my_collection',
    '[0.1, 0.2, 0.3]',
    10
)
```

### GRAPH_QUERY(cypher_query)

Execute Cypher graph query.

```sql
SELECT * FROM GRAPH_QUERY(
    'MATCH (n:Person)-[:KNOWS]->(f:Person) RETURN n, f'
)
```

### DOCUMENT_QUERY(collection, json_path)

Query JSON documents.

```sql
SELECT * FROM DOCUMENT_QUERY(
    'my_docs',
    '$.store.books[*].author'
)
```

### LOGS(namespace, time_range, filter)

Search log data.

```sql
SELECT * FROM LOGS(
    'production',
    '[2026-04-01T00:00:00Z, 2026-04-02T00:00:00Z]',
    'level = ERROR'
)
```

### METRICS(namespace, metric_name, time_range, aggregation)

Query metrics data.

```sql
SELECT * FROM METRICS(
    'production',
    'http_requests_total',
    '[2026-04-01T00:00:00Z, 2026-04-02T00:00:00Z]',
    'rate(5m)'
)
```

---

## Query Language Support

### ✅ SQL with Extensions (Production Ready)

**Supported Extensions**:
- `VECTOR_SEARCH(collection, query_vector, top_k)` - Vector similarity
- `GRAPH_QUERY(cypher_query)` - Graph pattern matching
- `LOGS(namespace, time_range, filter)` - Log search
- `METRICS(namespace, metric_name, time_range, aggregation)` - Metrics query

**Standard SQL**: Full SELECT, INSERT, UPDATE, DELETE, JOIN, AGGREGATE support

---

### ✅ Cypher Query Language (Production Ready)

**Supported Features**:
- ✅ MATCH, OPTIONAL MATCH, WHERE, RETURN
- ✅ CREATE, SET, DELETE, WITH
- ✅ Pattern matching and traversal
- ✅ Property filtering
- ✅ **NEW**: UNWIND (list expansion)
- ✅ **NEW**: REDUCE (list aggregation)
- ✅ **NEW**: List comprehensions
- ✅ **NEW**: Pattern comprehensions

**Performance**: Full Cypher language support, production-ready

---

## Unsupported Features

The following features are **NOT** supported in the current release:

### Distributed Operations
- ❌ Distributed transactions (no 2PC)
- ❌ Automatic failover (manual only)
- ❌ Cross-shard ACID guarantees
- ❌ Automatic shard rebalancing

### Advanced Query Features
- ❌ MultiModelPlan v1 (incomplete)
- ❌ Vectorized execution (partial)
- ❌ Adaptive query optimization (basic only)

### Multi-Model Engines (In-Memory Only)
- ⚠️ CEDAR document engine (no disk persistence yet)
- ⚠️ CHRONO observability engine (no disk persistence yet)
- ⚠️ SEQUOIA relational engine (no disk persistence yet)
- ⚠️ TST time-series engine (WAL-backed, disk persistence in progress)
- ⚠️ EventLog event sourcing (no disk persistence yet)

### Experimental/Deprecated Engines
- ❌ SWIFT hierarchical storage (deprecated)
- ❌ RAPTOR adaptive optimization (deprecated)
- ⚠️ PULSAR distributed graph (experimental, 75% complete)

---

## Capability Validation

This document is validated by CI tests on every pull request. The capability contract tests ensure:

1. **Declared = Actual**: Engines only claim capabilities they actually support
2. **No Drift**: Capabilities aren't removed without a major version bump
3. **Snapshot Consistency**: Capability snapshots match registry

To regenerate this document:

```bash
./scripts/generate_capability_snapshots.sh
```

To run validation tests:

```bash
cargo test --test capability_contract_test
```

---

## Version Compatibility

| Feature | v0.1.x | v0.2.x | v0.3.x (Planned) |
|---------|--------|--------|------------------|
| **Vector Engines** |
| SST Engine | ✅ | ✅ | ✅ |
| VIPER Engine | ✅ | ✅ | ✅ |
| NOVA Engine | ✅ | ✅ | ✅ |
| HELIX Engine | ✅ | ✅ | ✅ |
| ORION Graph | ✅ | ✅ | ✅ |
| **Multi-Model Engines** |
| CEDAR (Document) | ❌ | ⚠️ (New) | ✅ |
| CHRONO (Observability) | ❌ | ⚠️ (New) | ✅ |
| SEQUOIA (Relational) | ❌ | ⚠️ (New) | ✅ |
| TST (Time-Series) | ❌ | ⚠️ (New) | ✅ |
| EventLog (Events) | ❌ | ⚠️ (New) | ✅ |
| **Deprecated** |
| SWIFT Engine | ⚠️ | ⚠️ | ❌ (Deprecated) |
| RAPTOR Engine | ⚠️ | ⚠️ | ❌ (Deprecated) |
| PULSAR Graph | ⚠️ | ⚠️ | ⚠️ (Experimental) |
| **Query Languages** |
| SQL + Extensions | ✅ | ✅ | ✅ |
| Cypher | ✅ | ✅ (Enhanced) | ✅ |
| Unified StoreType Routing | ❌ | ✅ (New) | ✅ |
| **Advanced Features** |
| Filtered ANN | ❌ | ✅ | ✅ |
| Sparse Vectors | ❌ | ✅ | ✅ |
| UNWIND/REDUCE | ❌ | ✅ | ✅ |
| Vectorized Execution | ❌ | ⚠️ (Partial) | ✅ |

Legend: ✅ Production Ready, ⚠️ Experimental, ❌ Not Supported

**New in v0.2.0**:
- 5 new multi-model engines: CEDAR, CHRONO, SEQUOIA, TST, EventLog
- Unified `StoreType` routing across all protocols (REST, gRPC, PG wire, Arrow Flight)
- SQL extensions: `DOCUMENT_QUERY()`, `LOGS()`, `METRICS()`, `VECTOR_SEARCH()`, `GRAPH_QUERY()`
- SWIFT and RAPTOR deprecated (will be removed in v1.0)
- New Cypher features (UNWIND, REDUCE, comprehensions)
- Filtered ANN now production-ready

**Migration Guide**: See `/docs/storage/EXPERIMENTAL_ENGINES_STATUS.md`

---

## Support and Feedback

- **Issues**: https://github.com/vjsingh1984/proximaDB/issues
- **Documentation**: https://docs.proximadb.com
- **Roadmap**: See `docs/_internal/roadmap/STRATEGIC_ROADMAP.adoc`

---

*This document is auto-generated from capability snapshots. Do not edit manually. Use `./scripts/generate_capability_snapshots.sh` to regenerate.*
