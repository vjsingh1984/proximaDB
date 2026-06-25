//! Hardware-Accelerated Quantization
//!
//! **MIGRATION NOTICE**: SIMD-accelerated quantization has moved to the vector modality.
//!
//! Implementation:
//! `crates/modalities/proximadb-vector/src/quantization/hardware_accelerated.rs`
//!
//! GPU-specific stubs would live here once GPU support is implemented, as the
//! `gpu` feature flag belongs to the root crate, not the foundation layer.

pub use proximadb_vector::quantization::hardware_accelerated::AcceleratedQuantization;
