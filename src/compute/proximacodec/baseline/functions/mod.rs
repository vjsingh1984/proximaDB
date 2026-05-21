//! Compatibility re-export for canonical baseline codec functions.
//!
//! The scalar implementations live in `proximadb-codec`. Keep this module so
//! older compute/storage imports continue to resolve without carrying duplicate
//! implementation files in the root crate.

pub use proximadb_codec::baseline::functions::*;
