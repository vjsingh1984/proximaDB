# Victor + ProximaDB Integration Guide

**Purpose**: Integrate ProximaDB's multi-model store as Victor's backend, replacing SQLite + LanceDB

**Date**: 2026-03-10
**Status**: Ready for Victor session

---

## Current Victor Architecture (To Replace)

```
Victor AI (Code Intelligence Platform)
├── SQLite DB
│   ├── Call graphs (function relationships)
│   ├── AST metadata (tree-sitter parse results)
│   └── Code context snapshots
└── LanceDB
    ├── Vector embeddings (semantic search)
    └── Code chunks with embeddings
```

**Current Limitations**:
- No hybrid search (can't combine semantic + structural queries)
- No temporal tracking (code metrics over time)
- No graph traversals (find functions called by X that also call Y)
- Separate storage systems = complexity and data consistency issues

---

## ProximaDB Multi-Model Capabilities (To Leverage)

```
ProximaDB (Unified Multi-Model Store)
├── Vector Engine: Semantic embeddings, HNSW/IVF/DiskANN indexing
├── Document Engine: Full code files with rich metadata
├── Graph Engine (ORION): Call graphs, dependency graphs, AST relationships
├── Time-Series Engine (TST): Code metrics, complexity trends, churn analysis
└── Hybrid Query: Combine all models in single query (SQL + Cypher + vector)
```

**Key Advantage**: Single database with unified query engine for all code intelligence needs.

---

## Reference Implementation Available

**File**: `clients/python/src/proximadb_sdk/integrations/victor_multi.py` (923 lines)

**Key Classes**:
- `ProximaDBMultiModelProvider` - Enhanced provider with multi-model storage
- `hybrid_search()` - Vector + graph + document + time-series search
- `index_code_file()` - Single API to index across all 4 models

**Usage Example**:
```python
from proximadb_sdk.integrations.victor_multi import ProximaDBMultiModelProvider
from proximadb_sdk import ProximaDBClient

client = ProximaDBClient(url="http://localhost:5678")
provider = ProximaDBMultiModelProvider(client=client, workspace="my-repo")

# Index code across all 4 models
await provider.index_code_file("src/main.py", content="...", language="python")

# Hybrid query: find functions called by main that parse JSON
results = await provider.hybrid_search(
    query="parse JSON input",
    graph_query="MATCH (c:Function)-[:CALLS]->(f:Function) WHERE c.name = 'main'",
    document_filter={"language": "python"},
    top_k=10
)
```

---

## Victor Session Prompts (Organized by Topic)

### Topic 1: Examine Existing ProximaDB Provider Scaffolding

**Prompt**:
```
Examine the existing ProximaDB provider scaffolding in Victor's codebase.

I need to understand:
1. Where is the ProximaDB provider located in Victor's repo?
2. What functionality is currently implemented vs. stubbed out?
3. How does it integrate with Victor's BaseEmbeddingProvider interface?
4. What configuration options exist for ProximaDB connection?

Search for:
- "proximadb" in all Python files
- Any storage providers in victor/storage/vector_stores/
- BaseEmbeddingProvider interface definition
- Provider registration/factory code
```

**Expected Output**:
- Location of existing ProximaDB provider files
- Current implementation status (methods stubbed vs. implemented)
- Integration points with Victor's storage abstraction
- Configuration approach

---

### Topic 2: Map Current SQLite + LanceDB Usage

**Prompt**:
```
Analyze how Victor currently uses SQLite and LanceDB for code storage.

I need to understand:
1. What data is stored in SQLite? (call graphs, AST metadata, etc.)
2. What is stored in LanceDB? (embeddings, code chunks, etc.)
3. How are the two storage systems coordinated? (transactions, consistency)
4. What queries are performed against each system?

Search for:
- sqlite3, sqlite imports and usage
- lancedb imports and usage
- Database schema definitions
- Query patterns (SELECT, vector_search, etc.)
- Any abstraction layers around these databases
```

**Expected Output**:
- SQLite schema and data models
- LanceDB collection structure
- Query patterns and usage
- Integration/coordination between the two systems

---

### Topic 3: Design Multi-Model Migration Strategy

**Prompt**:
```
Design a migration strategy to replace SQLite + LanceDB with ProximaDB's multi-model store.

Based on the current usage analysis:

SQLite tables → ProximaDB Graph Engine (ORION)
- call_graph → Graph nodes (functions) and edges (CALLS relationship)
- ast_metadata → Graph node properties
- imports → Graph edges (IMPORTS relationship)

LanceDB collections → ProximaDB Vector Engine
- code_chunks → Vector collection with embeddings
- semantic_search → vector similarity search

New capabilities with ProximaDB:
- Document Engine: Full code files with syntax highlighting
- Time-Series Engine: Code metrics tracking (complexity, churn, coverage)
- Hybrid Search: Combine semantic + structural queries in single API

Design:
1. Data migration script (SQLite/LanceDB → ProximaDB)
2. New ProximaDBMultiModelProvider implementation
3. Backward compatibility layer (optional: keep old providers)
4. Testing strategy for migration validation

Reference implementation available at:
proximadb/clients/python/src/proximadb_sdk/integrations/victor_multi.py

Consider:
- Should this be a new provider class or modification of existing?
- How to handle existing Victor deployments with SQLite/LanceDB data?
- What configuration options to expose?
- How to validate migration correctness?
```

**Expected Output**:
- Migration plan with phases
- New provider class design
- Data mapping (old → new)
- Testing approach

---

### Topic 4: Implement ProximaDB Multi-Model Provider

**Prompt**:
```
Implement ProximaDBMultiModelProvider for Victor based on the reference implementation.

Reference file:
proximadb/clients/python/src/proximadb_sdk/integrations/victor_multi.py

Key methods to implement:
1. index_code_file(file_path, content, language) - Index across all 4 models
   - Vector: Chunk and embed code
   - Document: Store full file with metadata
   - Graph: Extract functions, calls, imports using tree-sitter
   - Time-Series: Extract metrics (complexity, LOC, etc.)

2. hybrid_search(query, graph_query, filters) - Multi-model search
   - Vector: Semantic similarity
   - Graph: Cypher query for relationships
   - Document: Metadata filtering
   - Time-Series: Temporal filters

3. Integration with Victor's BaseEmbeddingProvider:
   - search_similar() - Vector search
   - index_document() - Document storage
   - get_embeddings() - Embedding generation

Dependencies:
- proximadb-python SDK
- tree-sitter (already used by Victor)
- ProximaDB server running (default: localhost:5678)

Implementation approach:
1. Copy reference implementation patterns from victor_multi.py
2. Adapt to Victor's specific needs (call graphs, AST parsing)
3. Add tree-sitter integration for graph extraction
4. Implement hybrid search with Cypher queries
5. Add comprehensive error handling
6. Write unit tests

Start by creating the new provider class in:
victor/storage/vector_stores/proximadb_multi.py
```

**Expected Output**:
- Complete ProximaDBMultiModelProvider implementation
- Graph extraction logic using tree-sitter
- Hybrid search implementation
- Error handling and logging

---

### Topic 5: Tree-Sitter Integration for Graph Extraction

**Prompt**:
```
Implement tree-sitter-based code parsing for ProximaDB graph extraction.

Victor already uses tree-sitter for AST parsing. Extend this to populate
ProximaDB's graph engine with:

Nodes to create:
- Function: name, signature, file_path, line_start, line_end
- Class: name, file_path, line_start, line_end
- Module: file_path, language

Edges to create:
- CALLS: Function → Function (call relationships)
- DEFINES: Class → Function (method definitions)
- IMPORTS: Module → Module/Class (import relationships)
- INHERITS: Class → Class (inheritance)

For each code file:
1. Parse with tree-sitter (language-specific parser)
2. Extract function definitions
3. Extract function calls
4. Extract class definitions and inheritance
5. Extract imports
6. Create nodes/edges via ProximaDB client API

Example ProximaDB graph API:
```python
# Create nodes
client.create_node(
    collection="myrepo_graph",
    node_id="function:main.py:parse_json",
    labels=["Function"],
    properties={
        "name": "parse_json",
        "file_path": "main.py",
        "line_start": 42,
        "line_end": 55
    }
)

# Create edges
client.create_edge(
    collection="myrepo_graph",
    edge_id="edge:1",
    from_node="function:main.py:main",
    to_node="function:main.py:parse_json",
    edge_type="CALLS",
    properties={"line": 10}
)
```

Languages to support:
- Python (highest priority)
- JavaScript/TypeScript
- Go
- Rust
- Java

Implementation approach:
1. Create tree-sitter parser factory (get parser for language)
2. Implement node/edge extraction per language
3. Handle language-specific patterns (decorators, generics, etc.)
4. Add error handling for parse failures
5. Write tests for extraction accuracy
```

**Expected Output**:
- Language-specific graph extraction functions
- Node/edge creation via ProximaDB client
- Error handling for malformed code
- Test coverage

---

### Topic 6: Hybrid Query Examples for Victor Use Cases

**Prompt**:
```
Design hybrid queries for common Victor use cases using ProximaDB's multi-model API.

Victor use cases to support:

1. "Find functions that parse JSON and are called by main"
   Hybrid query: Vector search ("parse JSON") + Graph traversal (main → calls)

2. "Show me code churn in authentication module over last 30 days"
   Time-series query: Filter by module + time range + aggregation

3. "Find all callers of this function that handle errors"
   Graph query: Reverse CALLS + Vector search ("error handling")

4. "What changed since yesterday in file X?"
   Time-series diff + Document retrieval (before/after snapshots)

5. "Find similar bugs to this one"
   Vector search (bug description) + Graph (files modified together)

For each use case, provide:
1. Natural language description
2. Hybrid query implementation (vector + graph + document + time-series)
3. Expected results format
4. Performance considerations

Example hybrid query API:
```python
results = await provider.hybrid_search(
    query="parse JSON data",  # Vector similarity
    graph_query="MATCH (c:Function)-[:CALLS]->(f:Function) WHERE c.name = 'main'",
    document_filter={"language": "python"},
    time_range=(datetime(2026, 1, 1), datetime.now()),
    top_k=10
)
```

Also design Victor-specific convenience methods:
- find_callers(function_name) - Graph reverse traversal
- find_similar_bugs(bug_description) - Vector + graph
- get_code_metrics(file_path, days=30) - Time-series aggregation
- trace_execution_path(entry_function) - Graph traversal
```

**Expected Output**:
- 5-10 hybrid query examples for Victor use cases
- Convenience method signatures and implementations
- Performance optimization notes

---

### Topic 7: Configuration and Deployment

**Prompt**:
```
Design configuration and deployment approach for Victor with ProximaDB backend.

Configuration options:
1. ProximaDB connection:
   - Server URL (default: http://localhost:5678)
   - API key (optional, for authentication)
   - Connection pool size
   - Timeout settings

2. Collection naming:
   - Workspace prefix (e.g., "victor_<repo_name>")
   - Vector collection: "{workspace}_vectors"
   - Document collection: "{workspace}_documents"
   - Graph collection: "{workspace}_graph"
   - Time-series collection: "{workspace}_metrics"

3. Embedding configuration:
   - Model name (default: BAAI/bge-small-en-v1.5)
   - Dimension (default: 384)
   - Chunk size (default: 500 tokens)
   - Overlap (default: 50 tokens)

4. Indexing options:
   - Create indices on first use
   - HNSW parameters (M, ef_construction)
   - Graph engine (ORION vs PULSAR vs QUASAR)

5. Migration options:
   - Migrate existing SQLite/LanceDB data on startup
   - Validate migration before switching
   - Keep old data for rollback

Design:
1. Configuration file format (YAML/TOML/JSON)
2. Environment variable overrides
3. Validation logic
4. Migration command/script
5. Health check endpoint

Example config:
```yaml
victor:
  storage_backend: proximadb

  proxima_db:
    url: http://localhost:5678
    api_key: ${PROXIMADB_API_KEY}  # Optional
    workspace: myrepo
    timeout: 30

  embeddings:
    model: BAAI/bge-small-en-v1.5
    dimension: 384
    chunk_size: 500
    chunk_overlap: 50

  indexing:
    vector_index: hnsw
    hnsw_m: 16
    hnsw_ef_construction: 200

  migration:
    from_sqlite: ~/.victor/data.db
    from_lancedb: ~/.victor/lancedb
    validate: true
    backup: true
```

Deployment scenarios:
1. Local development: ProximaDB Docker + Victor local
2. Production: ProximaDB cloud + Victor container
3. Hybrid: ProximaDB local cache + cloud backup
```

**Expected Output**:
- Configuration schema
- Migration script/command
- Deployment guide (local + production)
- Health check implementation

---

### Topic 8: Testing Strategy

**Prompt**:
```
Design comprehensive testing strategy for ProximaDB integration in Victor.

Test categories:

1. Unit Tests:
   - ProximaDBMultiModelProvider methods
   - Graph extraction from tree-sitter
   - Hybrid query construction
   - Error handling

2. Integration Tests:
   - Real ProximaDB server (Docker)
   - End-to-end code indexing
   - Hybrid query execution
   - Migration validation

3. Performance Tests:
   - Indexing throughput (files/sec)
   - Query latency (p50, p95, p99)
   - Memory usage
   - Comparison with SQLite/LanceDB baseline

4. Correctness Tests:
   - Migration validation (SQLite/LanceDB → ProximaDB)
   - Graph extraction accuracy
   - Hybrid search result relevance

Test infrastructure:
- Fixtures: Sample codebases (Python, JS, Go)
- Mocks: ProximaDB client for unit tests
- Benchmarks: Standard Victor workloads

Example test cases:
```python
async def test_graph_extraction_python():
    """Test that Python code graph is extracted correctly."""
    code = """
def main():
    parse_json(data)

def parse_json(data):
    return json.loads(data)
"""
    provider = ProximaDBMultiModelProvider(...)
    result = await provider.index_code_file("test.py", code, "python")

    assert result["graph"]["functions"] == 2
    assert result["graph"]["calls"] == 1

    # Verify graph structure
    nodes = await client.get_nodes("victor_test_graph")
    edges = await client.get_edges("victor_test_graph")

    assert any(n["name"] == "main" for n in nodes)
    assert any(n["name"] == "parse_json" for n in nodes)
    assert any(e["type"] == "CALLS" for e in edges)
```

Testing tools:
- pytest with async support
- pytest-benchmark for performance
- docker-compose for ProximaDB test instance
- pytest-mock for client mocking
```

**Expected Output**:
- Test plan with coverage goals
- Sample test implementations
- Benchmark suite design
- CI/CD integration approach

---

## Quick Start Prompts (Copy-Paste Ready)

### Quick Start 1: Find Existing Provider
```
Find and examine the existing ProximaDB provider scaffolding in Victor's codebase. Look for:
1. Files in victor/storage/vector_stores/ with "proximadb" in the name
2. Provider registration code
3. Configuration options for ProximaDB
4. Current implementation status (what's stubbed vs. implemented)
```

### Quick Start 2: Analyze Current Storage
```
Analyze Victor's current SQLite and LanceDB usage:
1. What data models are stored in SQLite?
2. What collections exist in LanceDB?
3. How are the two systems coordinated?
4. What query patterns are used?

Search for sqlite3, lancedb, database schema, and query patterns.
```

### Quick Start 3: Design Migration
```
Design the migration from SQLite + LanceDB to ProximaDB:

SQLite → ProximaDB Graph Engine (ORION):
- call_graph → Graph nodes (functions) + edges (CALLS)
- ast_metadata → Node properties
- imports → Graph edges (IMPORTS)

LanceDB → ProximaDB Vector Engine:
- code_chunks → Vector collection
- embeddings → Vector similarity search

New capabilities:
- Document Engine: Full files with syntax highlighting
- Time-Series Engine: Code metrics tracking
- Hybrid Search: Combine all models

Design a migration plan with:
1. Data mapping (old → new)
2. Migration script
3. Validation strategy
4. Rollback approach
```

### Quick Start 4: Implement Graph Extraction
```
Implement tree-sitter-based graph extraction for ProximaDB:

For each code file, extract and create:
Nodes:
- Function (name, signature, file_path, line_start, line_end)
- Class (name, file_path, line_start, line_end)
- Module (file_path, language)

Edges:
- CALLS (function → function)
- DEFINES (class → function)
- IMPORTS (module → module/class)
- INHERITS (class → class)

Use Victor's existing tree-sitter parsers. Add ProximaDB graph API calls.
```

### Quick Start 5: Implement Hybrid Search
```
Implement hybrid search for Victor use cases:

1. "Find functions that parse JSON and are called by main"
   → Vector search + Graph traversal

2. "Show code churn in auth module (30 days)"
   → Time-series query + Document filter

3. "Find callers of this function that handle errors"
   → Graph reverse traversal + Vector search

Implement hybrid_search() method combining:
- Vector similarity (semantic)
- Graph queries (Cypher)
- Document filters (metadata)
- Time-series ranges (temporal)
```

---

## Reference Materials for Victor Session

### Files to Reference from ProximaDB:
1. `clients/python/src/proximadb_sdk/integrations/victor_multi.py` - Full multi-model provider implementation
2. `clients/python/src/proximadb_sdk/integrations/victor.py` - Base ProximaDB provider
3. `clients/python/src/proximadb_sdk/` - SDK client API
4. `proto/proximadb/v1/graph.proto` - Graph API definitions
5. `proto/proximadb/v1/query.proto` - Query API definitions

### ProximaDB Capabilities Quick Reference:
- **Vector Search**: `client.search(collection, vector, top_k, filters)`
- **Graph Query**: `client.execute_sql(cypher_query)` or `client.graph_query(collection, cypher)`
- **Document Query**: `client.query_documents(collection, filter)`
- **Time-Series**: `client.query_timeseries(collection, metric, time_range)`
- **Hybrid Query**: `client.federated_query(sql_with_extensions)`

### Multi-Model SQL Extensions:
```sql
-- Vector search
SELECT * FROM VECTOR_SEARCH('collection', query_vector, 10)

-- Graph query
SELECT * FROM GRAPH_QUERY('collection', 'MATCH (n)-[r]->(m) RETURN n, r, m')

-- Document query
SELECT * FROM DOCUMENT_QUERY('collection', 'metadata_filter')

-- Combined (hybrid)
SELECT v.id, v.score, g.node
FROM VECTOR_SEARCH('vectors', query, 10) v
JOIN GRAPH_QUERY('graph', 'MATCH ...') g ON v.id = g.id
WHERE v.metadata.language = 'python'
```

---

## Success Criteria

The integration is successful when:

1. **Functionality**:
   - ✅ All Victor features work with ProximaDB backend
   - ✅ Graph extraction from tree-sitter is accurate
   - ✅ Hybrid search returns relevant results
   - ✅ Migration from SQLite/LanceDB preserves data

2. **Performance**:
   - ✅ Indexing matches or exceeds SQLite/LanceDB throughput
   - ✅ Query latency is competitive (p95 < 100ms)
   - ✅ Memory usage is reasonable

3. **Quality**:
   - ✅ Test coverage > 80%
   - ✅ No regressions in existing Victor tests
   - ✅ Documentation is complete

4. **Deployment**:
   - ✅ Configuration is simple
   - ✅ Migration script works reliably
   - ✅ Health checks pass

---

## Next Steps After Victor Session

1. **Report back to ProximaDB session**:
   - What Victor needs from ProximaDB (API gaps, features)
   - Migration challenges and solutions
   - Performance benchmarks and optimizations

2. **Iterate on integration**:
   - Refine ProximaDB provider based on Victor feedback
   - Add missing ProximaDB features if needed
   - Optimize performance bottlenecks

3. **Finalize and test**:
   - End-to-end testing with real codebases
   - Performance benchmarking
   - Documentation and deployment guide

---

*Prepared for Victor AI integration with ProximaDB multi-model store*
*Reference: clients/python/src/proximadb_sdk/integrations/victor_multi.py*
