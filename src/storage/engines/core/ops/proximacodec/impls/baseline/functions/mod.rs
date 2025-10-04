// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Raw encoding/decoding functions (Pure Rust, no SIMD)
//!
//! These functions implement the actual compression algorithms.
//! They return raw compressed bytes WITHOUT headers.
//!
//! ## Implemented Schemes (16/17 Complete)
//! - [x] delta.rs - Delta encoding (Phase 2.1)
//! - [x] bitpack.rs - Bit-packing (Phase 2.2)
//! - [x] frame_of_ref.rs - Frame of Reference (Phase 2.3)
//! - [x] sparse_bitmap.rs - Sparse bitmap (Phase 2.4)
//! - [x] sparse_coo.rs - Sparse COO (Phase 2.5)
//! - [x] run_length.rs - Run-length encoding (Phase 2.6)
//! - [x] pfor_delta.rs - Patched Frame of Reference Delta (Phase 2.7)
//! - [x] patched_base.rs - Patched Base (outlier handling) (Phase 2.7b)
//! - [x] pfor_double_delta.rs - PFor DoubleDelta (Phase 2.7c)
//! - [x] zigzag.rs - Zigzag encoding (Phase 2.8)
//! - [x] double_delta.rs - Double-delta encoding (Phase 2.9)
//! - [x] gorilla.rs - Gorilla XOR compression (Phase 2.10)
//! - [x] vbyte.rs - Variable-byte encoding (Phase 2.11)
//! - [x] dictionary.rs - Dictionary encoding (Phase 2.12)
//! - [x] simple8b.rs - Simple8b variable-length (Phase 2.13)
//! - [x] adaptive.rs - Automatic scheme selection (Phase 2.14)
//! - [ ] hybrid.rs - Multi-scheme hybrid (Phase 2.15) - LOW PRIORITY

pub(crate) mod helpers;
pub mod raw;
pub mod delta;
pub mod bitpack;
pub mod frame_of_ref;
pub mod sparse_bitmap;
pub mod sparse_coo;
pub mod run_length;
pub mod pfor_delta;
pub mod patched_base;
pub mod pfor_double_delta;
pub mod zigzag;
pub mod double_delta;
pub mod gorilla;
pub mod vbyte;
pub mod simple8b;
pub mod adaptive;

// Re-export for convenience
pub use raw::{
    encode_f32 as raw_encode_f32,
    encode_i64 as raw_encode_i64,
    encode_i32 as raw_encode_i32,
    decode_f32 as raw_decode_f32,
    decode_i64 as raw_decode_i64,
    decode_i32 as raw_decode_i32,
};

pub use delta::{
    encode_f32 as delta_encode_f32,
    encode_i64 as delta_encode_i64,
    encode_i32 as delta_encode_i32,
    decode_f32 as delta_decode_f32,
    decode_i64 as delta_decode_i64,
    decode_i32 as delta_decode_i32,
};

pub use bitpack::{
    encode_f32 as bitpack_encode_f32,
    encode_i64 as bitpack_encode_i64,
    encode_i32 as bitpack_encode_i32,
    decode_f32 as bitpack_decode_f32,
    decode_i64 as bitpack_decode_i64,
    decode_i32 as bitpack_decode_i32,
};

pub use frame_of_ref::{
    encode_f32 as for_encode_f32,
    encode_i64 as for_encode_i64,
    encode_i32 as for_encode_i32,
    decode_f32 as for_decode_f32,
    decode_i64 as for_decode_i64,
    decode_i32 as for_decode_i32,
};

pub use sparse_bitmap::{
    encode_f32 as sparse_bitmap_encode_f32,
    encode_i64 as sparse_bitmap_encode_i64,
    encode_i32 as sparse_bitmap_encode_i32,
    decode_f32 as sparse_bitmap_decode_f32,
    decode_i64 as sparse_bitmap_decode_i64,
    decode_i32 as sparse_bitmap_decode_i32,
};

