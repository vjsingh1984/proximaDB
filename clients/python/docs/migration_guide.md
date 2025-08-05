# ProximaDB Python SDK Migration Guide

## Migrating from v0.x to v1.0

This guide helps you migrate from older versions of the ProximaDB Python SDK to the new v1.0 release. Version 1.0 introduces a complete rewrite with a cleaner API, better performance, and production-ready features.

## Table of Contents
1. [Overview](#overview)
2. [Breaking Changes](#breaking-changes)
3. [Installation](#installation)
4. [Client Initialization](#client-initialization)
5. [Collection Management](#collection-management)
6. [Vector Operations](#vector-operations)
7. [Search Operations](#search-operations)
8. [Configuration Changes](#configuration-changes)
9. [Error Handling](#error-handling)
10. [Advanced Features](#advanced-features)
11. [Migration Checklist](#migration-checklist)

## Overview

### What's New in v1.0
- **Unified client API** - Single client class for both REST and gRPC
- **Async-first design** - Native async/await support with sync wrappers
- **Production features** - Connection pooling, circuit breakers, retries
- **Better error handling** - Structured exceptions with context
- **Type safety** - Full type hints and dataclass models
- **Extensibility** - Interceptors, custom serializers, telemetry

### Removed in v1.0
- Legacy REST-only client
- Callback-based async patterns
- String-based configuration
- Untyped dictionary responses
- Global client instances

## Breaking Changes

### 1. Import Changes
```python
# Old (v0.x)
from proximadb import Client, RESTClient, GRPCClient
from proximadb.config import Config

# New (v1.0)
from proximadb import ProximaDBClient, ClientConfig
from proximadb.models import CollectionConfig, VectorRecord
```

### 2. Response Format Changes
```python
# Old (v0.x) - Dictionary responses
response = client.create_collection(...)
if response["success"]:
    collection_id = response["data"]["id"]

# New (v1.0) - Typed model responses
collection = client.create_collection(...)
collection_id = collection.id  # Direct attribute access
```

### 3. Async API Changes
```python
# Old (v0.x) - Callback-based
def callback(result):
    print(result)
client.search_async(query, callback=callback)

# New (v1.0) - Native async/await
result = await client.asearch_vectors(query)
# or sync wrapper
result = client.search_vectors(query)
```

## Installation

### Uninstall Old Version
```bash
pip uninstall proximadb-client proximadb
```

### Install New Version
```bash
# Basic installation
pip install proximadb>=1.0.0

# With all features
pip install proximadb[all]>=1.0.0
```

## Client Initialization

### Old Way (v0.x)
```python
# REST client
client = RESTClient(
    host="localhost",
    port=5678,
    timeout=30
)

# gRPC client
client = GRPCClient(
    host="localhost",
    port=5679,
    use_ssl=False
)

# Configuration file
client = Client.from_config("config.json")
```

### New Way (v1.0)
```python
# Unified client with URL
client = ProximaDBClient("http://localhost:5678")  # REST
client = ProximaDBClient("grpc://localhost:5679")  # gRPC

# With configuration object
config = ClientConfig(
    url="http://localhost:5678",
    timeout=30.0,
    max_retries=3
)
client = ProximaDBClient(config)

# With connection pooling (production)
from proximadb import ResilientProximaDBClient
client = ResilientProximaDBClient(
    config=config,
    pool_config={"min_size": 5, "max_size": 20}
)
```

## Collection Management

### Creating Collections

#### Old Way (v0.x)
```python
response = client.create_collection(
    name="my_vectors",
    dimension=384,
    metric="cosine",
    engine="viper"
)
collection_id = response["data"]["id"]
```

#### New Way (v1.0)
```python
from proximadb.models import CollectionConfig, DistanceMetric, StorageEngine

# Using config object
config = CollectionConfig(
    name="my_vectors",
    dimension=384,
    distance_metric=DistanceMetric.COSINE,
    storage_engine=StorageEngine.VIPER
)
collection = client.create_collection(config)

# Using builder pattern
collection = client.collections.create()
    .name("my_vectors")
    .dimension(384)
    .distance_metric("cosine")
    .storage_engine("viper")
    .build()
```

### Listing Collections

#### Old Way (v0.x)
```python
response = client.list_collections()
for coll in response["data"]:
    print(f"{coll['name']}: {coll['vector_count']} vectors")
```

#### New Way (v1.0)
```python
collections = client.list_collections()
for collection in collections:
    print(f"{collection.name}: {collection.vector_count} vectors")
```

## Vector Operations

### Inserting Vectors

#### Old Way (v0.x)
```python
# Single vector
response = client.insert_vector(
    collection="my_collection",
    vector_id="vec_123",
    values=[0.1, 0.2, 0.3],
    metadata={"type": "example"}
)

# Bulk insert
vectors = [
    {
        "id": f"vec_{i}",
        "values": embedding.tolist(),
        "metadata": {"batch": 1}
    }
    for i, embedding in enumerate(embeddings)
]
response = client.bulk_insert(collection, vectors)
```

#### New Way (v1.0)
```python
from proximadb.models import VectorRecord

# Single vector
vector = VectorRecord(
    id="vec_123",
    vector=[0.1, 0.2, 0.3],
    metadata={"type": "example"}
)
response = client.insert_vector("my_collection", vector)

# Bulk insert with options
vectors = [
    VectorRecord(
        id=f"vec_{i}",
        vector=embedding.tolist(),
        metadata={"batch": 1}
    )
    for i, embedding in enumerate(embeddings)
]
response = client.insert_vectors(
    "my_collection",
    vectors,
    options=InsertOptions(batch_size=1000, upsert=True)
)
```

### Updating Vectors

#### Old Way (v0.x)
```python
response = client.update_vector(
    collection="my_collection",
    vector_id="vec_123",
    values=[0.2, 0.3, 0.4],
    metadata={"updated": True}
)
```

#### New Way (v1.0)
```python
# Upsert by default with insert_vector
vector = VectorRecord(
    id="vec_123",
    vector=[0.2, 0.3, 0.4],
    metadata={"updated": True}
)
response = client.insert_vector(
    "my_collection",
    vector,
    options=InsertOptions(upsert=True)
)
```

## Search Operations

### Basic Search

#### Old Way (v0.x)
```python
results = client.search(
    collection="my_collection",
    query_vector=[0.1, 0.2, 0.3],
    top_k=10,
    include_metadata=True
)
for result in results["data"]:
    print(f"ID: {result['id']}, Score: {result['score']}")
```

#### New Way (v1.0)
```python
results = client.search_vectors(
    collection_name="my_collection",
    query_vector=[0.1, 0.2, 0.3],
    top_k=10
)
for result in results.results:
    print(f"ID: {result.id}, Score: {result.score}")
    print(f"Metadata: {result.metadata}")
```

### Filtered Search

#### Old Way (v0.x)
```python
results = client.search_with_filters(
    collection="my_collection",
    query_vector=query_embedding,
    top_k=20,
    filters={
        "category": "electronics",
        "price": {"$lt": 1000}
    }
)
```

#### New Way (v1.0)
```python
from proximadb.models import SearchOptions, FilterCondition, FilterOperator

options = SearchOptions(
    top_k=20,
    filter_conditions=[
        FilterCondition(
            field="metadata.category",
            operator=FilterOperator.EQUALS,
            value="electronics"
        ),
        FilterCondition(
            field="metadata.price",
            operator=FilterOperator.LESS_THAN,
            value=1000
        )
    ]
)
results = client.search_vectors(
    "my_collection",
    query_embedding,
    options=options
)
```

## Configuration Changes

### Environment Variables

#### Old Way (v0.x)
```python
# Limited environment variable support
PROXIMADB_HOST=localhost
PROXIMADB_PORT=5678
```

#### New Way (v1.0)
```python
# Comprehensive environment variable support
PROXIMADB_URL=http://localhost:5678
PROXIMADB_TIMEOUT=30
PROXIMADB_MAX_RETRIES=3
PROXIMADB_LOG_LEVEL=INFO
```

### Configuration Files

#### Old Way (v0.x)
```json
{
    "host": "localhost",
    "port": 5678,
    "protocol": "http",
    "timeout": 30
}
```

#### New Way (v1.0)
```yaml
# config.yaml
url: http://localhost:5678
timeout: 30.0
retry:
  max_attempts: 3
  backoff_strategy: exponential
pool:
  min_size: 5
  max_size: 20
telemetry:
  enabled: true
  export_interval: 60.0
```

## Error Handling

### Old Way (v0.x)
```python
try:
    response = client.search(...)
    if not response["success"]:
        print(f"Error: {response['error']}")
except Exception as e:
    print(f"Request failed: {e}")
```

### New Way (v1.0)
```python
from proximadb.exceptions import (
    ProximaDBError,
    CollectionNotFoundError,
    DimensionMismatchError,
    QuotaExceededError
)

try:
    results = client.search_vectors(...)
except CollectionNotFoundError as e:
    print(f"Collection {e.collection_name} not found")
except DimensionMismatchError as e:
    print(f"Expected dimension {e.expected}, got {e.provided}")
except QuotaExceededError as e:
    print(f"Quota exceeded: {e.quota_type}")
except ProximaDBError as e:
    print(f"Database error: {e}")
```

## Advanced Features

### Connection Pooling (New in v1.0)
```python
from proximadb import ResilientProximaDBClient

client = ResilientProximaDBClient(
    config=ClientConfig(url="http://localhost:5678"),
    pool_config={
        "min_size": 5,
        "max_size": 20,
        "max_idle_time": 300
    }
)

# Monitor pool health
stats = client.get_pool_stats()
print(f"Active connections: {stats['in_use_connections']}")
```

### Circuit Breakers (New in v1.0)
```python
client = ResilientProximaDBClient(
    config=config,
    circuit_breaker_config={
        "failure_threshold": 5,
        "timeout": 60.0,
        "success_threshold": 2
    }
)
```

### Request Interceptors (New in v1.0)
```python
from proximadb.interceptors import (
    InterceptorChain,
    AuthenticationInterceptor,
    LoggingInterceptor
)

interceptors = InterceptorChain([
    AuthenticationInterceptor(auth_token="your-api-key"),
    LoggingInterceptor(log_level=logging.DEBUG)
])
client.set_interceptors(interceptors)
```

### Streaming Operations (New in v1.0)
```python
from proximadb.streaming import VectorStream

# Stream large datasets
stream = VectorStream(
    client,
    collection_name="large_dataset",
    batch_size=1000
)

async def vector_generator():
    for i in range(1000000):
        yield VectorRecord(
            id=f"vec_{i}",
            vector=generate_embedding(i)
        )

metrics = await stream.insert_stream(vector_generator())
print(f"Throughput: {metrics.throughput} vectors/sec")
```

## Migration Checklist

### Before Migration
- [ ] Review all breaking changes
- [ ] Backup your data
- [ ] Test in development environment
- [ ] Update dependencies

### Code Changes
- [ ] Update all imports to new module structure
- [ ] Replace dictionary access with model attributes
- [ ] Update client initialization
- [ ] Convert callbacks to async/await
- [ ] Update error handling
- [ ] Replace filter syntax

### Configuration Updates
- [ ] Update environment variables
- [ ] Convert configuration files
- [ ] Add connection pooling settings
- [ ] Configure retry strategies

### Testing
- [ ] Run unit tests with new SDK
- [ ] Test all CRUD operations
- [ ] Verify search functionality
- [ ] Check error handling
- [ ] Validate performance

### Production Deployment
- [ ] Enable telemetry and monitoring
- [ ] Configure circuit breakers
- [ ] Set up proper connection pooling
- [ ] Implement graceful shutdown
- [ ] Monitor for errors

## Common Migration Patterns

### Pattern 1: Wrapper for Compatibility
```python
class LegacyCompatibleClient:
    """Wrapper to maintain old API during migration"""
    
    def __init__(self, url):
        self.client = ProximaDBClient(url)
    
    def search(self, collection, query_vector, top_k, **kwargs):
        # Convert old format to new
        results = self.client.search_vectors(
            collection_name=collection,
            query_vector=query_vector,
            top_k=top_k
        )
        # Convert response to old format
        return {
            "success": True,
            "data": [
                {
                    "id": r.id,
                    "score": r.score,
                    "metadata": r.metadata
                }
                for r in results.results
            ]
        }
```

### Pattern 2: Gradual Migration
```python
# Phase 1: Use new client with compatibility layer
client = ProximaDBClient(url)
legacy_client = LegacyClient(url)  # Keep for comparison

# Phase 2: Migrate read operations
results = client.search_vectors(...)  # New API
# Keep write operations on old client temporarily

# Phase 3: Migrate write operations
client.insert_vectors(...)  # Fully migrated

# Phase 4: Remove legacy client
```

### Pattern 3: Feature Detection
```python
def get_client(url):
    """Get appropriate client based on server version"""
    try:
        # Try v1.0 client first
        client = ProximaDBClient(url)
        client.list_collections()  # Test connection
        return client
    except Exception:
        # Fall back to legacy client
        return LegacyClient(url)
```

## Troubleshooting

### Connection Issues
```python
# Old client might work but new client fails
# Check protocol in URL
client = ProximaDBClient("http://localhost:5678")  # Explicit HTTP
client = ProximaDBClient("grpc://localhost:5679")  # Explicit gRPC
```

### Response Format Issues
```python
# If expecting dictionary but getting model
if hasattr(response, '__dict__'):
    # Convert model to dict if needed
    response_dict = response.to_dict()
```

### Async Compatibility
```python
# If your code is sync but new SDK is async
import asyncio

def sync_wrapper(async_func, *args, **kwargs):
    loop = asyncio.new_event_loop()
    try:
        return loop.run_until_complete(async_func(*args, **kwargs))
    finally:
        loop.close()

# Use sync methods provided by SDK instead
result = client.search_vectors(...)  # Sync version
# Instead of
# result = sync_wrapper(client.asearch_vectors, ...)
```

## Support and Resources

### Getting Help
- GitHub Issues: [github.com/proximadb/proximadb-python/issues](https://github.com/proximadb/proximadb-python/issues)
- Documentation: [docs.proximadb.com](https://docs.proximadb.com)
- Discord Community: [discord.gg/proximadb](https://discord.gg/proximadb)

### Migration Support
- Migration scripts: Available in `examples/migration/`
- Compatibility layer: `proximadb.compat` module
- Support email: support@proximadb.com

### Version Compatibility
| SDK Version | Server Version | Support Status |
|-------------|----------------|----------------|
| v0.x        | v0.x - v1.0    | Deprecated     |
| v1.0        | v1.0+          | Active         |

---

*Last updated: 2025-08-05*