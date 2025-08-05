# ProximaDB Python SDK Examples

This directory contains comprehensive examples demonstrating various features and use cases of the ProximaDB Python SDK v1.0.

## Examples Overview

### 1. Basic Usage (`basic_usage.py`)
- **Purpose**: Introduction to fundamental operations
- **Topics Covered**:
  - Creating collections
  - Inserting single and multiple vectors
  - Basic vector search
  - Metadata filtering
  - Updating and deleting vectors
  - Collection management

**Run**: `python basic_usage.py`

### 2. Advanced Search (`advanced_search.py`)
- **Purpose**: Demonstrate complex search scenarios
- **Topics Covered**:
  - Complex metadata filtering with multiple conditions
  - SQL-based vector search
  - Hybrid search (vector + metadata)
  - Search result caching
  - Streaming search for large result sets
  - Search optimization hints

**Run**: `python advanced_search.py`

### 3. Streaming Upload (`streaming_upload.py`)
- **Purpose**: Handle large-scale data ingestion efficiently
- **Topics Covered**:
  - Streaming vector uploads
  - File-based uploads (JSONL, CSV, NumPy)
  - Memory-efficient processing
  - Error recovery and retries
  - Parallel multi-source streaming
  - Progress monitoring

**Run**: `python streaming_upload.py`

### 4. Production Setup (`production_setup.py`)
- **Purpose**: Configure client for production environments
- **Topics Covered**:
  - Connection pooling
  - Circuit breakers
  - Retry strategies
  - Request interceptors
  - Caching configuration
  - Telemetry and monitoring
  - Health checks
  - Graceful shutdown

**Run**: `python production_setup.py`

### 5. Monitoring Example (`monitoring_example.py`)
- **Purpose**: Implement comprehensive monitoring and observability
- **Topics Covered**:
  - Custom metrics collection
  - Distributed tracing
  - Performance monitoring
  - Error tracking
  - SLI/SLO monitoring
  - Custom telemetry exporters
  - Real-time dashboards

**Run**: `python monitoring_example.py`

## Prerequisites

1. **ProximaDB Server**: Ensure ProximaDB server is running
   ```bash
   # Default REST endpoint: http://localhost:5678
   # Default gRPC endpoint: grpc://localhost:5679
   ```

2. **Python SDK**: Install the ProximaDB Python SDK
   ```bash
   pip install proximadb[all]
   ```

3. **Optional Dependencies**: Some examples require additional packages
   ```bash
   pip install numpy aiofiles psutil
   ```

## Running the Examples

### Quick Start
```bash
# Run basic example
python basic_usage.py

# Run with custom server URL
PROXIMADB_URL=http://your-server:5678 python basic_usage.py
```

### Environment Variables
All examples support these environment variables:
- `PROXIMADB_URL`: REST API endpoint (default: http://localhost:5678)
- `PROXIMADB_GRPC_URL`: gRPC endpoint (default: grpc://localhost:5679)
- `PROXIMADB_API_KEY`: API key for authentication
- `PROXIMADB_TIMEOUT`: Request timeout in seconds
- `PROXIMADB_LOG_LEVEL`: Logging level (DEBUG, INFO, WARNING, ERROR)

### Docker Setup
If using Docker:
```bash
# Start ProximaDB server
docker run -p 5678:5678 -p 5679:5679 proximadb/proximadb:latest

# Run examples
docker run -v $(pwd):/examples -w /examples python:3.9 python basic_usage.py
```

## Example Patterns

### Async vs Sync Usage
All examples use async/await patterns, but the SDK supports sync usage:

```python
# Async (examples default)
async def main():
    client = ProximaDBClient("http://localhost:5678")
    collection = await client.acreate_collection(config)

# Sync wrapper
def main():
    client = ProximaDBClient("http://localhost:5678")
    collection = client.create_collection(config)  # Sync method
```

### Error Handling
Examples demonstrate proper error handling:

```python
from proximadb.exceptions import (
    CollectionNotFoundError,
    DimensionMismatchError,
    QuotaExceededError
)

try:
    results = await client.asearch_vectors(...)
except CollectionNotFoundError:
    # Handle missing collection
except DimensionMismatchError as e:
    # Handle dimension mismatch
except QuotaExceededError:
    # Handle quota limits
```

### Performance Tips
- Use streaming for datasets > 10,000 vectors
- Enable connection pooling for production
- Configure appropriate batch sizes (100-1000)
- Use caching for repeated queries
- Monitor metrics to identify bottlenecks

## Customization

### Modify Examples
Feel free to modify examples for your use case:
- Change vector dimensions
- Adjust batch sizes
- Add custom metadata fields
- Implement different search strategies
- Configure production settings

### Create New Examples
When creating new examples:
1. Follow the existing structure
2. Include comprehensive error handling
3. Add progress indicators for long operations
4. Clean up resources in finally blocks
5. Document configuration options

## Troubleshooting

### Connection Issues
```python
# Check server connectivity
curl http://localhost:5678/health

# Use explicit protocol
client = ProximaDBClient("http://localhost:5678")  # REST
client = ProximaDBClient("grpc://localhost:5679")  # gRPC
```

### Performance Issues
- Enable debug logging: `PROXIMADB_LOG_LEVEL=DEBUG`
- Check server metrics: http://localhost:5678/metrics
- Monitor client-side metrics (see monitoring_example.py)

### Memory Issues
- Use streaming for large datasets
- Configure smaller batch sizes
- Enable connection pooling limits
- Monitor memory usage with monitoring tools

## Additional Resources

- [ProximaDB Documentation](https://docs.proximadb.com)
- [Python SDK API Reference](https://proximadb.github.io/python-sdk)
- [Migration Guide](../docs/migration_guide.md)
- [Developer Guide](../docs/developer_guide.md)

## Contributing

To contribute new examples:
1. Create a descriptive example file
2. Include comprehensive documentation
3. Test with various configurations
4. Submit a pull request

## License

These examples are provided under the same license as the ProximaDB Python SDK.