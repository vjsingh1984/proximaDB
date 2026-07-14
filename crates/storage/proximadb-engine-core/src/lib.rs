//! Shared engine-core ops + formats, hoisted out of the root crate (root-crate
//! decomposition, engines extraction). Root re-exports the public surface via a
//! thin shim. First occupant: the `simd_decode` ops subtree (SIMD bitpacked
//! decoders — hermetic: depends only on `anyhow` + `proximadb-storage-common`).

pub mod simd_decode;
