# Python SDK & Embedded Implementation Audit Report

**Date**: 2026-03-10
**Purpose**: Audit multi-model capabilities for Victor AI integration
**Status**: Complete

---

## Executive Summary

The ProximaDB Python SDK has **strong vector and basic graph capabilities**, but has **gaps in document operations, time-series operations, and hybrid queries** that are critical for Victor's code intelligence use cases.

### Key Findings

| Model | SDK Status | REST/gRPC Status | Embedded Status | Victor Readiness |
|-------|-----------|------------------|-----------------|------------------|
| **Vector** | ✅ Complete | ✅ Complete | ✅ Complete | ✅ Ready |
| **Graph** | 🟡 Partial | ✅ Complete | ❌ Missing | 🟡 Needs Work |
| **Document** | ❌ Missing | ✅ Complete | ❌ Missing | ❌ Needs Implementation |
| **Time-Series** | ❌ Missing | ⚠️ Engine exists, no API | ❌ Missing | ❌ Needs Implementation |
| **Hybrid Query** | ❌ Missing | ✅ Complete | ❌ Missing | ❌ Critical Gap |

---

## Part 1: Python SDK Audit

### 1.1 Current SDK Structure

**File**: `clients/python/src/proximadb_sdk/__init__.py` (998 lines)

**Exported Modules**:
- ✅ Vector operations: `ProximaDBClient`, `VectorRecord`, `SearchResult`
- ✅ Filter API: `FilterBuilder`, `FilterCondition`, `FilterGroup`
- ✅ Authentication: `ProximaDBAuth`, `AuthConfig`, `AuthMethod`
- ✅ Builders: `SearchBuilder`, `CollectionBuilder`, `InsertBuilder`
- ✅ Graph analytics: `GraphAnalytics` (high-level algorithms)
- ✅ Multi-modal query: `MultiModalQueryExecutor` (limited)
- ✅ Embedded mode: `EmbeddedProximaDB`, `EmbeddedCollection`
- ✅ Integrations: LangChain, LlamaIndex, Haystack, CrewAI, Victor (basic)

### 1.2 Vector Operations ✅

**Status**: Complete and production-ready

**Available in SDK** (`unified_client.py`):
```python
# Collection management
client.create_collection(name, dimension, distance_metric, storage_engine)
client.get_collection(collection_id)
client.list_collections()
client.delete_collection(collection_id)
client.get_collection_stats(collection_id)

# Vector operations
client.insert_vectors(collection_id, vectors)
client.upsert_vectors(collection_id, vectors)
client.search(collection_id, vector, top_k, filters, include_vector)
client.delete_vectors(collection_id, ids)
client.get_vector(collection_id, id)

# Batch operations
client.search_batch(queries)  # Multiple searches in one call
```

**REST Endpoints** (`/api/v1/collections/*`):
- ✅ All CRUD operations available
- ✅ Metadata filtering supported
- ✅ Batch operations supported

**Embedded**: ✅ Fully supported via `EmbeddedCollection`

---

### 1.3 Graph Operations 🟡

**Status**: Partial - Low-level CRUD exists, but missing critical features

**Available in SDK** (`unified_client.py` lines 2159-2464):
```python
# Basic CRUD (available)
client.create_node(graph_id, node_id, labels, properties)
client.create_edge(graph_id, edge_id, from_node, to_node, edge_type, properties)
client.traverse_graph(graph_id, start_node, max_depth, edge_types)
client.query_nodes(graph_id, labels, properties)
client.create_graph(graph_id)
client.delete_graph(graph_id)
client.get_graph(graph_id)
client.list_graphs()
client.get_graph_stats(graph_id)
```

**REST Endpoints** (`graph.rs` - 80KB module):
- ✅ `POST /api/v1/graph/graphs` - Create graph
- ✅ `POST /api/v1/graph/graphs/{id}/nodes` - Create node
- ✅ `POST /api/v1/graph/graphs/{id}/edges` - Create edge
- ✅ `POST /api/v1/graph/graphs/{id}/traverse` - BFS/DFS traversal
- ✅ `POST /api/v1/graph/graphs/{id}/shortest_path` - Dijkstra
- ✅ `POST /api/v1/graph/graphs/{id}/query` - Declarative queries
- ✅ `POST /api/v1/graph/graphs/{id}/nodes/batch` - Batch nodes
- ✅ `POST /api/v1/graph/graphs/{id}/edges/batch` - Batch edges

