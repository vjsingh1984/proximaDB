/// # ProximaEncoder/ProximaDecoder - Baseline SIMD-Friendly Encoding System
///
/// ## Architecture Overview
///
/// **ProximaEncoder** and **ProximaDecoder** provide a **baseline implementation** of Proxima
/// compression schemes optimized for LLVM auto-vectorization. These modules serve as the
/// **portable fallback layer** when hardware-specific SIMD is unavailable, and provide the
/// **reference implementation** for all encoding schemes.
///
/// ### Core Design Philosophy:
/// - **Pure Baseline Implementation**: No upward dependencies to UnifiedProximaSIMD
/// - **LLVM Auto-Vectorization**: Loop structures optimized for compiler SIMD generation
/// - **Fallback Layer**: Used when hardware SIMD unavailable or not yet implemented
/// - **Testing/Validation**: Reference implementation for correctness verification
///
/// ### Data Flow Architecture:
/// ```
/// ┌─────────────────────────────────────────────────────────────────┐
/// │ PRODUCTION PATH (Storage Engines)                                │
/// │                                                                   │
/// │ Storage Engine → UnifiedProximaSIMD → Hardware SIMD (AVX2/NEON)  │
/// │                           ↓ (fallback)                            │
/// │                    ProximaEncoder (this module)                  │
/// └─────────────────────────────────────────────────────────────────┘
///
/// ┌─────────────────────────────────────────────────────────────────┐
/// │ TESTING/VALIDATION PATH                                          │
/// │                                                                   │
/// │ Test Suite → ProximaEncoder → Verify Correctness                 │
/// │           ↓                                                       │
/// │   Compare with UnifiedProximaSIMD outputs                         │
/// └─────────────────────────────────────────────────────────────────┘
/// ```
///
/// ### Modular Structure:
/// - **markers.rs**: Marker byte constants for encoding identification
/// - **types.rs**: ProximaScheme, ProximaDataType, VectorEncodingLayout enums
/// - **encoder.rs**: ProximaEncoder struct and high-level encoding methods (future)
/// - **decoder.rs**: ProximaDecoder struct and high-level decoding methods (future)
/// - **encoding/**: Specialized encoding algorithms (future)
/// - **decoding/**: Specialized decoding algorithms (future)
/// - **analysis.rs**: Pattern analysis and scheme selection (future)
///
/// ## Usage Guidelines
///
/// ### ✅ CORRECT: Use for Testing and Validation
/// ```rust
/// use crate::storage::engines::core::ops::proximaencoder::*;
///
/// // Test correctness of encoding scheme
/// let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 0 });
/// let data = vec![100, 102, 105, 103, 107];
/// let encoded = encoder.encode_integers(&data, None)?;
///
/// let decoder = ProximaDecoder::new(ProximaScheme::Delta { base: 0 });
/// let decoded = decoder.decode_integers(&encoded, None)?;
/// assert_eq!(data, decoded);
/// ```
///
/// ### ❌ WRONG: Direct Production Use (Use UnifiedProximaSIMD Instead)
/// ```rust
/// // ❌ DON'T: Bypass UnifiedProximaSIMD in production
/// let encoder = ProximaEncoder::new(scheme);
/// let encoded = encoder.encode_f32(&vectors, None)?;
///
/// // ✅ DO: Use UnifiedProximaSIMD for production
/// let simd_encoder = UnifiedProximaSIMD::new_for_engine(profile, dim, count);
/// let encoded = simd_encoder.simd_encode_dimension(&vectors)?;
/// ```

// Module declarations
pub mod markers;
pub mod types;
pub mod encoding;
pub mod decoding;
pub mod analysis;
pub mod encoder;
pub mod decoder;

// Re-export commonly used types
pub use markers::{
    HAS_COUNT_FLAG,
    RAW_UNCOMPRESSED, PROXIMA_BITPACKED, PROXIMA_DELTA, PROXIMA_FRAME_OF_REFERENCE,
    PROXIMA_PATCHED_BASE, PROXIMA_DICTIONARY, PROXIMA_RUN_LENGTH,
    PROXIMA_PFOR_DELTA, PROXIMA_ZIGZAG, PROXIMA_SIMPLE8B, PROXIMA_VBYTE,
    PROXIMA_DOUBLE_DELTA, PROXIMA_SIMD_RLE, PROXIMA_HYBRID,
    PROXIMA_SPARSE_BITMAP, PROXIMA_SPARSE_COO,
    has_count, base_scheme, is_quantized, is_sparse,
};

pub use types::{
    ProximaDataType,
    ProximaScheme,
    VectorEncodingLayout,
    EncodedDimension,
};

pub use analysis::{
    analyze_and_choose_scheme,
    analyze_and_choose_scheme_f32,
};

pub use encoder::ProximaEncoder;
pub use decoder::ProximaDecoder;

// Temporary re-export of helper structs from legacy module
// These will be migrated in future phases
pub use super::proximaencoder_legacy::{
    DimensionGroup,
    ColumnarEncodedVectors,
    RowWiseEncodedVectors,
    EncodedVectors,
};
