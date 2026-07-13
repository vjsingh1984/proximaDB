// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! ProximaCodec — shim re-exporting the `proximadb-codec` horizontal crate.
//!
//! The ProximaCodec and all its sibling modules (codec, registry, wire_format,
//! traits, batching, adaptive, simd, gpu, baseline, impls, analysis, strategy,
//! types, simd_analysis) now live in the `proximadb-codec` crate
//! (`crates/horizontal/proximadb-codec`). This root module keeps the historical
//! `crate::compute::proximacodec::*` (and the re-export alias
//! `crate::storage::engines::core::ops::proximacodec::*`) paths resolving for
//! existing consumers during the root-crate decomposition (#127 shared blocker
//! for engine extraction).
//!
//! New code should depend on `proximadb-codec` directly.

// Re-export the entire crate surface at the module root.
pub use proximadb_codec::*;

// Submodule path aliases — consumers reference these as
// `crate::compute::proximacodec::<sub>::...` (and via the ops re-export alias),
// so republish each submodule namespace from the crate.
pub mod adaptive {
    pub use proximadb_codec::adaptive::*;
}
pub mod analysis {
    pub use proximadb_codec::analysis::*;
}
pub mod baseline {
    pub use proximadb_codec::baseline::*;
}
pub mod batching {
    pub use proximadb_codec::batching::*;
}
pub mod codec {
    pub use proximadb_codec::codec::*;
}
pub mod gpu {
    pub use proximadb_codec::gpu::*;
}
pub mod impls {
    pub use proximadb_codec::impls::*;
}
pub mod registry {
    pub use proximadb_codec::registry::*;
}
pub mod simd {
    pub use proximadb_codec::simd::*;
}
pub mod simd_analysis {
    pub use proximadb_codec::simd_analysis::*;
}
pub mod strategy {
    pub use proximadb_codec::strategy::*;
}
pub mod traits {
    pub use proximadb_codec::traits::*;
}
pub mod types {
    pub use proximadb_codec::types::*;
}
pub mod wire_format {
    pub use proximadb_codec::wire_format::*;
}
// Optional experimental SIMD entrypoint (forwards to the active SIMD module).
#[cfg(feature = "simd-experimental")]
pub mod simd_experimental {
    pub use proximadb_codec::simd_experimental::*;
}
