# ProximaDB gRPC API Documentation

The ProximaDB gRPC API provides a high-performance binary protocol interface using Protocol Buffers.

## Service Definition

```protobuf
service VectorDB {
  // Collection management
  rpc CreateCollection(CreateCollectionRequest) returns (CreateCollectionResponse);
  rpc ListCollections(ListCollectionsRequest) returns (ListCollectionsResponse);
  rpc DeleteCollection(DeleteCollectionRequest) returns (DeleteCollectionResponse);
  
  // Vector operations
  rpc Insert(InsertRequest) returns (InsertResponse);
  rpc Search(SearchRequest) returns (SearchResponse);
  rpc Get(GetRequest) returns (GetResponse);
  rpc Delete(DeleteRequest) returns (DeleteResponse);
  
  // Batch operations
  rpc BatchInsert(BatchInsertRequest) returns (BatchInsertResponse);
  rpc BatchGet(BatchGetRequest) returns (BatchGetResponse);
  
  // Health and status
  rpc Health(HealthRequest) returns (HealthResponse);
  rpc Status(StatusRequest) returns (StatusResponse);
}
```

## Connection

### Default Endpoint
```
localhost:9090
```

### Client Setup Examples

**Python (grpcio)**
```python
import grpc
from proximadb.proto import vectordb_pb2_grpc, vectordb_pb2

channel = grpc.insecure_channel('localhost:9090')
stub = vectordb_pb2_grpc.VectorDBStub(channel)
```

**Go**
```go
import (
    "google.golang.org/grpc"
    pb "path/to/vectordb"
)

conn, err := grpc.Dial("localhost:9090", grpc.WithInsecure())
client := pb.NewVectorDBClient(conn)
```

**Java**
```java
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;

ManagedChannel channel = ManagedChannelBuilder
    .forAddress("localhost", 9090)
    .usePlaintext()
    .build();
VectorDBGrpc.VectorDBBlockingStub stub = VectorDBGrpc.newBlockingStub(channel);
```

## Authentication

Include API key in metadata:

**Python**
```python
metadata = [('authorization', 'Bearer your-api-key')]
response = stub.CreateCollection(request, metadata=metadata)
```

**Go**
```go
ctx := metadata.AppendToOutgoingContext(context.Background(), 
    "authorization", "Bearer your-api-key")
response, err := client.CreateCollection(ctx, request)
```

**Java**
```java
Metadata metadata = new Metadata();
metadata.put(Metadata.Key.of("authorization", Metadata.ASCII_STRING_MARSHALLER), 
    "Bearer your-api-key");
response = stub.withInterceptors(MetadataUtils.newAttachHeadersInterceptor(metadata))
    .createCollection(request);
```

## Collections API

### CreateCollection

Create a new vector collection.

**Request:**
```protobuf
message CreateCollectionRequest {
  string collection_id = 1;
  string name = 2;
  uint32 dimension = 3;
  SchemaType schema_type = 4;
  google.protobuf.Struct schema = 5;
}
```

**Response:**
```protobuf
message CreateCollectionResponse {
  bool success = 1;
  string message = 2;
}
```

**Example (Python):**
```python
request = vectordb_pb2.CreateCollectionRequest(
    collection_id="embeddings",
    name="Text Embeddings",
    dimension=384,
    schema_type=vectordb_pb2.SCHEMA_TYPE_DOCUMENT
)
response = stub.CreateCollection(request)
```

### ListCollections

Get all collections.

**Request:**
```protobuf
message ListCollectionsRequest {}
```

**Response:**
```protobuf
message ListCollectionsResponse {
  repeated Collection collections = 1;
}

message Collection {
  string id = 1;
  string name = 2;
  uint32 dimension = 3;
  SchemaType schema_type = 4;
  google.protobuf.Timestamp created_at = 5;
  google.protobuf.Timestamp updated_at = 6;
  uint64 vector_count = 7;
}
```

**Example (Python):**
```python
request = vectordb_pb2.ListCollectionsRequest()
response = stub.ListCollections(request)

for collection in response.collections:
    print(f"Collection: {collection.id}, Vectors: {collection.vector_count}")
```

### DeleteCollection

Delete a collection and all its vectors.

**Request:**
```protobuf
message DeleteCollectionRequest {
  string collection_id = 1;
}
```

**Response:**
```protobuf
message DeleteCollectionResponse {
  bool success = 1;
  string message = 2;
}
```

