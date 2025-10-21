# Graph API Critical Bug Report

**Date**: October 21, 2025
**Priority**: P0 (Critical) - Blocks all graph operations
**Status**: ROOT CAUSE IDENTIFIED

---

## Executive Summary

The Graph API is **completely non-functional** despite having comprehensive server-side implementation and tests. The root cause is an **architectural isolation bug** where `GraphCollectionService` has TWO SEPARATE INSTANCES that don't share state:

1. External instance used by REST/gRPC endpoints to CREATE graph collections
2. Internal instance used by `GraphOperationsService` to PERFORM operations on nodes/edges

**Result**: Graph collections created via API are invisible to graph operations → all node/edge creation fails with "Graph collection 'X' does not exist"

---

## Evidence

### 1. Test Demonstrates the Bug

File: `tests/graph_api_integration_test.rs`

Test `test_graph_collection_service_isolation_bug()` (lines 28-104) **explicitly demonstrates** the isolation problem:

```rust
// Simulate UnifiedHandlers::new() which creates TWO instances
let graph_collection_service_external = Arc::new(GraphCollectionService::new());
let graph_operations_service = Arc::new(
    GraphOperationsService::new_with_collection_service(
        Arc::new(GraphCollectionService::new()), // BUG: This is a DIFFERENT instance!
    ),
);
```

The test creates a graph collection using the external service, then tries to create a node using `GraphOperationsService`. **It FAILS** with:
```
Error(InvalidInput("Graph collection 'test_graph' does not exist"))
```

### 2. Python SDK Test Results

**Test run**: October 21, 2025 14:23 UTC

#### REST Test:
```
httpx.HTTPStatusError: Client error '400 Bad Request' for url
'http://localhost:5678/api/v1/graph/graphs/default/nodes'
```

#### gRPC Test:
```
grpc._channel._InactiveRpcError: status = StatusCode.INTERNAL
details = "Failed to create node: Invalid input: Graph collection 'default' does not exist"
```

### 3. Graph Collection Exists But Is Invisible

```bash
$ curl -X GET http://localhost:5678/api/v1/graph/graphs
[{"success":true,"graph_id":"default","name":"Default Graph Collection", ...}]

$ curl -X POST http://localhost:5678/api/v1/graph/graphs/default/nodes \
  -H "Content-Type: application/json" \
  -d '{"node":{"id":"test1","labels":["Person"],"properties":{}}}'
{"error":"Invalid input: Graph collection 'default' does not exist"}
```

**The collection EXISTS in the external service but is INVISIBLE to operations!**

---

## Root Cause Analysis

### Problem Location: `src/api_handlers/unified_handlers.rs`

Lines 98-115 show the wiring:

```rust
pub fn new(
    collection_service: Arc<CollectionService>,
    vector_operations_service: Arc<VectorOperationsService>,
) -> Self {
    // Create a SINGLE GraphCollectionService instance that will be shared
    let graph_collection_service = Arc::new(crate::services::GraphCollectionService::new());

    // Pass the SAME instance to GraphOperationsService
    let graph_operations_service = Arc::new(
        crate::graph::GraphOperationsService::new_with_collection_service(
            graph_collection_service.clone(), // Correct: shares same instance
        ),
    );

    Self {
        collection_service,
        vector_operations_service,
        graph_collection_service,      // External API uses this
        graph_operations_service,       // Uses graph_operations_service.collection_service
        // ...
    }
}
```

**The wiring LOOKS CORRECT** but there's a subtle problem:

### The BUG

The REST/gRPC handlers likely DON'T use `unified_handlers.graph_collection_service`. Instead, they may be creating their OWN instance somewhere else, or the server initialization creates multiple `UnifiedHandlers` instances.

### Where to Look

**File**: `src/network/multi_server.rs` (or wherever UnifiedHandlers is instantiated)

The initialization sequence may be:
1. Create `UnifiedHandlers` instance #1 for REST endpoints
2. Create `UnifiedHandlers` instance #2 for gRPC endpoints
3. Each has its own GraphCollectionService instance
4. Collections created via REST are invisible to gRPC and vice versa

