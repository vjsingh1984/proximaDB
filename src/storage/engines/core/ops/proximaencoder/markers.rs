/// # Proxima Encoding Markers
///
/// Unified marker byte system used across all storage engines for encoding identification.
/// These markers ensure consistency across SST, SWIFT, RAPTOR, VIPER, NOVA, and HELIX engines.

// High bit (0x80) indicates if element count follows the marker
pub const HAS_COUNT_FLAG: u8 = 0x80;

// Base encoding schemes (0x00-0x7F range, without count flag)
pub const RAW_UNCOMPRESSED: u8 = 0x00;
pub const PROXIMA_BITPACKED: u8 = 0x10;
pub const PROXIMA_DELTA: u8 = 0x20;
pub const PROXIMA_FRAME_OF_REFERENCE: u8 = 0x30;
pub const PROXIMA_PATCHED_BASE: u8 = 0x40;
pub const PROXIMA_DICTIONARY: u8 = 0x50;
pub const PROXIMA_RUN_LENGTH: u8 = 0x60;

// === STATE-OF-THE-ART ENCODING MARKERS ===
// Advanced compression algorithms for optimal compression ratios
pub const PROXIMA_PFOR_DELTA: u8 = 0x35;      // Patched Frame of Reference Delta
pub const PROXIMA_ZIGZAG: u8 = 0x25;          // Zigzag encoding for signed values
pub const PROXIMA_SIMPLE8B: u8 = 0x45;        // Simple-8b variable bit-width
pub const PROXIMA_VBYTE: u8 = 0x55;           // Variable-byte encoding
pub const PROXIMA_DOUBLE_DELTA: u8 = 0x28;    // Double-delta for time series
pub const PROXIMA_SIMD_RLE: u8 = 0x65;        // SIMD-optimized RLE
pub const PROXIMA_HYBRID: u8 = 0x75;          // Hybrid multi-scheme encoding
pub const PROXIMA_SPARSE_BITMAP: u8 = 0x15;   // Sparse bitmap encoding
pub const PROXIMA_SPARSE_COO: u8 = 0x18;      // Sparse COO encoding

// Versions with count flag set (0x80-0xFF range, for sparse/variable data)
pub const RAW_UNCOMPRESSED_WITH_COUNT: u8 = RAW_UNCOMPRESSED | HAS_COUNT_FLAG;
pub const PROXIMA_BITPACKED_WITH_COUNT: u8 = PROXIMA_BITPACKED | HAS_COUNT_FLAG;
pub const PROXIMA_DELTA_WITH_COUNT: u8 = PROXIMA_DELTA | HAS_COUNT_FLAG;
pub const PROXIMA_FRAME_OF_REFERENCE_WITH_COUNT: u8 = PROXIMA_FRAME_OF_REFERENCE | HAS_COUNT_FLAG;
pub const PROXIMA_RUN_LENGTH_WITH_COUNT: u8 = PROXIMA_RUN_LENGTH | HAS_COUNT_FLAG;

// New advanced encodings with count flags
pub const PROXIMA_PFOR_DELTA_WITH_COUNT: u8 = PROXIMA_PFOR_DELTA | HAS_COUNT_FLAG;
pub const PROXIMA_ZIGZAG_WITH_COUNT: u8 = PROXIMA_ZIGZAG | HAS_COUNT_FLAG;
pub const PROXIMA_SIMPLE8B_WITH_COUNT: u8 = PROXIMA_SIMPLE8B | HAS_COUNT_FLAG;
pub const PROXIMA_VBYTE_WITH_COUNT: u8 = PROXIMA_VBYTE | HAS_COUNT_FLAG;
pub const PROXIMA_DOUBLE_DELTA_WITH_COUNT: u8 = PROXIMA_DOUBLE_DELTA | HAS_COUNT_FLAG;
pub const PROXIMA_SIMD_RLE_WITH_COUNT: u8 = PROXIMA_SIMD_RLE | HAS_COUNT_FLAG;
pub const PROXIMA_HYBRID_WITH_COUNT: u8 = PROXIMA_HYBRID | HAS_COUNT_FLAG;
pub const PROXIMA_SPARSE_BITMAP_WITH_COUNT: u8 = PROXIMA_SPARSE_BITMAP | HAS_COUNT_FLAG;
pub const PROXIMA_SPARSE_COO_WITH_COUNT: u8 = PROXIMA_SPARSE_COO | HAS_COUNT_FLAG;

// Engine-specific ranges (for special cases)
pub const SWIFT_SUPERBLOCK_START: u8 = 0x80;
pub const SWIFT_SUPERBLOCK_END: u8 = 0x8F;
pub const SWIFT_INHERIT: u8 = 0xFF; // Child blocks inherit from SuperBlock

pub const RAPTOR_TENSOR_START: u8 = 0xA0;
pub const RAPTOR_RAW_TENSOR: u8 = 0xA0;
pub const RAPTOR_PROXIMA_TENSOR: u8 = 0xA1;
pub const RAPTOR_SPARSE_TENSOR: u8 = 0xA2;
pub const RAPTOR_QUANTIZED_TENSOR: u8 = 0xA3;
pub const RAPTOR_HNSW_GRAPH: u8 = 0xA4;
pub const RAPTOR_TENSOR_END: u8 = 0xAF;

