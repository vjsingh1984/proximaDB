# proximadb

<img src="assets/logo.svg" alt="ProximaDB Logo" width="240">

**proximity at scale**

A cloud-native vector database engineered for AI-first applications. Built in Rust for enterprise performance with:
- MMAP-based reads with zero-copy performance
- LSM tree-based append-only writes  
- Distributed consensus via Raft
- Multi-disk storage optimization
- gRPC/REST APIs
- Document and ER schema support
- Multi-language client libraries

## Architecture

### Storage Layer
- **Read Path**: MMAP with OS page cache for fast vector search
- **Write Path**: LSM tree for append-only operations
- **Multi-disk**: Rotating disks for writes, SSDs for reads

### Distribution
- Raft consensus for strong consistency
- Horizontal scaling across instances
- Configurable consistency vs availability

### APIs
- gRPC for inter-service and client communication
- REST wrapper for development ease
- Multi-language client support

## Quick Start

```bash
cargo run --bin proximadb-server
```

## Development Plan

See DEVELOPMENT.md for detailed phases and implementation order.