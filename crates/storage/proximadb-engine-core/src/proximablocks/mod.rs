//! `proximablocks` sub-leaves hoisted from the root crate (root-crate
//! decomposition, engines extraction). The hermetic leaves moved verbatim;
//! `block_structures` (the crown jewel, 5.7K LOC / 29 tests) landed in
//! TD-DECOMP-71 once all its root paths resolved to extracted crates
//! (bloom, compression, codec, search-types, proto, storage-common).
//! Root keeps per-file re-export shims.

pub mod block_structures;
pub mod bloom_filter;
pub mod compression_config;
pub mod header_metadata;
pub mod index_structures;
pub mod per_column_alignment;
pub mod row_config;
pub mod spatial_clustering;
pub mod spatial_encoding;
pub mod spatial_pruning;
pub mod spatial_traits;
pub mod utilities;
