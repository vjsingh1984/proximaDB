# ProximaDB Supported Surface

**Version**: 0.2.0  
**Last Updated**: April 2, 2026  
**Status**: Foundation Phase - 55% Complete

This document describes the officially supported features, APIs, and capabilities of ProximaDB. It is auto-generated from the capability registry and validated by CI tests.

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

### SST (Scalar Sorted Table)

**Purpose**: High-performance analytical queries on tabular data  
**Strengths**: Vector similarity search, filtering, indexing  
**Limitations**: No graph traversal, no document queries

**Supported Capabilities**:
- ✅ Scan
- ✅ Filter
- ✅ Project
- ✅ Aggregate
- ✅ Sort
- ✅ Limit
- ✅ VectorSearch
- ✅ HNSWIndex
- ✅ IVFIndex
- ✅ PredicatePushdown
- ✅ Quantization (Scalar, Product)
- ✅ WALRecovery

**Best For**: Vector similarity search, analytical queries, filtering

---

### VIPER (Versatile Indexing and PRocessing Engine)

**Purpose**: Graph-native storage and traversal  
**Strengths**: Graph queries, pattern matching, traversal  
**Limitations**: No vector search, no document queries

**Supported Capabilities**:
- ✅ Scan
- ✅ Filter
- ✅ Project
- ✅ Aggregate
- ✅ Sort
- ✅ Limit
- ✅ GraphQuery
- ✅ GraphTraversal
- ✅ PredicatePushdown

**Best For**: Graph databases, social networks, knowledge graphs

---

### HELIX (Hierarchical Lexical Index for XML)

**Purpose**: Document storage and search  
**Strengths**: JSON/XML document queries, full-text search  
**Limitations**: No vector search, no graph traversal

**Supported Capabilities**:
- ✅ Scan
- ✅ Filter
- ✅ Project
- ✅ Aggregate
- ✅ Sort
- ✅ Limit
- ✅ DocumentQuery
- ✅ FullTextSearch
- ✅ PredicatePushdown

**Best For**: Document databases, content management, JSON data

---

### NOVA (Observability Native Vector Architecture)

**Purpose**: Logs, metrics, and traces storage  
**Strengths**: Time-series data, observability queries  
**Limitations**: No vector search, no graph traversal

**Supported Capabilities**:
- ✅ Scan
- ✅ Filter
- ✅ Project
- ✅ Aggregate
- ✅ Sort
- ✅ Limit
- ✅ LogQuery
- ✅ MetricsQuery
- ✅ TimeRangeQuery
- ✅ PredicatePushdown

**Best For**: Observability, monitoring, time-series analytics

---

### SWIFT (Structured Write Indexed File Format) ⚠️ Experimental

**Purpose**: Graph-native storage with distributed features  
**Status**: **EXPERIMENTAL** - Not recommended for production  
**Limitations**: Incomplete implementation, use SST or NOVA instead

**Supported Capabilities**:
- ✅ Scan
- ✅ Filter
- ✅ GraphQuery (Partial)
- ✅ GraphTraversal (Partial)

**Best For**: Testing only - use VIPER for production graph workloads

---

### RAPTOR (Real-time Analytics Processing Time-series Optimized Router) ⚠️ Experimental

**Purpose**: Time-series and metrics storage  
**Status**: **EXPERIMENTAL** - Not recommended for production  
**Limitations**: Incomplete implementation, use NOVA instead

**Supported Capabilities**:
- ✅ Scan
- ✅ Filter
- ✅ MetricsQuery (Partial)
- ✅ TimeRangeQuery (Partial)

**Best For**: Testing only - use NOVA for production observability

---

## Query Capabilities

### Vector Search

**Supported Engines**: SST

**Features**:
- HNSW (Hierarchical Navigable Small World) index
- IVF (Inverted File) index
- Annoy index
- LSH (Locality-Sensitive Hashing) index
- Distance metrics: L2, Cosine, Dot Product
- Scalar quantization
- Product quantization
- Predicate pushdown with filters

