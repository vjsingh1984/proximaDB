# ProximaDB API Documentation

## Overview

ProximaDB provides both REST and gRPC APIs for comprehensive vector database operations. This documentation covers all available endpoints, request/response formats, and usage examples.

## Authentication

All API requests require authentication using one of the following methods:

### API Key Authentication
```
Authorization: Bearer <api_key>
```

### JWT Token Authentication
```
Authorization: Bearer <jwt_token>
```

### Request Headers
- `x-tenant-id`: Tenant identifier for multi-tenant deployments
- `Content-Type`: `application/json` for REST, `application/grpc` for gRPC
- `User-Agent`: Client identification (e.g., `proximadb-python/0.1.0`)

## REST API Endpoints

### Base URL
- Development: `http://localhost:5678/api/v1`
- Production: `https://api.proximadb.ai/v1`

### Collections Management

#### Create Collection
```http
POST /collections
Content-Type: application/json

{
  "name": "my_collection",
  "dimension": 512,
  "schema_type": "Document",
  "index_config": {
    "index_type": "HNSW",
    "parameters": {
      "m": 16,
      "ef_construction": 200
    }
  },
  "retention_policy": {
    "ttl_seconds": 604800
  }
}
```

**Response:**
```json
{
  "id": "coll_123456789",
  "name": "my_collection",
  "dimension": 512,
  "schema_type": "Document",
  "index_config": {
    "index_type": "HNSW",
    "parameters": {
      "m": 16,
      "ef_construction": 200
    }
  },
  "retention_policy": {
    "ttl_seconds": 604800
  },
  "created_at": "2025-01-13T10:30:00Z",
  "updated_at": "2025-01-13T10:30:00Z",
  "vector_count": 0,
  "status": "active"
}
```

#### List Collections
```http
GET /collections?limit=50&offset=0
```

**Response:**
```json
{
  "collections": [
    {
      "id": "coll_123456789",
      "name": "my_collection",
      "dimension": 512,
      "vector_count": 1000,
      "status": "active",
      "created_at": "2025-01-13T10:30:00Z"
    }
  ],
  "total": 1,
  "limit": 50,
  "offset": 0
}
```

#### Get Collection
```http
GET /collections/{collection_id}
```

#### Update Collection
```http
PUT /collections/{collection_id}
Content-Type: application/json

{
  "name": "updated_collection",
  "retention_policy": {
    "ttl_seconds": 1209600
  }
}
```

#### Delete Collection
```http
DELETE /collections/{collection_id}
```

### Vector Operations

#### Insert Vector
```http
POST /collections/{collection_id}/vectors
Content-Type: application/json

{
  "id": "vec_12345",
  "vector": [0.1, 0.2, 0.3, ...],
  "metadata": {
    "title": "Example Document",
    "category": "research",
    "priority": 5,
    "tags": ["ml", "nlp"]
  },
  "expires_at": "2025-12-31T23:59:59Z"
}
```

**Response:**
```json
{
  "id": "vec_12345",
  "collection_id": "coll_123456789",
  "status": "inserted",
  "timestamp": "2025-01-13T10:30:00Z"
}
```

#### Batch Insert Vectors
```http
POST /collections/{collection_id}/vectors/batch
Content-Type: application/json

{
  "vectors": [
    {
      "id": "vec_1",
      "vector": [0.1, 0.2, ...],
      "metadata": {"category": "A"}
    },
    {
      "id": "vec_2", 
      "vector": [0.3, 0.4, ...],
      "metadata": {"category": "B"}
    }
  ]
}
```

**Response:**
```json
{
  "inserted": 2,
  "failed": 0,
  "results": [
    {
      "id": "vec_1",
      "status": "inserted",
      "timestamp": "2025-01-13T10:30:00Z"
    },
    {
      "id": "vec_2",
      "status": "inserted", 
      "timestamp": "2025-01-13T10:30:01Z"
    }
  ]
}
```

#### Get Vector
```http
GET /collections/{collection_id}/vectors/{vector_id}
```

**Response:**
```json
{
  "id": "vec_12345",
  "collection_id": "coll_123456789",
  "vector": [0.1, 0.2, 0.3, ...],
  "metadata": {
    "title": "Example Document",
    "category": "research",
    "priority": 5
  },
  "timestamp": "2025-01-13T10:30:00Z",
  "expires_at": "2025-12-31T23:59:59Z"
}
```

#### Update Vector
```http
PUT /collections/{collection_id}/vectors/{vector_id}
Content-Type: application/json

{
  "vector": [0.2, 0.3, 0.4, ...],
  "metadata": {
    "title": "Updated Document",
    "category": "research",
    "priority": 3
  }
}
```

#### Delete Vector
```http
DELETE /collections/{collection_id}/vectors/{vector_id}
```

### Vector Search

