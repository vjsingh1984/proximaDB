# ProximaDB Documentation

## Quick Links

- [Getting Started](user/getting_started.md) - Installation and first steps
- [Architecture Overview](architecture.md) - System design and components
- [API Reference](api/README.md) - REST and gRPC APIs
- [Storage Engines](storage-engines.md) - SST, VIPER, and other engines
- [Operations Guide](operations.md) - Deployment and monitoring

## Documentation Structure

```
docs/
├── README.md                    # This file
├── architecture.md              # Consolidated system architecture
├── storage-engines.md           # Unified storage engine documentation
├── operations.md                # Deployment and operations guide
├── api/                         # API specifications
│   ├── rest-api.md
│   └── grpc-api.md
├── user/                        # User guides
│   ├── getting_started.md
│   └── tutorials/
├── enhancements/                # Future work specifications (preserved)
│   └── [detailed enhancement specs]
└── diagrams/                    # Architecture diagrams
    └── architecture.mmd         # Consolidated Mermaid diagrams
```

## Key Documentation

### Core Documentation
- **[Architecture](architecture.md)** - System design, components, data flow
- **[Storage Engines](storage-engines.md)** - SST, VIPER, NOVA, SWIFT, PRISM, RAPTOR
- **[Operations](operations.md)** - Production deployment, monitoring, tuning

### API Documentation
- **[REST API](api/rest-api.md)** - HTTP/JSON endpoints
- **[gRPC API](api/grpc-api.md)** - Protocol buffer services
- **[Python SDK](../clients/python/README.md)** - Client library

### Development
- **[Contributing](../CONTRIBUTING.md)** - Development guidelines
- **[Building](../README.md#building)** - Build instructions
- **[Testing](../README.md#testing)** - Test suite

## Quick Start

```bash
# Install ProximaDB
docker run -d -p 5678:5678 -p 5679:5679 proximadb/proximadb:latest

# Create a collection
curl -X POST http://localhost:5678/collections \
  -H "Content-Type: application/json" \
  -d '{"name": "products", "dimensions": 384}'

# Insert vectors
curl -X POST http://localhost:5678/collections/products/vectors \
  -H "Content-Type: application/json" \
  -d '{"vectors": [{"id": "1", "vector": [0.1, 0.2, ...]}]}'

# Search
curl -X POST http://localhost:5678/collections/products/search \
  -H "Content-Type: application/json" \
  -d '{"vector": [0.1, 0.2, ...], "k": 10}'
```

## Performance Benchmarks

- **Insert**: 17,000 vectors/sec
- **Search**: < 1ms for 1M vectors (indexed)
- **Memory**: 30% less than alternatives
- **Compression**: 70% reduction with quantization

## Support

- [GitHub Issues](https://github.com/proximadb/proximadb/issues)
- [Discord Community](https://discord.gg/proximadb)
- [Documentation](https://docs.proximadb.com)