**API**: REST `/api/v1/vector/search`, gRPC `VectorSearch()`, SQL `VECTOR_SEARCH()`

---

### Graph Query

**Supported Engines**: VIPER

**Features**:
- Cypher query language
- Pattern matching
- Node and relationship traversal
- Path queries
- Property filtering

**API**: REST `/api/v1/graph/query`, gRPC `GraphQuery()`, SQL `GRAPH_QUERY()`

---

### Document Query

**Supported Engines**: HELIX

**Features**:
- JSONPath queries
- Full-text search
- Document filtering
- Nested document access

**API**: REST `/api/v1/document/query`, gRPC `DocumentQuery()`, SQL `DOCUMENT_QUERY()`

---

### Observability Queries

**Supported Engines**: NOVA

**Features**:
- Log aggregation and search
- PromQL-compatible metrics queries
- Time-range filtering
- Aggregation functions

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

---

## Feature Matrix

| Feature | SST | VIPER | HELIX | NOVA |
|---------|-----|-------|-------|------|
| **Data Models** |
| Vector Search | ✅ | ❌ | ❌ | ❌ |
| Graph Query | ❌ | ✅ | ❌ | ❌ |
| Document Query | ❌ | ❌ | ✅ | ❌ |
| Observability | ❌ | ❌ | ❌ | ✅ |
| **Operations** |
| Scan | ✅ | ✅ | ✅ | ✅ |
| Filter | ✅ | ✅ | ✅ | ✅ |
| Project | ✅ | ✅ | ✅ | ✅ |
| Aggregate | ✅ | ✅ | ✅ | ✅ |
| Sort | ✅ | ✅ | ✅ | ✅ |
| Limit | ✅ | ✅ | ✅ | ✅ |
| **Indexes** |
| HNSW | ✅ | ❌ | ❌ | ❌ |
| IVF | ✅ | ❌ | ❌ | ❌ |
| Annoy | ✅ | ❌ | ❌ | ❌ |
| Full-Text | ❌ | ❌ | ✅ | ❌ |
| **Advanced** |
| Predicate Pushdown | ✅ | ✅ | ✅ | ✅ |
| WAL Recovery | ✅ | ✅ | ✅ | ✅ |
| Quantization | ✅ | ❌ | ❌ | ❌ |

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

## Unsupported Features

The following features are **NOT** supported in the current release:

### Cross-Model Operations
- ❌ Multi-engine transactions
- ❌ Cross-model joins (planned)
- ❌ Federated queries across engines (planned)

### Advanced Indexing
- ❌ Hybrid vector+graph indexes (planned)
- ❌ Filtered vector search (planned for Phase 3)
- ❌ Sparse vector indexes (planned)

### Distributed Features
- ❌ Distributed query execution (experimental)
- ❌ Cross-node transactions (experimental)
- ❌ Automatic sharding (planned)

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

| Feature | v0.1.x | v0.2.x | v0.3.x |
|---------|--------|--------|--------|
| SST Engine | ✅ | ✅ | ✅ |
| VIPER Engine | ✅ | ✅ | ✅ |
| HELIX Engine | ✅ | ✅ | ✅ |
| NOVA Engine | ⚠️ | ✅ | ✅ |
| SWIFT Engine | ❌ | ⚠️ | ⚠️ |
| RAPTOR Engine | ❌ | ⚠️ | ⚠️ |
| Vector Search | ✅ | ✅ | ✅ |
| Graph Query | ✅ | ✅ | ✅ |
| Document Query | ✅ | ✅ | ✅ |
| Observability | ❌ | ✅ | ✅ |

Legend: ✅ Supported, ⚠️ Experimental, ❌ Not Supported

---

## Support and Feedback

- **Issues**: https://github.com/vjsingh1984/proximaDB/issues
- **Documentation**: https://docs.proximadb.com
- **Roadmap**: See `docs/_internal/roadmap/STRATEGIC_ROADMAP.adoc`

---

*This document is auto-generated from capability snapshots. Do not edit manually. Use `./scripts/generate_capability_snapshots.sh` to regenerate.*
