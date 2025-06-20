# ProximaDB Architecture V2: Lean Binary Avro Design

## Overview

ProximaDB V2 implements a lean, high-performance architecture using binary Avro records throughout the entire stack. This design eliminates wrapper objects and provides zero-copy operations from client to WAL.

## Core Principles

### 1. Zero Wrapper Objects
- **No Boxing/Unboxing**: Direct Avro record operations everywhere
- **Memory Efficiency**: Eliminate intermediate object allocations
- **Performance**: Direct binary serialization without conversions

### 2. Binary Avro First
- **Client → Server**: Binary Avro payloads in gRPC
- **Server → Service**: Zero-copy Avro record references
- **Service → WAL**: Direct binary Avro storage
- **Schema Versioning**: Consistent schema evolution safety

### 3. Protocol Layer Separation
- **REST Handler**: Thin JSON ↔ Avro conversion layer
- **gRPC Handler**: Direct binary Avro payload routing
- **Common Service**: Single Avro-based business logic

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                        CLIENT LAYER                             │
├─────────────────────────────────────────────────────────────────┤
│  Python SDK (Direct Avro)     │    gRPC Client (Avro Payload)   │
│  ┌─────────────────────────┐   │   ┌─────────────────────────┐    │
│  │ avro_dict = {...}       │   │   │ avro_bytes = serialize  │    │
│  │ avro_bytes = serialize  │   │   │ grpc_request.payload =  │    │
│  │ rest_post(avro_bytes)   │   │   │   avro_bytes           │    │
│  └─────────────────────────┘   │   └─────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                      PROTOCOL LAYER                             │
├─────────────────────────────────────────────────────────────────┤
│     REST Server (5678)         │      gRPC Server (5679)         │
│  ┌─────────────────────────┐   │   ┌─────────────────────────┐    │
│  │ JSON → Avro Conversion  │   │   │ Direct Avro Payload     │    │
│  │ Thin handler only       │   │   │ Zero-copy routing       │    │
│  └─────────────────────────┘   │   └─────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                   UNIFIED SERVICE LAYER                         │
├─────────────────────────────────────────────────────────────────┤
│                UnifiedAvroService                               │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │ • Schema versioning & validation                       │    │
│  │ • Plugin-based WAL strategy (Avro/Bincode)            │    │
│  │ • RT memtable selection                                │    │
│  │ • All business logic in binary Avro                   │    │
│  │ • Zero wrapper object operations                      │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                      STORAGE LAYER                              │
├─────────────────────────────────────────────────────────────────┤
│    WAL Manager (Strategy)       │      Storage Engine            │
│  ┌─────────────────────────┐   │   ┌─────────────────────────┐    │
│  │ Strategy: Avro/Bincode  │   │   │ VIPER/HNSW/IVF Engine  │    │
│  │ Memtable: RT/BTree      │   │   │ Index Management       │    │
│  │ Zero-copy operations    │   │   │ Vector Operations      │    │
│  └─────────────────────────┘   │   └─────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

## Data Flow

### 1. Vector Insert Flow (Zero-Copy)
```
Client:     avro_dict → avro_bytes
           ↓
gRPC:      avro_bytes (no conversion)
           ↓  
Service:   parse_versioned_payload(&avro_bytes) → &[u8] reference
           ↓
WAL:       append_avro_entry(&avro_bytes) → direct storage
           ↓
Storage:   vector_data extracted from avro_record
```

### 2. Schema Versioning
```
Versioned Payload Format:
┌──────────────┬──────────────┬──────────────┬──────────────┐
│ Schema Ver   │ Op Type Len  │ Operation    │ Avro Data    │
│ (4 bytes)    │ (4 bytes)    │ Type         │ (variable)   │
└──────────────┴──────────────┴──────────────┴──────────────┘
```

## Component Details

### UnifiedAvroService
**Purpose**: Single source of truth for all operations
**Features**:
- Plugin-based WAL strategy selection
- Schema version validation
- Zero wrapper object operations
- Performance metrics tracking

```rust
pub struct UnifiedAvroService {
    storage: Arc<RwLock<StorageEngine>>,
    wal: Arc<WALManager>,
    wal_strategy_type: WalStrategyType,  // Avro or Bincode
    avro_schema_version: u32,
}
```

### WAL Strategy Pattern
**Avro Strategy**: Schema evolution, cross-language compatibility
**Bincode Strategy**: Maximum performance, Rust-specific

