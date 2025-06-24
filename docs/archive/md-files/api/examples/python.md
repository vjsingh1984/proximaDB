# ProximaDB Python Client Examples

Complete examples for using ProximaDB with Python clients for both REST and gRPC APIs.

## Installation

```bash
# Install the official Python client
pip install proximadb-python

# Or install dependencies for manual integration
pip install requests grpcio grpcio-tools
```

## REST API Examples

### Basic Setup

```python
import requests
import json
from typing import List, Dict, Any, Optional

class ProximaDBRestClient:
    def __init__(self, base_url: str = "http://localhost:5678", api_key: Optional[str] = None):
        self.base_url = base_url.rstrip('/')
        self.headers = {
            'Content-Type': 'application/json'
        }
        if api_key:
            self.headers['Authorization'] = f'Bearer {api_key}'
    
    def _request(self, method: str, endpoint: str, data: Dict = None) -> Dict[str, Any]:
        url = f"{self.base_url}{endpoint}"
        response = requests.request(method, url, headers=self.headers, json=data)
        
        if response.status_code >= 400:
            raise Exception(f"API Error {response.status_code}: {response.text}")
        
        return response.json()

# Initialize client
client = ProximaDBRestClient(api_key="pk_live_1234567890abcdef")
```

### Collection Management

```python
# Create a collection
def create_collection_example():
    collection_data = {
        "collection_id": "document_embeddings",
        "dimension": 384,
        "distance_metric": "cosine",
        "description": "Text document embeddings"
    }
    
    result = client._request('POST', '/collections', collection_data)
    print(f"Collection created: {result['collection_id']}")
    return result

# List all collections
def list_collections_example():
    collections = client._request('GET', '/collections')
    print(f"Found {len(collections['collections'])} collections:")
    
    for collection in collections['collections']:
        print(f"- {collection['id']}: {collection['vector_count']} vectors")
    
    return collections

# Get collection details
def get_collection_example(collection_id: str):
    collection = client._request('GET', f'/collections/{collection_id}')
    print(f"Collection {collection['id']}:")
    print(f"  Dimension: {collection['dimension']}")
    print(f"  Vectors: {collection['vector_count']}")
    print(f"  Size: {collection['total_size_bytes']} bytes")
    return collection

# Delete a collection
def delete_collection_example(collection_id: str):
    result = client._request('DELETE', f'/collections/{collection_id}')
    print(f"Collection deleted: {result['message']}")
    return result
```

### Vector Operations

```python
import uuid
import numpy as np

# Insert a single vector
def insert_vector_example():
    vector_data = {
        "id": str(uuid.uuid4()),
        "vector": [0.1, 0.2, 0.3, 0.4] * 96,  # 384 dimensions
        "metadata": {
            "document_id": "doc_001",
            "title": "Sample Document",
            "category": "research",
            "tags": ["ai", "ml", "nlp"]
        }
    }
    
    result = client._request('POST', '/collections/document_embeddings/vectors', vector_data)
    print(f"Vector inserted: {result['vector_id']}")
    return result

# Get a vector by ID
def get_vector_example(collection_id: str, vector_id: str):
    vector = client._request('GET', f'/collections/{collection_id}/vectors/{vector_id}')
    print(f"Vector {vector['id']}:")
    print(f"  Dimension: {len(vector['vector'])}")
    print(f"  Metadata: {vector['metadata']}")
    return vector

# Search for similar vectors
def search_vectors_example():
    # Generate a random query vector
    query_vector = np.random.rand(384).tolist()
    
    search_data = {
        "vector": query_vector,
        "k": 10,
        "filter": {
            "category": "research"
        },
        "threshold": 0.7
    }
    
    results = client._request('POST', '/collections/document_embeddings/search', search_data)
    
    print(f"Found {results['total_count']} similar vectors:")
    for result in results['results']:
        print(f"- {result['id']}: score={result['score']:.3f}")
        if 'metadata' in result:
            print(f"  Title: {result['metadata'].get('title', 'N/A')}")
    
    return results

# Delete a vector
def delete_vector_example(collection_id: str, vector_id: str):
    result = client._request('DELETE', f'/collections/{collection_id}/vectors/{vector_id}')
    print(f"Vector deleted: {result['message']}")
    return result
```