pub use sparse_coo::{
    encode_f32 as sparse_coo_encode_f32,
    encode_i64 as sparse_coo_encode_i64,
    encode_i32 as sparse_coo_encode_i32,
    decode_f32 as sparse_coo_decode_f32,
    decode_i64 as sparse_coo_decode_i64,
    decode_i32 as sparse_coo_decode_i32,
};

pub use run_length::{
    encode_f32 as rle_encode_f32,
    encode_i64 as rle_encode_i64,
    encode_i32 as rle_encode_i32,
    decode_f32 as rle_decode_f32,
    decode_i64 as rle_decode_i64,
    decode_i32 as rle_decode_i32,
};

pub use pfor_delta::{
    encode_f32 as pfor_encode_f32,
    encode_i64 as pfor_encode_i64,
    encode_i32 as pfor_encode_i32,
    decode_f32 as pfor_decode_f32,
    decode_i64 as pfor_decode_i64,
    decode_i32 as pfor_decode_i32,
};

pub use patched_base::{
    encode_f32 as patched_base_encode_f32,
    encode_i64 as patched_base_encode_i64,
    encode_i32 as patched_base_encode_i32,
    decode_f32 as patched_base_decode_f32,
    decode_i64 as patched_base_decode_i64,
    decode_i32 as patched_base_decode_i32,
};

pub use pfor_double_delta::{
    encode_f32 as pfor_double_delta_encode_f32,
    encode_i64 as pfor_double_delta_encode_i64,
    encode_i32 as pfor_double_delta_encode_i32,
    decode_f32 as pfor_double_delta_decode_f32,
    decode_i64 as pfor_double_delta_decode_i64,
    decode_i32 as pfor_double_delta_decode_i32,
};

pub use zigzag::{
    encode_f32 as zigzag_encode_f32,
    encode_i64 as zigzag_encode_i64,
    encode_i32 as zigzag_encode_i32,
    decode_f32 as zigzag_decode_f32,
    decode_i64 as zigzag_decode_i64,
    decode_i32 as zigzag_decode_i32,
};

pub use double_delta::{
    encode_f32 as double_delta_encode_f32,
    encode_i64 as double_delta_encode_i64,
    encode_i32 as double_delta_encode_i32,
    decode_f32 as double_delta_decode_f32,
    decode_i64 as double_delta_decode_i64,
    decode_i32 as double_delta_decode_i32,
};

pub use gorilla::{
    encode_f32 as gorilla_encode_f32,
    encode_i64 as gorilla_encode_i64,
    encode_i32 as gorilla_encode_i32,
    decode_f32 as gorilla_decode_f32,
    decode_i64 as gorilla_decode_i64,
    decode_i32 as gorilla_decode_i32,
};

pub use vbyte::{
    encode_f32 as vbyte_encode_f32,
    encode_i64 as vbyte_encode_i64,
    encode_i32 as vbyte_encode_i32,
    decode_f32 as vbyte_decode_f32,
    decode_i64 as vbyte_decode_i64,
    decode_i32 as vbyte_decode_i32,
};
pub mod dictionary;

pub use dictionary::{
    encode_f32 as dictionary_encode_f32,
    encode_i64 as dictionary_encode_i64,
    encode_i32 as dictionary_encode_i32,
    decode_f32 as dictionary_decode_f32,
    decode_i64 as dictionary_decode_i64,
    decode_i32 as dictionary_decode_i32,
};

pub use simple8b::{
    encode_f32 as simple8b_encode_f32,
    encode_i64 as simple8b_encode_i64,
    encode_i32 as simple8b_encode_i32,
    decode_f32 as simple8b_decode_f32,
    decode_i64 as simple8b_decode_i64,
    decode_i32 as simple8b_decode_i32,
};

pub use adaptive::{
    encode_f32 as adaptive_encode_f32,
    encode_i64 as adaptive_encode_i64,
    encode_i32 as adaptive_encode_i32,
    decode_f32 as adaptive_decode_f32,
    decode_i64 as adaptive_decode_i64,
    decode_i32 as adaptive_decode_i32,
};