**Proto Definition** (`graph.proto`):
- ✅ Node, Edge, TraversalRequest, TraversalResponse
- ✅ PropertyFilter, PropertyFilterOperator
- ✅ HybridSearchRequest (vector + graph)

**CRITICAL GAPS**:
1. ❌ **No Cypher query support** - Victor needs "MATCH (n)-[r]->(m) RETURN n, r, m"
2. ❌ **No batch node/edge creation** exposed in SDK (REST has it)
3. ❌ **No reverse traversal** (find all callers of a function)
4. ❌ **No graph algorithms** (PageRank, centrality, community detection) - only analytics module
5. ❌ **No pattern matching** (find all functions that call X AND Y)

**Embedded**: ❌ **Graph operations NOT available in embedded mode**

---

### 1.4 Document Operations ❌

**Status**: **NOT exposed in Python SDK** - Major gap for Victor

**Proto Definition** (`document.proto`):
- ✅ DocumentContent, DocumentCollectionConfig
- ✅ IndexDefinition (BTREE, HASH, INVERTED, FULLTEXT, GEO)
- ✅ DocumentFilter (complex nested filters)
- ✅ DocFilterCondition with JSON path support

**REST Endpoints** (`document.rs` - 16KB module):
- ✅ `POST /api/v1/documents/collections` - Create collection
- ✅ `POST /api/v1/documents/collections/{id}/documents` - Insert document
- ✅ `GET /api/v1/documents/collections/{id}/documents/{id}` - Get document
- ✅ `POST /api/v1/documents/collections/{id}/query` - Query with filters
- ✅ `POST /api/v1/documents/collections/{id}/indexes` - Create index
- ✅ Full-text search with Tantivy integration

**SDK**: ❌ **No document operations exposed in `ProximaDBClient`**
- No `insert_document()` method
- No `query_documents()` method
- No `create_document_collection()` method

**Embedded**: ❌ **Document operations NOT available in embedded mode**

---

### 1.5 Time-Series Operations ❌

**Status**: **Engine exists but NO API exposure**

**Rust Implementation** (`src/storage/engines/impls/tst/`):
- ✅ `mod.rs` (56KB) - Complete TST engine
- ✅ `partition.rs` (29KB) - Time-partitioning
- ✅ `downsample.rs` (14KB) - OHLC downsampling
- ✅ `compression.rs` (13KB) - Gorilla compression
- ✅ `ohlc.rs` (8KB) - OHLC aggregation
- ✅ `asof_join.rs` (5KB) - ASOF joins

**Features**:
- ✅ Time-partitioned columnar storage
- ✅ OHLC downsampling with Gorilla compression
- ✅ >100K bars/second ingestion
- ✅ >10:1 compression ratio
- ✅ ASOF joins <1ms

**Proto**: ❌ **NO `timeseries.proto` file**
- No proto definitions for time-series operations
- No gRPC service definitions

**REST**: ❌ **NO time-series endpoints**
- No `/api/v1/timeseries/*` endpoints

**SDK**: ❌ **No time-series operations in SDK**
- No `ingest_metrics()` method
- No `query_timeseries()` method
- No `create_timeseries_collection()` method

**Embedded**: ❌ **Time-series NOT available in embedded mode**

---

### 1.6 Hybrid Query Operations ❌

**Status**: **NOT exposed in Python SDK** - Critical gap for Victor

**Proto Definition** (`graph.proto` lines 269-293):
```protobuf
message HybridSearchRequest {
  proximadb.v1.VectorSearchRequest vector_search_request = 1;
  TraversalRequest graph_traversal_request = 2;
  CombinationStrategy combination_strategy = 3;
  optional uint32 limit = 4;
  optional uint32 offset = 5;
}

enum CombinationStrategy {
  VECTOR_THEN_GRAPH = 1;
  GRAPH_THEN_VECTOR = 2;
  BALANCED = 3;
}
```

**REST Endpoints** (`hybrid.rs` - 19KB module):
- ✅ `GET /api/v1/hybrid` - Info endpoint
- ✅ `POST /api/v1/hybrid/search` - Hybrid vector + graph search

