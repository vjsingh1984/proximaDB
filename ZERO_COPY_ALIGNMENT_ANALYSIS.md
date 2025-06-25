# Zero-Copy Alignment Analysis: VectorRecord

## Current Field Misalignments

### Proto VectorRecord (gRPC)
```rust
pub struct VectorRecord {
    pub id: Option<String>,                    // Optional
    pub vector: Vec<f32>,                      // Required
    pub metadata: HashMap<String, String>,     // String values only
    pub timestamp: Option<i64>,                // Optional, microseconds
    pub version: i64,                          // Required
    pub expires_at: Option<i64>,              // Optional
}
```

### Avro VectorRecord (Storage)
```rust
pub struct VectorRecord {
    pub id: String,                           // Required
    pub collection_id: String,                // Required (NOT in proto!)
    pub vector: Vec<f32>,                     // Required
    pub metadata: HashMap<String, Value>,     // JSON values
    pub timestamp: i64,                       // Required, milliseconds
    pub created_at: i64,                      // Required (NOT in proto!)
    pub updated_at: i64,                      // Required (NOT in proto!)
    pub expires_at: Option<i64>,             // Optional
    pub version: i64,                         // Required
    pub rank: Option<i32>,                    // Optional (NOT in proto!)
    pub score: Option<f32>,                   // Optional (NOT in proto!)
    pub distance: Option<f32>,                // Optional (NOT in proto!)
}
```

## Key Misalignments Preventing Zero-Copy

1. **Field Presence Mismatch**:
   - Proto: `id` is Optional
   - Avro: `id` is Required
   
2. **Missing Fields in Proto**:
   - `collection_id` - Critical for storage routing
   - `created_at`, `updated_at` - Required for audit
   - `rank`, `score`, `distance` - Used in search results

3. **Metadata Type Mismatch**:
   - Proto: `HashMap<String, String>` (simplified)
   - Avro: `HashMap<String, serde_json::Value>` (complex types)

4. **Timestamp Unit Mismatch**:
   - Proto: Microseconds since epoch
   - Avro: Milliseconds since epoch

## Solutions for Zero-Copy Alignment

### Option 1: Update Proto to Match Avro (Recommended)
```protobuf
message VectorRecord {
  string id = 1;                             // Make required
  string collection_id = 2;                  // Add missing field
  repeated float vector = 3;
  map<string, google.protobuf.Value> metadata = 4;  // Use protobuf Value
  int64 timestamp = 5;                       // Milliseconds
  int64 created_at = 6;                      // Add missing field
  int64 updated_at = 7;                      // Add missing field
  optional int64 expires_at = 8;
  int64 version = 9;
  optional int32 rank = 10;                  // For search results
  optional float score = 11;                 // For search results
  optional float distance = 12;              // For search results
}
```

### Option 2: Create Conversion Layer
Keep proto simple but add explicit conversion methods that handle the mismatches:
- Convert Optional<String> to String with defaults
- Inject collection_id from request context
- Convert timestamp units (micro → milli)
- Set created_at/updated_at during conversion
- Transform string metadata to JSON values

### Option 3: Dual Message Types
- `VectorRecordInput` - Simplified for client requests
- `VectorRecordStorage` - Full structure for internal use
- Explicit conversion between them

## Recommendation

For true zero-copy, **Option 1** is required. The proto definition must exactly match the Avro schema field-by-field, including:
- Same field names and types
- Same optionality
- Same data representations
- Same field ordering

Without this alignment, we'll always need a conversion step that allocates new memory and copies data.