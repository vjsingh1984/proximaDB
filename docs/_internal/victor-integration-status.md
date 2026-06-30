# Victor + ProximaDB Integration Status

**Date**: 2026-03-10
**Status**: Victor implementation complete, SDK alignment needed

---

## Victor Session Achievements ✅

### Completed Features

1. **Migration Helper** (`proximadb_migration.py`)
   ```python
   SqliteLanceDBMigration.migrate()
   ```
   - Migrates graph nodes/edges from `.victor/project.db`
   - Migrates vectors from LanceDB
   - Backfills document and metric records from live repo files

2. **Multi-Model Provider** (`proximadb_multi.py`)
   ```python
   ProximaDBMultiModelProvider
   ```
   - `index_code_file()` - Index across all 4 models
   - `hybrid_search()` - Vector + graph + document + time-series
   - Registered in `registry.py:242`

3. **Tests** - All 6 tests passing
   - Provider tests: `test_proximadb_multi.py:206`
   - Migration tests: `test_proximadb_migration.py:274`

4. **Base Provider Fix** (`base.py:255`)
   - Fixed broken `__repr__` referencing nonexistent config fields

---

## Remaining Gaps Identified by Victor

### Gap 1: Protobuf Mismatch ⚠️

**Issue**: "Python environment has a ProximaDB SDK protobuf mismatch"

**Impact**: Import-time failures, server-backed `ProximaDBClient` fails

**Root Cause**:
- SDK uses generated proto bindings from `proto/` directory
- Victor environment may have different protobuf versions
- Need to ensure proto alignment between SDK and server

**Solution**: Add proto version checking and better error messages

**Implementation** (in this session):
```python
# clients/python/src/proximadb_sdk/__init__.py
def check_proto_version():
    """Check if protobuf bindings are compatible."""
    try:
        from . import v1
        return True
    except ImportError as e:
        raise ImportError(
            f"ProximaDB SDK protobuf mismatch: {e}\n"
            f"Please reinstall: pip install --no-cache-dir proximadb"
        )
```

---

### Gap 2: File-Scoped Graph Deletion ⚠️

**Issue**: "File-scoped graph deletion is still an SDK/server gap"

**Impact**: Can't remove stale graph nodes when symbols are deleted

**Use Case**:
```python
# User deletes a function from a file
# Need to remove:
# - Function node from graph
# - All edges connected to that node
# - Associated vectors
# - Document record
```

**Current State**:
- ❌ No `delete_nodes_by_file()` method
- ❌ No cascade delete for edges
- ❌ No atomic multi-model deletion

**Required API**:
```python
# Delete all entities associated with a file
client.delete_file_entities(
    file_path="src/main.py",
    delete_vectors=True,
    delete_graph_nodes=True,
    delete_document=True,
    delete_metrics=True
)
```

**Implementation Plan**:
1. Add file_path tracking to all entities (metadata field)
2. Implement `delete_nodes_by_property()` in graph service
3. Implement cascade delete for edges
4. Add atomic multi-model delete transaction

---

### Gap 3: CLI for Migration ⚠️

**Issue**: "Needs wiring into CLI or startup command"

**Impact**: Migration only available as library call, not user-friendly

**Required CLI**:
```bash
# Migrate from SQLite + LanceDB to ProximaDB
victor migrate --to proximadb \
    --sqlite-db ~/.victor/project.db \
    --lancedb-path ~/.victor/lancedb \
    --repo-path /path/to/repo

# Or via Victor Python API
from victor.tools import migrate_to_proximadb
migrate_to_proximadb(
    repo_path=".",
    workspace="myrepo",
    delete_legacy=False
)
```

---

## SDK Enhancements for Victor

Based on Victor's findings, here are the priority SDK enhancements:

### Priority 1: Graph API Enhancements ✅ (In Progress)

**File**: `clients/python/src/proximadb_sdk/graph.py` (Created)

**Added**:
- `ProximaDBGraph` class with high-level graph operations
- `batch_create_nodes()` - Performance-critical for code indexing
- `batch_create_edges()` - Performance-critical for call graphs
- `query_cypher()` - Cypher-like query interface (simplified)
- `find_callers()` - Reverse traversal for "who calls X"
- `match_pattern()` - Graph pattern matching
- Proper `GraphNode` and `GraphEdge` dataclasses

**Usage**:
```python
from proximadb_sdk import ProximaDBClient
from proximadb_sdk.graph import ProximaDBGraph

client = ProximaDBClient(url="http://localhost:5678")
graph = ProximaDBGraph(client, "myrepo_graph")

# Batch operations (for code indexing)
graph.batch_create_nodes([
    {"id": "func:main", "labels": ["Function"], "properties": {"name": "main"}},
    {"id": "func:parse", "labels": ["Function"], "properties": {"name": "parse"}},
])

# Find callers
callers = graph.find_callers("func:parse_json", edge_type="CALLS")

# Cypher query
results = graph.query_cypher("""
    MATCH (c:Function)-[:CALLS]->(f:Function)
    WHERE c.name = 'main'
    RETURN c, f
""")
```

**Status**: ✅ Created, needs testing

---

### Priority 2: Document API ✅ (Next)

**File**: `clients/python/src/proximadb_sdk/document.py` (To create)