**SDK**: ❌ **No hybrid query methods exposed**
- No `hybrid_search()` method
- No `federated_query()` method

**Existing SDK Module** (`multimodal_query.py`):
- ⚠️ `MultiModalQueryExecutor` exists but is **complex and not well integrated**
- ⚠️ No simple API like `client.hybrid_search(query, graph_query)`

**Embedded**: ❌ **Hybrid queries NOT available in embedded mode**

---

## Part 2: Embedded Implementation Audit

### 2.1 Current Embedded Architecture

**File**: `clients/python/src/proximadb_sdk/embedded.py` (900+ lines)

**Design**:
- Subprocess-based embedded mode (spawns ProximaDB server)
- Auto-embedding support (sentence-transformers, Ollama, OpenAI)
- Vector-only operations
- No graph, document, or time-series support

**Current Capabilities**:
```python
# ✅ Vector operations
db = EmbeddedProximaDB(data_dir="~/.proximadb")
await db.start()
collection = await db.create_collection("code", dimension=384)
await collection.insert_with_embedding([
    {"id": "func1", "text": "def hello(): ..."}
])
results = await collection.search_text("hello function")

# ✅ Embedding models
SentenceTransformerModel, OllamaEmbeddingModel,
OpenAIEmbeddingModel, FunctionEmbeddingModel
```

### 2.2 Embedded Gaps

| Feature | Status | Impact |
|---------|--------|--------|
| **Graph storage** | ❌ Not available | Can't store call graphs, dependencies |
| **Document storage** | ❌ Not available | Can't store full code files with metadata |
| **Time-series** | ❌ Not available | Can't track code metrics over time |
| **Hybrid queries** | ❌ Not available | Can't combine vector + graph search |
| **Cypher queries** | ❌ Not available | No graph query language |
| **Batch operations** | ❌ Not available | Slow code indexing |

**Embedded Server Capabilities** (from Rust codebase):
- ✅ Supports all storage engines (SST, HELIX, VIPER, SWIFT, NOVA, RAPTOR)
- ✅ Supports graph engine (ORION)
- ✅ Supports document engine
- ✅ Supports time-series engine (TST)
- ✅ Supports hybrid queries

**Embedded Python Client Gaps**:
- ❌ Only exposes vector operations
- ❌ No graph methods (`create_node`, `create_edge`, `traverse`)
- ❌ No document methods
- ❌ No time-series methods
- ❌ No hybrid query methods

---

## Part 3: Victor Requirements vs SDK Capabilities

### 3.1 Victor's Current Architecture

```
Victor AI (Code Intelligence)
├── SQLite DB
│   ├── call_graph (functions, calls, imports)
│   ├── ast_metadata (tree-sitter parse results)
│   └── code_context (snapshots)
└── LanceDB
    ├── embeddings (semantic search)
    └── code_chunks (with embeddings)
```

### 3.2 Victor Migration Requirements

| Victor Feature | Current Storage | ProximaDB Target | SDK Status |
|----------------|----------------|-----------------|------------|
| **Function calls** | SQLite graph table | Graph nodes/edges | 🟡 Partial (no Cypher) |
| **AST metadata** | SQLite JSON column | Node properties | 🟡 Partial |
| **Imports** | SQLite table | Graph edges (IMPORTS) | 🟡 Partial |
| **Embeddings** | LanceDB vectors | Vector collection | ✅ Complete |
| **Full code files** | File system reads | Document collection | ❌ Missing |
| **Code metrics** | Not tracked | Time-series | ❌ Missing |
| **Semantic + structural** | Separate queries | Hybrid query | ❌ Missing |

### 3.3 Victor Use Cases vs SDK Gaps

**Use Case 1**: "Find functions called by main that parse JSON"
- Victor needs: Reverse graph traversal + vector search
- SDK has: `traverse_graph()` (forward only) + `search()` (separate)
- Gap: ❌ No hybrid API combining both

**Use Case 2**: "Show code churn in auth module (30 days)"
- Victor needs: Time-series query with time range
- SDK has: ❌ No time-series operations
- Gap: ❌ No time-series API

**Use Case 3**: "Find similar bugs to this one"
- Victor needs: Vector search + graph (files modified together)
- SDK has: Separate APIs
- Gap: ❌ No hybrid query API

