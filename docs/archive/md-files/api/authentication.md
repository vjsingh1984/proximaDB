# ProximaDB Authentication Guide

ProximaDB supports API key-based authentication for both REST and gRPC APIs.

## API Key Authentication

### Generating API Keys

API keys are configured in the server configuration file:

```toml
# config.toml
[api.auth]
enabled = true
api_keys = [
    "pk_live_1234567890abcdef",
    "pk_test_abcdef1234567890",
    "pk_admin_admin123admin456"
]
```

### Key Formats

API keys follow a standard format:
- **Live keys**: `pk_live_` + 16 character string
- **Test keys**: `pk_test_` + 16 character string  
- **Admin keys**: `pk_admin_` + 16 character string

## REST API Authentication

### Authorization Header

Include the API key in the `Authorization` header with `Bearer` scheme:

```bash
curl -H "Authorization: Bearer pk_live_1234567890abcdef" \
     -H "Content-Type: application/json" \
     http://localhost:5678/collections
```

### Examples

**Create Collection:**
```bash
curl -X POST http://localhost:5678/collections \
  -H "Authorization: Bearer pk_live_1234567890abcdef" \
  -H "Content-Type: application/json" \
  -d '{
    "collection_id": "secure_vectors",
    "dimension": 128
  }'
```

**Search Vectors:**
```bash
curl -X POST http://localhost:5678/collections/secure_vectors/search \
  -H "Authorization: Bearer pk_live_1234567890abcdef" \
  -H "Content-Type: application/json" \
  -d '{
    "vector": [0.1, 0.2, 0.3, 0.4],
    "k": 10
  }'
```

## gRPC API Authentication

### Metadata Header

Include the API key in the gRPC metadata:

**Python:**
```python
import grpc
from proximadb.proto import vectordb_pb2_grpc, vectordb_pb2

channel = grpc.insecure_channel('localhost:9090')
stub = vectordb_pb2_grpc.VectorDBStub(channel)

# Include API key in metadata
metadata = [('authorization', 'Bearer pk_live_1234567890abcdef')]

request = vectordb_pb2.CreateCollectionRequest(
    collection_id="secure_vectors",
    dimension=128
)

response = stub.CreateCollection(request, metadata=metadata)
```

**Go:**
```go
import (
    "context"
    "google.golang.org/grpc"
    "google.golang.org/grpc/metadata"
    pb "path/to/vectordb"
)

conn, err := grpc.Dial("localhost:9090", grpc.WithInsecure())
client := pb.NewVectorDBClient(conn)

ctx := metadata.AppendToOutgoingContext(context.Background(), 
    "authorization", "Bearer pk_live_1234567890abcdef")

response, err := client.CreateCollection(ctx, request)
```

**Java:**
```java
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.Metadata;
import io.grpc.stub.MetadataUtils;

ManagedChannel channel = ManagedChannelBuilder
    .forAddress("localhost", 9090)
    .usePlaintext()
    .build();

VectorDBGrpc.VectorDBBlockingStub stub = VectorDBGrpc.newBlockingStub(channel);

Metadata metadata = new Metadata();
metadata.put(Metadata.Key.of("authorization", Metadata.ASCII_STRING_MARSHALLER), 
    "Bearer pk_live_1234567890abcdef");

VectorDBGrpc.VectorDBBlockingStub authenticatedStub = stub
    .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(metadata));

CreateCollectionResponse response = authenticatedStub.createCollection(request);
```

## Client SDK Authentication

### Python Client

```python
from proximadb import ProximaDBClient

client = ProximaDBClient(
    api_key="pk_live_1234567890abcdef",
    host="localhost",
    port=5678
)

# All requests will include the API key automatically
collection = client.create_collection("vectors", dimension=128)
```

### JavaScript Client

```javascript
import { ProximaDBClient } from 'proximadb-js';

const client = new ProximaDBClient({
  apiKey: 'pk_live_1234567890abcdef',
  host: 'localhost',
  port: 5678
});

// All requests will include the API key automatically
const collection = await client.createCollection('vectors', { dimension: 128 });
```

### Java Client

```java
import ai.proximadb.client.ProximaDBClient;
import ai.proximadb.client.config.ClientConfig;

ClientConfig config = ClientConfig.builder()
    .apiKey("pk_live_1234567890abcdef")
    .host("localhost")
    .port(5678)
    .build();

ProximaDBClient client = new ProximaDBClient(config);

// All requests will include the API key automatically
client.createCollection("vectors", 128);
```

## Configuration

### Server Configuration

Enable authentication in the server configuration:

