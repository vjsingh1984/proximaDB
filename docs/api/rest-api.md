# ProximaDB REST API Documentation

The ProximaDB REST API provides a complete HTTP/JSON interface for all vector database operations.

## Base URL

```
http://localhost:5678
```

## Authentication

Include the API key in the Authorization header:

```bash
curl -H "Authorization: Bearer your-api-key" \
     -H "Content-Type: application/json" \
     http://localhost:5678/collections
```

## Collections API

### Create Collection

Create a new vector collection.

**Endpoint:** `POST /collections`

**Request Body:**
```json
{
  "collection_id": "my_vectors",
  "dimension": 128,
  "distance_metric": "cosine",
  "description": "My vector collection"
}
```

**Response:**
```json
{
  "success": true,
  "collection_id": "my_vectors",
  "message": "Collection created successfully"
}
```

**Example:**
```bash
curl -X POST http://localhost:5678/collections \
  -H "Content-Type: application/json" \
  -d '{
    "collection_id": "embeddings",
    "dimension": 384,
    "distance_metric": "cosine"
  }'
```

### List Collections

Get all collections in the database.

**Endpoint:** `GET /collections`

**Response:**
```json
{
  "collections": [
    {
      "id": "embeddings",
      "name": "embeddings",
      "dimension": 384,
      "distance_metric": "cosine",
      "vector_count": 1500,
      "created_at": "2025-01-13T10:30:00Z",
      "updated_at": "2025-01-13T15:45:00Z"
    }
  ]
}
```

**Example:**
```bash
curl http://localhost:5678/collections
```

### Get Collection Details

Get detailed information about a specific collection.

**Endpoint:** `GET /collections/{collection_id}`

**Response:**
```json
{
  "id": "embeddings",
  "name": "embeddings", 
  "dimension": 384,
  "distance_metric": "cosine",
  "vector_count": 1500,
  "total_size_bytes": 2048000,
  "created_at": "2025-01-13T10:30:00Z",
  "updated_at": "2025-01-13T15:45:00Z",
  "index_stats": {
    "algorithm": "hnsw",
    "build_time_ms": 1250,
    "memory_usage_bytes": 512000
  }
}
```

**Example:**
```bash
curl http://localhost:5678/collections/embeddings
```

### Delete Collection

Delete a collection and all its vectors.

**Endpoint:** `DELETE /collections/{collection_id}`

**Response:**
```json
{
  "success": true,
  "message": "Collection embeddings deleted successfully"
}
```

**Example:**
```bash
curl -X DELETE http://localhost:5678/collections/embeddings
```

## Vectors API

### Insert Vector

Insert a single vector into a collection.

**Endpoint:** `POST /collections/{collection_id}/vectors`

**Request Body:**
```json
{
  "id": "vec_001",
  "vector": [0.1, 0.2, 0.3, 0.4],
  "metadata": {
    "document_id": "doc_123",
    "category": "technical",
    "tags": ["ai", "ml"]
  }
}
```

**Response:**
```json
{
  "success": true,
  "vector_id": "vec_001",
  "message": "Vector inserted successfully"
}
```

**Example:**
```bash
curl -X POST http://localhost:5678/collections/embeddings/vectors \
  -H "Content-Type: application/json" \
  -d '{
    "vector": [0.1, 0.2, 0.3, 0.4],
    "metadata": {"document": "example.txt"}
  }'
```

### Get Vector

Retrieve a vector by ID.

**Endpoint:** `GET /collections/{collection_id}/vectors/{vector_id}`

**Response:**
```json
{
  "id": "vec_001",
  "vector": [0.1, 0.2, 0.3, 0.4],
  "metadata": {
    "document_id": "doc_123",
    "category": "technical"
  },
  "timestamp": "2025-01-13T15:45:00Z"
}
```

**Example:**
```bash
curl http://localhost:5678/collections/embeddings/vectors/vec_001
```

### Search Vectors

Perform similarity search in a collection.

**Endpoint:** `POST /collections/{collection_id}/search`

**Request Body:**
```json
{
  "vector": [0.1, 0.2, 0.3, 0.4],
  "k": 10,
  "filter": {
    "category": "technical"
  },
  "threshold": 0.8
}
```

**Response:**
```json
{
  "results": [
    {
      "id": "vec_001",
      "score": 0.95,
      "metadata": {
        "document_id": "doc_123",
        "category": "technical"
      }
    },
    {
      "id": "vec_002", 
      "score": 0.87,
      "metadata": {
        "document_id": "doc_456",
        "category": "technical"
      }
    }
  ],
  "total_count": 2
}
```

**Example:**
```bash
curl -X POST http://localhost:5678/collections/embeddings/search \
  -H "Content-Type: application/json" \
  -d '{
    "vector": [0.1, 0.2, 0.3, 0.4],
    "k": 5
  }'
```

### Delete Vector

Delete a vector from a collection.

**Endpoint:** `DELETE /collections/{collection_id}/vectors/{vector_id}`

