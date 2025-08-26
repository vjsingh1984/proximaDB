# Search Result Architecture

## Overview
This document defines the standardized approach for handling search results across all storage engines in ProximaDB.

## Type Hierarchy

### 1. Internal Types (Core Domain)

#### `InternalSearchResult` (src/core/search/results.rs)
```rust
pub struct InternalSearchResult {
    pub id: String,
    pub vector_id: Option<String>,
    pub score: f32,
    pub similarity: Option<f32>,
    pub vector: Option<Vec<f32>>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub debug_info: Option<SearchDebugInfo>,
    pub source: Option<SourceContent>,
    pub expanded_context: Vec<SourceContent>,
}
```
- **Purpose**: Rich internal representation for processing
- **Used by**: All storage engines, internal services
- **Contains**: Full metadata, debug info, source content

### 2. Proto Types (API Layer)

#### `SearchVectorRecord` (src/proto/proximadb.proto)
```protobuf
message SearchVectorRecord {
    string id = 1;
    repeated float vector = 2;
    repeated MetadataItem metadata = 3;
    float score = 4;
    optional float similarity = 5;
    optional uint32 version = 6;
    optional uint32 timestamp = 7;
    optional SourceContent source = 10;
    repeated SourceContent expanded_context = 11;
}
```
- **Purpose**: Wire format for client communication
- **Used by**: REST/gRPC APIs
- **Contains**: Serializable data for network transfer

#### `SearchResult` (src/proto/proximadb.proto)
```protobuf
message SearchResult {
    repeated SearchVectorRecord results = 1;
    int64 total_found = 2;
    optional string collection_id = 3;
}
```
- **Purpose**: Container for multiple search results
- **Used by**: API responses
- **Contains**: Batch of results with metadata

## Data Flow

```
1. Client Request (REST/gRPC)
   ↓
2. API Handler (unified_handlers.rs)
   ↓
3. VectorOperationsService
   ↓
4. Storage Engine (via trait)
   ↓ Returns Vec<InternalSearchResult>
5. VectorOperationsService (converts)
   ↓ Converts to Vec<SearchVectorRecord>
6. API Handler
   ↓ Wraps in SearchResult
7. Client Response
```

## Implementation Guidelines

### Storage Engines

All storage engines MUST implement:
```rust
async fn search_vectors_unified(
    &self,
    ctx: &SearchContext,
) -> Result<Vec<InternalSearchResult>>
```

Example implementation:
```rust
impl UnifiedStorageEngine for ViperEngine {
    async fn search_vectors_unified(
        &self,
        ctx: &SearchContext,
    ) -> Result<Vec<InternalSearchResult>> {
        // Engine-specific search logic
        let results = self.search_internal(ctx).await?;
        
        // Convert to InternalSearchResult
        Ok(results.into_iter().map(|r| InternalSearchResult {
            id: r.id,
            score: r.score,
            vector: r.vector,
            metadata: r.metadata,
            similarity: r.similarity,
            source: r.source,
            expanded_context: r.expanded_context,
            ..Default::default()
        }).collect())
    }
}
```

### VectorOperationsService

Handles conversion from internal to proto format:
```rust
impl VectorOperationsService {
    pub async fn search_vectors(
        &self,
        request: SearchRequest,
    ) -> Result<SearchResult> {
        // Get internal results from storage
        let internal_results = self.storage_engine
            .search_vectors_unified(&ctx)
            .await?;
        
        // Convert to proto format
        let search_records: Vec<SearchVectorRecord> = 
            internal_results.into_iter()
                .map(|r| self.to_proto_record(r))
                .collect();
        
        // Return wrapped result
        Ok(SearchResult {
            results: search_records,
            total_found: search_records.len() as i64,
            collection_id: Some(collection_id),
        })
    }
    
    fn to_proto_record(&self, r: InternalSearchResult) -> SearchVectorRecord {
        SearchVectorRecord {
            id: r.id,
            vector: r.vector.unwrap_or_default(),
            metadata: convert_metadata(&r.metadata),
            score: r.score,
            similarity: r.similarity,
            version: None,
            timestamp: None,
            source: r.source,
            expanded_context: r.expanded_context,
        }
    }
}
```

## Benefits of This Architecture

1. **Separation of Concerns**
   - Storage engines focus on retrieval logic
   - Service layer handles format conversion
   - API layer deals with wire protocols

2. **Type Safety**
   - Internal types are strongly typed
   - Proto types match wire format exactly
   - Clear conversion boundaries

3. **Flexibility**
   - Storage engines can add custom fields to InternalSearchResult
   - Proto format can evolve independently
   - Easy to add new storage engines

4. **Performance**
   - Single conversion point (service layer)
   - Efficient internal processing
   - Zero-copy where possible with Arc

5. **Consistency**
   - All engines return same type
   - Unified conversion logic
   - Predictable behavior

## Migration Path

1. **Phase 1**: Ensure all engines return `Vec<InternalSearchResult>`
   - ✅ SST Engine
   - ✅ VIPER Engine (needs update)
   - ✅ Other engines

2. **Phase 2**: Update VectorOperationsService
   - Add conversion method
   - Handle all proto fields properly
   - Maintain backward compatibility

3. **Phase 3**: Clean up
   - Remove any direct proto dependencies from storage engines
   - Consolidate conversion logic
   - Add comprehensive tests

## Testing Strategy

1. **Unit Tests**: Each engine's search method
2. **Integration Tests**: End-to-end search flow
3. **Conversion Tests**: InternalSearchResult → SearchVectorRecord
4. **Compatibility Tests**: Ensure all fields are preserved

## Notes

- The `source` and `expanded_context` fields are crucial for RAG applications
- Debug info should only be included when requested (performance)
- Metadata conversion must handle all proto MetadataItem types
- Consider adding result caching at the service layer