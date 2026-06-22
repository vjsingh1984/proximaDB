//! Compatibility re-export for vector quantization internal types.
//!
//! Ownership has moved to the vector modality crate. Keep this shell while
//! root compute callers migrate to `proximadb_vector::quantization::internal_types`.

pub use proximadb_vector::quantization::internal_types::*;
