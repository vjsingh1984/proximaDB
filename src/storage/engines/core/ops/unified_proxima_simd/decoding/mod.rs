//! Decoding operations for UnifiedProximaSIMD
//!
//! This module provides hardware-accelerated decoding for different data types:
//! - f32_decoder: Decoding for f32 vector dimensions
//! - i64_decoder: Decoding for integer types (timestamps, IDs, counts)

pub mod f32_decoder;
pub mod i64_decoder;

pub use f32_decoder::F32Decoder;
pub use i64_decoder::I64Decoder;