**Use Case 4**: "What changed since yesterday in file X?"
- Victor needs: Time-series point-in-time query
- SDK has: ❌ No time-series API
- Gap: ❌ Missing

---

## Part 4: Critical MVP Features for Victor

### Priority 1: Graph API Enhancements 🚨

**Impact**: Enables call graph and dependency tracking

**Required Additions**:
```python
# 1. Batch node/edge operations (CRITICAL for performance)
client.batch_create_nodes(graph_id, [
    {"id": "func:main", "labels": ["Function"], "properties": {...}},
    {"id": "func:parse", "labels": ["Function"], "properties": {...}},
])
client.batch_create_edges(graph_id, [
    {"from": "func:main", "to": "func:parse", "type": "CALLS"},
])

# 2. Cypher query support (CRITICAL for complex queries)
results = client.query_cypher(graph_id, """
    MATCH (c:Function)-[:CALLS]->(f:Function)
    WHERE c.name = 'main'
    RETURN c, f
""")

# 3. Reverse traversal (find callers)
callers = client.find_callers(graph_id, node_id="func:parse_json")

# 4. Pattern matching
matches = client.match_pattern(graph_id, """
    (f:Function)-[:CALLS]->(g:Function)-[:CALLS]->(h:Function)
    WHERE f.name CONTAINS 'test'
""")
```

**Effort**: 2-3 days
**Files**:
- `clients/python/src/proximadb_sdk/unified_client.py` (add methods)
- `clients/python/src/proximadb_sdk/graph.py` (new file for graph-specific API)

---

### Priority 2: Document API 🚨

**Impact**: Enables full code file storage with rich metadata

**Required Additions**:
```python
# Document collection management
client.create_document_collection(
    name="code_files",
    json_schema=None,  # Optional validation
    indexes=[
        {"path": "$.language", "type": "hash"},
        {"path": "$.file_path", "type": "btree"},
        {"path": "$.ast.functions", "type": "inverted"},
    ],
    enable_fulltext=True,
    fulltext_paths=["$.content"]
)

# Document operations
client.insert_document(
    collection_id="code_files",
    document={
        "file_path": "src/main.py",
        "language": "python",
        "content": "def main(): ...",
        "ast": {...},
        "functions": ["main", "parse_json"],
        "metrics": {...}
    },
    id="file:main.py"
)

# Document queries
results = client.query_documents(
    collection_id="code_files",
    filter={"language": "python"},
    projection=["file_path", "functions"],
    limit=10
)

# Full-text search
files = client.search_documents(
    collection_id="code_files",
    text_query="function that parses JSON"
)
```

**Effort**: 3-4 days
**Files**:
- `clients/python/src/proximadb_sdk/unified_client.py` (add document methods)
- `clients/python/src/proximadb_sdk/models.py` (add DocumentRecord, DocumentCollection)

---

### Priority 3: Time-Series API 📊

**Impact**: Enables code metrics tracking and churn analysis

**Required Additions**:
```python
# Time-series collection
client.create_timeseries_collection(
    name="code_metrics",
    timestamp_column="timestamp",
    value_columns=[
        {"name": "complexity", "type": "float"},
        {"name": "lines_of_code", "type": "int"},
        {"name": "function_count", "type": "int"},
    ],
    tags_columns=["file_path", "language", "author"]
)

# Ingest metrics
client.ingest_metrics(
    collection_id="code_metrics",
    metrics=[
        {
            "timestamp": "2026-03-10T10:00:00Z",
            "file_path": "src/main.py",
            "complexity": 15.5,
            "lines_of_code": 250,
            "function_count": 8
        },
        ...
    ]
)

# Time-series query
metrics = client.query_timeseries(
    collection_id="code_metrics",
    start_time="2026-02-10T00:00:00Z",
    end_time="2026-03-10T00:00:00Z",
    filter={"file_path": "src/main.py"},
    aggregation="OHLC",  # For downsampling
    interval="1d"
)

# Get latest metric
latest = client.get_latest_metric(
    collection_id="code_metrics",
    tags={"file_path": "src/main.py"}
)
```

**Effort**: 4-5 days
**Dependencies**:
- Need to create `proto/proximadb/v1/timeseries.proto`
- Need to create gRPC service
- Need to create REST endpoints

---

### Priority 4: Hybrid Query API 🔀

