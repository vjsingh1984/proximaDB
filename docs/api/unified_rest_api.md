# ProximaDB Unified REST API

ProximaDB provides a clean, proto-aligned REST API that matches the gRPC interface for consistency.

## Overview

The unified REST API uses a single endpoint per resource type with operations specified in the request body, similar to gRPC's unary RPC pattern.

## Endpoints

### Health Check
```
GET /health
```

### Collection Operations
```
POST /api/v1/collection
```

Supported operations:
- `create` - Create a new collection
- `get` - Get collection by ID or name
- `list` - List all collections
- `update` - Update collection metadata
- `delete` - Delete a collection

### Vector Operations
```
POST /api/v1/vector/batch
```
Batch insert/update/delete vectors (using expires_at for soft deletes)

```
POST /api/v1/vector/search
```
Search vectors with optional metadata filters

### Internal Testing Endpoints
```
POST /internal/flush
POST /internal/flush/:collection_id
```
⚠️ **WARNING**: These endpoints are for testing only and should not be used in production.

## Request/Response Format

All requests and responses follow proto-aligned structures:

### Collection Operation Request
```json
{
  "operation": "create",
  "collection_id": "optional-for-some-ops",
  "config": {
    "name": "my_collection",
    "dimension": 128,
    "distance_metric": "cosine",
    "storage_engine": "viper",
    "primary_indexing_algorithm": "hnsw",
    "description": "My collection",
    "tags": ["tag1", "tag2"],
    "owner": "user@example.com"
  }
}
```

### Collection Response
```json
{
  "success": true,
  "operation": "create",
  "collection": {
    "id": "uuid",
    "config": { ... },
    "stats": {
      "vector_count": 0,
      "index_size_bytes": 0,
      "data_size_bytes": 0
    },
    "created_at": 1234567890,
    "updated_at": 1234567890
  },
  "processing_time_us": 1234
}
```

### Vector Batch Request
```json
{
  "collection_id": "collection-uuid",
  "vectors": [
    {
      "id": "vec1",
      "vector": [0.1, 0.2, ...],
      "metadata": {
        "key": "value"
      },
      "expires_at": null
    }
  ],
  "batch_timeout_ms": 5000,
  "request_id": "optional-request-id"
}
```

### Vector Search Request
```json
{
  "collection_id": "collection-uuid",
  "queries": [{
    "vector": [0.1, 0.2, ...],
    "metadata_filter": {
      "conditions": [{
        "field_name": "category",
        "operation": "equals",
        "value": "electronics"
      }],
      "operator": "and"
    }
  }],
  "top_k": 10,
  "include_fields": {
    "vector": false,
    "metadata": true,
    "score": true,
    "rank": true
  },
  "search_optimization": {
    "enable_two_stage": true,
    "quantization_hint": {
      "hint_type": "scalar",
      "parameters": {"bits": 8}
    }
  }
}
```

## Key Design Principles

1. **Proto-First**: All types match protobuf definitions
2. **Unified Operations**: Single endpoint per resource with operation field
3. **Clean Structure**: No legacy REST patterns (no /collections/:id/vectors/:id)
4. **Consistent Naming**: Field names match proto exactly
5. **Type Safety**: Proper types for all fields (i32, i64, f32, etc.)

## Migration from Legacy REST APIs

If migrating from traditional REST APIs:

| Legacy Endpoint | Unified Endpoint | Operation |
|----------------|------------------|-----------|
| POST /collections | POST /api/v1/collection | operation: "create" |
| GET /collections | POST /api/v1/collection | operation: "list" |
| GET /collections/:id | POST /api/v1/collection | operation: "get" |
| DELETE /collections/:id | POST /api/v1/collection | operation: "delete" |
| POST /collections/:id/vectors | POST /api/v1/vector/batch | - |
| POST /collections/:id/search | POST /api/v1/vector/search | - |

## Benefits

1. **Consistency**: Same patterns as gRPC
2. **Simplicity**: Fewer endpoints to maintain
3. **Extensibility**: Easy to add new operations
4. **Type Safety**: Proto-derived types ensure compatibility
5. **Performance**: Optimized for batch operations