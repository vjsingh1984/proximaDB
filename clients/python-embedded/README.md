# ProximaDB Embedded

**Zero-overhead embedded vector database for Python**

ProximaDB Embedded provides direct in-process access to ProximaDB's high-performance Rust core without any network overhead. Perfect for applications that need fast, local vector storage.

## Features

- **Zero Network Overhead**: Direct in-process API calls to Rust core
- **Multi-Disk Support**: Configure multiple storage locations with weighted distribution
- **SIMD Acceleration**: Automatic AVX2/NEON vector operation optimization
- **Full Persistence**: Write-ahead logging with configurable sync modes
- **NumPy Integration**: Zero-copy transfer of NumPy arrays
- **Context Manager**: Automatic resource cleanup

## Installation

```bash
pip install proximadb_embedded
```

### Build from Source

Requires Rust toolchain and maturin:

```bash
# Install maturin
pip install maturin

# Build and install
cd clients/python-embedded
maturin develop -m ../../Cargo.toml --release --features python,pylib -i python
```

Canonical import:

```python
import proximadb_embedded
```

## Quick Start

```python
import numpy as np
from proximadb_embedded import ProximaDB, DiskConfig

# Simple single-disk setup
db = ProximaDB(data_dirs="./my_database")

# Or multi-disk with weighted distribution
db = ProximaDB(
    data_dirs=[
        DiskConfig("/nvme/data", weight=2),  # Fast SSD - gets 2x data
        DiskConfig("/hdd/data", weight=1),   # Slower HDD
    ],
    metadata_dir="/nvme/metadata",
    cache_size_mb=2048,
)

# Create a collection
db.create_collection("embeddings", dimension=768, engine="sst")

# Insert vectors with NumPy
vectors = np.random.rand(10000, 768).astype(np.float32)
ids = [f"doc_{i}" for i in range(10000)]
metadata = [{"category": "A" if i % 2 == 0 else "B"} for i in range(10000)]

db.insert("embeddings", ids=ids, vectors=vectors, metadata=metadata)

# Search
query = np.random.rand(768).astype(np.float32)
results = db.search("embeddings", query=query, top_k=10)

for r in results:
    print(f"{r.id}: score={r.score:.4f}")

# Flush to ensure durability
db.flush()
```

## API Reference

### ProximaDB

Main database class for embedded operations.

```python
ProximaDB(
    data_dirs=None,           # Path or list of DiskConfig
    metadata_dir=None,        # Metadata storage path
    cache_size_mb=512,        # Cache size in MB
    default_engine="sst",     # Storage engine type
    enable_wal=True,          # Enable write-ahead logging
    wal_sync_mode="batch",    # WAL sync: "immediate", "batch", "async"
)
```

**Methods:**

- `create_collection(name, dimension, engine=None)` - Create a new collection
- `delete_collection(name)` - Delete a collection
- `get_collection(name)` - Get collection info
- `list_collections()` - List all collections
- `insert(collection, ids, vectors, metadata=None)` - Insert vectors
- `search(collection, query, top_k=10, filter=None)` - Search for similar vectors
- `flush()` - Flush pending writes to disk
- `stats()` - Get storage statistics

### DiskConfig

Configuration for multi-disk storage.

```python
DiskConfig(
    path="/data/proximadb",  # Storage directory path
    weight=1,                # Weight for data distribution
    tags=["hot", "ssd"],     # Optional tags
)
```

### Storage Engines

ProximaDB supports multiple storage engines optimized for different workloads:

| Engine | Best For | Characteristics |
|--------|----------|-----------------|
| `sst` | OLTP, real-time | Hybrid columnar, fast queries |
| `viper` | Analytics | Columnar Parquet, high compression |
| `nova` | Mixed workloads | Hybrid quantized |
| `swift` | Hot data | Hierarchical blocks, low latency |
| `raptor` | Adaptive | Matrix-optimized, workload learning |
| `helix` | High-dimensional | PCA + Hilbert clustering |

## Multi-Disk Configuration

ProximaDB can distribute data across multiple disks with weighted allocation:

```python
from proximadb_embedded import ProximaDB, DiskConfig

# Configure disks with different weights
disks = [
    DiskConfig("/nvme1/data", weight=3, tags=["hot", "ssd"]),
    DiskConfig("/nvme2/data", weight=3, tags=["hot", "ssd"]),
    DiskConfig("/hdd1/data", weight=1, tags=["cold", "hdd"]),
]

db = ProximaDB(
    data_dirs=disks,
    metadata_dir="/nvme1/metadata",  # Metadata on fast disk
)
```

Data is distributed based on weights:
- NVMe disks (weight=3 each) get 75% of data
- HDD (weight=1) gets 25% of data

## Context Manager

ProximaDB supports context manager for automatic cleanup:

```python
with ProximaDB(data_dirs="./data") as db:
    db.create_collection("test", dimension=128)
    # ... operations ...
# Automatically flushed on exit
```

## Performance Tips

1. **Use NumPy arrays**: Pass `np.float32` arrays directly for zero-copy transfer
2. **Batch inserts**: Insert vectors in batches of 1000-10000 for best throughput
3. **Choose the right engine**: Use `sst` for real-time, `viper` for analytics
4. **Enable batch WAL**: Use `wal_sync_mode="batch"` for better write throughput
5. **Size cache appropriately**: Larger cache = better read performance

## Comparison with Client SDK

| Feature | Embedded | Client SDK |
|---------|----------|------------|
| Network overhead | None | HTTP/gRPC |
| Deployment | In-process | Separate server |
| Scaling | Single node | Multi-node |
| Use case | Local apps | Distributed systems |

## License

Apache License 2.0

## Links

- [ProximaDB Repository](https://github.com/vjsingh1984/proximadb)
- [Documentation](https://github.com/vjsingh1984/proximadb#readme)
- [Issues](https://github.com/vjsingh1984/proximadb/issues)
