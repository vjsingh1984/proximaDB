# FastLanes Encoding and Quantized Vector Integration Analysis

## Executive Summary

FastLanesDataBlock provides **dual-mode encoding** (columnar vs row-wise) that automatically selects the optimal layout based on vector dimensions. This impacts how quantized vectors should be integrated across the 6 storage engines.

## 1. FastLanes Dual-Mode Architecture

### 1.1 Encoding Modes

```rust
pub enum VectorEncodingLayout {
    /// Columnar: Transpose vectors into dimension arrays
    /// Better for: compression ratio, analytics queries
    Columnar {
        dims_per_group: usize, // Typically 64 for SIMD
    },

    /// RowWise: Store vectors as contiguous byte arrays
    /// Better for: fast reconstruction, random access
    RowWise {
        compress_individual: bool,
    },
}
```

### 1.2 Automatic Selection Logic

```rust
// From encode_vectors_auto() method
if dimension < 1536 {
    // Use columnar for better compression
    encode_vectors_columnar(vectors, dims_per_group: 64)
} else {
    // Use row-wise for faster reconstruction
    encode_vectors_rowwise(vectors, apply_simd: dimension <= 2048)
}
```

## 2. Engine-Specific Storage Patterns

### 2.1 SST Engine
- **Current**: Uses FastLanesDataBlock with likely row-wise encoding
- **Quantization Integration**:
  ```rust
  struct FastLanesDataBlock {
      records: Vec<VectorRecord>, // VectorRecord.quantized field inline
      encoding_marker: u8,        // Determines columnar vs row-wise
  }
  ```
- **Recommendation**: Store quantized vectors inline in VectorRecord for row-wise locality

### 2.2 SWIFT Engine
- **Current**: Uses FastLanesDataBlock with SuperBlock hierarchy
- **Quantization Integration**:
  ```rust
  struct SuperBlock {
      blocks: Vec<FastLanesDataBlock>,
      quantized_signature: Vec<u8>, // SuperBlock-level quantization
  }
  ```
- **Recommendation**: Add quantized vectors both at block and superblock level

### 2.3 HELIX Engine
- **Current**: FastLanesDataBlock with columnar encoding + Hilbert ordering
- **Quantization Integration**:
  ```rust
  // HELIX uses columnar for spatial locality
  block.encoding_marker = 0x10; // Forces columnar
  block.quantized_section = Some(QuantizedSection {
      binary_vectors: Option<Vec<Vec<u8>>>, // Columnar binary
      int8_vectors: Option<Vec<Vec<i8>>>,   // Columnar INT8
      pq_vectors: Option<Vec<Vec<u8>>>,     // Columnar PQ
  });
  ```
- **Recommendation**: Store quantized vectors in columnar sections for cache efficiency

### 2.4 RAPTOR Engine
- **Current**: Direct FastLanes encoding (no DataBlock wrapper)
- **Quantization Integration**:
  ```rust
  struct RowGroup {
      columnar_data: Option<ColumnarBlock>,
      quantized_data: Option<QuantizedColumnarData>, // Separate columnar
  }

  struct QuantizedColumnarData {
      binary: Option<FastLanesEncodedData>,  // Direct encoding
      int8: Option<FastLanesEncodedData>,    // Direct encoding
      pq_codes: Option<FastLanesEncodedData>, // Direct encoding
  }
  ```
- **Recommendation**: Keep quantized data separate with direct FastLanes encoding

### 2.5 VIPER Engine
- **Current**: Pure Parquet columnar (no FastLanes)
- **Quantization Integration**:
  ```rust
  // Parquet columns
  columns.push(("vector", Float32Array));
  columns.push(("vector_binary", BinaryArray));  // Quantized binary
  columns.push(("vector_int8", Int8Array));      // Quantized INT8
  columns.push(("vector_pq8", BinaryArray));     // PQ codes
  ```
- **Recommendation**: Add separate Parquet columns for each quantization level

### 2.6 NOVA Engine
- **Current**: Pure columnar with progressive storage (no FastLanes)
- **Quantization Integration**: Similar to VIPER with Parquet columns

## 3. Quantized Vectors in FastLanesDataBlock

### 3.1 Current Structure

```rust
pub struct FastLanesDataBlock {
    // Main data
    pub records: Vec<VectorRecord>,

    // Quantization fields (currently exists!)
    pub quantized_vectors: Option<Vec<Vec<u8>>>,
    pub quantization_level: Option<UnifiedQuantizationLevel>,
    pub quantized_section: Option<QuantizedSection>,
}

pub struct QuantizedSection {
    pub binary_vectors: Option<Vec<Vec<u8>>>,
    pub int8_vectors: Option<Vec<Vec<i8>>>,
    pub pq_vectors: Option<Vec<Vec<u8>>>,
    pub codebooks: Option<Vec<Vec<f32>>>,
}
```