**Impact**: Enables multi-model code intelligence queries

**Required Additions**:
```python
# Simple hybrid search
results = client.hybrid_search(
    query="parse JSON input",  # Vector similarity
    graph_query="MATCH (c:Function)-[:CALLS]->(f:Function) WHERE c.name = 'main'",
    document_filter={"language": "python"},
    time_range=("2026-02-01", "2026-03-01"),
    top_k=10
)

# Federated SQL (all models in one query)
results = client.federated_query("""
    SELECT v.id, v.score, n.properties
    FROM VECTOR_SEARCH('code_embeddings', ?, 10) v
    JOIN GRAPH_QUERY('call_graph', 'MATCH (n)-[r:CALLS]->(m) RETURN n, r, m') g
      ON v.id = g.node_id
    JOIN DOCUMENT_QUERY('code_files', '{"language": "python"}') d
      ON v.metadata.file_path = d.file_path
    WHERE v.metadata.language = 'python'
""", query_vector)
```

**Effort**: 2-3 days
**Files**:
- `clients/python/src/proximadb_sdk/unified_client.py` (add hybrid methods)
- Leverage existing `multimodal_query.py` module

---

### Priority 5: Embedded Mode Enhancements 💻

**Impact**: Enables Victor to use ProximaDB without separate server

**Required Additions**:
```python
# Embedded with all models
db = EmbeddedProximaDB(data_dir="~/.victor/db")

# Vector collection
code_vectors = await db.create_vector_collection("embeddings", dimension=384)

# Graph collection
call_graph = await db.create_graph_collection("call_graph")

# Document collection
code_files = await db.create_document_collection("files")

# Time-series collection
metrics = await db.create_timeseries_collection("metrics")

# Hybrid query
results = await db.hybrid_search(
    vector_query="parse JSON",
    graph_query="MATCH (c:Function)-[:CALLS]->(f:Function)",
    document_collection="files"
)
```

**Effort**: 3-4 days
**Files**:
- `clients/python/src/proximadb_sdk/embedded.py` (extend EmbeddedProximaDB)
- Add `EmbeddedGraphCollection`, `EmbeddedDocumentCollection`, etc.

---

## Part 5: Implementation Roadmap

### Phase 1: Core Graph API (Week 1)

**Goal**: Enable Victor to store and query call graphs

**Tasks**:
1. Add batch node/edge operations to SDK
2. Add Cypher query support
3. Add reverse traversal (`find_callers`)
4. Add pattern matching API
5. Write unit tests
6. Document API with examples

**Deliverables**:
- `clients/python/src/proximadb_sdk/graph.py` (new)
- Updated `unified_client.py` with graph methods
- Test suite in `tests/test_graph_api.py`

---

### Phase 2: Document API (Week 2)

**Goal**: Enable Victor to store full code files with metadata

**Tasks**:
1. Add document collection management
2. Add document insert/query operations
3. Add full-text search integration
4. Add projection support
5. Write unit tests
6. Document API

**Deliverables**:
- Document models in `models.py`
- Document methods in `unified_client.py`
- Test suite in `tests/test_document_api.py`

---

### Phase 3: Time-Series API (Week 3-4)

**Goal**: Enable Victor to track code metrics over time

**Tasks**:
1. **Rust side**:
   - Create `proto/proximadb/v1/timeseries.proto`
   - Create gRPC service in `src/server/grpc_service.rs`
   - Create REST endpoints in `src/network/rest/v1/timeseries.rs`
2. **Python side**:
   - Generate proto bindings
   - Add time-series models
   - Add time-series methods to SDK
   - Write tests

**Deliverables**:
- `proto/proximadb/v1/timeseries.proto`
- Rust gRPC/REST implementations
- Python SDK time-series API
- Test suite

---

### Phase 4: Hybrid Query API (Week 5)

**Goal**: Enable Victor to combine vector + graph + document + time-series

**Tasks**:
1. Add `hybrid_search()` method to SDK
2. Add `federated_query()` method for SQL
3. Integrate existing `MultiModalQueryExecutor`
4. Write comprehensive examples
5. Write tests

**Deliverables**:
- Hybrid query API in SDK
- Documentation with Victor examples
- Test suite

---

### Phase 5: Embedded Mode Enhancement (Week 6)

