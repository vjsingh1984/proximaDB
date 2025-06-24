# VectorFlow Python Client SDK

The official Python client for VectorFlow - the cloud-native serverless vector database for the AI era.

## Features

- 🚀 **High Performance**: Optimized for ML workloads with numpy integration
- 🔄 **Async/Sync APIs**: Both synchronous and asynchronous client support
- 🛡️ **Type Safe**: Full typing support with pydantic models
- 📊 **Batching**: Efficient batch operations for bulk inserts and searches
- 🔍 **Rich Filtering**: Advanced metadata filtering with complex queries
- 🌐 **Multi-Protocol**: gRPC and REST API support
- 📈 **Observability**: Built-in metrics, tracing, and logging
- 🔐 **Security**: JWT authentication and TLS encryption

## Installation

```bash
pip install vectorflow-python
```

For development:
```bash
pip install vectorflow-python[dev]
```

For enhanced performance:
```bash
pip install vectorflow-python[performance]
```

## Quick Start

### Synchronous Client

```python
import numpy as np
from vectorflow import VectorFlowClient

# Initialize client
client = VectorFlowClient(
    url="https://api.vectorflow.ai",
    api_key="your-api-key"
)

# Create a collection
collection = client.create_collection(
    name="documents",
    dimension=768,
    distance_metric="cosine"
)

# Insert vectors
vectors = np.random.random((1000, 768)).astype(np.float32)
ids = [f"doc_{i}" for i in range(1000)]
metadata = [{"category": "tech", "source": f"doc_{i}.pdf"} for i in range(1000)]

client.insert_vectors(
    collection_id=collection.id,
    vectors=vectors,
    ids=ids,
    metadata=metadata
)

# Search vectors
query = np.random.random(768).astype(np.float32)
results = client.search(
    collection_id=collection.id,
    query=query,
    k=10,
    filter={"category": "tech"}
)

for result in results:
    print(f"ID: {result.id}, Score: {result.score:.4f}")
```

### Asynchronous Client

```python
import asyncio
import numpy as np
from vectorflow import AsyncVectorFlowClient

async def main():
    # Initialize async client
    async with AsyncVectorFlowClient(
        url="https://api.vectorflow.ai",
        api_key="your-api-key"
    ) as client:
        
        # Create collection
        collection = await client.create_collection(
            name="embeddings",
            dimension=1536,
            distance_metric="cosine"
        )
        
        # Batch insert with progress tracking
        vectors = np.random.random((10000, 1536)).astype(np.float32)
        
        async for batch_result in client.insert_vectors_streaming(
            collection_id=collection.id,
            vectors=vectors,
            batch_size=1000
        ):
            print(f"Inserted batch: {batch_result.count} vectors")
        
        # Parallel search
        queries = np.random.random((100, 1536)).astype(np.float32)
        
        search_tasks = [
            client.search(collection_id=collection.id, query=query, k=5)
            for query in queries
        ]
        
        results = await asyncio.gather(*search_tasks)
        print(f"Completed {len(results)} searches")

if __name__ == "__main__":
    asyncio.run(main())
```

## Advanced Usage

### Collection Management

```python
from vectorflow import VectorFlowClient
from vectorflow.models import CollectionConfig, IndexConfig

client = VectorFlowClient(url="https://api.vectorflow.ai", api_key="your-key")

# Advanced collection configuration
config = CollectionConfig(
    dimension=768,
    distance_metric="cosine",
    index_config=IndexConfig(
        algorithm="hnsw",
        parameters={
            "m": 16,
            "ef_construction": 200,
            "ef": 100
        }
    ),
    storage_config={
        "compression": "lz4",
        "replication_factor": 3
    }
)

collection = client.create_collection(name="advanced", config=config)
```

### Complex Filtering

```python
# Advanced metadata filtering
results = client.search(
    collection_id=collection.id,
    query=query_vector,
    k=20,
    filter={
        "and": [
            {"category": {"in": ["tech", "science"]}},
            {"published_date": {"gte": "2023-01-01"}},
            {"rating": {"gt": 4.0}},
            {"or": [
                {"author": "john_doe"},
                {"verified": True}
            ]}
        ]
    }
)
```

### Batch Operations

