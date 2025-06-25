# Zero-Copy Migration Guide

## Overview

This guide outlines the steps to migrate from the current misaligned proto/Avro schemas to a fully aligned zero-copy implementation.

## Migration Steps

### 1. Update Proto Definition

Replace the current `VectorRecord` message in `proximadb.proto` with the aligned version:

```protobuf
import "google/protobuf/struct.proto";

message VectorRecord {
  string id = 1;                                          
  string collection_id = 2;                               
  repeated float vector = 3;                              
  map<string, google.protobuf.Value> metadata = 4;        
  int64 timestamp = 5;                                    
  int64 created_at = 6;                                   
  int64 updated_at = 7;                                   
  optional int64 expires_at = 8;                          
  int64 version = 9;                                      
  optional int32 rank = 10;                               
  optional float score = 11;                              
  optional float distance = 12;                           
}
```

### 2. Regenerate Proto Code

```bash
# Regenerate the Rust proto bindings
cargo build --workspace
```

### 3. Update Conversion Functions

The existing conversion functions in the codebase need to be updated or removed:

#### Remove Conversions (True Zero-Copy)
- Delete conversion functions between proto and Avro VectorRecord
- Use the same struct for both proto and storage layers

#### Update Metadata Handling
- Change metadata serialization from `HashMap<String, String>` to `HashMap<String, Value>`
- Update all metadata access code to handle protobuf Value types

### 4. Update Client Code

#### Collection ID Handling
The proto now includes `collection_id` in VectorRecord, so:
- Remove code that extracts collection_id from request context
- Update insert handlers to use the collection_id from each record

#### Timestamp Handling
- Remove microsecond to millisecond conversions
- Ensure all timestamps are consistently in milliseconds

### 5. Update Storage Layer

#### Avro Schema Update
Update the Avro schema to use the same field ordering as the proto:
1. id
2. collection_id  
3. vector
4. metadata
5. timestamp
6. created_at
7. updated_at
8. expires_at
9. version
10. rank
11. score
12. distance

### 6. Testing Strategy

1. **Unit Tests**: Update all VectorRecord construction to include new required fields
2. **Integration Tests**: Verify zero-copy path works end-to-end
3. **Performance Tests**: Benchmark to confirm zero-copy improvements
4. **Compatibility Tests**: Ensure existing data can be migrated

## Implementation Code Changes

### Update avro_unified.rs

```rust
// Ensure field order matches proto exactly
pub const VECTOR_RECORD_SCHEMA_JSON: &str = r#"{
  "type": "record",
  "name": "VectorRecord",
  "fields": [
    {"name": "id", "type": "string"},
    {"name": "collection_id", "type": "string"},
    {"name": "vector", "type": {"type": "array", "items": "float"}},
    {"name": "metadata", "type": {"type": "map", "values": ["null", "boolean", "int", "long", "float", "double", "string", {"type": "array", "items": "string"}]}},
    {"name": "timestamp", "type": "long"},
    {"name": "created_at", "type": "long"},
    {"name": "updated_at", "type": "long"},
    {"name": "expires_at", "type": ["null", "long"], "default": null},
    {"name": "version", "type": "long", "default": 1},
    {"name": "rank", "type": ["null", "int"], "default": null},
    {"name": "score", "type": ["null", "float"], "default": null},
    {"name": "distance", "type": ["null", "float"], "default": null}
  ]
}"#;
```

### Remove Conversion Functions

Delete or deprecate these conversion implementations:
- `impl From<proto::VectorRecord> for avro::VectorRecord`
- `impl From<avro::VectorRecord> for proto::VectorRecord`
- Any timestamp conversion utilities
- Metadata type conversion helpers

### Update Insert Handler

```rust
// Before: Extract collection_id from request
let collection_id = request.collection_id.clone();
for record in records {
    let mut record = record;
    record.collection_id = collection_id.clone();
    // ...
}

// After: Use collection_id from each record directly
for record in records {
    // record.collection_id already set
    // ...
}
```

## Benefits After Migration

1. **True Zero-Copy**: Direct memory mapping between proto and storage
2. **Reduced Latency**: No conversion overhead on insert path
3. **Lower Memory Usage**: No intermediate allocations
4. **Simplified Code**: Remove conversion layers
5. **Better Performance**: Especially for batch inserts

## Rollback Plan

If issues arise:
1. Keep old proto definition available as `proximadb_v1.proto`
2. Implement a compatibility layer that can handle both formats
3. Use feature flags to switch between old and new implementations
4. Gradually migrate clients to new format