### Batch Operations

```python
# Batch insert vectors
def batch_insert_example():
    vectors = []
    for i in range(100):
        vector_data = {
            "id": str(uuid.uuid4()),
            "vector": np.random.rand(384).tolist(),
            "metadata": {
                "document_id": f"doc_{i:03d}",
                "batch": "example_batch",
                "index": i
            }
        }
        vectors.append(vector_data)
    
    batch_data = {"vectors": vectors}
    result = client._request('POST', '/collections/document_embeddings/vectors/batch', batch_data)
    
    print(f"Batch inserted {result['inserted_count']} vectors")
    if result['errors']:
        print(f"Errors: {result['errors']}")
    
    return result

# Batch search
def batch_search_example():
    searches = []
    for i in range(5):
        search = {
            "collection_id": "document_embeddings", 
            "vector": np.random.rand(384).tolist(),
            "k": 5,
            "filter": {"batch": "example_batch"}
        }
        searches.append(search)
    
    batch_data = {"searches": searches}
    results = client._request('POST', '/batch/search', batch_data)
    
    print(f"Performed {len(results['results'])} searches:")
    for i, search_result in enumerate(results['results']):
        print(f"  Search {i}: {search_result['total_count']} results")
    
    return results
```

### Complete Example Application

```python
#!/usr/bin/env python3
"""
ProximaDB REST API Example Application
Demonstrates document similarity search using embeddings.
"""

import requests
import numpy as np
import json
from sentence_transformers import SentenceTransformer
from typing import List, Dict

class DocumentSearchApp:
    def __init__(self, api_key: str = None):
        self.client = ProximaDBRestClient(api_key=api_key)
        self.model = SentenceTransformer('all-MiniLM-L6-v2')  # 384 dimensions
        self.collection_id = "document_search"
    
    def setup(self):
        """Initialize the collection for document search."""
        try:
            collection_data = {
                "collection_id": self.collection_id,
                "dimension": 384,
                "distance_metric": "cosine",
                "description": "Document similarity search"
            }
            self.client._request('POST', '/collections', collection_data)
            print("Collection created successfully")
        except Exception as e:
            if "already exists" in str(e):
                print("Collection already exists")
            else:
                raise
    
    def add_documents(self, documents: List[Dict[str, str]]):
        """Add documents to the search index."""
        vectors = []
        
        for doc in documents:
            # Generate embedding
            embedding = self.model.encode(doc['content'])
            
            vector_data = {
                "vector": embedding.tolist(),
                "metadata": {
                    "title": doc['title'],
                    "content": doc['content'][:200],  # Truncate for storage
                    "author": doc.get('author', 'Unknown'),
                    "category": doc.get('category', 'general')
                }
            }
            vectors.append(vector_data)
        
        # Batch insert
        batch_data = {"vectors": vectors}
        result = self.client._request('POST', f'/collections/{self.collection_id}/vectors/batch', batch_data)
        
        print(f"Added {result['inserted_count']} documents to search index")
        return result
    
    def search_documents(self, query: str, k: int = 5, category_filter: str = None) -> List[Dict]:
        """Search for similar documents."""
        # Generate query embedding
        query_embedding = self.model.encode(query)
        
        search_data = {
            "vector": query_embedding.tolist(),
            "k": k,
            "threshold": 0.3
        }
        
        # Add category filter if specified
        if category_filter:
            search_data["filter"] = {"category": category_filter}
        
        results = self.client._request('POST', f'/collections/{self.collection_id}/search', search_data)
        
        documents = []
        for result in results['results']:
            doc = {
                "id": result['id'],
                "score": result['score'],
                "title": result['metadata']['title'],
                "author": result['metadata']['author'],
                "content": result['metadata']['content']
            }
            documents.append(doc)
        
        return documents

def main():
    # Initialize the application
    app = DocumentSearchApp(api_key="pk_test_example123")
    app.setup()
    
    # Sample documents
    documents = [
        {
            "title": "Introduction to Machine Learning",
            "content": "Machine learning is a subset of artificial intelligence that focuses on algorithms that can learn from data.",
            "author": "Dr. Smith",
            "category": "education"
        },
        {
            "title": "Deep Learning Fundamentals", 
            "content": "Deep learning uses neural networks with multiple layers to model complex patterns in data.",
            "author": "Prof. Johnson",
            "category": "education"
        },
        {
            "title": "Cooking Italian Pasta",
            "content": "Italian pasta is a traditional dish made from wheat flour and water, often served with various sauces.",
            "author": "Chef Mario",
            "category": "cooking"
        },
        {
            "title": "Python Programming Guide",
            "content": "Python is a high-level programming language known for its simplicity and readability.",
            "author": "Dev Expert",
            "category": "programming"
        }
    ]
    
    # Add documents to the index
    app.add_documents(documents)
    
    # Search for documents
    print("\n--- Search Results ---")
    queries = [
        "artificial intelligence algorithms",
        "neural networks and deep learning", 
        "Italian food recipes",
        "programming languages"
    ]
    
    for query in queries:
        print(f"\nQuery: '{query}'")
        results = app.search_documents(query, k=3)
        
        for i, doc in enumerate(results, 1):
            print(f"{i}. {doc['title']} (score: {doc['score']:.3f})")
            print(f"   Author: {doc['author']}")
            print(f"   Content: {doc['content']}...")

if __name__ == "__main__":
    main()
```