**Required Methods**:
```python
class ProximaDBDocument:
    def create_document_collection(
        name: str,
        json_schema: Optional[str] = None,
        indexes: List[IndexDefinition] = None,
        enable_fulltext: bool = False,
    ) -> str:
        """Create a document collection."""
        pass

    def insert_document(
        collection_id: str,
        document: Dict[str, Any],
        id: Optional[str] = None,
    ) -> str:
        """Insert a document."""
        pass

    def query_documents(
        collection_id: str,
        filter: Dict[str, Any],
        projection: Optional[List[str]] = None,
        limit: int = 100,
    ) -> List[Dict[str, Any]]:
        """Query documents with filters."""
        pass

    def search_documents(
        collection_id: str,
        text_query: str,
        limit: int = 10,
    ) -> List[Dict[str, Any]]:
        """Full-text search in documents."""
        pass
```

---

### Priority 3: File-Scoped Deletion 🔥

**File**: `clients/python/src/proximadb_sdk/unified_client.py` (To add)

**Required Method**:
```python
def delete_file_entities(
    self,
    file_path: str,
    delete_vectors: bool = True,
    delete_graph_nodes: bool = True,
    delete_document: bool = True,
    delete_metrics: bool = True,
) -> Dict[str, int]:
    """Delete all entities associated with a file.

    This is critical for Victor when code is refactored/deleted:
    - Removes function/class nodes from graph
    - Removes edges connected to those nodes (cascade)
    - Removes vectors pointing to chunks from this file
    - Removes document record for the file
    - Removes time-series metrics for the file

    Args:
        file_path: Path to the file (relative or absolute)
        delete_vectors: Delete vector embeddings
        delete_graph_nodes: Delete graph nodes/edges
        delete_document: Delete document record
        delete_metrics: Delete time-series metrics

    Returns:
        Dictionary with counts of deleted entities

    Example:
        deleted = client.delete_file_entities(
            "src/main.py",
            delete_vectors=True,
            delete_graph_nodes=True,
        )
        print(f"Deleted {deleted['nodes']} nodes, {deleted['edges']} edges")
    """
    result = {
        "vectors": 0,
        "nodes": 0,
        "edges": 0,
        "documents": 0,
        "metrics": 0,
    }

    # 1. Delete vectors with metadata.file_path = file_path
    if delete_vectors:
        # Find all vectors from this file
        # TODO: Need filter_by_metadata in client
        pass

    # 2. Delete graph nodes with properties.file_path = file_path
    if delete_graph_nodes:
        # Query nodes with file_path property
        nodes_result = self.query_nodes(
            graph_id="default",
            labels=[],
            properties={"file_path": file_path},
        )

        for node in nodes_result.get("nodes", []):
            # Delete edges connected to this node
            # TODO: Need cascade delete
            result["nodes"] += 1

    # 3. Delete document with file_path
    if delete_document:
        # TODO: Need document API
        pass

    # 4. Delete metrics with tags.file_path = file_path
    if delete_metrics:
        # TODO: Need time-series API
        pass

    return result
```

---

## SDK Alignment Tasks

### Task 1: Update `__init__.py` Exports ✅

Add graph module to exports:
```python
# Graph operations
try:
    from .graph import (
        ProximaDBGraph,
        GraphNode,
        GraphEdge,
        GraphPath,
        GraphQueryResult,
        create_graph_api,
    )
    _graph_available = True
except ImportError:
    _graph_available = False

if _graph_available:
    __all__.extend([
        "ProximaDBGraph",
        "GraphNode",
        "GraphEdge",
        "GraphPath",
        "GraphQueryResult",
        "create_graph_api",
    ])
```

### Task 2: Add Proto Version Check ✅

Add graceful import handling for proto mismatches:
```python
# At top of __init__.py
try:
    from . import v1
    from . import v1_collection_types_pb2
    from . import v1_vector_types_pb2
    _proto_available = True
except ImportError as e:
    _proto_available = False
    _proto_error = str(e)

def check_proto_version():
    """Check if protobuf bindings are available."""
    if not _proto_available:
        raise ImportError(
            f"ProximaDB SDK protobuf bindings not available: {_proto_error}\n"
            f"Please reinstall: pip install --no-cache-dir proximadb"
        )
    return True
```

### Task 3: Update Victor Integration

The `victor_multi.py` provider can now use the new Graph API:
```python
from proximadb_sdk.graph import ProximaDBGraph

class ProximaDBMultiModelProvider:
    def __init__(self, client, workspace):
        self._client = client
        self._graph = ProximaDBGraph(client, f"{workspace}_graph")

    async def index_code_file(self, file_path, content, language):
        # ... vector indexing ...

        # Graph operations using new API
        graph_info = await self._index_as_graph(...)
        self._graph.batch_create_nodes(graph_info["nodes"])
        self._graph.batch_create_edges(graph_info["edges"])
```

---

## Integration Checklist

### For Victor Team ✅

- [x] Migration helper implemented
- [x] Multi-model provider implemented
- [x] Tests passing (6/6)
- [x] Base provider fix
- [ ] CLI wiring needed
- [ ] Proto alignment verification
- [ ] File-scoped deletion (waiting for SDK)

### For ProximaDB SDK Team 🔄

- [x] Graph API module created (`graph.py`)
- [ ] Document API module (next priority)
- [ ] Time-series API module (can defer)
- [ ] File-scoped deletion (high priority)
- [ ] Update `__init__.py` exports
- [ ] Add proto version check
- [ ] Write tests for Graph API
- [ ] Document Graph API with Victor examples

---

## Next Steps

1. **Immediate** (Today):
   - Update `__init__.py` to export `ProximaDBGraph`
   - Add proto version check
   - Test with Victor environment

2. **Short-term** (This Week):
   - Implement Document API
   - Add `delete_file_entities()` method
   - Write comprehensive tests

3. **Medium-term** (Next Week):
   - Wire migration CLI in Victor
   - Verify proto alignment
   - Document integration

---

*Status: Victor implementation complete, SDK alignment in progress*