```rust
// Configuration determines strategy at startup
let config = UnifiedServiceConfig {
    wal_strategy: WalStrategyType::Avro,  // or Bincode
    memtable_type: MemTableType::BTreeMap, // RT memtable
    avro_schema_version: 1,
};
```

### Protocol Handlers

#### gRPC Handler (Lean)
```rust
async fn insert_vector(request: AvroRequest) -> AvroResponse {
    // Zero conversion - direct payload routing
    let result = service.insert_vector(&request.avro_payload).await?;
    AvroResponse { avro_payload: result, .. }
}
```

#### REST Handler (Lean)
```rust
async fn insert_vector(json: Json<Value>) -> Result<Vec<u8>> {
    // Thin conversion layer only
    let avro_bytes = json_to_avro(&json)?;
    let result = service.insert_vector(&avro_bytes).await?;
    avro_to_json(&result)
}
```

## Performance Benefits

### Memory Efficiency
- **Zero Wrapper Allocation**: No intermediate objects
- **Reference-Based**: `&[u8]` references throughout Rust stack
- **Direct Operations**: Binary Avro → Storage without conversions

### CPU Efficiency
- **No Serialization Overhead**: Single format end-to-end
- **Zero-Copy WAL**: Direct byte storage
- **SIMD Opportunities**: Aligned binary data

### Network Efficiency
- **Binary Protocol**: Smaller payloads than JSON
- **gRPC Compression**: Built-in payload compression
- **Schema Evolution**: Backward compatibility without versioning overhead

## Configuration

### Service Configuration
```rust
let service_config = UnifiedServiceConfig {
    wal_strategy: WalStrategyType::Avro,      // Default for compatibility
    memtable_type: MemTableType::BTreeMap,   // RT memtable for performance  
    avro_schema_version: 1,                  // Schema version
    enable_schema_evolution: true,           // Safety checks
};
```

### WAL Configuration
```rust
let wal_config = WalConfig {
    strategy_type: WalStrategyType::Avro,
    memtable: MemTableConfig {
        memtable_type: MemTableType::BTreeMap,
        max_memory_size: 256 * 1024 * 1024,  // 256MB
    },
    compression: CompressionConfig {
        enabled: true,
        algorithm: CompressionAlgorithm::Lz4,
    },
};
```

## Client SDK Changes

### Python SDK (Zero Wrapper)
```python
# OLD (with wrappers)
record = VectorRecord(id="123", vector=[1,2,3], metadata={"tag": "test"})
client.insert_vector(collection_id, record)

# NEW (direct Avro)
avro_record = {
    "id": "123",
    "vector": [1, 2, 3], 
    "metadata": {"tag": "test"},
    "timestamp": int(time.time() * 1_000_000)
}
client.insert_vector(collection_id, avro_record)
```

### Protocol Selection
```python
# Auto-select best protocol (gRPC preferred)
client = proximadb.connect(url="http://localhost:5678")

# Force specific protocols
grpc_client = proximadb.connect_grpc(url="http://localhost:5679") 
rest_client = proximadb.connect_rest(url="http://localhost:5678")
```

## Migration Path

### Phase 1: Infrastructure ✅
- [x] Remove AvroRPC server
- [x] Create UnifiedAvroService with strategy pattern
- [x] Update WAL for binary Avro operations
- [x] Implement schema versioning

### Phase 2: Protocol Handlers
- [ ] Create lean gRPC handlers with Avro payload routing
- [ ] Create lean REST handlers with JSON↔Avro conversion
- [ ] Update server builders and configuration

### Phase 3: Client Updates
- [ ] Remove wrapper objects from Python SDK
- [ ] Implement direct Avro operations
- [ ] Add protocol selection and fallback

### Phase 4: Testing & Optimization
- [ ] End-to-end testing
- [ ] Performance benchmarking
- [ ] Schema evolution testing

## Compatibility

### Backward Compatibility
- **Schema Evolution**: Avro forward/backward compatibility
- **Version Negotiation**: Client/server version matching
- **Graceful Degradation**: Protocol fallback support

### Cross-Language Support
- **Python**: Direct Avro with `avro-python3`
- **Java**: Avro code generation
- **JavaScript**: Avro serialization libraries
- **Go**: Avro binary protocols

This architecture provides a clean separation of concerns while maximizing performance through zero-copy operations and eliminating unnecessary object allocations.