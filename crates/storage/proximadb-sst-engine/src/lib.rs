//! SST engine leaves — the seed of the SST engine extraction (root-crate
//! decomposition, TD-DECOMP-82).
//!
//! The full SST engine (`src/storage/engines/sst/`, ~60K LOC) lands here over
//! successive phases; this crate starts with the files whose coupling was
//! already dissolved: the SST error vocabulary, row filters, staged writes,
//! the deletion-vector store, and the decompression cache. Root
//! `storage::engines::sst` re-exports everything, so `crate::storage::engines
//! ::sst::*` paths resolve unchanged.
//!
//! Filesystem access goes through `Arc<dyn FilesystemPort>` (storage-ports)
//! — the composition root injects the concrete factory.

pub mod decompression_cache;
pub mod deletion_vector_store;
pub mod error;
pub mod row_filter;
pub mod staged_write;

#[cfg(test)]
mod test_local_port;

pub use error::{Result, SstError};
