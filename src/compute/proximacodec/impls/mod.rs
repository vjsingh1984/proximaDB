// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Implementation modules for ProximaCodec
//!
//! This module re-exports the various implementation modules:
//! - baseline: Portable CPU implementations
//! - simd: Hardware-accelerated SIMD implementations
//! - gpu: GPU-accelerated implementations

// Re-export baseline module (sibling module)
pub mod baseline {
    pub use crate::compute::proximacodec::baseline::functions;
    // Re-export BaselineEncoder and BaselineDecoder at module level
    pub use crate::compute::proximacodec::baseline::encoder::BaselineEncoder;
    pub use crate::compute::proximacodec::baseline::decoder::BaselineDecoder;
    // Re-export encoder and decoder submodules
    pub use crate::compute::proximacodec::baseline::encoder;
    pub use crate::compute::proximacodec::baseline::decoder;
}

// Re-export GPU module (sibling module)
pub mod gpu {
    pub use crate::compute::proximacodec::gpu::*;
}

// Re-export SIMD implementations
// Note: SIMD functions are in simd.rs, encoder/decoder wrappers are in simd_impls/
pub mod simd {
    // Re-export encoder submodule
    pub use crate::compute::proximacodec::simd_impls::encoder;
    // Re-export decoder submodule
    pub use crate::compute::proximacodec::simd_impls::decoder;
    // Re-export at module level for convenience
    pub use crate::compute::proximacodec::simd_impls::encoder::SimdEncoder;
    pub use crate::compute::proximacodec::simd_impls::decoder::SimdDecoder;
}