## gRPC API Examples

### Basic Setup

```python
import grpc
from google.protobuf import struct_pb2, timestamp_pb2
from proximadb.proto import vectordb_pb2, vectordb_pb2_grpc
import uuid
import numpy as np

class ProximaDBGrpcClient:
    def __init__(self, host: str = "localhost", port: int = 9090, api_key: str = None):
        self.host = host
        self.port = port
        self.api_key = api_key
        self.channel = grpc.insecure_channel(f'{host}:{port}')
        self.stub = vectordb_pb2_grpc.VectorDBStub(self.channel)
    
    def _get_metadata(self):
        """Get gRPC metadata with API key."""
        if self.api_key:
            return [('authorization', f'Bearer {self.api_key}')]
        return None
    
    def close(self):
        """Close the gRPC channel."""
        self.channel.close()

# Initialize client
grpc_client = ProximaDBGrpcClient(api_key="pk_live_1234567890abcdef")
```

### Collection Management with gRPC

```python
def create_collection_grpc():
    request = vectordb_pb2.CreateCollectionRequest(
        collection_id="grpc_embeddings",
        name="gRPC Document Embeddings",
        dimension=384,
        schema_type=vectordb_pb2.SCHEMA_TYPE_DOCUMENT
    )
    
    response = grpc_client.stub.CreateCollection(request, metadata=grpc_client._get_metadata())
    print(f"Collection created: {response.success}")
    return response

def list_collections_grpc():
    request = vectordb_pb2.ListCollectionsRequest()
    response = grpc_client.stub.ListCollections(request, metadata=grpc_client._get_metadata())
    
    print(f"Found {len(response.collections)} collections:")
    for collection in response.collections:
        print(f"- {collection.id}: {collection.vector_count} vectors")
    
    return response

def delete_collection_grpc(collection_id: str):
    request = vectordb_pb2.DeleteCollectionRequest(collection_id=collection_id)
    response = grpc_client.stub.DeleteCollection(request, metadata=grpc_client._get_metadata())
    print(f"Collection deleted: {response.success}")
    return response
```

### Vector Operations with gRPC

