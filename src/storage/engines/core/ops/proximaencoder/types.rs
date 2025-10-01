/// # Proxima Type Definitions
///
/// Core type definitions for the ProximaEncoder/Decoder system including data types,
/// encoding schemes, and layout strategies.

/// **Proxima Data Type** - Classification for optimal encoding selection
///
/// Different data types have different optimal encoding schemes based on
/// their characteristics and usage patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProximaDataType {
    /// f32 vector dimensions - Use Simple8b (normalized) or PForDelta (gaussian)
    F32Vector,
    /// f64 double-precision vectors
    F64Vector,
    /// i64 timestamps - Use DoubleDelta for monotonic sequences
    I64Timestamp,
    /// i64 ID columns - Use VByte for sparse IDs
    I64Id,
    /// i64 count/size columns - Use PForDelta
    I64Count,
    /// u64 hash values - Use BitPacked
    U64Hash,
    /// i8 quantized vectors - Use Simple8b
    Int8Quantized,
    /// u16 values
    U16Value,
    /// u32 values
    U32Value,
    /// PQ4 codes (4-bit product quantization)
    PQ4Codes,
    /// PQ8 codes (8-bit product quantization)
    PQ8Codes,
    /// Binary vectors (1-bit per dimension)
    BinaryVector,
}

impl ProximaDataType {
    /// Get optimal encoding scheme for this data type
    pub fn recommended_scheme(&self) -> ProximaScheme {
        match self {
            ProximaDataType::F32Vector => ProximaScheme::PForDelta { majority_bits: 20, base: 0 },
            ProximaDataType::F64Vector => ProximaScheme::PForDelta { majority_bits: 20, base: 0 },
            ProximaDataType::I64Timestamp => ProximaScheme::DoubleDelta { first_value: 0, first_delta: 1 },
            ProximaDataType::I64Id => ProximaScheme::VByte,
            ProximaDataType::I64Count => ProximaScheme::PForDelta { majority_bits: 16, base: 0 },
            ProximaDataType::U64Hash => ProximaScheme::BitPacked { bits: 64 },
            ProximaDataType::Int8Quantized => ProximaScheme::Simple8b,
            ProximaDataType::U16Value => ProximaScheme::VByte,
            ProximaDataType::U32Value => ProximaScheme::PForDelta { majority_bits: 24, base: 0 },
            ProximaDataType::PQ4Codes => ProximaScheme::BitPacked { bits: 4 },
            ProximaDataType::PQ8Codes => ProximaScheme::BitPacked { bits: 8 },
            ProximaDataType::BinaryVector => ProximaScheme::BitPacked { bits: 1 },
        }
    }
}

