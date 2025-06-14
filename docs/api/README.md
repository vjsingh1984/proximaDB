# ProximaDB API Documentation

ProximaDB provides two API interfaces for interacting with the vector database:

- **REST API** - HTTP/JSON interface for easy integration
- **gRPC API** - High-performance binary protocol for optimized throughput

## Quick Links

- [REST API Documentation](rest-api.md)
- [gRPC API Documentation](grpc-api.md)
- [Authentication Guide](authentication.md)
- [Error Handling](error-handling.md)

## Client Examples

- [Python Examples](examples/python.md) - Complete Python client examples for REST and gRPC
- [JavaScript Examples](examples/javascript.md) - Complete JavaScript/TypeScript client examples for REST and gRPC
- [Java Examples](examples/java.md) - Complete Java client examples for REST and gRPC
- [Rust Examples](examples/rust.md) - Complete Rust client examples for REST and gRPC

## Getting Started

### Starting ProximaDB Server

```bash
# Start with default configuration
cargo run --bin proximadb-server

# Start with custom configuration
cargo run --bin proximadb-server -- --config custom.toml

# Start with specific ports
cargo run --bin proximadb-server -- --rest-port 8080 --grpc-port 9090
```

### Quick Test

```bash
# Health check via REST
curl http://localhost:5678/health

# Create a collection via REST
curl -X POST http://localhost:5678/collections \
  -H "Content-Type: application/json" \
  -d '{
    "collection_id": "test_vectors",
    "dimension": 128,
    "distance_metric": "cosine"
  }'
```

## API Comparison

| Feature | REST API | gRPC API |
|---------|----------|----------|
| Protocol | HTTP/JSON | Binary/Protobuf |
| Performance | Good | Excellent |
| Browser Support | ✅ Native | ❌ Requires proxy |
| Streaming | ❌ Limited | ✅ Full support |
| Type Safety | ❌ Runtime | ✅ Compile-time |
| Human Readable | ✅ Yes | ❌ Binary |

## Base URLs

- **REST API**: `http://localhost:5678` (default)
- **gRPC API**: `localhost:9090` (default)

## Authentication

Both APIs support API key authentication:

```bash
# REST API
curl -H "Authorization: Bearer your-api-key" http://localhost:5678/collections

# gRPC API
# Include metadata: authorization: Bearer your-api-key
```

See [Authentication Guide](authentication.md) for detailed setup.

## Rate Limiting

Both APIs implement token bucket rate limiting:

- Default: 100 requests per second per client
- Configurable per endpoint and client
- 429 status code when limit exceeded

## Error Handling

Both APIs provide consistent error responses:

- **REST**: HTTP status codes with JSON error details
- **gRPC**: gRPC status codes with error messages

See [Error Handling Guide](error-handling.md) for complete reference.