#### Similarity Search
```http
POST /collections/{collection_id}/search
Content-Type: application/json

{
  "vector": [0.1, 0.2, 0.3, ...],
  "k": 10,
  "similarity_threshold": 0.8,
  "metadata_filters": {
    "category": "research",
    "priority": {"$gte": 3}
  },
  "return_vectors": true,
  "return_metadata": true
}
```

**Response:**
```json
{
  "results": [
    {
      "id": "vec_12345",
      "similarity_score": 0.95,
      "vector": [0.1, 0.2, 0.3, ...],
      "metadata": {
        "title": "Similar Document",
        "category": "research"
      }
    }
  ],
  "total_found": 1,
  "query_time_ms": 2.5,
  "next_page_token": null
}
```

#### Hybrid Search (AXIS)
```http
POST /collections/{collection_id}/search/hybrid
Content-Type: application/json

{
  "vector_query": {
    "vector": [0.1, 0.2, 0.3, ...],
    "weight": 0.7
  },
  "metadata_filters": [
    {
      "field": "category",
      "operator": "equals",
      "value": "research"
    },
    {
      "field": "priority",
      "operator": "greater_than",
      "value": 2
    }
  ],
  "id_filters": ["vec_1", "vec_2", "vec_3"],
  "k": 20,
  "similarity_threshold": 0.6,
  "return_vectors": true,
  "return_metadata": true
}
```

#### Range Search
```http
POST /collections/{collection_id}/search/range
Content-Type: application/json

{
  "vector": [0.1, 0.2, 0.3, ...],
  "similarity_threshold": 0.7,
  "max_results": 1000,
  "metadata_filters": {
    "created_date": {
      "$gte": "2025-01-01T00:00:00Z",
      "$lte": "2025-01-31T23:59:59Z"
    }
  }
}
```

#### Multi-Vector Search
```http
POST /collections/{collection_id}/search/multi
Content-Type: application/json

{
  "vectors": [
    [0.1, 0.2, 0.3, ...],
    [0.4, 0.5, 0.6, ...]
  ],
  "aggregation_method": "average",
  "k": 15,
  "similarity_threshold": 0.75
}
```

### Analytics and Statistics

#### Collection Statistics
```http
GET /collections/{collection_id}/stats
```

**Response:**
```json
{
  "vector_count": 10000,
  "total_size_bytes": 41943040,
  "average_vector_size": 2048,
  "index_size_bytes": 8388608,
  "last_updated": "2025-01-13T10:30:00Z",
  "distribution": {
    "sparsity_levels": {
      "dense": 7500,
      "sparse": 2500
    },
    "metadata_fields": {
      "category": 10000,
      "priority": 8500,
      "tags": 6000
    }
  }
}
```

#### Query Performance Metrics
```http
GET /collections/{collection_id}/metrics?start_time=2025-01-01T00:00:00Z&end_time=2025-01-31T23:59:59Z
```

## gRPC API

### Protocol Buffer Definitions

```protobuf
syntax = "proto3";

package proximadb.v1;

service VectorService {
  // Collection management
  rpc CreateCollection(CreateCollectionRequest) returns (Collection);
  rpc GetCollection(GetCollectionRequest) returns (Collection);
  rpc ListCollections(ListCollectionsRequest) returns (ListCollectionsResponse);
  rpc UpdateCollection(UpdateCollectionRequest) returns (Collection);
  rpc DeleteCollection(DeleteCollectionRequest) returns (DeleteCollectionResponse);
  
  // Vector operations
  rpc InsertVector(InsertVectorRequest) returns (InsertVectorResponse);
  rpc BatchInsertVectors(stream BatchInsertVectorRequest) returns (BatchInsertVectorResponse);
  rpc GetVector(GetVectorRequest) returns (Vector);
  rpc UpdateVector(UpdateVectorRequest) returns (UpdateVectorResponse);
  rpc DeleteVector(DeleteVectorRequest) returns (DeleteVectorResponse);
  
  // Search operations
  rpc SearchSimilar(SearchSimilarRequest) returns (SearchSimilarResponse);
  rpc SearchHybrid(SearchHybridRequest) returns (SearchHybridResponse);
  rpc SearchRange(SearchRangeRequest) returns (SearchRangeResponse);
  rpc SearchMultiVector(SearchMultiVectorRequest) returns (SearchMultiVectorResponse);
  
  // Streaming operations
  rpc StreamSearch(SearchSimilarRequest) returns (stream SearchResult);
  rpc StreamInsert(stream InsertVectorRequest) returns (stream InsertVectorResponse);
}

message Vector {
  string id = 1;
  string collection_id = 2;
  repeated float vector = 3;
  map<string, google.protobuf.Value> metadata = 4;
  google.protobuf.Timestamp timestamp = 5;
  optional google.protobuf.Timestamp expires_at = 6;
}

message Collection {
  string id = 1;
  string name = 2;
  int32 dimension = 3;
  SchemaType schema_type = 4;
  IndexConfig index_config = 5;
  RetentionPolicy retention_policy = 6;
  google.protobuf.Timestamp created_at = 7;
  google.protobuf.Timestamp updated_at = 8;
  int64 vector_count = 9;
  CollectionStatus status = 10;
}

message SearchSimilarRequest {
  string collection_id = 1;
  repeated float vector = 2;
  int32 k = 3;
  optional float similarity_threshold = 4;
  map<string, google.protobuf.Value> metadata_filters = 5;
  bool return_vectors = 6;
  bool return_metadata = 7;
}

message SearchSimilarResponse {
  repeated SearchResult results = 1;
  int64 total_found = 2;
  float query_time_ms = 3;
  optional string next_page_token = 4;
}

message SearchResult {
  string id = 1;
  float similarity_score = 2;
  optional Vector vector = 3;
  map<string, google.protobuf.Value> metadata = 4;
}
```

