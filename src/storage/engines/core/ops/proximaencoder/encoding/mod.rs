/// # Proxima Encoding Algorithms
//
/// Modular encoding algorithm implementations organized by category.
//
/// ## Module Organization:
//
/// - **baseline.rs**: Core encoding algorithms (BitPacked, Delta, FOR, RLE, PatchedBase)
/// - **advanced.rs**: State-of-the-art encoders (PForDelta, Zigzag, Simple8b, VByte, DoubleDelta)
/// - **sparse.rs**: Sparse data encoders (SparseBitmap, SparseCOO)
/// - **vector.rs**: Vector-specific encoders (columnar, rowwise layouts)
/// - **specialized.rs**: Type-specific encoders (timestamps, IDs, counts, hashes)
//
/// ## Design Philosophy:
//
/// All encoding functions follow a consistent signature:
/// ```rust
/// fn encode_*(&self, data: &[T], ...) -> Result<Vec<u8>>
/// ```
//
/// - Input: Slice of source data
/// - Output: Encoded bytes with scheme-specific format
/// - Error handling: Returns anyhow::Result for robustness
//
/// ## Performance Characteristics:
//
/// - **LLVM Auto-Vectorization**: All loops optimized for compiler SIMD
/// - **Block Processing**: Process data in self.block_size chunks for efficiency
/// - **Zero-Copy**: Minimize allocations and copies where possible
/// - **Wrapping Arithmetic**: Used to avoid overflow checks in hot paths

pub mod baseline;
pub mod advanced;
pub mod sparse;
pub mod vector;
pub mod specialized;

// Phase 3: Re-export baseline and advanced encoding functions
pub use baseline::{
    bitpack_integers,
    delta_encode,
    frame_of_reference_encode,
    patched_base_encode,
    run_length_encode,
    encode_uncompressed,
};

pub use advanced::{
    pfor_delta_encode,
    zigzag_encode,
    simple8b_encode,
    vbyte_encode,
    double_delta_encode,
};

pub use sparse::{
    sparse_bitmap_encode,
    sparse_coo_encode,
    detect_sparsity,
    recommend_sparse_encoding,
    SparsityRecommendation,
};

pub use specialized::{
    encode_timestamps,
    encode_ids,
    encode_counts,
    encode_hashes,
    is_monotonic,
    is_sparse_small,
    detect_column_type,
    ColumnType,
};

// Future re-exports (Phase 3 - vector encoding requires struct definitions):
// pub use vector::{encode_vectors_columnar, encode_vectors_rowwise, encode_vectors_auto};
