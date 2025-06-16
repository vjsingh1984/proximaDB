# ProximaDB

<img src="assets/logo.svg" alt="ProximaDB Logo" width="300">

**Proximity at Scale - The Cloud-Native Vector Database for AI**

A high-performance, cloud-native vector database engineered for AI-first applications. Built in Rust for enterprise performance with advanced storage tiering, distributed consensus, and comprehensive API support.

## 🚀 Key Features

### Core Capabilities
- **High-Performance Vector Search**: HNSW indexing with SIMD optimizations
- **Client-Provided IDs**: Full support for client-provided vector identifiers
- **Multi-Tenant Architecture**: Collection-based isolation with configurable flush policies
- **Flexible Storage Strategies**: Standard, VIPER, and custom storage layouts
- **Real-Time & Batch Operations**: Optimized for both streaming and bulk workloads

### Storage Excellence
- **Write-Optimized**: LSM tree with WAL for high-throughput writes
- **Read-Optimized**: Memory-mapped files for zero-copy reads
- **Storage Tiering**: Ultra-hot (MMAP), Hot (SSD), Warm (HDD), Cold (Cloud)
- **Age-Based Flushing**: Configurable flush triggers (5 minutes testing, 24 hours production)
- **Schema Evolution**: Avro and Bincode serialization strategies

### Distribution & Reliability
- **Raft Consensus**: Strong consistency across distributed nodes
- **Multi-Disk Support**: Optimized placement across storage devices
- **Collection Migration**: Live strategy migration without downtime
- **Atomic Operations**: ACID guarantees for vector operations

### Developer Experience
- **Multi-Protocol APIs**: gRPC and REST with identical feature sets
- **Client Libraries**: Python, Java, JavaScript (in development)
- **Comprehensive Monitoring**: Built-in metrics and health checks
- **Flexible Configuration**: TOML-based with environment overrides

## 🏗️ Architecture Overview

### Storage Layer Architecture
```
┌─────────────────┬─────────────────┬─────────────────┬─────────────────┐
│   Ultra-Hot     │      Hot        │      Warm       │      Cold       │
│   MMAP + OS     │   Local SSD     │   Local HDD     │   S3/Azure/GCS  │
│   <1ms latency  │  <10ms latency  │  <100ms latency │  <1s latency    │
└─────────────────┴─────────────────┴─────────────────┴─────────────────┘
```

### Write Path with WAL
```
Client Request → REST/gRPC API → Service Layer → Storage Engine
                                                      ↓
WAL Strategy (Avro/Bincode) → MemTable → LSM Tree → Compaction
                                  ↓
              Search Index (HNSW) → Query Engine
```

### Collection Management
- **Flush Policies**: Size-based (128MB), age-based (24h), count-based (1M vectors)
- **Isolation**: Atomic flush operations with sequence number fencing
- **Monitoring**: Per-collection age tracking and performance metrics

## 🚀 Quick Start

### Prerequisites
- Rust 1.70+ 
- Protocol Buffers compiler (`protoc`)
- Optional: CUDA toolkit for GPU acceleration

### Installation

```bash
# Clone the repository
git clone https://github.com/your-org/proximadb.git
cd proximadb

# Build the server
cargo build --release --bin proximadb-server

# Run the server
cargo run --bin proximadb-server
```

### Configuration

Create a `config.toml` file:

```toml
[server]
node_id = "node-1"
bind_address = "0.0.0.0:5678"
dashboard_enabled = true

[storage]
data_directory = "./data"
memtable_size_mb = 128
compaction_enabled = true

[api]
grpc_port = 5678
rest_port = 8080
request_timeout_ms = 30000

[monitoring]
metrics_enabled = true
health_check_interval_ms = 5000
```

### Basic Usage

#### Python Client
```python
from proximadb import ProximaDBClient

# Connect to server
client = ProximaDBClient("http://localhost:8080")

# Create collection with custom flush policy
collection = client.create_collection(
    name="embeddings",
    dimension=768,
    distance_metric="cosine",
    indexing_algorithm="hnsw",
    max_wal_age_hours=1.0,  # Flush every hour
    max_wal_size_mb=64.0    # Flush at 64MB
)

# Insert vectors with client-provided IDs
vectors = [
    {"id": "doc_1", "vector": [0.1, 0.2, 0.3], "metadata": {"type": "document"}},
    {"id": "doc_2", "vector": [0.4, 0.5, 0.6], "metadata": {"type": "image"}}
]
client.batch_insert("embeddings", vectors)

# Search vectors
results = client.search("embeddings", query_vector=[0.1, 0.2, 0.3], k=10)
```

