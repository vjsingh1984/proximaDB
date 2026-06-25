//! Compile-time quantization optimizations
//!
//! **MIGRATION NOTICE**: This module has been moved to the vector modality.
//!
//! The compile-time quantization functionality is now maintained in:
//! `crates/modalities/proximadb-vector/src/quantization/compile_time.rs`
//!
//! This file remains as a compatibility re-export for backward compatibility during
//! the Phase 6C migration. All new code should use the vector modality directly.

// Re-export everything from vector modality
pub use proximadb_vector::quantization::compile_time::*;
