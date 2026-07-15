//! `proximablocks` hermetic sub-leaves hoisted from the root crate
//! (root-crate decomposition, engines extraction). These have no root `crate::`
//! deps (only foundation/horizontal crates), so they move verbatim — except
//! `spatial_traits`, whose single `crate::...helix::hilbert_curve` ref swaps to
//! `proximadb_storage_common::hilbert_curve` (the helix file is a re-export shim).
//! Root keeps per-file re-export shims.

pub mod header_metadata;
pub mod per_column_alignment;
pub mod spatial_clustering;
pub mod spatial_encoding;
pub mod spatial_pruning;
pub mod spatial_traits;
