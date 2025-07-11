# ProximaDB Metadata Backend Architecture

## Overview

ProximaDB supports multiple metadata backend implementations to meet different performance, scalability, and deployment requirements. The metadata backend is responsible for storing and managing collection metadata, including collection configurations, schemas, statistics, and mappings.

## Available Backends

### 1. Filestore Backend (Default)
The primary metadata backend that uses the filesystem API for storage with Avro serialization.

**Features:**
- Avro-based serialization with schema evolution
- Atomic writes with staging and rename operations
- Incremental operation log for recovery
- Support for local filesystem and cloud storage (S3, GCS, Azure)
- Snapshot-based recovery with archival
- Single unified index for consistent lookups

**Use Cases:**
- Default choice for most deployments
- Cloud-native deployments with object storage
- Scenarios requiring simple deployment without external dependencies

**Configuration:**
```toml
[metadata_backend]
backend_type = "filestore"
storage_url = "file:///var/lib/proximadb/metadata"
cache_size_mb = 64
compression_enabled = true
```

### 2. Cached Filestore Backend
An enhanced version of the filestore backend with multi-level caching for improved performance.

**Features:**
- Two-level cache architecture (L1 hot cache, L2 warm cache)
- LRU eviction policy with configurable TTL
- Write-through and write-back caching modes
- Cache warming on startup
- Detailed cache statistics and monitoring
- Automatic cache promotion based on access patterns

**Use Cases:**
- High-read workloads with frequently accessed collections
- Deployments with thousands of collections
- Scenarios where metadata access latency is critical

**Configuration:**
```toml
[metadata_backend]
backend_type = "cached_filestore"
storage_url = "file:///var/lib/proximadb/metadata"

[metadata_backend.cache]
l1_cache_size = 1000      # Number of hot collections
l2_cache_size = 10000     # Number of warm collections  
l1_ttl_seconds = 300      # 5 minutes
l2_ttl_seconds = 3600     # 1 hour
write_back_enabled = false # Use write-through by default
warm_up_on_start = true   # Preload cache on startup
```

### 3. RocksDB Backend
A high-performance embedded key-value store backend using RocksDB.

**Features:**
- LSM tree architecture for write optimization
- Column families for organized data storage
- Built-in compression and bloom filters
- ACID transactions with optimistic concurrency
- Efficient range queries and prefix scans
- Point-in-time backups and snapshots
- Configurable compaction strategies

**Use Cases:**
- Very high write throughput requirements
- Large number of collections (100K+)
- Deployments requiring ACID transactions
- Scenarios needing efficient range queries

**Configuration:**
```toml
[metadata_backend]
backend_type = "rocksdb"
db_path = "/var/lib/proximadb/metadata/rocksdb"
enable_compression = true
use_bloom_filters = true
block_cache_size_mb = 64
write_buffer_size_mb = 16
enable_transactions = true
```

### 4. Memory Backend
An in-memory backend primarily used for testing and development.

**Features:**
- Ultra-fast operations with no disk I/O
- Full API compatibility for testing
- No persistence (data lost on restart)

**Use Cases:**
- Unit testing
- Development environments
- Temporary deployments

## Backend Selection Guide

Choose your metadata backend based on these criteria:

| Backend | Best For | Pros | Cons |
|---------|----------|------|------|
| **Filestore** | General use, cloud deployments | Simple, reliable, cloud-compatible | Slower for large datasets |
| **Cached Filestore** | Read-heavy workloads | Fast reads, configurable caching | Higher memory usage |
| **RocksDB** | Write-heavy, large scale | High performance, ACID transactions | Requires local disk, complex tuning |
| **Memory** | Testing only | Fastest possible | No persistence |

## Implementation Details

### Dependency Injection Pattern

The metadata backend is injected into the storage engine using a trait-based dependency injection pattern:

```rust
#[async_trait]
pub trait CollectionMetadataProvider: Send + Sync {
    async fn get_collection_uuid(&self, collection_id: &str) -> Result<Option<String>>;
    async fn get_collection_metadata(&self, collection_id: &str) -> Result<Option<CollectionRecord>>;
    async fn get_collection(&self, collection_id: &str) -> Result<Option<Collection>>;
    async fn list_collections(&self) -> Result<Vec<CollectionRecord>>;
    async fn collection_exists(&self, collection_id: &str) -> Result<bool>;
}
```

This design:
- Breaks circular dependencies between StorageEngine and CollectionService
- Allows runtime backend selection
- Enables easy testing with mock implementations
- Supports backend switching without code changes

### Initialization Flow

1. **SharedServices** creates the metadata backend based on configuration
2. **CollectionService** is initialized with the metadata backend
3. **StorageEngine** is created without collection service dependency
4. **SharedServices** injects CollectionService into StorageEngine
5. Both REST and gRPC handlers share the same CollectionService instance

### Performance Optimizations

#### Filestore Backend
- Single unified index eliminates dual-index synchronization
- Atomic writes using staging directories
- Incremental operation log for fast recovery
- Avro compression for reduced storage

#### Cached Filestore Backend
- Two-level cache reduces disk I/O by 90%+
- Automatic promotion of hot collections to L1 cache
- Batch write-back for improved write throughput
- Cache warming eliminates cold start penalty

#### RocksDB Backend
- Column families optimize different access patterns
- Bloom filters reduce unnecessary disk reads
- Block cache keeps hot data in memory
- Compression reduces storage requirements

## Migration Guide

### Migrating from Filestore to RocksDB

1. Export existing metadata:
```bash
proximadb-admin metadata export --format json > metadata_backup.json
```

2. Update configuration:
```toml
[metadata_backend]
backend_type = "rocksdb"
db_path = "/var/lib/proximadb/metadata/rocksdb"
```

3. Import metadata:
```bash
proximadb-admin metadata import --format json < metadata_backup.json
```

### Enabling Caching

1. Update configuration to use cached backend:
```toml
[metadata_backend]
backend_type = "cached_filestore"
# ... rest of configuration
```

2. Restart ProximaDB server
3. Monitor cache statistics via metrics endpoint

## Monitoring

All backends expose metrics for monitoring:

- **Operation counts**: reads, writes, deletes
- **Latency percentiles**: p50, p95, p99
- **Cache statistics**: hit rates, evictions (cached backend)
- **Storage usage**: disk space, memory usage
- **Error rates**: failed operations

Access metrics at: `http://localhost:5678/metrics`

## Future Enhancements

1. **Redis Backend**: For distributed caching across nodes
2. **etcd Backend**: For distributed consensus-based metadata
3. **PostgreSQL Backend**: For SQL-based queries and reporting
4. **Multi-Backend Replication**: Primary/secondary configurations
5. **Encryption at Rest**: For all backend types

## Best Practices

1. **Choose the right backend** for your workload characteristics
2. **Monitor metrics** to ensure backend performance meets requirements
3. **Configure caching** appropriately for read-heavy workloads
4. **Plan for growth** - RocksDB scales better for large deployments
5. **Test thoroughly** when switching backends in production
6. **Backup regularly** regardless of backend choice