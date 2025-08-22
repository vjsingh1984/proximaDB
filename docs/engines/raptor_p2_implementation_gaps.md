# RAPTOR P² Matrix Implementation Gaps

## Critical Gaps Found

### 1. **P² Matrix Structure Missing**
- **Location**: `src/storage/engines/raptor/common.rs`
- **Gap**: No `P2Matrix` or `IntraRowgroupMatrix` struct defined
- **Need**: Structure to store upper-triangle distances within rowgroup

### 2. **Writer Still Using HNSW**
- **Location**: `src/storage/engines/raptor/writer.rs:1123-1195`
- **Gap**: `build_local_hnsw_segment()` still being called
- **Need**: Replace with `build_p2_matrix()` function

### 3. **Config Flag for HNSW**
- **Location**: `src/storage/engines/raptor/config.rs`
- **Gap**: `enable_local_hnsw` flag still exists
- **Need**: Remove flag or repurpose for P² matrix

### 4. **P² Matrix Builder Not Implemented**
- **Location**: `src/storage/engines/raptor/writer.rs`
- **Gap**: No function to build P² matrix
- **Need**: Function to compute P×(P-1)/2 distances

### 5. **Reader Not Loading P² Matrix**
- **Location**: `src/storage/engines/raptor/consolidated_reader.rs`
- **Gap**: No code to read P² matrix from rowgroup
- **Need**: Deserialization and usage in search

### 6. **FastLanes Integration Missing**
- **Gap**: P² matrix not using FastLanes encoding
- **Need**: Compress with FastLanes + quantization

### 7. **SIMD Acceleration Not Applied**
- **Gap**: No SIMD usage for P² matrix operations
- **Need**: Use UnifiedDistanceCompute with hardware acceleration

## Implementation Plan

### Phase 1: Define P² Matrix Structure
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2Matrix {
    /// Number of vectors in rowgroup
    pub num_vectors: u32,
    
    /// Upper triangle distances (P×(P-1)/2 values)
    /// Stored as linear array, indexed by formula
    pub distances: Vec<u8>,  // Quantized INT8
    
    /// Quantization parameters
    pub min_distance: f32,
    pub max_distance: f32,
    
    /// Compression metadata
    pub compression: CompressionStrategy,
    pub compressed_size: u32,
}
```

### Phase 2: Implement P² Matrix Builder
```rust
impl RaptorWriter {
    fn build_p2_matrix(
        &self,
        vectors: &[Vec<f32>],
    ) -> Result<P2Matrix> {
        let n = vectors.len();
        let upper_triangle_size = n * (n - 1) / 2;
        let mut distances = Vec::with_capacity(upper_triangle_size);
        
        // Use UnifiedDistanceCompute with SIMD
        let distance_compute = UnifiedDistanceCompute::new();
        
        // Compute upper triangle only
        for i in 0..n {
            for j in (i + 1)..n {
                let dist = distance_compute.cosine(&vectors[i], &vectors[j]);
                distances.push(dist);
            }
        }
        
        // Quantize to INT8
        let (quantized, min, max) = self.quantization_engine
            .quantize_to_u8(&distances);
        
        // Apply FastLanes encoding
        let compressed = self.fastlanes_encoder
            .encode(&quantized)?;
        
        Ok(P2Matrix {
            num_vectors: n as u32,
            distances: compressed,
            min_distance: min,
            max_distance: max,
            compression: CompressionStrategy::Quantized8,
            compressed_size: compressed.len() as u32,
        })
    }
}
```

### Phase 3: Replace HNSW Calls in Writer
```rust
// In flush_row_page()
// BEFORE:
let hnsw_segment_data = if self.config.enable_local_hnsw {
    let local_segment = self.build_local_hnsw_segment(...)?;
    Some(bincode::serialize(&local_segment)?)
} else {
    None
};

// AFTER:
let p2_matrix_data = {
    let p2_matrix = self.build_p2_matrix(&page_vectors)?;
    bincode::serialize(&p2_matrix)?
};
```

### Phase 4: Update Reader to Use P² Matrix
```rust
impl RaptorReader {
    fn search_within_rowgroup(
        &self,
        rowgroup: &RowGroup,
        query: &[f32],
        p2_matrix: &P2Matrix,
    ) -> Vec<SearchResult> {
        // Use P² matrix for intra-rowgroup navigation
        // Access distance: idx = i×(2n-i-1)/2 + j - i - 1
    }
}
```

### Phase 5: Update Config
```rust
pub struct RaptorConfig {
    // Remove or rename
    // pub enable_local_hnsw: bool,
    pub enable_p2_matrix: bool,  // Default: true
    pub p2_quantization: UnifiedQuantizationLevel,  // Default: INT8
    pub p2_compression: CompressionStrategy,  // Default: FastLanes
}
```

## Testing Requirements

1. **Unit Tests**:
   - P² matrix builder correctness
   - Upper triangle indexing formula
   - Quantization accuracy
   - FastLanes compression ratio

2. **Integration Tests**:
   - End-to-end write/read with P² matrix
   - Search accuracy with P² vs HNSW
   - Performance benchmarks

3. **Validation**:
   - Memory usage (should be ~512KB for P=1024)
   - SIMD acceleration working
   - No accuracy loss vs full matrix

## Estimated Effort

- **Phase 1**: 2 hours (define structures)
- **Phase 2**: 4 hours (implement builder)
- **Phase 3**: 2 hours (update writer)
- **Phase 4**: 4 hours (update reader)
- **Phase 5**: 1 hour (update config)
- **Testing**: 3 hours

**Total**: ~16 hours of implementation work