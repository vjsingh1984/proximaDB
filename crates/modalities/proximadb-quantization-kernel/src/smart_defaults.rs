//! Compatibility re-export for vector quantization smart defaults.
//!
//! Ownership has moved to the vector modality crate. Keep this shell while
//! root compute callers migrate to `proximadb_vector::quantization::smart_defaults`.

pub use proximadb_vector::quantization::smart_defaults::*;
