# ProximaDB Developer Documentation

## Quick Links
- [Technical Reference](technical_reference.adoc) - Detailed technical specifications
- [AXIS Architecture](axis_unified_tiering_architecture.adoc) - Index system design
- [SST Format](hierarchical_sst_format.adoc) - Storage format specification
- [Search Optimization](search_optimization_analysis.adoc) - Query optimization details

## Development Setup

### Building from Source
```bash
# Clone repository
git clone https://github.com/proximadb/proximadb.git
cd proximadb

# Build debug version
cargo build

# Build release version
cargo build --release

# Run tests
cargo test
```

### Project Structure
```
src/
├── api_handlers/      # REST/gRPC handlers
├── core/              # Core data structures
├── compute/           # Vector operations
├── index/             # AXIS index system
├── network/           # API servers
├── query/             # SQL engine
├── services/          # Business logic
├── storage/           # Storage engines
└── bin/
    └── server.rs      # Main server binary
```

## Contributing

### Code Style
- Use `cargo fmt` before committing
- Run `cargo clippy` for lints
- Add tests for new features
- Document public APIs

### Testing
```bash
# Unit tests
cargo test --lib

# Integration tests
cargo test --test '*'

# Specific module
cargo test storage::

# With logging
RUST_LOG=debug cargo test -- --nocapture
```

## Architecture Decisions

### Proto-First Design
All data flows through the system as Protocol Buffers:
- Zero serialization overhead
- Type safety across languages
- Efficient wire format

### Dual Storage Engines
- **SST**: Row-based for OLTP workloads
- **VIPER**: Columnar for OLAP workloads
- Automatic selection based on access patterns

### Hardware Adaptation
- Runtime CPU feature detection
- Automatic SIMD optimization
- Optional GPU acceleration

## Module Documentation

### Core Modules
- `core/` - Foundation types and utilities
- `storage/` - Persistence layer
- `index/` - Vector indexing (AXIS)
- `compute/` - Distance calculations
- `services/` - Service layer orchestration

### Storage Engines
- `storage/engines/impls/sst/` - SST implementation
- `storage/engines/impls/viper/` - VIPER implementation
- `storage/engines/impls/nova/` - NOVA hybrid engine
- `storage/engines/impls/swift/` - SWIFT streaming engine

### Networking
- `network/rest/` - REST API implementation
- `network/grpc/` - gRPC service implementation
- `network/middleware/` - Auth, rate limiting, etc.

## Performance Profiling

### CPU Profiling
```bash
# Using perf (Linux)
perf record --call-graph=dwarf cargo run --release
perf report

# Using Instruments (macOS)
cargo instruments -t "Time Profiler"
```

### Memory Profiling
```bash
# Using Valgrind
valgrind --tool=massif cargo run
ms_print massif.out.*

# Using heaptrack
heaptrack cargo run
heaptrack_gui heaptrack.*.gz
```

## Debugging

### Debug Builds
```bash
# Enable debug symbols
cargo build --profile=dev

# Run with GDB
gdb target/debug/proximadb-server
```

### Logging
```bash
# Module-specific logging
RUST_LOG=proximadb::storage=trace cargo run

# All debug logs
RUST_LOG=debug cargo run

# Structured logging
RUST_LOG=proximadb=info,proximadb::storage=debug cargo run
```

## Release Process

1. Update version in `Cargo.toml`
2. Run full test suite
3. Build release binaries
4. Create GitHub release
5. Publish Docker image
6. Update documentation