### 3.2 Integration Approach

**Option 1: Inline in VectorRecord** (Better for row-wise encoding)
```rust
// Quantized data travels with the record
record.quantized = Some(QuantizedVectors {
    binary: Some(binary_data),
    int8: Some(int8_data),
    pq8: Some(pq_codes),
});
```

**Option 2: Separate QuantizedSection** (Better for columnar encoding)
```rust
// Quantized data stored separately for columnar access
block.quantized_section = Some(QuantizedSection {
    binary_vectors: Some(all_binary_vectors),
    int8_vectors: Some(all_int8_vectors),
    pq_vectors: Some(all_pq_vectors),
});
```

## 4. Implementation Recommendations

### 4.1 For FastLanesDataBlock Engines (SST, SWIFT, HELIX)

1. **Check encoding mode first**:
   ```rust
   let is_columnar = block.encoding_marker & 0xF0 == 0x10; // Columnar markers
   ```

2. **Store accordingly**:
   - If row-wise: Use inline `VectorRecord.quantized`
   - If columnar: Use separate `block.quantized_section`

3. **Modify serialization**:
   ```rust
   impl FastLanesDataBlock {
       pub fn serialize_with_quantization(&self) -> Result<Vec<u8>> {
           // Existing vector encoding
           let encoded_vectors = self.encode_vectors()?;

           // Add quantized data based on layout
           if self.is_columnar() {
               // Encode quantized_section columnar
               result.extend(self.encode_quantized_columnar()?);
           } else {
               // Quantized data already inline in records
           }
       }
   }
   ```

### 4.2 For RAPTOR (Direct FastLanes)

Keep the current approach with separate `QuantizedColumnarData` using direct FastLanes encoding.

### 4.3 For Pure Columnar (VIPER, NOVA)

Add separate Parquet/Arrow columns for each quantization level.

## 5. Migration Path

### Phase 1: Extend VectorRecord Proto
```protobuf
message VectorRecord {
    // ... existing fields ...
    optional QuantizedVectors quantized = 20;
}
```

### Phase 2: Update FastLanesDataBlock Serialization
- Add quantization support to `serialize_with_config()`
- Handle both inline and separate storage based on encoding mode

### Phase 3: Modify Engine Flush Operations
- SST/SWIFT: Check encoding mode, store accordingly
- HELIX: Use columnar quantized sections
- RAPTOR: Keep separate with direct encoding
- VIPER/NOVA: Add Parquet columns

## 6. Performance Implications

### Row-wise Storage (dimensions >= 1536)
- **Pros**: Fast random access to complete quantized vectors
- **Cons**: Less compression, more memory usage
- **Best for**: SST, SWIFT point queries

### Columnar Storage (dimensions < 1536)
- **Pros**: Better compression, cache-friendly for scans
- **Cons**: Slower reconstruction of individual vectors
- **Best for**: HELIX spatial queries, RAPTOR analytics

## 7. Key Decisions Needed

1. **Should we force encoding mode for quantized data?**
   - Option A: Let auto-selection decide (current)
   - Option B: Force columnar for quantized data always
   - **Recommendation**: Option A for consistency

2. **Should quantized data use same encoding as main vectors?**
   - Option A: Same encoding scheme (Delta, BitPacked, etc.)
   - Option B: Specialized encoding for quantized data
   - **Recommendation**: Option B - use BitPacked for quantized

3. **Should we version the format?**
   - Option A: Use existing format version markers
   - Option B: Add quantization-specific version
   - **Recommendation**: Option A with new markers (0x70-0x74)

## 8. Testing Requirements

### Unit Tests
- Test row-wise quantization storage/retrieval
- Test columnar quantization storage/retrieval
- Test encoding mode auto-selection with quantized data

### Integration Tests
- End-to-end tests for each engine with quantization
- Performance benchmarks: precomputed vs runtime quantization
- Memory usage analysis

### Compatibility Tests
- Ensure backward compatibility with existing data
- Test migration from non-quantized to quantized storage

## Conclusion

FastLanesDataBlock's dual-mode encoding requires careful integration of quantized vectors. The approach should be:

1. **Adaptive**: Respect the automatic encoding selection
2. **Efficient**: Store quantized data according to the chosen layout
3. **Compatible**: Maintain backward compatibility
4. **Performant**: Optimize for the specific access patterns of each engine

The existing `quantized_section` field in FastLanesDataBlock provides the foundation - we just need to properly populate and serialize it based on the encoding mode.