//! Experimental SIMD codec compatibility shim.
//!
//! This feature-gated module forwards to the active SIMD implementation so the
//! experimental entrypoint compiles without depending on archived backup files.

pub use super::simd::*;