```python
def create_metadata_struct(metadata_dict: dict) -> struct_pb2.Struct:
    """Convert Python dict to protobuf Struct."""
    metadata = struct_pb2.Struct()
    for key, value in metadata_dict.items():
        if isinstance(value, str):
            metadata[key] = value
        elif isinstance(value, (int, float)):
            metadata[key] = value
        elif isinstance(value, bool):
            metadata[key] = value
        elif isinstance(value, list):
            metadata[key] = value
    return metadata

def insert_vector_grpc():
    # Create metadata
    metadata = create_metadata_struct({
        "document_id": "grpc_doc_001",
        "title": "gRPC Example Document",
        "category": "technical",
        "tags": ["grpc", "protobuf", "api"]
    })
    
    # Create vector record
    record = vectordb_pb2.VectorRecord(
        id=str(uuid.uuid4()),
        collection_id="grpc_embeddings",
        vector=np.random.rand(384).tolist(),
        metadata=metadata
    )
    
    request = vectordb_pb2.InsertRequest(
        collection_id="grpc_embeddings",
        record=record
    )
    
    response = grpc_client.stub.Insert(request, metadata=grpc_client._get_metadata())
    print(f"Vector inserted: {response.vector_id}")
    return response

def search_vectors_grpc():
    query_vector = np.random.rand(384).tolist()
    
    # Create filter
    filters = create_metadata_struct({"category": "technical"})
    
    request = vectordb_pb2.SearchRequest(
        collection_id="grpc_embeddings",
        vector=query_vector,
        k=10,
        filters=filters,
        threshold=0.7
    )
    
    response = grpc_client.stub.Search(request, metadata=grpc_client._get_metadata())
    
    print(f"Found {response.total_count} similar vectors:")
    for result in response.results:
        print(f"- {result.id}: score={result.score:.3f}")
        if result.metadata:
            title = result.metadata.get("title", "N/A")
            print(f"  Title: {title}")
    
    return response

def batch_insert_grpc():
    records = []
    for i in range(50):
        metadata = create_metadata_struct({
            "document_id": f"grpc_batch_{i:03d}",
            "index": i,
            "batch": "grpc_example"
        })
        
        record = vectordb_pb2.VectorRecord(
            collection_id="grpc_embeddings",
            vector=np.random.rand(384).tolist(),
            metadata=metadata
        )
        records.append(record)
    
    request = vectordb_pb2.BatchInsertRequest(
        collection_id="grpc_embeddings",
        records=records
    )
    
    response = grpc_client.stub.BatchInsert(request, metadata=grpc_client._get_metadata())
    print(f"Batch inserted {response.inserted_count} vectors")
    
    if response.errors:
        print(f"Errors: {list(response.errors)}")
    
    return response
```

### Advanced gRPC Example

```python
import asyncio
import grpc.aio
from contextlib import asynccontextmanager

class AsyncProximaDBClient:
    """Async gRPC client for high-performance applications."""
    
    def __init__(self, host: str = "localhost", port: int = 9090, api_key: str = None):
        self.host = host
        self.port = port
        self.api_key = api_key
    
    @asynccontextmanager
    async def get_stub(self):
        async with grpc.aio.insecure_channel(f'{self.host}:{self.port}') as channel:
            stub = vectordb_pb2_grpc.VectorDBStub(channel)
            yield stub
    
    def _get_metadata(self):
        if self.api_key:
            return (('authorization', f'Bearer {self.api_key}'),)
        return None
    
    async def parallel_search(self, queries: List[np.ndarray], collection_id: str, k: int = 10):
        """Perform multiple searches in parallel."""
        async with self.get_stub() as stub:
            tasks = []
            
            for query_vector in queries:
                request = vectordb_pb2.SearchRequest(
                    collection_id=collection_id,
                    vector=query_vector.tolist(),
                    k=k
                )
                
                task = stub.Search(request, metadata=self._get_metadata())
                tasks.append(task)
            
            results = await asyncio.gather(*tasks)
            return results
    
    async def stream_inserts(self, vectors: List[Dict], collection_id: str):
        """Insert vectors with streaming for better performance."""
        async with self.get_stub() as stub:
            tasks = []
            
            # Batch into smaller chunks for optimal performance
            chunk_size = 100
            for i in range(0, len(vectors), chunk_size):
                chunk = vectors[i:i + chunk_size]
                
                records = []
                for vector_data in chunk:
                    metadata = create_metadata_struct(vector_data.get('metadata', {}))
                    record = vectordb_pb2.VectorRecord(
                        collection_id=collection_id,
                        vector=vector_data['vector'],
                        metadata=metadata
                    )
                    records.append(record)
                
                request = vectordb_pb2.BatchInsertRequest(
                    collection_id=collection_id,
                    records=records
                )
                
                task = stub.BatchInsert(request, metadata=self._get_metadata())
                tasks.append(task)
            
            results = await asyncio.gather(*tasks)
            
            total_inserted = sum(r.inserted_count for r in results)
            print(f"Total inserted: {total_inserted} vectors")
            return results

# Usage example
async def async_example():
    client = AsyncProximaDBClient(api_key="pk_test_example123")
    
    # Parallel searches
    queries = [np.random.rand(384) for _ in range(10)]
    search_results = await client.parallel_search(queries, "grpc_embeddings", k=5)
    
    print(f"Completed {len(search_results)} parallel searches")
    for i, result in enumerate(search_results):
        print(f"Search {i}: {result.total_count} results")

# Run the async example
# asyncio.run(async_example())
```