### gRPC Client Examples

#### Python Client
```python
import grpc
from proximadb.grpc import vector_service_pb2
from proximadb.grpc import vector_service_pb2_grpc

# Create channel
channel = grpc.secure_channel('api.proximadb.ai:443', grpc.ssl_channel_credentials())
stub = vector_service_pb2_grpc.VectorServiceStub(channel)

# Create collection
collection_request = vector_service_pb2.CreateCollectionRequest(
    name="my_collection",
    dimension=512,
    schema_type=vector_service_pb2.SCHEMA_TYPE_DOCUMENT
)
collection = stub.CreateCollection(collection_request)

# Insert vector
vector_request = vector_service_pb2.InsertVectorRequest(
    collection_id=collection.id,
    vector=vector_service_pb2.Vector(
        id="vec_1",
        vector=[0.1, 0.2, 0.3] * 170 + [0.4, 0.5],  # 512 dimensions
        metadata={"category": "example"}
    )
)
response = stub.InsertVector(vector_request)

# Search vectors
search_request = vector_service_pb2.SearchSimilarRequest(
    collection_id=collection.id,
    vector=[0.1, 0.2, 0.3] * 170 + [0.4, 0.5],
    k=10,
    similarity_threshold=0.8,
    return_vectors=True,
    return_metadata=True
)
search_response = stub.SearchSimilar(search_request)
```

#### Java Client
```java
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import ai.proximadb.grpc.VectorServiceGrpc;
import ai.proximadb.grpc.VectorService.*;

// Create channel
ManagedChannel channel = ManagedChannelBuilder
    .forAddress("api.proximadb.ai", 443)
    .useTransportSecurity()
    .build();

VectorServiceGrpc.VectorServiceBlockingStub stub = 
    VectorServiceGrpc.newBlockingStub(channel);

// Create collection
CreateCollectionRequest collectionRequest = CreateCollectionRequest.newBuilder()
    .setName("my_collection")
    .setDimension(512)
    .setSchemaType(SchemaType.SCHEMA_TYPE_DOCUMENT)
    .build();

Collection collection = stub.createCollection(collectionRequest);

// Insert vector
InsertVectorRequest vectorRequest = InsertVectorRequest.newBuilder()
    .setCollectionId(collection.getId())
    .setVector(Vector.newBuilder()
        .setId("vec_1")
        .addAllVector(Arrays.asList(0.1f, 0.2f, 0.3f, /* ... */))
        .putMetadata("category", Value.newBuilder().setStringValue("example").build())
        .build())
    .build();

InsertVectorResponse response = stub.insertVector(vectorRequest);
```

## Error Handling

### HTTP Status Codes
- `200 OK`: Request successful
- `201 Created`: Resource created successfully
- `400 Bad Request`: Invalid request parameters
- `401 Unauthorized`: Authentication required
- `403 Forbidden`: Insufficient permissions
- `404 Not Found`: Resource not found
- `409 Conflict`: Resource already exists
- `422 Unprocessable Entity`: Validation errors
- `429 Too Many Requests`: Rate limit exceeded
- `500 Internal Server Error`: Server error
- `503 Service Unavailable`: Service temporarily unavailable

### Error Response Format
```json
{
  "error": {
    "code": "INVALID_VECTOR_DIMENSION",
    "message": "Vector dimension must be 512, got 256",
    "details": {
      "field": "vector",
      "expected": 512,
      "actual": 256
    },
    "request_id": "req_123456789",
    "timestamp": "2025-01-13T10:30:00Z"
  }
}
```