**Example (Python):**
```python
request = vectordb_pb2.DeleteCollectionRequest(
    collection_id="embeddings"
)
response = stub.DeleteCollection(request)
```

## Vectors API

### Insert

Insert a single vector.

**Request:**
```protobuf
message InsertRequest {
  string collection_id = 1;
  VectorRecord record = 2;
}

message VectorRecord {
  string id = 1;
  string collection_id = 2;
  repeated float vector = 3;
  google.protobuf.Struct metadata = 4;
  google.protobuf.Timestamp timestamp = 5;
}
```

**Response:**
```protobuf
message InsertResponse {
  bool success = 1;
  string vector_id = 2;
  string message = 3;
}
```

**Example (Python):**
```python
from google.protobuf import struct_pb2, timestamp_pb2

# Create metadata
metadata = struct_pb2.Struct()
metadata["document_id"] = "doc_123"
metadata["category"] = "technical"

# Create vector record
record = vectordb_pb2.VectorRecord(
    id="vec_001",
    collection_id="embeddings",
    vector=[0.1, 0.2, 0.3, 0.4],
    metadata=metadata,
    timestamp=timestamp_pb2.Timestamp()
)

request = vectordb_pb2.InsertRequest(
    collection_id="embeddings",
    record=record
)
response = stub.Insert(request)
```

### Search

Perform similarity search.

**Request:**
```protobuf
message SearchRequest {
  string collection_id = 1;
  repeated float vector = 2;
  uint32 k = 3;
  google.protobuf.Struct filters = 4;
  optional float threshold = 5;
}
```

**Response:**
```protobuf
message SearchResponse {
  repeated SearchResult results = 1;
  uint32 total_count = 2;
}

message SearchResult {
  string id = 1;
  float score = 2;
  google.protobuf.Struct metadata = 3;
}
```

**Example (Python):**
```python
# Basic search
request = vectordb_pb2.SearchRequest(
    collection_id="embeddings",
    vector=[0.1, 0.2, 0.3, 0.4],
    k=10
)
response = stub.Search(request)

for result in response.results:
    print(f"Vector: {result.id}, Score: {result.score}")

# Search with filters
filters = struct_pb2.Struct()
filters["category"] = "technical"

request = vectordb_pb2.SearchRequest(
    collection_id="embeddings",
    vector=[0.1, 0.2, 0.3, 0.4],
    k=10,
    filters=filters,
    threshold=0.8
)
response = stub.Search(request)
```

### Get

Retrieve a vector by ID.

**Request:**
```protobuf
message GetRequest {
  string collection_id = 1;
  string vector_id = 2;
}
```

**Response:**
```protobuf
message GetResponse {
  optional VectorRecord record = 1;
}
```

**Example (Python):**
```python
request = vectordb_pb2.GetRequest(
    collection_id="embeddings",
    vector_id="vec_001"
)
response = stub.Get(request)

if response.HasField('record'):
    record = response.record
    print(f"Vector: {record.vector}")
    print(f"Metadata: {record.metadata}")
```

### Delete

Delete a vector.

**Request:**
```protobuf
message DeleteRequest {
  string collection_id = 1;
  string vector_id = 2;
}
```

**Response:**
```protobuf
message DeleteResponse {
  bool success = 1;
  string message = 2;
}
```

**Example (Python):**
```python
request = vectordb_pb2.DeleteRequest(
    collection_id="embeddings",
    vector_id="vec_001"
)
response = stub.Delete(request)
```

## Batch Operations

### BatchInsert

Insert multiple vectors in a single request.

**Request:**
```protobuf
message BatchInsertRequest {
  string collection_id = 1;
  repeated VectorRecord records = 2;
}
```

**Response:**
```protobuf
message BatchInsertResponse {
  uint32 inserted_count = 1;
  repeated string vector_ids = 2;
  repeated string errors = 3;
}
```

**Example (Python):**
```python
records = []
for i in range(100):
    metadata = struct_pb2.Struct()
    metadata["index"] = str(i)
    
    record = vectordb_pb2.VectorRecord(
        collection_id="embeddings",
        vector=[0.1 * i, 0.2 * i, 0.3 * i, 0.4 * i],
        metadata=metadata
    )
    records.append(record)

request = vectordb_pb2.BatchInsertRequest(
    collection_id="embeddings",
    records=records
)
response = stub.BatchInsert(request)
print(f"Inserted {response.inserted_count} vectors")
```