// PRISM multi-resolution markers (0xB0-0xBF)
pub const PRISM_RESOLUTION_START: u8 = 0xB0;
pub const PRISM_MULTI_RESOLUTION: u8 = 0xB0;
pub const PRISM_PROGRESSIVE: u8 = 0xB1;
pub const PRISM_BINARY_SKETCH: u8 = 0xB2;
pub const PRISM_INT8_QUANTIZED: u8 = 0xB3;
pub const PRISM_PQ_ENCODED: u8 = 0xB4;
pub const PRISM_FP32_FULL: u8 = 0xB5;
pub const PRISM_RESOLUTION_END: u8 = 0xBF;

pub const PRISM_BINARY_START: u8 = 0xB0;
pub const PRISM_INT8_START: u8 = 0xC0;
pub const PRISM_PQ_START: u8 = 0xD0;
pub const PRISM_FP32_START: u8 = 0xE0;

// Quantization markers (shared across engines)
pub const QUANTIZED_INT8: u8 = 0x70;
pub const QUANTIZED_PQ4: u8 = 0x71;
pub const QUANTIZED_PQ8: u8 = 0x72;
pub const QUANTIZED_PQ16: u8 = 0x73;
pub const QUANTIZED_BINARY: u8 = 0x74;

// Sparse tensor markers (shared across engines)
pub const SPARSE_COO: u8 = 0x75;
pub const SPARSE_CSR: u8 = 0x76;
pub const SPARSE_CSC: u8 = 0x77;

/// Check if a marker indicates count is stored
#[inline]
pub fn has_count(marker: u8) -> bool {
    (marker & HAS_COUNT_FLAG) != 0
}

/// Get base scheme without count flag
#[inline]
pub fn base_scheme(marker: u8) -> u8 {
    marker & !HAS_COUNT_FLAG
}

/// Get marker for a Proxima scheme
pub fn from_scheme(scheme: &super::ProximaScheme) -> u8 {
    match scheme {
        super::ProximaScheme::BitPacked { .. } => PROXIMA_BITPACKED,
        super::ProximaScheme::Delta { .. } => PROXIMA_DELTA,
        super::ProximaScheme::FrameOfReference { .. } => PROXIMA_FRAME_OF_REFERENCE,
        super::ProximaScheme::PatchedBase { .. } => PROXIMA_PATCHED_BASE,
        super::ProximaScheme::Dictionary => PROXIMA_DICTIONARY,
        super::ProximaScheme::RunLength => PROXIMA_RUN_LENGTH,
        // New schemes from SIMD optimization
        super::ProximaScheme::PForDelta { .. } => 0x07,
        super::ProximaScheme::Zigzag { .. } => 0x08,
        super::ProximaScheme::Simple8b => 0x09,
        super::ProximaScheme::VByte => 0x0A,
        super::ProximaScheme::SparseBitmap => 0x10,
        super::ProximaScheme::SparseCOO => 0x11,
        super::ProximaScheme::DoubleDelta { .. } => 0x0B,
        super::ProximaScheme::Gorilla => 0x0C,
        super::ProximaScheme::Adaptive => 0x0D,
        super::ProximaScheme::SIMDRunLength { .. } => 0x0E,
        super::ProximaScheme::Hybrid { .. } => 0x0F,
    }
}

/// Get scheme from marker
pub fn to_scheme(marker: u8) -> Option<super::ProximaScheme> {
    match marker {
        PROXIMA_BITPACKED => Some(super::ProximaScheme::BitPacked { bits: 16 }),
        PROXIMA_DELTA => Some(super::ProximaScheme::Delta { base: 0 }),
        PROXIMA_FRAME_OF_REFERENCE => Some(super::ProximaScheme::FrameOfReference {
            reference: 0,
            bits: 16,
        }),
        PROXIMA_PATCHED_BASE => Some(super::ProximaScheme::PatchedBase {
            base: 0,
            patch_bits: 16,
        }),
        PROXIMA_DICTIONARY => Some(super::ProximaScheme::Dictionary),
        PROXIMA_RUN_LENGTH => Some(super::ProximaScheme::RunLength),
        0x10 => Some(super::ProximaScheme::SparseBitmap),
        0x11 => Some(super::ProximaScheme::SparseCOO),
        _ => None,
    }
}

/// Check if marker is a quantized type
pub fn is_quantized(marker: u8) -> bool {
    matches!(
        marker,
        QUANTIZED_INT8 | QUANTIZED_PQ4 | QUANTIZED_PQ8 | QUANTIZED_PQ16 | QUANTIZED_BINARY
    )
}

/// Check if marker is a sparse type
pub fn is_sparse(marker: u8) -> bool {
    matches!(marker, SPARSE_COO | SPARSE_CSR | SPARSE_CSC | 0x10 | 0x11)
}
