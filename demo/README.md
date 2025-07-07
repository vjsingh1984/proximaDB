# ProximaDB End-to-End Demo

This directory contains a comprehensive demonstration of ProximaDB's capabilities, including all major features and performance benchmarks.

## 🚀 Quick Start

### Option 1: Docker Compose (Recommended)

```bash
# Navigate to demo directory
cd demo

# Start the complete demo environment
docker-compose up --build

# View demo logs
docker-compose logs -f demo

# View ProximaDB logs
docker-compose logs -f proximadb

# Clean up
docker-compose down -v
```

### Option 2: Local Development

```bash
# Install dependencies
pip install -r requirements.txt

# Install ProximaDB Python SDK
cd ../clients/python
pip install -e .
cd ../../demo

# Start ProximaDB server (in another terminal)
cd ..
cargo run --bin proximadb-server

# Run demo
python demo.py
```

## 📊 What the Demo Shows

### 1. Vector Operations
- **Batch Upsert**: Efficiently insert large numbers of vectors
- **Similarity Search**: Find nearest neighbors using various distance metrics
- **Vector Management**: Add, update, and delete vectors

### 2. Advanced Indexing
- **HNSW Integration**: Hierarchical Navigable Small World indexing
- **AXIS Support**: Adaptive eXtensible Indexing System
- **Partitioned Indices**: Automatic partitioning for scalability

### 3. Metadata Filtering
- **Logical Operators**: AND, OR, NOT queries
- **Comparison Operators**: =, !=, <, >, <=, >=
- **Complex Queries**: Nested logical expressions

### 4. Quantization
- **Product Quantization**: Compressed vector representations
- **Configurable Compression**: Adjustable compression ratios
- **Performance Optimization**: Memory usage vs. accuracy trade-offs

### 5. Performance Benchmarking
- **Search Latency**: Response time measurements
- **Throughput Testing**: Queries per second
- **Memory Usage**: Resource consumption analysis
- **Scalability Testing**: Performance under load

## 🏗️ Demo Architecture

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   Demo Script   │───▶│  ProximaDB      │───▶│   Monitoring    │
│   (Python)      │    │   Server        │    │  (Prometheus)   │
└─────────────────┘    └─────────────────┘    └─────────────────┘
         │                       │                       │
         │                       │                       ▼
         │                       │              ┌─────────────────┐
         │                       │              │    Grafana      │
         │                       │              │  (Dashboards)   │
         │                       │              └─────────────────┘
         │                       ▼
         │              ┌─────────────────┐
         │              │   Data Storage  │
         │              │ (WAL + VIPER +  │
         │              │      LSM)       │
         │              └─────────────────┘
         │
         ▼
┌─────────────────┐
│     Results     │
│   & Metrics     │
└─────────────────┘
```

## 📁 File Structure

```
demo/
├── demo.py                 # Main demo script
├── docker-compose.yml      # Complete demo environment
├── Dockerfile.demo         # Demo container configuration
├── requirements.txt        # Python dependencies
├── run_demo.sh            # Demo execution script
├── prometheus.yml         # Monitoring configuration
├── README.md              # This file
└── grafana/               # Grafana dashboards (optional)
    ├── dashboards/
    └── datasources/
```

## 🎯 Demo Scenarios

### Scenario 1: E-commerce Product Search
- **Vectors**: Product embeddings (768-dimensional)
- **Metadata**: Category, price, rating, brand
- **Queries**: "Find similar electronics under $500 with rating > 4.0"

### Scenario 2: Performance Benchmarking
- **Load Testing**: 1000+ vectors with batch operations
- **Latency Analysis**: P50, P95, P99 response times
- **Throughput Testing**: Concurrent search operations

### Scenario 3: Advanced Features
- **Quantization**: Demonstrate compression effects
- **HNSW Indexing**: Show indexing performance benefits
- **Metadata Filtering**: Complex logical expressions

## 📊 Expected Output

The demo produces comprehensive results including:

### Performance Metrics
```
📊 Benchmark Results:
  Search Latency (avg): 12.5ms
  Search Latency (p95): 18.2ms
  Search Throughput: 850 queries/second
  Memory Usage: 45.3 MB
  Compression Ratio: 4.2x
```

### Feature Demonstrations
- ✅ Vector upsert: 1000 vectors in 2.3s (435 vectors/sec)
- ✅ Basic search: 10 results in 8.7ms
- ✅ Filtered search: Complex metadata query in 15.2ms
- ✅ Quantization: 4.2x compression with 98.5% accuracy
- ✅ HNSW indexing: 60% faster search performance

### Collection Statistics
```
📊 Collection Statistics:
  Total vectors: 1000
  Memory usage: 45.32 MB
  Index size: 12.4 MB
  Compression ratio: 4.20x
  Quantization enabled: true
  HNSW partitions: 2
```

## 🔧 Configuration

### Environment Variables
- `PROXIMADB_SERVER_URL`: REST API endpoint (default: http://localhost:5678)
- `PROXIMADB_GRPC_URL`: gRPC endpoint (default: localhost:5679)
- `DEMO_COLLECTION_NAME`: Collection name for demo (default: demo_collection)
- `DEMO_NUM_VECTORS`: Number of vectors to generate (default: 1000)

### Demo Configuration
Edit `demo.py` to customize:
```python
@dataclass
class DemoConfig:
    vector_dimension: int = 768      # Embedding dimension
    num_vectors: int = 1000          # Dataset size
    enable_quantization: bool = True  # Use quantization
    enable_hnsw: bool = True         # Use HNSW indexing
    benchmark_iterations: int = 5     # Benchmark runs
```

## 📈 Monitoring

### Prometheus Metrics
Access Prometheus at http://localhost:9091 to view:
- Request rates and latency percentiles
- Memory and CPU usage
- Index performance metrics
- Error rates and health status

### Grafana Dashboards
Access Grafana at http://localhost:3000 (admin/admin) for:
- Real-time performance monitoring
- Historical trend analysis
- Custom dashboard creation
- Alert configuration

## 🐛 Troubleshooting

### Common Issues

1. **Connection Refused**
   ```bash
   # Check if ProximaDB server is running
   curl http://localhost:5678/health
   
   # Check Docker logs
   docker-compose logs proximadb
   ```

2. **Python SDK Import Error**
   ```bash
   # Install SDK in development mode
   cd ../clients/python
   pip install -e .
   ```

3. **Memory Issues**
   ```bash
   # Reduce vector count in demo
   export DEMO_NUM_VECTORS=100
   ```

4. **Port Conflicts**
   ```bash
   # Check port usage
   netstat -tulpn | grep -E ':(5678|5679|9090|3000)'
   
   # Modify ports in docker-compose.yml if needed
   ```

### Debug Mode
```bash
# Run with debug logging
RUST_LOG=debug docker-compose up

# Run demo with verbose output
python demo.py --verbose
```

## 🚀 Next Steps

After running the demo:

1. **Explore API Documentation**: Visit `/docs` endpoint for OpenAPI spec
2. **Try Custom Queries**: Modify demo script for your use case
3. **Performance Tuning**: Adjust configuration for your dataset
4. **Integration**: Use Python SDK in your applications
5. **Scaling**: Deploy with Kubernetes for production

## 📚 Additional Resources

- [ProximaDB Documentation](../docs/)
- [API Reference](../docs/api/)
- [Python SDK Guide](../clients/python/README.md)
- [Performance Tuning](../docs/performance.md)
- [Deployment Guide](../docs/deployment.md)

---

🎉 **Enjoy exploring ProximaDB's powerful vector database capabilities!**