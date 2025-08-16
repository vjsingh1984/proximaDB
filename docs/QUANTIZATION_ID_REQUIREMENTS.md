# Vector ID Requirements for Quantized Collections

## Overview
Collections with quantization enabled **REQUIRE** all vectors to have unique IDs. This is essential for tracking quantized representations across storage and indexes.

## Why IDs are Required for Quantization

### 1. Quantized Representation Tracking
- Each vector has multiple representations (FP32 original, PQ codes, binary sketches)
- IDs link these representations across storage layers and indexes
- Without IDs, we cannot maintain consistency between representations

### 2. Progressive Search Pipeline
- Binary filter stage needs to map candidates to IDs
- PQ ranking stage needs to retrieve quantized codes by ID
- FP32 reranking stage needs to fetch original vectors by ID
- IDs enable efficient multi-stage resolution

### 3. Index Synchronization
- AXIS indexes maintain in-memory quantized representations
- Storage maintains disk-based quantized representations
- IDs ensure both stay synchronized during updates

### 4. Compaction and Maintenance
- During compaction, vectors are reorganized
- IDs track vectors as they move between files
- Quantized representations must be updated consistently

## Validation Enforcement

### Server-Side (Rust)

#### VectorOperationsService
```rust
// In insert_vectors_direct method
if quant_config.enabled {
    for (i, vector) in vectors.iter().enumerate() {
        if vector.id.is_none() || vector.id.as_ref().map_or(true, |id| id.is_empty()) {
            return Err(anyhow::anyhow!(
                "Vector at index {} missing ID. Quantized collections require all vectors to have unique IDs",
                i
            ));
        }
    }
}
```

#### CollectionService  
```rust
// During collection creation
if quant_config.enabled {
    info!("⚠️ Collection '{}' has quantization enabled. All vectors MUST have unique IDs", name);
}
```

#### AXIS Manager
```rust
// Gracefully handles missing IDs
if let Some(id) = &vector.id {
    if !id.is_empty() {
        self.global_id_index.insert(id.clone(), collection_id, &vector).await?;
    }
}
```

### Client-Side (Python SDK)

```python
# In insert_vectors method
if config.quantization_config and config.quantization_config.enabled:
    for i, record in enumerate(records):
        if not record.id or record.id.strip() == "":
            raise ValueError(
                f"Vector at index {i} missing ID. "
                f"Collection '{collection_id}' has quantization enabled. "
                f"All vectors MUST have unique IDs."
            )
```

## Error Messages

### Insert without ID (Quantized Collection)
```
Error: Vector at index 0 missing ID. Quantized collections require all vectors to have unique IDs for tracking quantized representations
```

### Collection Creation Warning
```
⚠️ Collection 'embeddings' has quantization enabled. All vectors MUST have unique IDs for tracking quantized representations
```

## Best Practices

### 1. Always Provide IDs for Quantized Collections
```python
# Good ✅
vectors = [
    VectorRecord(
        id=f"doc_{i}",  # Unique ID required
        vector=embedding,
        metadata={"source": "document"}
    )
    for i, embedding in enumerate(embeddings)
]
client.insert_vectors("quantized_collection", records=vectors)

# Bad ❌ - Will fail for quantized collections
vectors = [
    VectorRecord(
        # Missing ID!
        vector=embedding,
        metadata={"source": "document"}
    )
    for i, embedding in enumerate(embeddings)
]
```

### 2. Use Meaningful ID Schemes
```python
# Document-based IDs
id = f"doc_{document_id}_chunk_{chunk_index}"

# Time-based IDs
id = f"event_{timestamp}_{sequence_num}"

# Hash-based IDs
id = hashlib.sha256(content.encode()).hexdigest()[:16]
```

### 3. Check Collection Configuration
```python
# Check if collection requires IDs
collection = client.get_collection("my_collection")
if collection.config.quantization_config and collection.config.quantization_config.enabled:
    print("This collection requires IDs for all vectors")
```

## Migration Guide

### For Existing Collections Without IDs
If you have an existing collection without IDs and want to enable quantization:

1. **Export existing vectors** with generated IDs
2. **Create new collection** with quantization enabled
3. **Re-insert vectors** with IDs
4. **Delete old collection** after verification

```python
# Step 1: Export with generated IDs
old_vectors = client.search_vectors("old_collection", query_vector=[0]*dim, top_k=10000)
vectors_with_ids = [
    VectorRecord(
        id=f"migrated_{i}",
        vector=v.vector,
        metadata=v.metadata
    )
    for i, v in enumerate(old_vectors)
]

# Step 2: Create quantized collection
client.create_collection(
    "new_collection",
    config=CollectionConfig(
        dimension=dim,
        quantization_config=QuantizationConfig(enabled=True)
    )
)

# Step 3: Insert with IDs
client.insert_vectors("new_collection", records=vectors_with_ids)

# Step 4: Verify and delete old
client.delete_collection("old_collection")
```

## FAQ

### Q: What happens if I try to insert without IDs into a quantized collection?
A: The insert will fail with a clear error message explaining that IDs are required.

### Q: Can I disable this requirement?
A: No. IDs are fundamental to how quantization works. You can disable quantization for the collection if IDs are not available.

### Q: What makes a good vector ID?
A: A good ID is:
- Unique within the collection
- Stable (doesn't change)
- Meaningful (helps identify the source)
- Reasonably short (8-64 characters)

### Q: Do non-quantized collections require IDs?
A: No. IDs are optional for non-quantized collections. The system will auto-generate IDs if not provided.

## Summary

- **Quantized collections REQUIRE unique IDs** for all vectors
- **Validation enforced** at both server and client levels
- **Clear error messages** guide users to provide IDs
- **Migration path** available for existing collections

This design ensures data integrity and enables the full benefits of quantization including progressive search and efficient storage.