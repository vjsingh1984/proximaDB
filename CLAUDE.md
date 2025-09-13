# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build and Development Commands

### Building the Project
```bash
# Debug build
cargo build

# Release build (production optimization)
cargo build --release

# Optimized server build
cargo build --profile release-server

# Check for compilation errors specifically
cargo build 2>&1 | tee current_error.log
```

### Running Tests
```bash
# Run all tests (Rust + Python)
make test

# Rust tests only
cargo test --verbose

# Integration tests
cargo test --test integration --verbose

# Python SDK tests (from clients/python directory)
cd clients/python && pytest tests/ -v

# Python integration tests (from tests/python directory)
cd tests/python && PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python PYTHONPATH=/workspace/clients/python/src python3 -m pytest -v

# Run specific test category
cargo test --test integration storage::

# Single test with debug output
RUST_LOG=debug cargo test test_name -- --nocapture

# Check current compilation status
cargo build 2>&1 | tee current_error.log
cat current_error.log | head -20  # Review recent errors
```

### Running the Server
```bash
# Development mode
cargo run --bin proximadb-server

# Production mode with config
./target/release/proximadb-server --config config/config.toml

# With specific logging
RUST_LOG=proximadb=debug cargo run --bin proximadb-server

# Docker deployment (recommended for production)
docker run -d -p 5678:5678 -p 5679:5679 -v proximadb_data:/data proximadb/proximadb:latest
```

### Code Quality
```bash
# Format code
cargo fmt

# Lint with clippy
cargo clippy -- -D warnings

# Full quality check (format + lint + test)
make check

# Run benchmarks
cargo bench

# Specific benchmarks
cargo bench --bench simd_distance_bench
cargo bench --bench engine_comparison_bench
cargo bench --bench flush_optimization_bench
cargo bench --bench vector_optimization_bench

# Run benchmark binary
cargo run --bin proximadb-bench
```

## Architecture Overview

ProximaDB is a unified intelligence platform combining vector search, graph relationships, and semantic knowledge in a single system. Built with a proto-first architecture for maximum performance.

### Core Architecture Layers

1. **Storage Layer** (`src/storage/`)
   - **Multiple Storage Engines**: SST, VIPER, NOVA, SWIFT, RAPTOR, PRISM, HELIX
   - **Unified Storage Interface**: All engines implement `UnifiedStorageEngine` trait
   - **IntelligentFilesystem**: Unified filesystem abstraction with caching (wraps Local, S3, Azure, GCS)
   - **Write-Ahead Log (WAL)**: Located in `src/storage/persistence/write_ahead_log/`
   - **Metadata Store**: Atomic operations with cloud backend support

2. **Compute Layer** (`src/compute/`)
   - **Unified Quantization**: All engines use `compute::quantization::unified` module
   - **Distance Computation**: Hardware-accelerated SIMD processing (`UnifiedDistanceCompute`)
   - **Quantization Levels**: Binary, INT8, PQ4, PQ8, PQ16, PQ32 with automatic selection
   - **Hardware Acceleration**: Automatic AVX2/AVX512/NEON/SSE detection

3. **API Layer** (`src/api_handlers/`, `src/network/`)
   - **Unified Handlers**: Single implementation for REST and gRPC
   - **Protocol Buffers**: Native VectorRecord flow without serialization overhead
   - **Multi-Server**: Concurrent REST (port 5678) and gRPC (port 5679) servers

4. **Index Layer** (`src/index/`)
   - **AXIS Engine**: Primary indexing system with tiering support
   - **Multiple Index Types**: HNSW, IVF, PQ, FLAT, ANNOY, LSH
   - **Progressive Search**: Multi-tier deduplication and early termination

5. **Services Layer** (`src/services/`)
   - **CollectionService**: Manages vector collections with engine selection
   - **VectorOperationsService**: Direct memtable access for operations
   - **EventLogService**: Persistent event logging for recovery

### Storage Engine Specializations

- **SST Engine**: Row-based, write-optimized with three-stage filtering
  - Best for: Real-time queries, frequent updates
  - Location: `src/storage/engines/impls/sst/`

- **VIPER Engine**: Columnar Parquet format with advanced quantization
  - Best for: Analytics, batch operations, compression
  - Location: `src/storage/engines/impls/viper/`

- **NOVA Engine**: Progressive columnar storage with multi-level quantization
  - Best for: Mixed workloads, progressive search
  - Location: `src/storage/engines/impls/nova/`

- **SWIFT Engine**: High-speed row-based with FastLanes encoding
  - Best for: Low-latency operations
  - Location: `src/storage/engines/impls/swift/`

- **RAPTOR Engine**: Adaptive row-group management with PxK optimization
  - Best for: Dynamic workloads
  - Location: `src/storage/engines/impls/raptor/`

- **PRISM Engine**: Memory-optimized with multi-resolution quantization
  - Best for: Memory-constrained environments
  - Location: `src/storage/engines/impls/prism/`

- **HELIX Engine**: Spiral-pattern storage for time-series data
  - Best for: Temporal data patterns
  - Location: `src/storage/engines/impls/helix/`

### Key Design Patterns

1. **Proto-First Pipeline**: VectorRecord is the native format throughout the system
2. **Zero-Copy Operations**: Direct memory access without intermediate serialization
3. **Unified Quantization**: All engines delegate to `compute::quantization::unified`
4. **Hardware Adaptive**: Automatic CPU/GPU feature detection and optimization
5. **IntelligentFilesystem**: Single abstraction for all storage backends with caching

### Configuration

Primary configuration file: `config/config.toml`

