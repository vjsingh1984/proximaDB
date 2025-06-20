# ProximaDB Schema Synchronization

## Overview

ProximaDB uses a **unified schema approach** to prevent drift between client SDK and WAL implementations. All components reference the same Avro schema definition to ensure data compatibility.

## Schema Synchronization Strategy

### Single Source of Truth
- **Master Schema**: `schemas/proximadb_core.avsc`
- **Distribution**: Automatic copying to all components
- **Validation**: Build-time verification of schema consistency

### Architecture Benefits

```
┌─────────────────────────────────────────────────────────────────┐
│                    SINGLE SCHEMA SOURCE                         │
│                schemas/proximadb_core.avsc                      │
└─────────────────┬───────────────────────────────────────────────┘
                  │ (synchronized by build_scripts/sync_schemas.py)
                  ▼
┌─────────────────────────────────────────────────────────────────┐
│                    DISTRIBUTED COPIES                           │
├─────────────────────────────────────────────────────────────────┤
│  Python SDK          WAL Storage         Documentation          │
│  ┌─────────────┐    ┌─────────────┐     ┌─────────────┐        │
│  │ schemas/    │    │ src/wal/    │     │ docs/       │        │
│  │ core.avsc   │    │ schemas/    │     │ schemas/    │        │
│  └─────────────┘    │ core.avsc   │     │ core.avsc   │        │
│                     └─────────────┘     └─────────────┘        │
└─────────────────────────────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────────┐
│                 GENERATED CONSTANTS                             │
├─────────────────────────────────────────────────────────────────┤
│  Python Constants        Rust Constants                         │
│  ┌─────────────────┐    ┌─────────────────┐                    │
│  │ schema_         │    │ schema_         │                    │
│  │ constants.py    │    │ constants.rs    │                    │
│  └─────────────────┘    └─────────────────┘                    │
└─────────────────────────────────────────────────────────────────┘
```

## Lean Schema Design

### Optional Fields with Smart Defaults

#### VectorRecord (Lean Design)
```json
{
  "name": "VectorRecord",
  "fields": [
    {
      "name": "id",
      "type": ["null", "string"],
      "default": null,
      "doc": "Auto-generated if null - reduces client payload"
    },
    {
      "name": "vector", 
      "type": {"type": "array", "items": "float"},
      "doc": "REQUIRED - core vector data"
    },
    {
      "name": "metadata",
      "type": ["null", "map"],
      "default": null,
      "doc": "Optional - omitted if empty for network efficiency"
    },
    {
      "name": "timestamp",
      "type": ["null", "long"],
      "default": null,
      "doc": "Auto-generated server-side if null"
    },
    {
      "name": "expires_at",
      "type": ["null", "long"], 
      "default": null,
      "doc": "TTL support - null means no expiration"
    }
  ]
}
```

#### SearchResult (Network Optimized)
```json
{
  "name": "SearchResult",
  "fields": [
    {
      "name": "score",
      "type": "float",
      "doc": "REQUIRED - similarity score"
    },
    {
      "name": "id",
      "type": ["null", "string"],
      "default": null,
      "doc": "Optional - null for anonymous results"
    },
    {
      "name": "vector",
      "type": ["null", "array"],
      "default": null,
      "doc": "Only included if explicitly requested (network efficiency)"
    },
    {
      "name": "metadata",
      "type": ["null", "map"],
      "default": null,
      "doc": "Only included if explicitly requested (network efficiency)"
    }
  ]
}
```

## Implementation Details

### Build Process
```bash
# 1. Synchronize schemas across all components
python3 build_scripts/sync_schemas.py

# 2. Generate language-specific constants
# Python: clients/python/src/proximadb/schema_constants.py  
# Rust: src/schema_constants.rs

# 3. Validate schema consistency
# Ensures all copies are identical
```

### Client SDK Usage (Python)
```python
from proximadb.avro_ops import avro_ops

# Lean vector creation (smart defaults)
vector_record = avro_ops.create_vector_record(
    vector=[0.1, 0.2, 0.3],              # REQUIRED
    # id=None,                           # Auto-generated
    # metadata=None,                     # Omitted if empty
    # timestamp=None,                    # Auto-generated
    # expires_at=None                    # No expiration
)

# Result: Only required/provided fields in payload
# Network payload: ~50% smaller than full record
```

### Server Side (Rust)
```rust
// WAL uses identical schema - zero conversion needed
let vector_record = parse_avro_vector_record(&payload)?;

// Smart defaults applied automatically
let id = vector_record.id.unwrap_or_else(|| generate_id());
let timestamp = vector_record.timestamp.unwrap_or_else(|| now_micros());

// Direct storage without conversion
wal.append_avro_entry("insert_vector", &payload).await?;
```

## Schema Evolution

### Backward Compatibility
- **Field Addition**: New optional fields with defaults
- **Field Deprecation**: Mark as optional, maintain for N versions
- **Version Tracking**: Schema version in all payloads

### Forward Compatibility  
- **Unknown Fields**: Ignored by older clients
- **Schema Registry**: Version negotiation between client/server
- **Graceful Degradation**: Fallback to supported features

## Benefits

### Performance
- **Lean Payloads**: 30-50% reduction in network traffic
- **Zero Conversion**: Same schema from client → WAL
- **Memory Efficiency**: No duplicate schema definitions

### Reliability
- **Schema Drift Prevention**: Build-time validation
- **Consistent Behavior**: Identical data structures everywhere  
- **Type Safety**: Compile-time schema validation

### Maintainability
- **Single Source**: One schema file to maintain
- **Automatic Sync**: Build process handles distribution
- **Documentation**: Schema serves as API contract

## Validation Process

### Build-Time Checks
1. **Schema Syntax**: Valid Avro schema format
2. **Schema Distribution**: All copies identical to source
3. **Generated Constants**: Language-specific validation
4. **Backward Compatibility**: Evolution rules enforced

### Runtime Checks
1. **Payload Validation**: Schema compliance verification
2. **Version Compatibility**: Client/server version matching
3. **Required Fields**: Presence validation for critical fields
4. **Default Application**: Smart defaults for optional fields

## Best Practices

### Schema Design
- **Required Fields**: Only truly essential data
- **Optional Fields**: Everything else with sensible defaults
- **Lean Defaults**: Null for optional reduces payload size
- **Evolution Ready**: Plan for future field additions

### Client Implementation
- **Smart Defaults**: Auto-generate IDs, timestamps
- **Payload Optimization**: Omit optional null fields
- **Validation**: Check required fields before serialization
- **Version Tracking**: Include schema version in requests

### Server Implementation  
- **Schema Validation**: Verify payload compatibility
- **Default Application**: Handle missing optional fields
- **Version Support**: Multiple schema versions during transitions
- **Error Handling**: Clear messages for schema mismatches

---

This unified schema approach ensures ProximaDB maintains data consistency while optimizing for performance and network efficiency.