/// **Proxima Encoding Schemes** - Compression algorithms for columnar data
///
/// Each scheme is optimized for specific data patterns and provides different
/// compression ratios and decoding performance trade-offs.
///
/// ### Scheme Selection Guidelines:
/// - **Constant data (100% same value)**: RunLength (best compression)
/// - **Very sparse (>95% zeros)**: SparseCOO (30x compression)
/// - **Moderately sparse (70-95% zeros)**: SparseBitmap (15x compression)
/// - **Sequential/monotonic**: Delta or DoubleDelta
/// - **Small range normalized**: FrameOfReference
/// - **With outliers**: PatchedBase or PForDelta
/// - **General case**: BitPacked or Adaptive
///
/// ### Compression Ratio Expectations:
/// - RunLength: 100:1+ for constant data
/// - SparseCOO: 30:1 for 95% sparse
/// - SparseBitmap: 15:1 for 90% sparse
/// - Delta: 2-4:1 for sequential
/// - FrameOfReference: 3-6:1 for normalized
/// - BitPacked: 1.5-3:1 for general
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum ProximaScheme {
    /// Bit-packing with configurable bit width
    BitPacked { bits: u8 },
    /// Delta encoding with base value
    Delta { base: i64 },
    /// Frame of Reference encoding
    FrameOfReference { reference: i64, bits: u8 },
    /// Dictionary encoding for repeated values
    Dictionary,
    /// Run-length encoding for sequences
    RunLength,
    /// Patched encoding for outliers
    PatchedBase { base: i64, patch_bits: u8 },

    // === STATE-OF-THE-ART ENCODING SCHEMES ===

    /// PForDelta: Patched Frame of Reference with Delta encoding
    /// Optimal for sequences with outliers - stores exceptions separately
    /// Best compression for data with majority of small values and few large outliers
    PForDelta { majority_bits: u8, base: i64 },

    /// Zigzag encoding: Maps signed integers to unsigned using interleaved encoding
    /// Optimal for signed integers with small absolute values
    /// Formula: (n << 1) ^ (n >> 31) - excellent for time-series deltas
    Zigzag { bits: u8 },

    /// Simple-8b: Variable bit-width integer encoding in 32-bit words
    /// Packs multiple integers per word with optimal bit allocation
    /// Superior compression for mixed-range integer sequences
    Simple8b,

    /// Variable-byte encoding: 7 bits data + 1 continuation bit per byte
    /// Excellent for small positive integers, self-delimiting
    /// Optimal for sparse vectors and identifier sequences
    VByte,

    /// Sparse bitmap encoding: bitmap + non-zero values
    /// Optimal for 70-95% zero sparsity
    /// Format: [bitmap_size: u32][non_zero_count: u32][bitmap][values]
    /// Performance: 15x compression for 90% sparsity, +17% throughput
    SparseBitmap,

    /// Sparse COO (Coordinate) encoding: (index, value) pairs
    /// Optimal for >95% zero sparsity
    /// Format: [count: u32][(index: u16, value: f32), ...]
    /// Performance: 30x compression for 95% sparsity
    SparseCOO,

    /// Double-delta encoding: Delta of deltas for monotonic sequences
    /// Exceptional compression for time-series and ordered data
    /// Two-level differential encoding: Δ(Δ(values))
    DoubleDelta { first_value: i64, first_delta: i64 },

    /// SIMD-optimized run-length with bit-packed counts
    /// Enhanced RLE with SIMD acceleration and compact count representation
    SIMDRunLength { value_bits: u8, count_bits: u8 },

    /// Hybrid encoding: Combines multiple schemes within single block
    /// Automatically selects optimal encoding per chunk
    /// Meta-encoding for maximum compression across diverse patterns
    Hybrid { primary_scheme: u8, secondary_scheme: u8 },

    /// Gorilla encoding: XOR-based compression for time-series data
    /// Optimal for floating-point time-series with similar consecutive values
    Gorilla,

    /// Adaptive encoding: Automatically selects best encoding based on data
    /// Uses statistics to choose optimal encoding scheme
    Adaptive,
}

/// **Vector Encoding Layout Strategy** - How to organize vectors in storage
///
/// Determines whether to store vectors in columnar (transposed) or row-wise (contiguous) format.
/// This choice significantly impacts compression ratio, reconstruction speed, and query patterns.
///
/// ### Performance Trade-offs:
/// ```
/// ┌──────────────────┬─────────────┬─────────────────┬──────────────────┐
/// │ Layout           │ Compression │ Reconstruction  │ Best For         │
/// ├──────────────────┼─────────────┼─────────────────┼──────────────────┤
/// │ Columnar         │ 2-4x better │ Slower (2-3x)   │ Analytics, Batch │
/// │ RowWise          │ 1x baseline │ Faster (1x)     │ Point queries    │
/// └──────────────────┴─────────────┴─────────────────┴──────────────────┘
/// ```
///
/// ### When to Use:
/// - **Columnar**: dimension ≤ 512, analytics queries, batch operations
/// - **RowWise**: dimension > 512, point queries, low-latency requirements
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum VectorEncodingLayout {
    /// Columnar: Transpose vectors into dimension arrays for better compression
    /// Each dimension stored separately across all vectors
    /// Better for: compression ratio, analytics queries
    Columnar {
        /// Number of dimensions per group (typically 64 for SIMD)
        dims_per_group: usize,
    },

    /// RowWise: Store vectors together as contiguous byte arrays
    /// Each vector stored as a complete unit using bytemuck
    /// Better for: fast reconstruction, random access, high-dimensional vectors
    RowWise {
        /// Whether to apply compression per vector
        compress_individual: bool,
    },
}

/// Encoded dimension data
#[derive(Debug, Clone)]
pub struct EncodedDimension {
    pub dimension_index: usize,
    pub encoded_data: Vec<u8>,
    pub encoding_scheme: ProximaScheme,
}