### Common Error Codes
- `INVALID_REQUEST`: Malformed request
- `INVALID_VECTOR_DIMENSION`: Vector dimension mismatch
- `COLLECTION_NOT_FOUND`: Collection does not exist
- `VECTOR_NOT_FOUND`: Vector does not exist
- `INVALID_METADATA_FILTER`: Invalid metadata filter syntax
- `RATE_LIMIT_EXCEEDED`: Too many requests
- `QUOTA_EXCEEDED`: Resource quota exceeded
- `AUTHENTICATION_FAILED`: Invalid credentials
- `AUTHORIZATION_FAILED`: Insufficient permissions

## Rate Limiting

### Default Limits
- **Free Tier**: 100 requests/minute, 1,000 vectors/day
- **Starter Tier**: 1,000 requests/minute, 100,000 vectors/day
- **Pro Tier**: 10,000 requests/minute, 10,000,000 vectors/day
- **Enterprise Tier**: Custom limits

### Rate Limit Headers
```
X-RateLimit-Limit: 1000
X-RateLimit-Remaining: 999
X-RateLimit-Reset: 1642099200
X-RateLimit-Retry-After: 60
```

## Pagination

### Request Parameters
- `limit`: Number of results per page (default: 50, max: 1000)
- `offset`: Number of results to skip (default: 0)
- `cursor`: Cursor-based pagination token

### Response Format
```json
{
  "results": [...],
  "pagination": {
    "limit": 50,
    "offset": 0,
    "total": 1000,
    "has_more": true,
    "next_cursor": "eyJpZCI6IjEyMyJ9"
  }
}
```

## Webhooks

### Event Types
- `collection.created`: Collection created
- `collection.updated`: Collection updated
- `collection.deleted`: Collection deleted
- `vector.inserted`: Vector inserted
- `vector.updated`: Vector updated
- `vector.deleted`: Vector deleted
- `search.completed`: Search query completed

### Webhook Payload
```json
{
  "event_type": "vector.inserted",
  "event_id": "evt_123456789",
  "timestamp": "2025-01-13T10:30:00Z",
  "tenant_id": "tenant_abc",
  "data": {
    "collection_id": "coll_123456789",
    "vector_id": "vec_12345",
    "metadata": {
      "category": "research"
    }
  }
}
```

## SDKs and Client Libraries

### Official SDKs
- **Python**: `pip install proximadb`
- **JavaScript/TypeScript**: `npm install @proximadb/client`
- **Java**: `implementation 'ai.proximadb:proximadb-java:0.1.0'`
- **Go**: `go get github.com/proximadb/proximadb-go`
- **Rust**: `cargo add proximadb-client`

### Community SDKs
- **C#**: `dotnet add package ProximaDB.Client`
- **PHP**: `composer require proximadb/client`
- **Ruby**: `gem install proximadb`

## Performance Optimization

### Best Practices
1. **Batch Operations**: Use batch insert for multiple vectors
2. **Connection Pooling**: Reuse connections for better performance
3. **Metadata Indexing**: Structure metadata for efficient filtering
4. **Vector Normalization**: Normalize vectors for consistent similarity
5. **Caching**: Cache frequently accessed vectors locally

### Query Optimization
1. **Use Metadata Filters**: Pre-filter with metadata before vector search
2. **Adjust k Value**: Use appropriate k value for your use case
3. **Similarity Threshold**: Set reasonable similarity thresholds
4. **Index Configuration**: Choose appropriate index type for your data
5. **Pagination**: Use cursor-based pagination for large result sets

## Monitoring and Observability

### Metrics Endpoints
```http
GET /metrics/prometheus
GET /health
GET /health/ready
GET /health/live
```

### Available Metrics
- Request latency percentiles (P50, P95, P99)
- Request rate and error rate
- Vector operations per second
- Index performance metrics
- Memory and storage utilization
- Cache hit/miss ratios

### Distributed Tracing
ProximaDB supports OpenTelemetry for distributed tracing:
- Jaeger integration
- Zipkin integration
- Custom trace exporters

## Security

### Data Encryption
- **At Rest**: AES-256 encryption
- **In Transit**: TLS 1.3 with perfect forward secrecy
- **Key Management**: Customer-managed encryption keys (CMEK)

### Access Control
- **RBAC**: Role-based access control
- **API Keys**: Scoped API key permissions
- **JWT**: JSON Web Token authentication
- **SAML/OAuth2**: Enterprise identity provider integration

### Compliance
- **SOC 2 Type II**: Security and availability
- **GDPR**: Data protection and privacy
- **HIPAA**: Healthcare data protection
- **CCPA**: California privacy compliance

## Glossary

- **Collection**: A named group of vectors with the same dimension
- **Vector**: A numerical representation of data (embeddings)
- **Metadata**: Key-value pairs associated with vectors
- **Similarity Score**: Measure of vector similarity (0.0 to 1.0)
- **Index**: Data structure for efficient vector search
- **Dimension**: Number of components in a vector
- **TTL**: Time-to-live for automatic data expiration
- **Tenant**: Isolated workspace for multi-tenant deployments