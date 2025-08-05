# ProximaDB Python SDK Developer Guide

## Table of Contents
1. [Introduction](#introduction)
2. [Installation](#installation)
3. [Quick Start](#quick-start)
4. [Architecture Overview](#architecture-overview)
5. [Core Features](#core-features)
6. [Advanced Features](#advanced-features)
7. [Production Best Practices](#production-best-practices)
8. [API Reference](#api-reference)
9. [Troubleshooting](#troubleshooting)

## Introduction

The ProximaDB Python SDK v1.0 is a modern, async-first client library for interacting with ProximaDB vector database. It provides a clean, intuitive API with enterprise-grade features including connection pooling, circuit breakers, retry strategies, and comprehensive telemetry.

### Key Features
- **Async-first design** with sync wrappers for compatibility
- **Protocol agnostic** - seamlessly switch between REST and gRPC
- **Production ready** - connection pooling, circuit breakers, retries
- **Observable** - built-in telemetry and monitoring
- **Extensible** - interceptors, custom serializers, pluggable components

## Installation

### Basic Installation
```bash
pip install proximadb
```

### With Optional Dependencies
```bash
# For gRPC support
pip install proximadb[grpc]

# For advanced serialization
pip install proximadb[serializers]

# For all features
pip install proximadb[all]
```

### Development Installation
```bash
git clone https://github.com/proximadb/proximadb-python
cd proximadb-python
pip install -e ".[dev]"
```

## Quick Start

### Basic Usage

```python
from proximadb import ProximaDBClient, ClientConfig
from proximadb.models import CollectionConfig, VectorRecord

# Initialize client
client = ProximaDBClient(
    ClientConfig(url="http://localhost:5678")
)

# Create a collection
async def create_collection():
    config = CollectionConfig(
        name="my_vectors",
        dimension=384,
        distance_metric="cosine"
    )
    collection = await client.acreate_collection(config)
    print(f"Created collection: {collection.id}")

# Insert vectors
async def insert_vectors():
    vectors = [
        VectorRecord(
            id="vec_1",
            vector=[0.1] * 384,
            metadata={"category": "example"}
        )
    ]
    response = await client.ainsert_vectors("my_vectors", vectors)
    print(f"Inserted {response.success_count} vectors")

# Search vectors
async def search_vectors():
    results = await client.asearch_vectors(
        collection_name="my_vectors",
        query_vector=[0.1] * 384,
        top_k=10
    )
    for result in results.results:
        print(f"ID: {result.id}, Score: {result.score}")

# Run async functions
import asyncio
asyncio.run(create_collection())
asyncio.run(insert_vectors())
asyncio.run(search_vectors())
```

### Synchronous Usage

```python
# The SDK provides sync wrappers for all async methods
client = ProximaDBClient("http://localhost:5678")

# Sync operations
collection = client.create_collection(config)
response = client.insert_vectors("my_vectors", vectors)
results = client.search_vectors("my_vectors", query_vector, top_k=10)
```

## Architecture Overview

### Transport Layer
The SDK uses a transport abstraction that allows seamless switching between REST and gRPC protocols:

```python
# REST client (default)
client = ProximaDBClient("http://localhost:5678")

# gRPC client
client = ProximaDBClient("grpc://localhost:5679")

# Explicit protocol selection
client = ProximaDBClient(
    ClientConfig(
        url="localhost:5679",
        protocol=Protocol.GRPC
    )
)
```

### Client Hierarchy

```
ProximaDBClient          # Base client with core functionality
    ↓
ResilientProximaDBClient # Enhanced with pooling & circuit breakers
    ↓
InstrumentedClient       # With telemetry and monitoring
```

## Core Features

### Collection Management

```python
from proximadb.models import CollectionConfig, DistanceMetric, StorageEngine

# Create collection with full configuration
config = CollectionConfig(
    name="product_embeddings",
    dimension=768,
    distance_metric=DistanceMetric.COSINE,
    storage_engine=StorageEngine.VIPER,
    metadata={
        "description": "Product catalog embeddings",
        "model": "BERT-base"
    }
)

collection = await client.acreate_collection(config)

# List collections
collections = await client.alist_collections()
for coll in collections:
    print(f"{coll.name}: {coll.vector_count} vectors")

# Get collection details
collection = await client.aget_collection("product_embeddings")
print(f"Dimension: {collection.dimension}")
print(f"Storage: {collection.storage_engine}")

# Delete collection
await client.adelete_collection("product_embeddings")
```

### Vector Operations

```python
# Single vector insertion
vector = VectorRecord(
    id="product_123",
    vector=embeddings.tolist(),
    metadata={
        "name": "Laptop",
        "price": 999.99,
        "category": "electronics"
    }
)
response = await client.ainsert_vector("products", vector)

# Bulk insertion with progress tracking
vectors = [
    VectorRecord(
        id=f"product_{i}",
        vector=embedding,
        metadata={"batch": batch_id}
    )
    for i, embedding in enumerate(embeddings)
]

response = await client.ainsert_vectors(
    "products",
    vectors,
    options=InsertOptions(
        batch_size=1000,
        upsert=True  # Update if exists
    )
)

print(f"Success: {response.success_count}/{response.total_count}")
if response.errors:
    for error in response.errors:
        print(f"Error: {error}")
```

### Search Operations

```python
from proximadb.models import SearchOptions, FilterOperator

# Basic search
results = await client.asearch_vectors(
    collection_name="products",
    query_vector=query_embedding,
    top_k=20
)

# Advanced search with filters
search_options = SearchOptions(
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
    ],
    include_metadata=True,
    include_vectors=False
)

results = await client.asearch_vectors(
    "products",
    query_embedding,
    options=search_options
)

# Process results
for result in results.results:
    print(f"ID: {result.id}")
    print(f"Score: {result.score}")
    print(f"Metadata: {result.metadata}")
```

### SQL Interface

```python
# SQL query with vector similarity
sql_query = """
SELECT id, metadata.name, metadata.price
FROM products
WHERE metadata.category = 'electronics'
  AND metadata.in_stock = true
ORDER BY VECTOR_SIMILARITY(vector, :query_vector, 'cosine')
LIMIT 10
"""

results = await client.aexecute_sql(
    sql_query,
    parameters={
        "query_vector": query_embedding.tolist()
    }
)

for row in results.rows:
    print(f"{row['id']}: {row['name']} - ${row['price']}")
```

## Advanced Features

### Connection Pooling

```python
from proximadb import ResilientProximaDBClient

# Client with connection pooling
client = ResilientProximaDBClient(
    config=ClientConfig(url="http://localhost:5678"),
    pool_config={
        "min_size": 5,        # Minimum connections
        "max_size": 20,       # Maximum connections
        "max_idle_time": 300, # Seconds before closing idle connections
        "health_check_interval": 30
    }
)

# Monitor pool statistics
stats = client.get_pool_stats()
print(f"Active connections: {stats['in_use_connections']}")
print(f"Available connections: {stats['available_connections']}")
```

### Circuit Breakers

```python
# Client with circuit breaker protection
client = ResilientProximaDBClient(
    config=ClientConfig(url="http://localhost:5678"),
    circuit_breaker_config={
        "failure_threshold": 5,      # Failures before opening
        "success_threshold": 2,      # Successes to close
        "timeout": 60.0,            # Seconds before half-open
        "error_threshold_percentage": 50.0
    }
)

# Monitor circuit breaker status
cb_stats = client.get_circuit_breaker_stats()
for operation, stats in cb_stats.items():
    print(f"{operation}: {stats['state']}")
    print(f"  Error rate: {stats['error_percentage']:.1f}%")
```

### Retry Strategies

```python
from proximadb.retry import RetryConfig, BackoffStrategy, with_retry

# Configure retry behavior
retry_config = RetryConfig(
    max_attempts=5,
    initial_delay=0.5,
    max_delay=30.0,
    backoff_strategy=BackoffStrategy.EXPONENTIAL_JITTER,
    retry_on_exceptions={TransportError, ServerError},
    retry_on_status_codes={429, 502, 503, 504}
)

# Apply to specific operations
@with_retry(max_attempts=3, backoff_strategy=BackoffStrategy.LINEAR)
async def reliable_search():
    return await client.asearch_vectors(...)

# Or configure globally
client.set_retry_config(retry_config)
```

### Request Interceptors

```python
from proximadb.interceptors import (
    InterceptorChain,
    AuthenticationInterceptor,
    LoggingInterceptor,
    MetricsInterceptor,
    ValidationInterceptor
)

# Create interceptor chain
interceptors = InterceptorChain([
    # Add authentication
    AuthenticationInterceptor(
        auth_token=os.getenv("PROXIMADB_API_KEY"),
        auth_scheme="Bearer"
    ),
    
    # Add request/response logging
    LoggingInterceptor(
        log_level=logging.DEBUG,
        log_request_body=True,
        max_body_length=1000
    ),
    
    # Add validation
    ValidationInterceptor(
        max_vector_dimension=2048,
        max_metadata_size=1024 * 1024  # 1MB
    ),
    
    # Add metrics collection
    MetricsInterceptor()
])

client.set_interceptors(interceptors)
```

### Caching

```python
from proximadb.cache import CacheManager, QueryCache, EvictionPolicy

# Configure caching
cache_manager = CacheManager(
    query_cache=QueryCache(
        backend=InMemoryCache(
            max_size=1000,
            max_memory=100 * 1024 * 1024,  # 100MB
            eviction_policy=EvictionPolicy.LRU
        ),
        default_ttl=300.0,  # 5 minutes
        cache_search=True,
        cache_get=True
    )
)

client.set_cache_manager(cache_manager)

# Monitor cache performance
stats = cache_manager.get_stats()
print(f"Cache hit rate: {stats['query_cache']['hit_rate']:.1%}")
```

### Streaming Large Datasets

```python
from proximadb.streaming import VectorStream, ChunkedUploader

# Stream vectors for insertion
stream = VectorStream(
    client,
    collection_name="large_dataset",
    batch_size=1000,
    max_concurrent_batches=5,
    progress_callback=lambda metrics: print(
        f"Progress: {metrics.processed_items}/{metrics.total_items}"
    )
)

# From async generator
async def vector_generator():
    for i in range(1000000):
        yield VectorRecord(
            id=f"vec_{i}",
            vector=generate_embedding(i),
            metadata={"index": i}
        )

metrics = await stream.insert_stream(vector_generator())
print(f"Throughput: {metrics.throughput:.0f} vectors/sec")

# From files
uploader = ChunkedUploader(client, "large_dataset")

# Upload from JSONL
await uploader.upload_jsonl("vectors.jsonl")

# Upload from CSV
await uploader.upload_csv(
    "vectors.csv",
    vector_column="embedding",
    metadata_columns=["name", "category"]
)

# Upload from numpy
await uploader.upload_numpy(
    embeddings=np_array,
    ids=[f"vec_{i}" for i in range(len(np_array))],
    metadata_list=[{"batch": 1} for _ in range(len(np_array))]
)
```

### Telemetry and Monitoring

```python
from proximadb.telemetry import init_telemetry, ConsoleExporter, HTTPExporter
from proximadb.telemetry_decorators import instrument_client

# Initialize telemetry
telemetry = init_telemetry(
    exporters=[
        ConsoleExporter(),  # Log to console
        HTTPExporter("http://metrics.example.com/v1/metrics")
    ],
    export_interval=60.0  # Export every minute
)

await telemetry.start()

# Instrument client
client = instrument_client(client)

# Custom metrics
telemetry.metrics_collector.increment_counter(
    "custom_operations",
    value=1.0,
    labels={"operation": "batch_process", "status": "success"}
)

# Custom spans for tracing
from proximadb.telemetry_decorators import SpanContext

async with SpanContext("batch_processing") as span:
    span.set_tag("batch_size", 1000)
    span.set_tag("collection", "products")
    
    # Process batch
    for i in range(batches):
        span.log("processing_batch", batch_id=i)
        await process_batch(i)
```

## Production Best Practices

### 1. Connection Management

```python
# Use connection pooling for production
client = ResilientProximaDBClient(
    config=ClientConfig(
        url="http://proximadb.prod.example.com",
        timeout=30.0,
        max_retries=3
    ),
    pool_config={
        "min_size": 10,
        "max_size": 50,
        "max_idle_time": 300,
        "health_check_interval": 30
    }
)

# Proper cleanup
try:
    await client.aconnect()
    # Use client
finally:
    await client.adisconnect()
```

### 2. Error Handling

```python
from proximadb.exceptions import (
    ProximaDBError,
    CollectionNotFoundError,
    DimensionMismatchError,
    QuotaExceededError,
    TransportError
)

try:
    results = await client.asearch_vectors(...)
except CollectionNotFoundError:
    # Handle missing collection
    await client.acreate_collection(...)
except DimensionMismatchError as e:
    # Handle dimension mismatch
    logger.error(f"Vector dimension {e.provided} doesn't match {e.expected}")
except QuotaExceededError:
    # Handle quota limits
    await implement_backpressure()
except TransportError:
    # Network issues - rely on retry logic
    pass
except ProximaDBError as e:
    # General ProximaDB errors
    logger.error(f"Operation failed: {e}")
```

### 3. Batch Operations

```python
from proximadb.batching import RequestBatcher, BatchConfig, BatchStrategy

# Configure intelligent batching
batcher = RequestBatcher(
    client,
    config=BatchConfig(
        strategy=BatchStrategy.ADAPTIVE,
        max_batch_size=1000,
        max_wait_time=0.1,  # 100ms
        target_latency=0.05  # 50ms target
    )
)

# Batch operations automatically
async def process_items(items):
    tasks = []
    for item in items:
        task = batcher.batch_insert_vectors(
            "collection",
            [create_vector(item)]
        )
        tasks.append(task)
    
    results = await asyncio.gather(*tasks)
    return results

# Monitor batching performance
metrics = batcher.get_metrics()
for operation, stats in metrics.items():
    print(f"{operation}:")
    print(f"  Average batch size: {stats['average_batch_size']:.1f}")
    print(f"  Average latency: {stats['average_latency']:.3f}s")
```

### 4. Performance Optimization

```python
# Use appropriate storage engines
config = CollectionConfig(
    name="analytics_data",
    dimension=256,
    storage_engine=StorageEngine.VIPER,  # Columnar for analytics
    index_type=IndexType.HNSW,
    index_params={
        "m": 16,
        "ef_construction": 200
    }
)

# Optimize search operations
search_options = SearchOptions(
    top_k=100,
    ef_search=200,  # Higher for better recall
    include_metadata=True,
    include_vectors=False,  # Skip if not needed
    timeout=5.0
)

# Use streaming for large results
search_stream = SearchStream(
    client,
    collection_name="large_collection",
    query_vector=query,
    page_size=100,
    max_results=10000
)

async for result in search_stream:
    process_result(result)
```

### 5. Monitoring and Alerting

```python
# Set up comprehensive monitoring
class MonitoringClient(ResilientProximaDBClient):
    async def health_check(self):
        """Custom health check implementation"""
        try:
            # Check basic connectivity
            collections = await self.alist_collections()
            
            # Check specific collection
            test_collection = await self.aget_collection("health_check")
            
            # Perform test search
            results = await self.asearch_vectors(
                "health_check",
                query_vector=[0.1] * 128,
                top_k=1
            )
            
            return {
                "status": "healthy",
                "collections": len(collections),
                "latency_ms": results.query_time_ms
            }
        except Exception as e:
            return {
                "status": "unhealthy",
                "error": str(e)
            }

# Export metrics to monitoring system
from proximadb.telemetry import PrometheusExporter

telemetry = init_telemetry(
    exporters=[
        PrometheusExporter(port=9090),
        DatadogExporter(api_key=os.getenv("DD_API_KEY"))
    ]
)
```

## API Reference

### Client Classes

#### ProximaDBClient
Base client class providing core functionality.

```python
class ProximaDBClient:
    def __init__(self, config: Union[str, ClientConfig, Dict[str, Any]])
    
    # Collection operations
    async def acreate_collection(self, config: CollectionConfig) -> Collection
    async def aget_collection(self, name: str) -> Collection
    async def alist_collections(self) -> List[Collection]
    async def adelete_collection(self, name: str) -> OperationResponse
    
    # Vector operations
    async def ainsert_vector(self, collection: str, vector: VectorRecord) -> OperationResponse
    async def ainsert_vectors(self, collection: str, vectors: List[VectorRecord]) -> BulkOperationResponse
    async def aget_vector(self, collection: str, id: str) -> VectorRecord
    async def adelete_vector(self, collection: str, id: str) -> OperationResponse
    async def asearch_vectors(self, collection: str, query: List[float], **kwargs) -> SearchResponse
    
    # SQL operations
    async def aexecute_sql(self, query: str, parameters: Dict[str, Any] = None) -> SQLResponse
```

#### ResilientProximaDBClient
Enhanced client with production features.

```python
class ResilientProximaDBClient(ProximaDBClient):
    def __init__(
        self,
        config: ClientConfig,
        pool_config: Optional[Dict[str, Any]] = None,
        circuit_breaker_config: Optional[Dict[str, Any]] = None
    )
    
    def get_pool_stats(self) -> Dict[str, Any]
    def get_circuit_breaker_stats(self) -> Dict[str, Dict[str, Any]]
    def get_health_status(self) -> Dict[str, Any]
```

### Model Classes

#### CollectionConfig
```python
@dataclass
class CollectionConfig:
    name: str
    dimension: int
    distance_metric: Union[str, DistanceMetric] = "cosine"
    storage_engine: Union[str, StorageEngine] = "viper"
    index_type: Union[str, IndexType] = "hnsw"
    index_params: Optional[Dict[str, Any]] = None
    metadata: Optional[Dict[str, Any]] = None
```

#### VectorRecord
```python
@dataclass
class VectorRecord:
    id: str
    vector: List[float]
    metadata: Optional[Dict[str, Any]] = None
    version: Optional[int] = None
```

#### SearchOptions
```python
@dataclass
class SearchOptions:
    top_k: int = 10
    filter_conditions: Optional[List[FilterCondition]] = None
    include_metadata: bool = True
    include_vectors: bool = False
    ef_search: Optional[int] = None
    timeout: Optional[float] = None
```

### Exception Hierarchy

```
ProximaDBError
├── ClientError
│   ├── ConfigurationError
│   ├── ValidationError
│   └── SerializationError
├── ServerError
│   ├── CollectionNotFoundError
│   ├── VectorNotFoundError
│   ├── DimensionMismatchError
│   ├── QuotaExceededError
│   └── OperationTimeoutError
└── TransportError
    ├── ConnectionError
    ├── TimeoutError
    └── ProtocolError
```

## Troubleshooting

### Common Issues

#### 1. Connection Refused
```python
# Check server is running
try:
    await client.aconnect()
except ConnectionError as e:
    print(f"Cannot connect to ProximaDB: {e}")
    print("Ensure server is running on the specified host:port")
```

#### 2. Dimension Mismatch
```python
# Always verify collection dimension
collection = await client.aget_collection("my_collection")
if len(vector) != collection.dimension:
    raise ValueError(f"Vector dimension {len(vector)} doesn't match collection dimension {collection.dimension}")
```

#### 3. Memory Issues with Large Datasets
```python
# Use streaming instead of bulk loading
# Instead of:
# vectors = load_all_vectors()  # May OOM
# await client.ainsert_vectors("collection", vectors)

# Do this:
stream = VectorStream(client, "collection", batch_size=1000)
await stream.insert_stream(vector_generator())
```

#### 4. Slow Search Performance
```python
# Optimize search parameters
search_options = SearchOptions(
    top_k=10,  # Request only what you need
    include_vectors=False,  # Skip vector data if not needed
    filter_conditions=[...],  # Use filters to reduce search space
    ef_search=100  # Tune based on recall requirements
)

# Use caching for repeated queries
cache_manager = CacheManager(
    query_cache=QueryCache(default_ttl=300.0)
)
client.set_cache_manager(cache_manager)
```

### Debug Mode

```python
import logging

# Enable debug logging
logging.basicConfig(level=logging.DEBUG)
logging.getLogger("proximadb").setLevel(logging.DEBUG)

# Use debug interceptor
from proximadb.interceptors import LoggingInterceptor

client.add_interceptor(LoggingInterceptor(
    log_level=logging.DEBUG,
    log_request_body=True,
    log_response_body=True
))

# Enable telemetry for detailed metrics
telemetry = init_telemetry(exporters=[ConsoleExporter()])
await telemetry.start()
```

### Performance Profiling

```python
import cProfile
import pstats

# Profile async operations
async def profile_operation():
    profiler = cProfile.Profile()
    profiler.enable()
    
    # Your operation
    await client.asearch_vectors(...)
    
    profiler.disable()
    stats = pstats.Stats(profiler)
    stats.sort_stats('cumulative')
    stats.print_stats(10)  # Top 10 functions

# Memory profiling
from memory_profiler import profile

@profile
async def memory_intensive_operation():
    vectors = generate_large_dataset()
    await client.ainsert_vectors("collection", vectors)
```

## Migration Guide

See [migration_guide.md](migration_guide.md) for upgrading from older SDK versions.

## Examples

See the [examples/](../examples/) directory for complete working examples:
- `basic_usage.py` - Getting started
- `advanced_search.py` - Complex search scenarios  
- `streaming_upload.py` - Large dataset handling
- `production_setup.py` - Production configuration
- `monitoring_example.py` - Telemetry and monitoring

## Support

- GitHub Issues: [github.com/proximadb/proximadb-python/issues](https://github.com/proximadb/proximadb-python/issues)
- Documentation: [docs.proximadb.com](https://docs.proximadb.com)
- Community Discord: [discord.gg/proximadb](https://discord.gg/proximadb)