```toml
# config.toml
[api]
rest_port = 5678
grpc_port = 9090

[api.auth]
enabled = true
api_keys = [
    "pk_live_1234567890abcdef",
    "pk_test_abcdef1234567890"
]

# Optional: Rate limiting per key
[api.rate_limit]
enabled = true
requests_per_second = 100
burst_size = 200

# Different limits per key type
[api.rate_limit.live_keys]
requests_per_second = 1000
burst_size = 2000

[api.rate_limit.test_keys]
requests_per_second = 100
burst_size = 200
```

### Environment Variables

API keys can also be configured via environment variables:

```bash
export PROXIMADB_API_KEYS="pk_live_1234567890abcdef,pk_test_abcdef1234567890"
export PROXIMADB_AUTH_ENABLED=true
```

## Security Best Practices

### Key Management

1. **Rotate Keys Regularly**: Update API keys periodically
2. **Environment-Specific Keys**: Use different keys for dev/staging/production
3. **Principle of Least Privilege**: Use test keys for development
4. **Secure Storage**: Store keys in secure configuration management

### Key Types

**Live Keys (`pk_live_`):**
- Full access to all operations
- Use in production environments
- Higher rate limits

**Test Keys (`pk_test_`):**
- Full access to all operations  
- Use in development/testing
- Lower rate limits

**Admin Keys (`pk_admin_`):**
- Administrative operations
- Server management endpoints
- Monitoring and metrics access

### Network Security

1. **HTTPS/TLS**: Always use encrypted connections in production
2. **Firewall Rules**: Restrict access to database ports
3. **VPC/Private Networks**: Deploy in isolated networks
4. **Certificate Pinning**: Pin server certificates for gRPC

## Error Responses

### Invalid API Key

**REST API:**
```json
{
  "error": {
    "code": "UNAUTHORIZED",
    "message": "Invalid API key",
    "details": {
      "hint": "Check your Authorization header format"
    }
  }
}
```
**HTTP Status:** `401 Unauthorized`

**gRPC API:**
```
Status: UNAUTHENTICATED
Details: Invalid API key provided
```

### Missing API Key

**REST API:**
```json
{
  "error": {
    "code": "MISSING_AUTH",
    "message": "Authorization header required",
    "details": {
      "hint": "Include 'Authorization: Bearer your-api-key' header"
    }
  }
}
```
**HTTP Status:** `401 Unauthorized`

**gRPC API:**
```
Status: UNAUTHENTICATED  
Details: Authorization metadata required
```

### Rate Limited

**REST API:**
```json
{
  "error": {
    "code": "RATE_LIMITED",
    "message": "Too many requests",
    "details": {
      "retry_after_seconds": 60,
      "limit": "100 requests per minute"
    }
  }
}
```
**HTTP Status:** `429 Too Many Requests`

**gRPC API:**
```
Status: RESOURCE_EXHAUSTED
Details: Rate limit exceeded, retry after 60 seconds
```

## Testing Authentication

### Curl Examples

**Valid API Key:**
```bash
curl -H "Authorization: Bearer pk_test_validkey123" \
     http://localhost:5678/health

# Response: 200 OK
```

**Invalid API Key:**
```bash
curl -H "Authorization: Bearer invalid_key" \
     http://localhost:5678/health

# Response: 401 Unauthorized
```

**Missing API Key:**
```bash
curl http://localhost:5678/collections

# Response: 401 Unauthorized
```

### Integration Tests

```python
import pytest
import requests

def test_valid_api_key():
    headers = {"Authorization": "Bearer pk_test_validkey123"}
    response = requests.get("http://localhost:5678/health", headers=headers)
    assert response.status_code == 200

def test_invalid_api_key():
    headers = {"Authorization": "Bearer invalid_key"}
    response = requests.get("http://localhost:5678/health", headers=headers)
    assert response.status_code == 401

def test_missing_api_key():
    response = requests.get("http://localhost:5678/health")
    assert response.status_code == 401
```

## Troubleshooting

### Common Issues

1. **"Invalid API key" errors**
   - Check key format and spelling
   - Verify key is configured on server
   - Ensure proper Bearer prefix

2. **"Authorization header required" errors**
   - Include Authorization header
   - Check header name spelling
   - Verify Bearer scheme format

3. **Rate limiting errors**
   - Implement exponential backoff
   - Check rate limit configuration
   - Consider upgrading key type

### Debug Tips

1. **Enable Debug Logging:**
   ```toml
   [logging]
   level = "debug"
   auth_debug = true
   ```

2. **Check Server Logs:**
   ```
   2025-01-13T15:45:00Z [INFO] Auth success: key=pk_live_****def
   2025-01-13T15:45:01Z [WARN] Auth failed: invalid key format
   ```

3. **Validate Configuration:**
   ```bash
   cargo run --bin proximadb-server -- --validate-config
   ```