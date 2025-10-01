//! Encoding operations for UnifiedProximaSIMD
//!
//! This module provides hardware-accelerated encoding for different data types:
//! - f32_encoder: Encoding for f32 vector dimensions
//! - i64_encoder: Encoding for integer types (timestamps, IDs, counts)
//! - batch: Batch encoding operations

pub mod f32_encoder;
pub mod i64_encoder;
pub mod batch;

pub use f32_encoder::F32Encoder;
pub use i64_encoder::I64Encoder;
pub use batch::BatchEncoder;
