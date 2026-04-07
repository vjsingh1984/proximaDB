# ProximaDB Multi-Model API Guide

This guide demonstrates how to use ProximaDB's multi-model capabilities with the Python SDK, including Document, Hybrid, and Time-Series APIs.

## Table of Contents

1. [Document API](#document-api)
2. [Hybrid Search API](#hybrid-search-api)
3. [Time-Series API](#time-series-api)
4. [Cross-Model Queries](#cross-model-queries)
5. [Best Practices](#best-practices)

---

## Document API

The Document API provides MongoDB-like JSON document storage with full-text search, flexible indexing, and aggregation capabilities.

### Creating a Document Collection

```python
from proximadb_sdk import ProximaDBClient
from proximadb_sdk.document import (
    ProximaDBDocument,
    DocumentCollectionConfig,
    IndexDefinition,
    DocIndexType,
)

client = ProximaDBClient(url="http://localhost:5678")
docs = ProximaDBDocument(client)

# Create a collection with indexes
config = DocumentCollectionConfig(
    name="code_files",
    indexes=[
        IndexDefinition(path="$.language", type=DocIndexType.HASH),
        IndexDefinition(path="$.file_path", type=DocIndexType.BTREE),
        IndexDefinition(path="$.content", type=DocIndexType.FULLTEXT),
    ],
    enable_fulltext=True,
    fulltext_paths=["$.content", "$.description"],
)

result = docs.create_collection(config=config)
print(f"Collection created: {result}")
```

### Inserting Documents

```python
# Insert a single document
document = {
    "file_path": "src/main.py",
    "language": "python",
    "content": "def hello(): print('Hello, World!')",
    "description": "A simple hello world function",
    "lines_of_code": 2,
    "tags": ["example", "tutorial"],
}

result = docs.insert_document(
    collection_id="code_files",
    document=document,
    id="doc:main.py"
)
print(f"Document inserted: {result}")

# Insert multiple documents
documents = []
for i in range(100):
    documents.append({
        "file_path": f"src/file_{i}.py",
        "language": "python" if i % 2 == 0 else "javascript",
        "content": f"# File {i}",
        "lines_of_code": 10 + i,
    })

# Batch insert
for doc in documents:
    docs.insert_document(
        collection_id="code_files",
        document=doc
    )
```

### Querying Documents

```python
from proximadb_sdk.document import DocumentFilter

# Query with filter
filter_obj = DocumentFilter().eq("language", "python")

results = docs.query(
    collection_id="code_files",
    filter=filter_obj,
    projection=["file_path", "language", "lines_of_code"],
    limit=10
)

print(f"Found {len(results['documents'])} documents")
for doc in results['documents']:
    print(f"  {doc['document']['file_path']}: {doc['document']['lines_of_code']} LOC")

# Complex query with multiple conditions
filter_obj = (
    DocumentFilter()
    .eq("language", "python")
    .gt("lines_of_code", 50)
)

results = docs.query(
    collection_id="code_files",
    filter=filter_obj,
    limit=20
)
```

### Full-Text Search

```python
# Full-text search on indexed fields
filter_obj = DocumentFilter().fulltext("content", "hello world")

results = docs.query(
    collection_id="code_files",
    filter=filter_obj,
    limit=10
)

for result in results['documents']:
    score = result.get('score', 0)
    print(f"Score: {score:.4f} - {result['document']['file_path']}")
```

### Aggregation Pipeline

```python
# MongoDB-style aggregation
pipeline = [
    {
        "stage": "match",
        "filter": DocumentFilter().eq("language", "python").to_dict()
    },
    {
        "stage": "group",
        "key": "$.file_path",
        "aggregations": [
            {"field": "avg_loc", "type": "avg", "path": "$.lines_of_code"},
            {"field": "count", "type": "count", "path": "$.file_path"}
        ]
    },
    {
        "stage": "sort",
        "fields": [{"path": "avg_loc", "order": "desc"}]
    },
    {
        "stage": "limit",
        "limit": 10
    }
]

results = docs.aggregate(
    collection_id="code_files",
    pipeline=pipeline
)

print("Aggregation results:")
for result in results['results']:
    print(f"  {result}")
```

### Using the Adapter Directly

```python
from proximadb_sdk import ProximaDBClient

client = ProximaDBClient(url="http://localhost:5678")

# Create collection
client.create_document_collection(
    name="my_docs",
    config={
        "indexes": [
            {"path": "$.category", "type": "hash"}
        ]
    }
)

# Insert document
client.insert_document(
    collection_name="my_docs",
    document={"category": "tech", "title": "ProximaDB Guide"},
    id="doc1"
)

# Query documents
results = client.query_documents(
    collection_name="my_docs",
    filter={"category": "tech"},
    limit=10
)
```

---

## Hybrid Search API

The Hybrid Search API combines BM25 full-text search with vector similarity search using configurable fusion strategies.

### Fusion Strategies

```python
from proximadb_sdk.hybrid import FusionStrategy

# Available strategies:
# - RRF: Reciprocal Rank Fusion (default)
# - WEIGHTED_LINEAR: Weighted linear combination
# - CASCADE: Cascade fusion (primary then secondary)
# - RANK_BIASED_PRECISION: Rank Biased Precision
# - BORDA_COUNT: Borda count voting
# - COMB_SUM: Sum of normalized scores
# - COMB_MIN: Minimum of normalized scores
# - COMB_MAX: Maximum of normalized scores
```

### Basic Hybrid Search

```python
from proximadb_sdk import ProximaDBClient
from proximadb_sdk.hybrid import ProximaDBHybrid, FusionStrategy

client = ProximaDBClient(url="http://localhost:5678")
hybrid = ProximaDBHybrid(client)

# Prepare query vector (e.g., from an embedding model)
query_vector = [0.1, 0.2, 0.3, ...]  # Your embedding vector

# Execute hybrid search
results = hybrid.search(
    vector_collection="products",
    query_vector=query_vector,
    text_query="laptop computer for programming",
    fusion_strategy=FusionStrategy.RRF,
    top_k=10
)

print(f"Found {len(results)} results")
for result in results:
    print(f"  ID: {result.id}")
    print(f"  Fused Score: {result.fused_score:.4f}")
    print(f"  BM25 Score: {result.bm25_score:.4f}")
    print(f"  Vector Score: {result.vector_score:.4f}")
    print()
```

### Weighted Linear Fusion

```python
from proximadb_sdk.hybrid import WeightedFusion

# Create weighted fusion strategy
fusion = WeightedFusion(
    alpha=0.6,              # BM25 weight (0.0 = all vector, 1.0 = all BM25)
    bm25_normalize=True,    # Normalize BM25 scores
    vector_normalize=True   # Normalize vector scores
)

results = hybrid.search(
    vector_collection="products",
    query_vector=query_vector,
    text_query="gaming laptop",
    fusion_strategy=fusion,
    top_k=10
)
```

### Hybrid Search with Filters

```python
# Add metadata filters to hybrid search
results = hybrid.search(
    vector_collection="products",
    query_vector=query_vector,
    text_query="laptop under $1000",
    fusion_strategy=FusionStrategy.RRF,
    top_k=10,
    filters={
        "price_max": 1000,
        "category": "electronics",
        "in_stock": True
    }
)
```

### Cascade Fusion

```python
from proximadb_sdk.hybrid import CascadeFusion

# Create cascade fusion (vector first, then BM25 for low scores)
fusion = CascadeFusion(
    primary_model="vector",
    secondary_model="bm25",
    threshold=0.7
)

results = hybrid.search(
    vector_collection="products",
    query_vector=query_vector,
    text_query="high performance laptop",
    fusion_strategy=fusion,
    top_k=10
)
```

### Using the Adapter Directly

```python
from proximadb_sdk import ProximaDBClient

client = ProximaDBClient(url="http://localhost:5678")

# Hybrid search via adapter
results = client.hybrid_search(
    collection="products",
    text_query="laptop computer",
    query_vector=[0.1, 0.2, ...],
    fusion_strategy="rrf",
    top_k=10
)

print(f"Fusion strategy: {results['fusion_strategy']}")
print(f"Query time: {results['metrics']['total_time_ms']:.2f}ms")

for result in results['results']:
    print(f"  {result['id']}: {result['fused_score']:.4f}")
```

---

## Time-Series API

The Time-Series API provides high-throughput time-series data storage with compression, downsampling, and aggregation.

### Creating a Time-Series Collection

```python
from proximadb_sdk import ProximaDBClient
from proximadb_sdk.timeseries import (
    ProximaDBTimeSeries,
    TimeSeriesCollectionConfig,
    ValueColumn,
    ValueType,
    AggregationType,
)

client = ProximaDBClient(url="http://localhost:5678")
ts = ProximaDBTimeSeries(client)

# Create collection with value columns
config = TimeSeriesCollectionConfig(
    name="metrics",
    timestamp_column="timestamp",
    value_columns=[
        ValueColumn(
            name="cpu_usage",
            data_type=ValueType.FLOAT,
            aggregation=AggregationType.AVG
        ),
        ValueColumn(
            name="memory_usage",
            data_type=ValueType.FLOAT,
            aggregation=AggregationType.MAX
        ),
        ValueColumn(
            name="request_count",
            data_type=ValueType.INT,
            aggregation=AggregationType.SUM
        ),
    ],
    tag_columns=["host", "region", "service"],
    retention_ms=7 * 24 * 60 * 60 * 1000,  # 7 days
)

result = ts.create_collection(config=config)
print(f"Time-series collection created: {result}")
```

### Ingesting Time-Series Data

```python
from datetime import datetime, timedelta

# Generate test data points
now = datetime.utcnow()
points = []

for i in range(1000):
    timestamp = now + timedelta(seconds=i)
    points.append({
        "timestamp": timestamp.isoformat() + "Z",
        "values": {
            "cpu_usage": 50.0 + (i % 50),
            "memory_usage": 40.0 + (i % 30),
            "request_count": 100 + i * 10,
        },
        "tags": {
            "host": f"server-{i % 5}",
            "region": "us-west" if i % 2 == 0 else "us-east",
            "service": "api"
        }
    })

# Ingest data
result = ts.ingest(
    collection_id="metrics",
    points=points
)

print(f"Ingested: {result['ingested_count']} points")
print(f"Failed: {result['failed_count']} points")
print(f"Throughput: {result['ingested_count'] / result['elapsed_time_ms'] * 1000:.0f} points/sec")
```

### Querying Time-Series Data

```python
# Query raw time-series data
start_time = (datetime.utcnow() - timedelta(hours=1)).isoformat() + "Z"
end_time = datetime.utcnow().isoformat() + "Z"

results = ts.query(
    collection_id="metrics",
    start_time=start_time,
    end_time=end_time,
)

print(f"Retrieved {len(results['raw_points'])} raw points")
for point in results['raw_points'][:5]:
    print(f"  {point['timestamp']}: {point['values']}")
```

### Querying with Aggregation

```python
# Query with AVG aggregation and 1-minute buckets
results = ts.query(
    collection_id="metrics",
    start_time=start_time,
    end_time=end_time,
    aggregation="AVG",
    bucket_ms=60000,  # 1 minute
)

print(f"Aggregated into {len(results['metrics'])} buckets")
for metric in results['metrics'][:5]:
    print(f"  {metric['timestamp']}: {metric['value']:.2f} (count: {metric['count']})")
```

### OHLC Aggregation for Financial Data

```python
# Create a financial data collection
config = TimeSeriesCollectionConfig(
    name="stock_prices",
    timestamp_column="timestamp",
    value_columns=[
        ValueColumn(
            name="price",
            data_type=ValueType.FLOAT,
            aggregation=AggregationType.OHLC  # Open-High-Low-Close
        ),
        ValueColumn(
            name="volume",
            data_type=ValueType.INT,
            aggregation=AggregationType.SUM
        ),
    ],
    tag_columns=["symbol"],
)

ts.create_collection(config=config)

# Ingest stock price data
now = datetime.utcnow()
points = []
base_price = 100.0

for i in range(100):
    timestamp = now + timedelta(minutes=i)
    price = base_price + (i % 10) - 5 + (i % 3) * 0.1
    points.append({
        "timestamp": timestamp.isoformat() + "Z",
        "values": {
            "price": price,
            "volume": 1000 + i * 100,
        },
        "tags": {"symbol": "AAPL"}
    })

ts.ingest(collection_id="stock_prices", points=points)

# Query with OHLC aggregation
results = ts.query(
    collection_id="stock_prices",
    start_time=now.isoformat() + "Z",
    end_time=(now + timedelta(hours=2)).isoformat() + "Z",
    aggregation="OHLC",
    bucket_ms=300000,  # 5 minutes
)

for metric in results['metrics']:
    print(f"  {metric['timestamp']}: "
          f"O={metric['open']:.2f} "
          f"H={metric['high']:.2f} "
          f"L={metric['low']:.2f} "
          f"C={metric['close']:.2f}")
```

### Querying with Tag Filters

```python
# Query with tag filters
results = ts.query(
    collection_id="metrics",
    start_time=start_time,
    end_time=end_time,
    aggregation="AVG",
    bucket_ms=60000,
    tag_filters={"region": "us-west", "service": "api"}
)
```

### Aggregation Pipeline

```python
# Create downsampling pipeline
pipeline = [
    {
        "stage": "downsample",
        "bucket_ms": 60000,  # 1 minute
        "aggregation": "AVG",
        "value_columns": ["cpu_usage", "memory_usage"]
    },
    {
        "stage": "group_by",
        "tag_columns": ["region"],
        "aggregation": "AVG",
        "bucket_ms": 300000  # 5 minutes
    }
]

results = ts.aggregate(
    collection_id="metrics",
    start_time=start_time,
    end_time=end_time,
    pipeline=pipeline
)

print("Aggregated results:")
for result in results['results']:
    print(f"  {result}")
```

### Using the Adapter Directly

```python
from proximadb_sdk import ProximaDBClient

client = ProximaDBClient(url="http://localhost:5678")

# Create time-series collection
client.create_timeseries_collection(
    name="system_metrics",
    config={
        "timestamp_column": "timestamp",
        "value_columns": [
            {"name": "cpu", "data_type": "float", "aggregation": "avg"},
            {"name": "memory", "data_type": "float", "aggregation": "max"},
        ],
        "tag_columns": ["host"],
    }
)

# Ingest data
client.ingest_timeseries(
    collection_name="system_metrics",
    points=[{
        "timestamp": "2026-03-11T10:00:00Z",
        "values": {"cpu": 75.5, "memory": 60.2},
        "tags": {"host": "server1"}
    }]
)

# Query data
results = client.query_timeseries(
    collection_name="system_metrics",
    start_time="2026-03-11T00:00:00Z",
    end_time="2026-03-12T00:00:00Z",
    aggregation="AVG",
    bucket_ms=60000
)

print(f"Retrieved {len(results['metrics'])} data points")
```

---

## Cross-Model Queries

ProximaDB supports querying across multiple data models in a single query.

### Federated SQL with Multi-Model Joins

```python
from proximadb_sdk import ProximaDBClient

client = ProximaDBClient(url="http://localhost:5678")

# Cross-model join: vector search + document lookup
query = """
SELECT v.id, v.score, d.document
FROM VECTOR_SEARCH('code_embeddings', ?, 10) v
JOIN DOCUMENT_QUERY('code_files', '{"language": "python"}') d
ON v.metadata.file_id = d.id
WHERE v.metadata.language = 'python'
"""

results = client.execute_federated_sql(
    query=query,
    parameters={"query_vector": [0.1, 0.2, ...]}
)

for row in results:
    print(f"ID: {row['id']}, Score: {row['score']}")
    print(f"Document: {row['document']['file_path']}")
```

### Graph + Vector + Document Hybrid

```python
# Multi-model query combining graph, vector, and document
query = """
SELECT
    n.id as node_id,
    n.properties as node_data,
    v.score as vector_score,
    d.document as doc_data
FROM GRAPH_QUERY('code_graph', 'MATCH (n)-[r:IMPORTS]->(m) RETURN n, r, m') g
JOIN VECTOR_SEARCH('code_embeddings', ?, 10) v
ON g.node_id = v.id
JOIN DOCUMENT_QUERY('code_files', '{}') d
ON v.metadata.file_path = d.id
"""

results = client.execute_federated_sql(
    query=query,
    parameters={"query_vector": [0.1, 0.2, ...]}
)
```

---

## Best Practices

### Document API

1. **Index Strategy**: Create indexes on frequently queried fields
   - Hash indexes for equality lookups
   - B-tree indexes for range queries
   - Full-text indexes for text search

2. **Projection**: Use projection to limit returned fields
   ```python
   results = docs.query(
       collection_id="code_files",
       projection=["file_path", "language"],
       limit=100
   )
   ```

3. **Aggregation**: Use aggregation pipeline for complex analytics
   - Perform filtering early in the pipeline
   - Limit results with `$limit` stage

### Hybrid Search

1. **Fusion Strategy Selection**:
   - Use **RRF** for general-purpose search (default)
   - Use **Weighted Linear** when you want to control BM25 vs vector importance
   - Use **Cascade** for vector-first refinement with BM25 fallback

2. **Performance**:
   - Batch queries when possible
   - Use appropriate `top_k` values (5-50 for most cases)
   - Add filters to reduce search space

### Time-Series

1. **Collection Design**:
   - Use appropriate aggregation types (AVG for metrics, SUM for counters)
   - Set reasonable retention periods
   - Use tag columns for efficient filtering

2. **Ingestion**:
   - Batch points for high throughput (1000-10000 points per batch)
   - Use downsample mode for high-frequency data

3. **Querying**:
   - Always use time range filters
   - Use aggregation to reduce data volume
   - Choose appropriate bucket sizes based on resolution

### General

1. **Connection Management**:
   ```python
   # Use context manager for automatic cleanup
   with ProximaDBClient(url="http://localhost:5678") as client:
       # Your operations here
       pass
   ```

2. **Error Handling**:
   ```python
   from proximadb_sdk.exceptions import ProximaDBError

   try:
       result = docs.query(collection_id="...", filter=...)
   except ProximaDBError as e:
       print(f"Error: {e}")
   ```

3. **Performance**:
   - Use batch operations when possible
   - Set appropriate timeouts
   - Monitor query performance with built-in metrics

---

## API Reference

### Document API

- `ProximaDBDocument` - High-level Document API
- `DocumentCollectionConfig` - Collection configuration
- `DocumentFilter` - Query filter builder
- `IndexDefinition` - Index configuration
- `DocIndexType` - Index types (BTREE, HASH, FULLTEXT, etc.)

### Hybrid Search API

- `ProximaDBHybrid` - High-level Hybrid Search API
- `FusionStrategy` - Fusion strategy enum
- `ReciprocalRankFusion` - RRF fusion implementation
- `WeightedFusion` - Weighted linear fusion
- `CascadeFusion` - Cascade fusion

### Time-Series API

- `ProximaDBTimeSeries` - High-level Time-Series API
- `TimeSeriesCollectionConfig` - Collection configuration
- `ValueColumn` - Value column definition
- `ValueType` - Value types (FLOAT, INT, STRING)
- `AggregationType` - Aggregation types (AVG, SUM, OHLC, etc.)

For more detailed API documentation, see the [Python SDK Reference](https://docs.proximadb.com/sdk/python/).
