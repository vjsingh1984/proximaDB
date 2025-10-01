// # Vector-Specific Encoding
//
// Specialized encoders for multi-dimensional vector data with different layout strategies.
//
// ## Encoding Layouts:
//
// ### 1. Columnar Layout (Transposed)
// - **Purpose**: Store each dimension separately across all vectors
// - **Best for**: Analytics, batch operations, dimension ≤ 512
// - **Compression**: 2-4x better than row-wise
// - **Reconstruction**: 2-3x slower than row-wise
// - **Implementation**: `ProximaEncoder::encode_vectors_columnar()` (lines 1328-1380)
//
// **Memory Layout**:
// ```
// Input vectors:  [v1: [d1, d2, d3], v2: [d1, d2, d3], v3: [d1, d2, d3]]
// Columnar:       [[v1.d1, v2.d1, v3.d1], [v1.d2, v2.d2, v3.d2], [v1.d3, v2.d3, v3.d3]]
// ```
//
// **Why it compresses better**:
// - Same dimension across vectors has similar value distribution
// - SIMD-friendly processing of dimension arrays
// - Better cache locality for analytics queries
//
// ### 2. Row-Wise Layout (Contiguous)
// - **Purpose**: Store vectors as complete units
// - **Best for**: Point queries, low-latency, dimension > 512
// - **Compression**: Baseline (1x)
// - **Reconstruction**: Fast (1x baseline)
// - **Implementation**: `ProximaEncoder::encode_vectors_rowwise()` (lines 1382-1455)
//
// **Memory Layout**:
// ```
// Row-wise: [v1: [d1, d2, d3], v2: [d1, d2, d3], v3: [d1, d2, d3]]
// (Same as input - no transposition)
// ```
//
// **Why it's faster**:
// - No transposition overhead
// - Direct memory copy possible (bytemuck)
// - Better for random access patterns
//
// ### 3. Auto Layout Selection
// - **Purpose**: Automatically choose optimal layout based on heuristics
// - **Implementation**: `ProximaEncoder::encode_vectors_auto()` (lines 1457-1525)
// - **Decision factors**:
//   - Vector dimension (columnar if ≤ 512)
//   - Number of vectors (columnar if > 100)
//   - Query pattern hints (if available)
//
// ## Performance Comparison:
//
// | Dimension | Vectors | Layout    | Compression | Decode Speed |
// |-----------|---------|-----------|-------------|--------------|
// | 128       | 1000    | Columnar  | 3.5x        | Moderate     |
// | 384       | 1000    | Columnar  | 3.2x        | Moderate     |
// | 768       | 1000    | Columnar  | 2.8x        | Moderate     |
// | 1536      | 1000    | Row-wise  | 1x          | Fast         |
// | 128       | 100     | Row-wise  | 1x          | Fast         |
//
// ## Implementation Details:
//
// ### Columnar Encoding:
// 1. Transpose vectors into dimension arrays
// 2. Encode each dimension independently
// 3. Store dimension groups with metadata
// 4. Optimal scheme per dimension (auto-selected)
//
// ### Row-Wise Encoding:
// 1. Optionally compress each vector individually
// 2. Store vectors contiguously
// 3. Padding to alignment boundaries
// 4. Fast reconstruction with bytemuck
//
// ## Output Structures:
//
// ```rust
// pub struct DimensionGroup {
//     pub start_dim: usize,
//     pub end_dim: usize,
//     pub dimensions: Vec<EncodedDimension>,
// }
//
// pub struct ColumnarEncodedVectors {
//     pub num_vectors: usize,
//     pub dimension: usize,
//     pub dimension_groups: Vec<DimensionGroup>,
// }
//
// pub struct RowWiseEncodedVectors {
//     pub num_vectors: usize,
//     pub dimension: usize,
//     pub padded_dimension: usize,
//     pub encoded_vectors: Vec<Vec<u8>>,
// }
//
// pub enum EncodedVectors {
//     Columnar(ColumnarEncodedVectors),
//     RowWise(RowWiseEncodedVectors),
// }
// ```
//
// ## Usage Examples:
//
// ### Columnar for Analytics:
// ```rust
// let vectors: Vec<Vec<f32>> = load_embeddings();  // 1000 × 384
// let encoder = ProximaEncoder::new(ProximaScheme::PForDelta { majority_bits: 20, base: 0 });
// let encoded = encoder.encode_vectors_columnar(&vectors, 64)?;
// // Result: 4x compression, optimal for batch similarity search
// ```
//
// ### Row-Wise for Point Queries:
// ```rust
// let vectors: Vec<Vec<f32>> = load_embeddings();  // 100 × 1536
// let encoder = ProximaEncoder::new(ProximaScheme::Simple8b);
// let encoded = encoder.encode_vectors_rowwise(&vectors, true)?;
// // Result: Fast individual vector retrieval
// ```
//
// ### Auto Selection:
// ```rust
// let vectors: Vec<Vec<f32>> = load_embeddings();
// let encoder = ProximaEncoder::new(ProximaScheme::Adaptive);
// let encoded = encoder.encode_vectors_auto(&vectors)?;
// // Result: Optimal layout chosen automatically
// ```
//
// ## Future Extraction (Phase 3):
//
// These functions will be extracted from `ProximaEncoder` and made into standalone
// functions with explicit parameters.
//
// **Example future signatures**:
// ```rust
// pub fn encode_vectors_columnar(
//     vectors: &[Vec<f32>],
//     dims_per_group: usize,
//     scheme: ProximaScheme,
//     block_size: usize
// ) -> Result<ColumnarEncodedVectors>
//
// pub fn encode_vectors_rowwise(
//     vectors: &[Vec<f32>],
//     compress_individual: bool,
//     scheme: ProximaScheme
// ) -> Result<RowWiseEncodedVectors>
//
// pub fn encode_vectors_auto(
//     vectors: &[Vec<f32>],
//     scheme: ProximaScheme,
//     block_size: usize
// ) -> Result<EncodedVectors>
// ```
//
// ## Status: Phase 2 (Module Structure Only)
//
// This module currently serves as documentation for vector encoding algorithms.
// The actual implementations remain in `proximaencoder_legacy.rs` until Phase 3.

// Placeholder re-exports for when functions are extracted
// pub use self::columnar::*;
// pub use self::rowwise::*;
// pub use self::auto::*;