### Health and Monitoring

```python
def health_check_grpc():
    """Check server health via gRPC."""
    request = vectordb_pb2.HealthRequest()
    response = grpc_client.stub.Health(request, metadata=grpc_client._get_metadata())
    
    print(f"Health status: {response.status}")
    print(f"Timestamp: {response.timestamp}")
    return response

def get_status_grpc():
    """Get detailed server status."""
    request = vectordb_pb2.StatusRequest()
    response = grpc_client.stub.Status(request, metadata=grpc_client._get_metadata())
    
    print(f"Node ID: {response.node_id}")
    print(f"Version: {response.version}")
    print(f"Role: {response.role}")
    
    if response.HasField('storage'):
        storage = response.storage
        print(f"Total vectors: {storage.total_vectors}")
        print(f"Total size: {storage.total_size_bytes} bytes")
    
    return response
```

## Error Handling and Best Practices

```python
import time
import logging
from typing import Callable, Any

logger = logging.getLogger(__name__)

class ProximaDBClientError(Exception):
    """Custom exception for ProximaDB client errors."""
    pass

def retry_with_backoff(func: Callable, max_retries: int = 3, base_delay: float = 1.0):
    """Retry function with exponential backoff."""
    for attempt in range(max_retries):
        try:
            return func()
        except grpc.RpcError as e:
            if e.code() == grpc.StatusCode.RESOURCE_EXHAUSTED and attempt < max_retries - 1:
                delay = base_delay * (2 ** attempt)
                logger.warning(f"Rate limited, retrying in {delay}s...")
                time.sleep(delay)
                continue
            raise ProximaDBClientError(f"gRPC error: {e.code()} - {e.details()}")
        except Exception as e:
            if attempt < max_retries - 1:
                delay = base_delay * (2 ** attempt)
                logger.warning(f"Request failed, retrying in {delay}s...")
                time.sleep(delay)
                continue
            raise

# Usage with retry
def robust_search(query_vector: List[float], collection_id: str):
    def search_operation():
        request = vectordb_pb2.SearchRequest(
            collection_id=collection_id,
            vector=query_vector,
            k=10
        )
        return grpc_client.stub.Search(request, metadata=grpc_client._get_metadata())
    
    return retry_with_backoff(search_operation)

# Example usage
if __name__ == "__main__":
    try:
        # REST API examples
        print("=== REST API Examples ===")
        create_collection_example()
        insert_vector_example()
        search_results = search_vectors_example()
        
        # gRPC API examples  
        print("\n=== gRPC API Examples ===")
        create_collection_grpc()
        insert_vector_grpc()
        search_vectors_grpc()
        health_check_grpc()
        
    except Exception as e:
        logger.error(f"Example failed: {e}")
    finally:
        grpc_client.close()
```

This comprehensive Python guide covers both REST and gRPC APIs with practical examples for building production applications with ProximaDB.