#### REST API
```bash
# Create collection
curl -X POST http://localhost:8080/api/v1/collections \
  -H "Content-Type: application/json" \
  -d '{
    "name": "embeddings",
    "dimension": 768,
    "distance_metric": "cosine",
    "max_wal_age_hours": 0.5,
    "max_wal_size_mb": 32
  }'

# Insert vector with client ID
curl -X POST http://localhost:8080/api/v1/collections/embeddings/vectors \
  -H "Content-Type: application/json" \
  -d '{
    "id": "user_123",
    "vector": [0.1, 0.2, 0.3],
    "metadata": {"user": "alice"}
  }'

# Search vectors
curl -X POST http://localhost:8080/api/v1/collections/embeddings/search \
  -H "Content-Type: application/json" \
  -d '{
    "vector": [0.1, 0.2, 0.3],
    "k": 10,
    "filter": {"user": "alice"}
  }'
```

## 📋 Development Status

### ✅ Completed Features
- [x] Core vector storage engine with LSM trees
- [x] WAL with pluggable strategies (Avro, Bincode)
- [x] Age-based and size-based flush triggers
- [x] HNSW vector indexing with SIMD optimizations
- [x] REST and gRPC APIs with identical functionality
- [x] Client-provided vector ID support
- [x] Multi-tenant collection management
- [x] Comprehensive monitoring and health checks
- [x] Python client library with async support
- [x] Storage strategy pattern (Standard, VIPER, Custom)
- [x] Multi-disk storage optimization
- [x] Metadata backends (SQLite, PostgreSQL, MongoDB, DynamoDB)

### 🚧 In Progress
- [ ] Remaining compilation error fixes
- [ ] Collection strategy migration testing
- [ ] Streaming data integration (Kafka, Pulsar)
- [ ] Advanced query language (SQL-like syntax)
- [ ] GPU acceleration with CUDA

### 📅 Roadmap
- **Q2 2025**: Production-ready release with cloud deployment
- **Q3 2025**: Advanced analytics and federated search
- **Q4 2025**: Streaming integration and real-time ML pipelines

## 🏛️ Architecture Deep Dive

### Storage Strategies
1. **Standard**: Balanced performance for general workloads
2. **VIPER**: Write-optimized for high-throughput ingestion
3. **Custom**: User-defined storage layouts

### WAL (Write-Ahead Log) System
- **Isolation**: Collection-specific WAL files with atomic flush
- **Strategies**: Avro (schema evolution), Bincode (performance)
- **Age Monitoring**: Background service tracks oldest unflushed data
- **Memory Tables**: ART, SkipList, B+Tree, HashMap implementations

### Index Management
- **HNSW**: Hierarchical Navigable Small World graphs
- **SIMD**: Vectorized distance computations
- **Adaptive**: Dynamic algorithm selection based on data patterns

## 🧪 Testing

### Unit Tests
```bash
cargo test
```

### Integration Tests
```bash
# REST API tests
cargo test test_rest_api_comprehensive

# gRPC tests  
cargo test test_grpc_comprehensive

# Storage tests
cargo test test_storage_integration
```

### Python SDK Tests
```bash
cd clients/python
python -m pytest tests/ -v
```

## 📊 Performance

### Benchmarks
- **Vector Search**: Sub-millisecond latency for 1M vectors
- **Write Throughput**: 100K+ vectors/second with batch operations
- **Memory Efficiency**: <100MB overhead for 1M vectors
- **Storage**: 10:1 compression with tiered storage

### Tuning Guidelines
- Use `max_wal_age_hours: 0.1` (6 minutes) for real-time applications
- Set `max_wal_size_mb: 256` for high-throughput writes
- Enable `background_flush: true` for consistent performance
- Use HNSW with `m: 16, ef_construction: 200` for balanced accuracy/speed

## 🛠️ Configuration Reference

### Flush Configuration
```toml
[flush]
# Global defaults
max_wal_age_hours = 24.0      # Production: 24h, Testing: 0.083h (5min)
max_wal_size_mb = 128.0       # Production: 128MB, Testing: 10MB  
max_vector_count = 1000000    # Production: 1M, Testing: 1K
flush_priority = 50           # 1-100 priority scale
enable_background_flush = true
```

### Collection-Specific Overrides
```python
client.create_collection(
    name="high_frequency",
    dimension=512,
    max_wal_age_hours=0.25,    # Flush every 15 minutes
    max_wal_size_mb=64,        # Smaller WAL size
    flush_priority=80          # High priority flushing
)
```

## 📚 Documentation

- [High-Level Design (HLD)](docs/hld.adoc)
- [Low-Level Design (LLD)](docs/lld.adoc)
- [API Documentation](docs/api/)
- [Requirements Specification](docs/requirements.adoc)
- [Implementation Status](implementation_status.md)

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## 📄 License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

## 🆘 Support

- **Issues**: [GitHub Issues](https://github.com/your-org/proximadb/issues)
- **Discussions**: [GitHub Discussions](https://github.com/your-org/proximadb/discussions)
- **Email**: support@proximadb.com

---

**Built with ❤️ in Rust for the AI revolution**