**Response:**
```json
{
  "success": true,
  "message": "Vector deleted successfully"
}
```

**Example:**
```bash
curl -X DELETE http://localhost:5678/collections/embeddings/vectors/vec_001
```

## Batch Operations

### Batch Insert Vectors

Insert multiple vectors in a single request.

**Endpoint:** `POST /collections/{collection_id}/vectors/batch`

**Request Body:**
```json
{
  "vectors": [
    {
      "id": "vec_001",
      "vector": [0.1, 0.2, 0.3, 0.4],
      "metadata": {"doc": "1"}
    },
    {
      "id": "vec_002", 
      "vector": [0.5, 0.6, 0.7, 0.8],
      "metadata": {"doc": "2"}
    }
  ]
}
```

**Response:**
```json
{
  "inserted_count": 2,
  "vector_ids": ["vec_001", "vec_002"],
  "errors": []
}
```

**Example:**
```bash
curl -X POST http://localhost:5678/collections/embeddings/vectors/batch \
  -H "Content-Type: application/json" \
  -d '{
    "vectors": [
      {
        "vector": [0.1, 0.2, 0.3, 0.4],
        "metadata": {"source": "doc1"}
      },
      {
        "vector": [0.5, 0.6, 0.7, 0.8], 
        "metadata": {"source": "doc2"}
      }
    ]
  }'
```

### Batch Search

Perform multiple searches in a single request.

**Endpoint:** `POST /batch/search`

**Request Body:**
```json
{
  "searches": [
    {
      "collection_id": "embeddings",
      "vector": [0.1, 0.2, 0.3, 0.4],
      "k": 5
    },
    {
      "collection_id": "embeddings",
      "vector": [0.5, 0.6, 0.7, 0.8],
      "k": 3,
      "filter": {"category": "tech"}
    }
  ]
}
```

**Response:**
```json
{
  "results": [
    {
      "results": [
        {"id": "vec_001", "score": 0.95},
        {"id": "vec_002", "score": 0.87}
      ],
      "total_count": 2
    },
    {
      "results": [
        {"id": "vec_003", "score": 0.92}
      ],
      "total_count": 1
    }
  ]
}
```

## Health and Status

### Health Check

Check if the server is healthy.

**Endpoint:** `GET /health`

**Response:**
```json
{
  "status": "healthy",
  "timestamp": "2025-01-13T15:45:00Z"
}
```

### Readiness Check

Check if the server is ready to accept requests.

**Endpoint:** `GET /ready`

**Response:**
```json
{
  "status": "ready",
  "services": {
    "storage": "ready",
    "indexing": "ready",
    "search": "ready"
  }
}
```

### Liveness Check

Check if the server is alive.

**Endpoint:** `GET /live`

**Response:**
```json
{
  "status": "alive",
  "uptime_seconds": 3600
}
```

### Server Status

Get detailed server status and statistics.

**Endpoint:** `GET /status`

**Response:**
```json
{
  "node_id": "node_12345",
  "version": "0.1.0", 
  "role": "leader",
  "cluster": {
    "peers": [],
    "leader": "node_12345",
    "term": 1
  },
  "storage": {
    "total_vectors": 15000,
    "total_size_bytes": 20480000,
    "collections_count": 5
  },
  "performance": {
    "avg_write_latency_ms": 1.2,
    "avg_read_latency_ms": 0.8,
    "avg_search_latency_ms": 5.4
  }
}
```

## Error Responses

All error responses follow this format:

```json
{
  "error": {
    "code": "COLLECTION_NOT_FOUND",
    "message": "Collection 'nonexistent' not found",
    "details": {
      "collection_id": "nonexistent"
    }
  }
}
```

### HTTP Status Codes

| Code | Description |
|------|-------------|
| 200 | Success |
| 201 | Created |
| 400 | Bad Request |
| 401 | Unauthorized |
| 404 | Not Found |
| 409 | Conflict |
| 429 | Too Many Requests |
| 500 | Internal Server Error |

### Error Codes

| Code | Description |
|------|-------------|
| `COLLECTION_NOT_FOUND` | Collection does not exist |
| `VECTOR_NOT_FOUND` | Vector does not exist |
| `INVALID_DIMENSION` | Vector dimension mismatch |
| `INVALID_VECTOR` | Malformed vector data |
| `COLLECTION_EXISTS` | Collection already exists |
| `RATE_LIMITED` | Too many requests |
| `STORAGE_ERROR` | Internal storage error |

## Request Limits

| Parameter | Limit |
|-----------|-------|
| Vector dimension | 1 - 2048 |
| Batch size | 1000 vectors |
| Search k | 1 - 1000 |
| Collection ID length | 64 characters |
| Vector ID length | 128 characters |
| Metadata size | 64KB |

## SDK Examples

See the [examples directory](examples/) for language-specific SDK examples:

- [Python](examples/python.md)
- [JavaScript](examples/javascript.md) 
- [Java](examples/java.md)
- [Rust](examples/rust.md)