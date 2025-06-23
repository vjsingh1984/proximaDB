# ProximaDB Vector Operations Testing Guide

## Prerequisites

1. **Rust 1.75+ Required**: The project uses Rust edition 2024, which requires Rust 1.75 or later.
   ```bash
   # Update Rust to latest stable
   rustup update stable
   rustup default stable
   
   # Verify version (should be 1.75+)
   rustc --version
   ```

2. **Python Dependencies**:
   ```bash
   pip install transformers torch sentence-transformers lorem numpy psutil
   ```

## Building and Starting the Server

### Option 1: Using the start script
```bash
./start_and_test.sh
```

### Option 2: Manual steps
```bash
# Create data directories
mkdir -p ./data/{wal,metadata,store}

# Build the server
cargo build --bin proximadb-server --release

# Start the server
RUST_LOG=info cargo run --bin proximadb-server --release
```

## Running the Test Suite

Once the server is running, execute the tests in order:

### 1. SDK Verification (Quick Check)
```bash
python test_grpc_sdk_verification.py
```
- Tests basic CRUD operations
- Verifies gRPC client implementation

### 2. Comprehensive gRPC Operations Test
```bash
python test_comprehensive_grpc_operations.py
```
- Tests all vector operations with BERT embeddings
- Single & bulk insert/update/delete
- Similarity search, filtered search, search by ID
- Performance benchmarking

### 3. Vector Coordinator Integration Test
```bash
python test_vector_coordinator_integration.py
```
- Tests Vector Coordinator routing
- VIPER engine integration
- Parquet storage and compression
- WAL integration

### 4. Pipeline Data Flow Test
```bash
python test_pipeline_data_flow.py
```
- Traces data through: gRPC → UnifiedAvroService → VectorCoordinator → WAL
- Verifies each component processes data correctly
- Tests error propagation

### 5. Zero-Copy Performance Test
```bash
python test_zero_copy_performance.py
```
- Benchmarks WAL write performance
- Memory efficiency testing
- Concurrent operations testing
- Latency percentiles (P50, P95, P99)

### 6. BERT Operations Test
```bash
python test_vector_operations_bert.py
```
- Real-world scenario with BERT embeddings
- 10KB document processing
- Filterable metadata operations

## Run All Tests
```bash
./run_vector_tests.sh
```

## What Gets Tested

### Pipeline Components:
- **gRPC Service** → Handles protocol translation
- **UnifiedAvroService** → Zero-copy Avro operations
- **VectorCoordinator** → Routes to storage engines
- **VIPER Engine** → Parquet storage with compression
- **WAL** → Write-ahead logging for durability

### Operations Tested:
- ✅ Collection management (create, get, list, delete)
- ✅ Vector insert (single & bulk with zero-copy)
- ✅ Vector update (single & bulk)
- ✅ Vector delete (single & bulk)
- ✅ Similarity search with BERT embeddings
- ✅ Filtered search by metadata
- ✅ Search by vector ID
- ✅ Concurrent operations
- ✅ Error handling

### Performance Characteristics:
- Zero-copy Avro for inserts
- Trust-but-verify WAL writes
- VIPER storage with Parquet format
- HNSW indexing for fast similarity search

## Monitoring

While tests are running, monitor:
1. **Server logs**: Shows tracing output from Rust components
2. **Test output**: Shows operation success/failure and performance metrics
3. **Data directory**: Check `./data/` for WAL and storage files

## Troubleshooting

1. **Server won't start**: 
   - Check port 5678 is free: `lsof -i :5678`
   - Verify config.toml paths are correct

2. **Build fails**:
   - Ensure Rust 1.75+ is installed
   - Clear cargo cache: `cargo clean`
   - Remove Cargo.lock if version conflicts

3. **Tests fail**:
   - Ensure server is running: `curl localhost:5678/health`
   - Check Python dependencies are installed
   - Review server logs for errors