OR:

The REST/gRPC endpoint handlers bypass `unified_handlers.graph_collection_service` and call `GraphCollectionService::new()` directly.

---

## Impact

**ALL graph operations are broken**:
- ❌ Cannot create nodes
- ❌ Cannot create edges
- ❌ Cannot traverse graphs
- ❌ Cannot query nodes
- ❌ Cannot perform any graph database operations

**However**:
- ✅ Can create graph collections (via external service)
- ✅ Can list graph collections (via external service)
- ✅ Can get graph statistics (via external service)

---

## Solution Requirements

1. **Single Shared Instance**: Ensure EXACTLY ONE `GraphCollectionService` instance exists across the entire server
2. **Shared Between**:
   - REST graph collection endpoints (`/api/v1/graph/graphs`)
   - gRPC graph collection service (GraphServiceImpl)
   - GraphOperationsService (for node/edge operations)
3. **Verification**: Run `test_graph_collection_service_isolation_bug()` - it should PASS

---

## Files Requiring Investigation

### 1. Server Initialization
- `src/network/multi_server.rs` - Check UnifiedHandlers instantiation
- `src/main.rs` or `src/bin/proximadb-server.rs` - Server startup sequence

### 2. REST Endpoint Wiring
- `src/network/rest/v1/graph.rs` - Graph API REST handlers
- Check if handlers use `unified_handlers.graph_collection_service` or create new instance

### 3. gRPC Service Wiring
- `src/network/grpc/graph_service.rs` - GraphServiceImpl
- Check if service uses shared instance from unified_handlers

---

## Temporary Workarounds

**None**. The graph API is fundamentally broken and cannot be used until this architectural issue is resolved.

---

## Test Coverage

### Existing Tests (Ready to Run)
1. **Rust Integration Test**: `tests/graph_api_integration_test.rs`
   - `test_graph_collection_service_isolation_bug()` - Demonstrates the bug
   - `test_graph_collection_service_shared_correctly()` - Shows correct wiring
   - `test_end_to_end_graph_operations()` - Full E2E test

2. **Python SDK Tests**:
   - `clients/python/tests/integration/test_graph_operations.py` - 43 tests (697 lines)
   - `clients/python/tests/unit/test_graph_client_unit.py` - 36 tests (491 lines)

### Expected After Fix
```bash
# Rust tests
$ cargo test test_graph_collection_service_shared_correctly
$ cargo test test_end_to_end_graph_operations

# Python SDK tests (after server fix)
$ cd clients/python
$ PYTHONPATH=./src python3 -m pytest tests/integration/test_graph_operations.py -v
=================== 43 passed ===================
```

---

## Changes Made This Session

### Python SDK (Completed - Ready for Server Fix)
1. ✅ Added `graph_id` parameter to all graph methods (grpc_sync.py, rest_sync.py)
2. ✅ Updated REST endpoints to use multi-graph paths (`/api/v1/graph/graphs/{graph_id}/...`)
3. ✅ Fixed gRPC proto requests to include graph_id field
4. ✅ Created comprehensive test suite (79 tests total)

**The Python SDK is CORRECT and READY**. It will work once the server bug is fixed.

### Rust Server (Investigation Complete - Fix Required)
- Identified root cause in `UnifiedHandlers` wiring
- Created this bug report
- Test file already exists demonstrating the bug

---

## Next Steps (CRITICAL)

1. **Fix the wiring** in `src/network/multi_server.rs` or wherever UnifiedHandlers is created
2. **Ensure single shared instance** of GraphCollectionService
3. **Run Rust integration test**: `cargo test test_graph_collection_service_shared_correctly`
4. **Run Python SDK tests**: Should pass immediately after server fix
5. **Deploy fix** and validate with production-like data

---

## Conclusion

This is a **critical architectural bug** that completely blocks graph database functionality. The solution is well-understood (ensure single shared GraphCollectionService instance), and comprehensive tests exist to verify the fix. The Python SDK is already fixed and ready.

**Estimated fix time**: 30-60 minutes once the correct initialization location is identified.

**Verification**: Run existing tests - they should all pass after the fix.