Key configuration sections:
- `[server]`: HTTP/gRPC ports, data directories
- `[storage]`: Engine selection, storage locations, metadata URLs
- `[storage.write_buffer]`: Flush thresholds and memory limits
- `[storage.compaction]`: Background optimization settings
- `[compute.quantization]`: Compression algorithm selection

### Feature Flags
Important Cargo feature flags (use with `--features`):
- `sql_frontend` (default): Modern SQL frontend vs legacy sql_engine
- `cloud-full`: Enable all cloud storage backends (AWS + Azure + GCP)
- `aws`, `azure`, `gcp`: Individual cloud storage backends
- `rocksdb`: RocksDB metadata backend support
- `distributed`, `standalone`: Deployment mode selection
- `gpu`: GPU acceleration support (CUDA, ROCm, MPS, OpenCL)
- `debug-filters`: Enable debug filtering for search operations

### Data Directories
- `/data/wal/`: Write-ahead log files
- `/data/metadata/`: Metadata storage with subdirs: current/, archive/, __staging/
- `/data/collections/`: Per-collection engine-specific files
- `/data/viper_data/`: VIPER engine columnar storage

## Important Architecture Concepts

### Quantization System
All storage engines use the unified quantization module at `src/compute/quantization/unified.rs`:
- **UnifiedQuantizationEngine**: Hardware-accelerated quantization with k-means++ clustering
- **Codebook Storage**: Persistent codebook management via `CodebookStore` trait  
- **Multiple Levels**: Binary, INT8, PQ4/8/16/32 with automatic quality selection
- **Hardware Acceleration**: AVX2/AVX512/NEON SIMD optimizations

### Filesystem Architecture
IntelligentFilesystem (`src/storage/persistence/filesystem/intelligent_filesystem.rs`):
- **Unified Interface**: Wraps Local, S3, Azure, GCS filesystems
- **Caching Layer**: Metadata and file content caching with LRU eviction
- **Zero-Copy I/O**: Direct memory mapping where possible
- **Factory Pattern**: FilesystemFactory routes URLs to appropriate implementations

### Storage Engine Integration
All engines implement the `UnifiedStorageEngine` trait with:
- **Common Interface**: Standardized insert, search, flush, compact operations
- **Engine-Specific Optimization**: Each engine optimizes for its use case
- **Metadata Consistency**: Shared metadata format across all engines
- **Cross-Engine Operations**: Engines can delegate operations to each other

## Development Guidelines

### When Fixing Compilation Errors
1. **Check current_error.log**: Always review the latest compilation log with `cargo build 2>&1 | tee current_error.log`
2. **Common Error Patterns**:
   - `struct import 'AxisConfig' is private`: Use public interfaces from index::axis modules
   - `this function takes 1 argument but 0 arguments were supplied`: Check UnifiedDistanceCompute requires DistanceMetric parameter
   - Lifetime errors: Review async/await usage and reference management
3. **Fix by Engine**: Group fixes by storage engine (NOVA, VIPER, SST, SWIFT, RAPTOR, PRISM, HELIX)
4. **Quantization Issues**: All engines should use `compute::quantization::unified`
5. **Filesystem Issues**: All engines should use `IntelligentFilesystem`
6. **Proto Types**: Use internal types, proto conversion only at service boundaries

### Testing Strategy
1. **Rust Unit Tests**: Located in individual modules and `tests/` directory
2. **Integration Tests**: `cargo test --test integration` - test system interactions  
3. **Engine-Specific Tests**: Each storage engine has its own test suite
4. **Python SDK Tests**: `clients/python/tests/` - test SDK functionality
5. **Python Integration Tests**: `tests/python/` - comprehensive system tests
6. **Benchmarks**: `benches/` - performance testing with criterion
7. **Current Status**: Use `current_error.log` to track compilation issues

### Important Files
- `src/lib.rs`: Main library entry point
- `src/bin/server.rs`: Server binary implementation
- `proto/proximadb.proto`: Protocol buffer definitions
- `src/storage/engines/factory.rs`: Storage engine selection logic
- `src/compute/quantization/unified.rs`: Unified quantization engine
- `src/storage/persistence/filesystem/intelligent_filesystem.rs`: Unified filesystem

### Health Checks
```bash
# REST API health check
curl http://localhost:5678/health

# gRPC health check (requires grpc-health-probe)
grpc-health-probe -addr=localhost:5679
```

### Performance Optimization
The system automatically detects and uses:
- SIMD instructions (AVX2/NEON)
- GPU acceleration (CUDA/ROCm/MPS)
- CPU cache sizes for optimal batching
- 13 compression algorithms with context-aware selection

### Python Client SDK
Location: `clients/python/`

Supports automatic protocol selection (REST/gRPC) with:
- Collection management
- Vector insertion/updates
- Similarity search with metadata filtering
- SQL-style queries
- Compression configuration

## Troubleshooting

### Common Issues
1. **Port conflicts**: Check `lsof -i :5678` and kill conflicting processes
2. **Permission issues**: `sudo chown -R $USER:$USER ./data`
3. **ARM64 build issues**: Use `cargo build --no-default-features`
4. **Quantization errors**: Ensure all engines use unified quantization module
5. **Filesystem errors**: Ensure all engines use IntelligentFilesystem

### Debugging Commands
```bash
# Debug logging for specific module
RUST_LOG=proximadb::storage=trace cargo run --bin proximadb-server

# Memory profiling
valgrind --tool=massif cargo run --bin proximadb-server

# Performance profiling (Linux)
perf record --call-graph=dwarf cargo run --release --bin proximadb-server
```