```python
# Efficient batch processing
batch_size = 1000
total_vectors = 100000

for i in range(0, total_vectors, batch_size):
    batch_vectors = vectors[i:i+batch_size]
    batch_ids = ids[i:i+batch_size]
    batch_metadata = metadata[i:i+batch_size]
    
    try:
        result = client.insert_vectors(
            collection_id=collection.id,
            vectors=batch_vectors,
            ids=batch_ids,
            metadata=batch_metadata,
            upsert=True  # Update if exists
        )
        print(f"Batch {i//batch_size + 1}: Inserted {result.count} vectors")
    except Exception as e:
        print(f"Batch {i//batch_size + 1} failed: {e}")
        # Handle errors (retry, skip, etc.)
```

### Streaming Operations

```python
import asyncio
from vectorflow import AsyncVectorFlowClient

async def stream_insert_example():
    async with AsyncVectorFlowClient(url="...", api_key="...") as client:
        
        # Stream large dataset insertion
        async def vector_generator():
            for i in range(1000000):  # 1M vectors
                vector = np.random.random(768).astype(np.float32)
                yield {
                    "id": f"vec_{i}",
                    "vector": vector,
                    "metadata": {"batch": i // 10000}
                }
        
        # Process with backpressure control
        async for result in client.insert_vectors_stream(
            collection_id=collection.id,
            vector_stream=vector_generator(),
            batch_size=1000,
            max_concurrent_batches=5
        ):
            print(f"Processed: {result.total_processed} vectors")

asyncio.run(stream_insert_example())
```

## Configuration

### Environment Variables

```bash
export VECTORFLOW_URL="https://api.vectorflow.ai"
export VECTORFLOW_API_KEY="your-api-key"
export VECTORFLOW_TIMEOUT=30
export VECTORFLOW_MAX_RETRIES=3
export VECTORFLOW_POOL_SIZE=10
```

### Client Configuration

```python
from vectorflow import VectorFlowClient, ClientConfig

config = ClientConfig(
    url="https://api.vectorflow.ai",
    api_key="your-api-key",
    timeout=30.0,
    max_retries=3,
    retry_backoff=2.0,
    connection_pool_size=10,
    enable_compression=True,
    protocol="grpc",  # or "rest"
    tls_verify=True,
    user_agent="MyApp/1.0.0"
)

client = VectorFlowClient(config=config)
```

## Error Handling

```python
from vectorflow import VectorFlowClient, VectorFlowError
from vectorflow.exceptions import (
    AuthenticationError,
    RateLimitError,
    CollectionNotFoundError,
    VectorDimensionError
)

try:
    results = client.search(
        collection_id="invalid",
        query=query_vector,
        k=10
    )
except CollectionNotFoundError:
    print("Collection does not exist")
except VectorDimensionError as e:
    print(f"Vector dimension mismatch: {e}")
except RateLimitError as e:
    print(f"Rate limit exceeded. Retry after: {e.retry_after}")
except AuthenticationError:
    print("Invalid API key")
except VectorFlowError as e:
    print(f"VectorFlow error: {e}")
```

## Performance Tips

1. **Use numpy arrays**: Always use `numpy.ndarray` with `dtype=np.float32` for vectors
2. **Batch operations**: Use batch inserts/searches for better throughput
3. **Connection pooling**: Reuse client instances across requests
4. **Async for concurrency**: Use async client for high-concurrency workloads
5. **Compression**: Enable compression for large vectors
6. **Local caching**: Cache frequently accessed vectors locally

## API Reference

### Client Classes

- `VectorFlowClient`: Synchronous client
- `AsyncVectorFlowClient`: Asynchronous client

### Core Methods

- `create_collection()`: Create a new vector collection
- `delete_collection()`: Delete a collection
- `insert_vectors()`: Insert vectors into collection
- `search()`: Search for similar vectors
- `delete_vectors()`: Delete vectors by ID
- `get_collection_stats()`: Get collection statistics

### Models

- `Collection`: Collection metadata
- `SearchResult`: Search result with score and metadata
- `CollectionConfig`: Collection configuration
- `IndexConfig`: Index algorithm configuration

## Contributing

We welcome contributions! Please see our [Contributing Guide](CONTRIBUTING.md) for details.

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

## Support

- 📖 [Documentation](https://docs.vectorflow.ai)
- 💬 [Discord Community](https://discord.gg/vectorflow)
- 🐛 [Issue Tracker](https://github.com/vectorflow/vectorflow/issues)
- 📧 [Email Support](mailto:support@vectorflow.ai)