# ProximaDB API Documentation

## Overview

ProximaDB provides both REST and gRPC APIs for vector operations. Both APIs share the same underlying implementation for consistency.

## REST API (Port 5678)

### Base URL
```
http://localhost:5678
```

### Endpoints

#### Collections

**Create Collection**
```http
POST /collections
Content-Type: application/json

{
  "name": "products",
  "dimensions": 384,
  "metric": "cosine",
  "engine": "sst"
}
```

**Get Collection**
```http
GET /collections/{name}
```

**List Collections**
```http
GET /collections
```

**Delete Collection**
```http
DELETE /collections/{name}
```

#### Vectors

**Insert Vectors**
```http
POST /collections/{name}/vectors
Content-Type: application/json

{
  "vectors": [
    {
      "id": "vec1",
      "vector": [0.1, 0.2, ...],
      "metadata": {"category": "electronics"}
    }
  ]
}
```

**Get Vector**
```http
GET /collections/{name}/vectors/{id}
```

**Update Vector**
```http
PUT /collections/{name}/vectors/{id}
Content-Type: application/json

{
  "vector": [0.1, 0.2, ...],
  "metadata": {"category": "updated"}
}
```

**Delete Vector**
```http
DELETE /collections/{name}/vectors/{id}
```

#### Search

**Vector Search**
```http
POST /collections/{name}/search
Content-Type: application/json

{
  "vector": [0.1, 0.2, ...],
  "k": 10,
  "filter": {
    "category": {"$eq": "electronics"},
    "price": {"$lt": 1000}
  }
}
```

**SQL Query**
```http
POST /sql
Content-Type: application/json

{
  "query": "SELECT * FROM products WHERE category = 'electronics' ORDER BY COSINE_DISTANCE(embedding, [0.1, 0.2, ...]) LIMIT 10"
}
```

#### System

**Health Check**
```http
GET /health

Response:
{
  "status": "healthy",
  "version": "1.0.0",
  "uptime_seconds": 3600
}
```

**Metrics**
```http
GET /metrics

Response: Prometheus format metrics
```

## gRPC API (Port 5679)

### Proto Definition
```protobuf
service VectorService {
  rpc CreateCollection(CreateCollectionRequest) returns (CreateCollectionResponse);
  rpc InsertVectors(InsertVectorsRequest) returns (InsertVectorsResponse);
  rpc SearchVectors(SearchRequest) returns (SearchResponse);
  rpc StreamSearch(SearchRequest) returns (stream SearchResult);
}

message VectorRecord {
  string id = 1;
  repeated float vector = 2;
  map<string, Value> metadata = 3;
}

message SearchRequest {
  string collection = 1;
  repeated float vector = 2;
  uint32 k = 3;
  MetadataFilter filter = 4;
}
```

### gRPC Client Examples

**Python**
```python
import grpc
import proximadb_pb2
import proximadb_pb2_grpc

channel = grpc.insecure_channel('localhost:5679')
stub = proximadb_pb2_grpc.VectorServiceStub(channel)

# Search vectors
request = proximadb_pb2.SearchRequest(
    collection="products",
    vector=[0.1, 0.2, ...],
    k=10
)
response = stub.SearchVectors(request)
```

**Go**
```go
conn, _ := grpc.Dial("localhost:5679", grpc.WithInsecure())
client := pb.NewVectorServiceClient(conn)

response, _ := client.SearchVectors(context.Background(), &pb.SearchRequest{
    Collection: "products",
    Vector: []float32{0.1, 0.2, ...},
    K: 10,
})
```

## Authentication

### API Key
```http
Authorization: Bearer your-api-key
```

### JWT Token
```http
Authorization: Bearer eyJhbGciOiJIUzI1NiIs...
```

## Rate Limiting

Default limits:
- 1000 requests/second per IP
- 10MB max request size
- 60 second timeout

## Error Codes

| Code | Description |
|------|-------------|
| 200 | Success |
| 400 | Bad Request |
| 401 | Unauthorized |
| 404 | Not Found |
| 429 | Rate Limited |
| 500 | Internal Error |

## SDK Support

Official SDKs:
- [Python](https://github.com/proximadb/python-sdk)
- [Go](https://github.com/proximadb/go-sdk)
- [Java](https://github.com/proximadb/java-sdk)
- [JavaScript](https://github.com/proximadb/js-sdk)