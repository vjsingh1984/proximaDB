/// # Proxima Decoding Algorithms
//
/// Modular decoding algorithm implementations organized by category.
//
/// ## Module Organization:
//
/// - **baseline.rs**: Core decoding algorithms (BitPacked, Delta, FOR, RLE, PatchedBase)
/// - **advanced.rs**: State-of-the-art decoders (PForDelta, Zigzag, Simple8b, VByte, DoubleDelta)
/// - **sparse.rs**: Sparse data decoders (SparseBitmap, SparseCOO)
/// - **specialized.rs**: Type-specific decoders (timestamps, IDs, counts, hashes)
//
/// ## Design Philosophy:
//
/// All decoding functions follow a consistent signature:
/// ```rust
/// fn decode_*(&self, data: &[u8], count: usize, ...) -> Result<Vec<T>>
/// ```
//
/// - Input: Encoded bytes and count of expected values
/// - Output: Decoded values
/// - Error handling: Returns anyhow::Result for robustness
//
/// ## Performance Characteristics:
//
/// - **LLVM Auto-Vectorization**: All loops optimized for compiler SIMD
/// - **Zero-Copy**: Minimize allocations where possible
/// - **Wrapping Arithmetic**: Used to match encoder behavior

pub mod baseline;
pub mod advanced;
pub mod sparse;
pub mod specialized;

// Phase 4: Re-export baseline and advanced decoding functions
pub use baseline::{
    unpack_integers,
    delta_decode,
    frame_of_reference_decode,
    patched_base_decode,
    run_length_decode,
    decode_uncompressed,
};

pub use advanced::{
    pfor_delta_decode,
    zigzag_decode,
    simple8b_decode,
    vbyte_decode,
    double_delta_decode,
};

pub use sparse::{
    sparse_bitmap_decode,
    sparse_coo_decode,
};

pub use specialized::{
    decode_timestamps,
    decode_ids,
    decode_counts,
    decode_hashes,
};