### BatchGet

Retrieve multiple vectors by ID.

**Request:**
```protobuf
message BatchGetRequest {
  string collection_id = 1;
  repeated string vector_ids = 2;
}
```

**Response:**
```protobuf
message BatchGetResponse {
  repeated VectorRecord records = 1;
}
```

**Example (Python):**
```python
request = vectordb_pb2.BatchGetRequest(
    collection_id="embeddings",
    vector_ids=["vec_001", "vec_002", "vec_003"]
)
response = stub.BatchGet(request)

for record in response.records:
    print(f"Vector {record.id}: {record.vector}")
```

## Health and Status

### Health

Check server health.

**Request:**
```protobuf
message HealthRequest {}
```

**Response:**
```protobuf
message HealthResponse {
  string status = 1;
  google.protobuf.Timestamp timestamp = 2;
}
```

**Example (Python):**
```python
request = vectordb_pb2.HealthRequest()
response = stub.Health(request)
print(f"Health: {response.status}")
```

### Status

Get detailed server status.

**Request:**
```protobuf
message StatusRequest {}
```

**Response:**
```protobuf
message StatusResponse {
  string node_id = 1;
  string version = 2;
  NodeRole role = 3;
  ClusterInfo cluster = 4;
  StorageInfo storage = 5;
}

message ClusterInfo {
  repeated string peers = 1;
  string leader = 2;
  uint64 term = 3;
}

message StorageInfo {
  uint64 total_vectors = 1;
  uint64 total_size_bytes = 2;
  repeated DiskInfo disks = 3;
}
```

**Example (Python):**
```python
request = vectordb_pb2.StatusRequest()
response = stub.Status(request)

print(f"Node ID: {response.node_id}")
print(f"Version: {response.version}")
print(f"Total Vectors: {response.storage.total_vectors}")
```

## Error Handling

gRPC errors use standard status codes:

### Status Codes

| Code | Description |
|------|-------------|
| `OK` | Success |
| `INVALID_ARGUMENT` | Invalid request parameters |
| `NOT_FOUND` | Collection/vector not found |
| `ALREADY_EXISTS` | Collection already exists |
| `RESOURCE_EXHAUSTED` | Rate limited |
| `INTERNAL` | Internal server error |
| `UNAUTHENTICATED` | Invalid API key |

### Error Handling Example (Python)

```python
import grpc

try:
    response = stub.CreateCollection(request)
except grpc.RpcError as e:
    if e.code() == grpc.StatusCode.ALREADY_EXISTS:
        print("Collection already exists")
    elif e.code() == grpc.StatusCode.INVALID_ARGUMENT:
        print(f"Invalid argument: {e.details()}")
    else:
        print(f"Error: {e.code()} - {e.details()}")
```

## Performance Considerations

### Connection Pooling

Reuse gRPC channels for better performance:

```python
# Good: Reuse channel
channel = grpc.insecure_channel('localhost:9090')
stub = vectordb_pb2_grpc.VectorDBStub(channel)

# Use stub for multiple requests
for i in range(1000):
    response = stub.Insert(request)
```

### Batch Operations

Use batch operations for better throughput:

```python
# Good: Batch insert
records = [create_record(i) for i in range(1000)]
response = stub.BatchInsert(BatchInsertRequest(records=records))

# Less efficient: Individual inserts
for record in records:
    response = stub.Insert(InsertRequest(record=record))
```

### Async/Streaming

Use async stubs for non-blocking operations:

```python
import grpc.aio

async def async_search():
    async with grpc.aio.insecure_channel('localhost:9090') as channel:
        stub = vectordb_pb2_grpc.VectorDBStub(channel)
        response = await stub.Search(request)
        return response
```

## Protocol Buffer Schema

The complete `.proto` files are available in the repository:

- [vectordb.proto](../../proto/vectordb.proto) - Main service definition
- Generated client libraries for multiple languages

## Client Libraries

Pre-built client libraries are available:

- **Python**: `pip install proximadb-python`
- **JavaScript**: `npm install proximadb-js`
- **Java**: Maven/Gradle dependencies
- **Go**: `go get github.com/proximadb/proximadb-go`
- **Rust**: `cargo add proximadb-client`

See [Client SDKs](../clients/) for detailed documentation.