**Goal**: Enable Victor to use ProximaDB embedded without server

**Tasks**:
1. Add graph collection support to embedded mode
2. Add document collection support
3. Add time-series collection support
4. Add hybrid query support
5. Write tests

**Deliverables**:
- Enhanced `EmbeddedProximaDB` class
- New `EmbeddedGraphCollection`, etc.
- Test suite

---

## Part 6: Recommended Immediate Actions

### For Victor Integration (Next 1-2 Weeks)

**DO FIRST** (Critical Path):

1. **Batch Graph Operations** (2 days)
   - Victor processes thousands of functions per codebase
   - Single inserts will be too slow
   - Implement `batch_create_nodes()`, `batch_create_edges()`

2. **Document API** (3 days)
   - Victor needs to store full code files with AST metadata
   - Implement basic document CRUD
   - Skip advanced features initially (full-text, complex indexes)

3. **Simple Hybrid Query** (2 days)
   - Implement `hybrid_search()` that combines vector + graph
   - Start with simple use case: "find functions similar to X called by Y"

**DO LATER** (After MVP):

4. Time-series API (can use vector storage with timestamps as workaround)
5. Advanced Cypher queries (use graph traversals initially)
6. Embedded mode enhancements (use network server initially)

---

## Part 7: Code Examples for Victor

### Example 1: Indexing Code with Multi-Model Storage

```python
from proximadb_sdk import ProximaDBClient
from proximadb_sdk.integrations.victor_multi import ProximaDBMultiModelProvider

client = ProximaDBClient(url="http://localhost:5678")
provider = ProximaDBMultiModelProvider(client=client, workspace="myrepo")

# Index code across all models
result = await provider.index_code_file(
    file_path="src/main.py",
    content=open("src/main.py").read(),
    language="python"
)

# Result shows:
# - vectors: 5 (chunked and embedded)
# - document: true (full file stored)
# - graph: {"functions": 3, "calls": 5, "imports": 2} (extracted)
# - timeseries: 2 (complexity, LOC metrics)
```

### Example 2: Hybrid Query for Code Intelligence

```python
# Find functions called by main that parse JSON
results = await provider.hybrid_search(
    query="parse JSON input validation",
    graph_query="MATCH (c:Function)-[:CALLS]->(f:Function) WHERE c.name = 'main'",
    document_filter={"language": "python"},
    top_k=10
)

# Results combine:
# - Vector similarity (semantic match)
# - Graph traversal (main → parse_json)
# - Document filter (Python only)
```

---

## Part 8: Summary and Recommendations

### Current State

| Component | Status | Blocker for Victor? |
|-----------|--------|---------------------|
| Vector SDK | ✅ Complete | No |
| Graph SDK | 🟡 Partial | **Yes** - needs batch + Cypher |
| Document SDK | ❌ Missing | **Yes** - critical gap |
| Time-Series SDK | ❌ Missing | **Yes** - metrics tracking |
| Hybrid Query SDK | ❌ Missing | **Yes** - core feature |
| Embedded Graph | ❌ Missing | **Yes** - for local use |
| Embedded Document | ❌ Missing | **Yes** - for local use |

### Recommended Priority

1. **Immediate** (This Week):
   - ✅ Batch graph operations
   - ✅ Basic document API
   - ✅ Simple hybrid query

2. **Short-term** (Next 2 Weeks):
   - ⏳ Time-series API (or workaround with vector + timestamps)
   - ⏳ Cypher query support
   - ⏳ Enhanced hybrid queries

3. **Medium-term** (Next Month):
   - ⏳ Embedded mode enhancements
   - ⏳ Advanced graph algorithms
   - ⏳ Performance optimizations

### Estimated Effort

- **Minimum Viable for Victor**: 7-10 days
- **Full Feature Parity**: 4-6 weeks

### Success Criteria

Victor integration is successful when:
1. ✅ Can store call graphs with batch operations
2. ✅ Can store full code files with metadata
3. ✅ Can query code with hybrid (vector + graph) search
4. ✅ Can track basic metrics (complexity, LOC) over time
5. ✅ Performance acceptable (1000+ files indexed in <1 minute)

---

*End of Audit Report*

**Next Steps**:
1. Review and prioritize features
2. Create implementation tasks
3. Begin with Priority 1 (Batch Graph Operations)
