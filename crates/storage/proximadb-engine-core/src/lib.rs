//! Shared engine-core ops + formats, hoisted out of the root crate (root-crate
//! decomposition, engines extraction). Root re-exports the public surface via
//! thin shims. Occupants: the `simd_decode` ops subtree, hermetic
//! `proximablocks` sub-leaves (header_metadata, per_column_alignment), and the
//! raptor `smart_rowgroup_sizing` heuristics.

pub mod proximablocks;
pub mod simd_decode;
pub mod smart_rowgroup_sizing;
