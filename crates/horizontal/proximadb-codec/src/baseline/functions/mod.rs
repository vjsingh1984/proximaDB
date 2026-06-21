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

pub mod adaptive;
pub mod bitpack;
pub mod delta;
pub mod double_delta;
pub mod frame_of_ref;
pub mod gorilla;
pub(crate) mod helpers;
pub mod patched_base;
pub mod pfor_delta;
pub mod pfor_double_delta;
pub mod raw;
pub mod run_length;
pub mod simple8b;
pub mod sparse_bitmap;
pub mod sparse_coo;
pub mod vbyte;
pub mod vector_base_xor;
pub mod zigzag;

// Re-export for convenience
pub use raw::{
    decode_f32 as raw_decode_f32, decode_i32 as raw_decode_i32, decode_i64 as raw_decode_i64,
    encode_f32 as raw_encode_f32, encode_i32 as raw_encode_i32, encode_i64 as raw_encode_i64,
};

pub use delta::{
    decode_f32 as delta_decode_f32, decode_i32 as delta_decode_i32, decode_i64 as delta_decode_i64,
    encode_f32 as delta_encode_f32, encode_i32 as delta_encode_i32, encode_i64 as delta_encode_i64,
};

pub use bitpack::{
    decode_f32 as bitpack_decode_f32, decode_i32 as bitpack_decode_i32,
    decode_i64 as bitpack_decode_i64, encode_f32 as bitpack_encode_f32,
    encode_i32 as bitpack_encode_i32, encode_i64 as bitpack_encode_i64,
};

pub use frame_of_ref::{
    decode_f32 as for_decode_f32, decode_i32 as for_decode_i32, decode_i64 as for_decode_i64,
    encode_f32 as for_encode_f32, encode_i32 as for_encode_i32, encode_i64 as for_encode_i64,
};

pub use sparse_bitmap::{
    decode_f32 as sparse_bitmap_decode_f32, decode_i32 as sparse_bitmap_decode_i32,
    decode_i64 as sparse_bitmap_decode_i64, encode_f32 as sparse_bitmap_encode_f32,
    encode_i32 as sparse_bitmap_encode_i32, encode_i64 as sparse_bitmap_encode_i64,
};

pub use sparse_coo::{
    decode_f32 as sparse_coo_decode_f32, decode_i32 as sparse_coo_decode_i32,
    decode_i64 as sparse_coo_decode_i64, encode_f32 as sparse_coo_encode_f32,
    encode_i32 as sparse_coo_encode_i32, encode_i64 as sparse_coo_encode_i64,
};

pub use run_length::{
    decode_f32 as rle_decode_f32, decode_i32 as rle_decode_i32, decode_i64 as rle_decode_i64,
    encode_f32 as rle_encode_f32, encode_i32 as rle_encode_i32, encode_i64 as rle_encode_i64,
};

pub use pfor_delta::{
    decode_f32 as pfor_decode_f32, decode_i32 as pfor_decode_i32, decode_i64 as pfor_decode_i64,
    encode_f32 as pfor_encode_f32, encode_i32 as pfor_encode_i32, encode_i64 as pfor_encode_i64,
};

pub use patched_base::{
    decode_f32 as patched_base_decode_f32, decode_i32 as patched_base_decode_i32,
    decode_i64 as patched_base_decode_i64, encode_f32 as patched_base_encode_f32,
    encode_i32 as patched_base_encode_i32, encode_i64 as patched_base_encode_i64,
};

pub use pfor_double_delta::{
    decode_f32 as pfor_double_delta_decode_f32, decode_i32 as pfor_double_delta_decode_i32,
    decode_i64 as pfor_double_delta_decode_i64, encode_f32 as pfor_double_delta_encode_f32,
    encode_i32 as pfor_double_delta_encode_i32, encode_i64 as pfor_double_delta_encode_i64,
};

pub use zigzag::{
    decode_f32 as zigzag_decode_f32, decode_i32 as zigzag_decode_i32,
    decode_i64 as zigzag_decode_i64, encode_f32 as zigzag_encode_f32,
    encode_i32 as zigzag_encode_i32, encode_i64 as zigzag_encode_i64,
};

pub use double_delta::{
    decode_f32 as double_delta_decode_f32, decode_i32 as double_delta_decode_i32,
    decode_i64 as double_delta_decode_i64, encode_f32 as double_delta_encode_f32,
    encode_i32 as double_delta_encode_i32, encode_i64 as double_delta_encode_i64,
};

pub use gorilla::{
    decode_f32 as gorilla_decode_f32, decode_f64 as gorilla_decode_f64,
    decode_i32 as gorilla_decode_i32, decode_i64 as gorilla_decode_i64,
    encode_f32 as gorilla_encode_f32, encode_f64 as gorilla_encode_f64,
    encode_i32 as gorilla_encode_i32, encode_i64 as gorilla_encode_i64,
};

pub use vbyte::{
    decode_f32 as vbyte_decode_f32, decode_i32 as vbyte_decode_i32, decode_i64 as vbyte_decode_i64,
    encode_f32 as vbyte_encode_f32, encode_i32 as vbyte_encode_i32, encode_i64 as vbyte_encode_i64,
};
pub mod dictionary;

pub use dictionary::{
    decode_f32 as dictionary_decode_f32, decode_i32 as dictionary_decode_i32,
    decode_i64 as dictionary_decode_i64, encode_f32 as dictionary_encode_f32,
    encode_i32 as dictionary_encode_i32, encode_i64 as dictionary_encode_i64,
};

pub use simple8b::{
    decode_f32 as simple8b_decode_f32, decode_i32 as simple8b_decode_i32,
    decode_i64 as simple8b_decode_i64, encode_f32 as simple8b_encode_f32,
    encode_i32 as simple8b_encode_i32, encode_i64 as simple8b_encode_i64,
};

pub use adaptive::{
    decode_f32 as adaptive_decode_f32, decode_i32 as adaptive_decode_i32,
    decode_i64 as adaptive_decode_i64, encode_f32 as adaptive_encode_f32,
    encode_i32 as adaptive_encode_i32, encode_i64 as adaptive_encode_i64,
};

pub use vector_base_xor::{
    VectorBaseXorProfile, decode_f32_vectors as vector_base_xor_decode_f32_vectors,
    encode_f32_vectors as vector_base_xor_encode_f32_vectors,
    encode_f32_vectors_with_profile as vector_base_xor_encode_f32_vectors_with_profile,
    profile_f32_vectors as vector_base_xor_profile_f32_vectors,
};

pub mod sq8;
pub use sq8::{
    Sq8Params, decode as sq8_decode, decode_into as sq8_decode_into, encode as sq8_encode,
    fit_params as sq8_fit_params,
};

pub mod rabitq;
pub use rabitq::{
    RaBitQCode, RaBitQParams, build_rotation as rabitq_build_rotation, encode as rabitq_encode,
    encode_column as rabitq_encode_column, fit_params as rabitq_fit_params,
    reconstruct as rabitq_reconstruct, rotate_query as rabitq_rotate_query,
};
